use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use minijinja::Environment;
use shovelsup_web::{app, AppState};
use sqlx::PgPool;
use std::sync::{Arc, Once};
use tower::ServiceExt;

const ADMIN_USER: &str = "admin";
const ADMIN_PASSWORD: &str = "test-password";

static INIT_ENV: Once = Once::new();

fn ensure_admin_env() {
    INIT_ENV.call_once(|| {
        let hash = bcrypt::hash(ADMIN_PASSWORD, bcrypt::DEFAULT_COST).unwrap();
        std::env::set_var("ADMIN_USER", ADMIN_USER);
        std::env::set_var("ADMIN_PASSWORD_HASH", hash);
    });
}

fn basic_auth_header(user: &str, password: &str) -> String {
    format!(
        "Basic {}",
        base64::Engine::encode(
            &base64::engine::general_purpose::STANDARD,
            format!("{user}:{password}")
        )
    )
}

fn test_state(pool: PgPool) -> AppState {
    AppState {
        env: Arc::new(Environment::new()),
        db: pool,
    }
}

async fn seed_fetch_job(pool: &PgPool, status: &str) -> uuid::Uuid {
    let municipality_id = sqlx::query_scalar!(
        "INSERT INTO municipalities (name, slug, domain_allowlist) \
         VALUES ('Test City', 'test-city', ARRAY['test-city.example']) RETURNING id"
    )
    .fetch_one(pool)
    .await
    .unwrap();

    sqlx::query_scalar!(
        "INSERT INTO fetch_jobs (municipality_id, scheduled_for, status) \
         VALUES ($1, now(), $2) RETURNING id",
        municipality_id,
        status
    )
    .fetch_one(pool)
    .await
    .unwrap()
}

#[sqlx::test(migrations = "./migrations")]
async fn test_reprocess_without_auth_header_is_forbidden(pool: PgPool) {
    ensure_admin_env();
    let job_id = seed_fetch_job(&pool, "failed").await;
    let router = app(test_state(pool));

    let response = router
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/admin/fetch_jobs/{job_id}/reprocess"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

#[sqlx::test(migrations = "./migrations")]
async fn test_reprocess_with_wrong_password_is_forbidden(pool: PgPool) {
    ensure_admin_env();
    let job_id = seed_fetch_job(&pool, "failed").await;
    let router = app(test_state(pool));

    let response = router
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/admin/fetch_jobs/{job_id}/reprocess"))
                .header(header::AUTHORIZATION, basic_auth_header(ADMIN_USER, "wrong"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

#[sqlx::test(migrations = "./migrations")]
async fn test_reprocess_missing_job_returns_404(pool: PgPool) {
    ensure_admin_env();
    let router = app(test_state(pool));
    let missing_id = uuid::Uuid::new_v4();

    let response = router
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/admin/fetch_jobs/{missing_id}/reprocess"))
                .header(
                    header::AUTHORIZATION,
                    basic_auth_header(ADMIN_USER, ADMIN_PASSWORD),
                )
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[sqlx::test(migrations = "./migrations")]
async fn test_reprocess_pending_job_returns_409(pool: PgPool) {
    ensure_admin_env();
    let job_id = seed_fetch_job(&pool, "pending").await;
    let router = app(test_state(pool));

    let response = router
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/admin/fetch_jobs/{job_id}/reprocess"))
                .header(
                    header::AUTHORIZATION,
                    basic_auth_header(ADMIN_USER, ADMIN_PASSWORD),
                )
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::CONFLICT);
}

#[sqlx::test(migrations = "./migrations")]
async fn test_reprocess_failed_job_resets_to_pending(pool: PgPool) {
    ensure_admin_env();
    let job_id = seed_fetch_job(&pool, "failed").await;
    let router = app(test_state(pool.clone()));

    let response = router
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/admin/fetch_jobs/{job_id}/reprocess"))
                .header(
                    header::AUTHORIZATION,
                    basic_auth_header(ADMIN_USER, ADMIN_PASSWORD),
                )
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);

    let status: String =
        sqlx::query_scalar!("SELECT status FROM fetch_jobs WHERE id = $1", job_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(status, "pending");
}
