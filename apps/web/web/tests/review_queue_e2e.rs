//! IMP-REQ-009-10: candidate → queue → confirm → timeline, end to end
//! through the real HTTP routes (TC-REQ-009-1, -3, -4, -5).
//!
//! `REVIEW_QUEUE_ENABLED` is read from the environment at request time and
//! set once here (mirroring `admin_routes.rs`'s `ensure_admin_env` `Once`
//! pattern) rather than toggled per-test — `cargo test` runs test functions
//! in parallel by default, so flipping a shared env var between "on" and
//! "off" across tests in the same binary would race. The flag-off → 404
//! behavior is instead covered at the unit level in
//! `src/config/flags.rs`'s own tests.

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use minijinja::{path_loader, Environment};
use serde_json::{json, Value};
use shovelsup_web::pipeline::resolver::resolve_mention;
use shovelsup_web::{app, AppState};
use sqlx::PgPool;
use std::sync::Once;
use tower::ServiceExt;
use uuid::Uuid;

const ADMIN_USER: &str = "admin";
const ADMIN_PASSWORD: &str = "test-password";

static INIT_ENV: Once = Once::new();

fn ensure_env() {
    INIT_ENV.call_once(|| {
        let hash = bcrypt::hash(ADMIN_PASSWORD, bcrypt::DEFAULT_COST).unwrap();
        std::env::set_var("ADMIN_USER", ADMIN_USER);
        std::env::set_var("ADMIN_PASSWORD_HASH", hash);
        std::env::set_var("REVIEW_QUEUE_ENABLED", "true");
    });
}

fn basic_auth_header(user: &str, password: &str) -> String {
    format!(
        "Basic {}",
        base64::Engine::encode(&base64::engine::general_purpose::STANDARD, format!("{user}:{password}"))
    )
}

async fn test_state(pool: PgPool) -> AppState {
    let mut env = Environment::new();
    env.set_loader(path_loader("../templates"));
    let redis_client = redis::Client::open("redis://localhost:6380").unwrap();
    let redis = redis::aio::ConnectionManager::new(redis_client).await.unwrap();
    AppState {
        env: std::sync::Arc::new(env),
        db: pool,
        redis,
    }
}

/// Seeds two mentions at the same address+type with *different* existing
/// projects already present, forcing REQ-005's resolver to flag a genuine
/// ambiguous-match review candidate (RULE-003) rather than auto-linking.
async fn seed_ambiguous_candidate(pool: &PgPool) -> (Uuid, Uuid) {
    let municipality_id = sqlx::query_scalar!(
        "INSERT INTO municipalities (name, slug, domain_allowlist) \
         VALUES ('Test City', 'test-city', ARRAY['test-city.example']) RETURNING id"
    )
    .fetch_one(pool)
    .await
    .unwrap();
    let doc_id = sqlx::query_scalar!(
        "INSERT INTO source_documents (municipality_id, source_url, checksum, content, content_type) \
         VALUES ($1, 'https://test-city.example/doc', 'chk', ''::bytea, 'text/html') RETURNING id",
        municipality_id
    )
    .fetch_one(pool)
    .await
    .unwrap();
    let chunk_id = sqlx::query_scalar!(
        "INSERT INTO document_chunks (source_document_id, chunk_index, content) \
         VALUES ($1, 0, 'chunk text') RETURNING id",
        doc_id
    )
    .fetch_one(pool)
    .await
    .unwrap();

    // An existing project of a *different* type at the same address makes
    // the next mention's resolution genuinely ambiguous (see
    // resolver::mod.rs's `other_type_matches` branch).
    let existing_project_id = sqlx::query_scalar!(
        "INSERT INTO projects (civic_address_normalized, project_type) \
         VALUES ('99 ambiguous street', 'industrial') RETURNING id"
    )
    .fetch_one(pool)
    .await
    .unwrap();

    let mention_id = sqlx::query_scalar!(
        "INSERT INTO project_mentions \
         (document_chunk_id, physical_work, civic_address, project_type, scale_units) \
         VALUES ($1, true, '99 Ambiguous Street', 'residential', 1) RETURNING id",
        chunk_id
    )
    .fetch_one(pool)
    .await
    .unwrap();

    let outcome = resolve_mention(pool, mention_id).await.unwrap();
    let candidate_id = match outcome {
        shovelsup_web::pipeline::resolver::ResolutionOutcome::FlaggedAmbiguous { review_candidate_id } => {
            review_candidate_id
        }
        other => panic!("expected FlaggedAmbiguous, got {other:?}"),
    };

    let _ = existing_project_id;
    (candidate_id, mention_id)
}

/// TC-REQ-009-4: a multi-match candidate created by the REQ-005 resolver
/// appears in the Open tab's list endpoint.
#[sqlx::test(migrations = "./migrations")]
async fn tc_req_009_4_multi_match_candidate_appears_in_open_tab(pool: PgPool) {
    ensure_env();
    let (candidate_id, _) = seed_ambiguous_candidate(&pool).await;
    let router = app(test_state(pool).await);

    let response = router
        .oneshot(
            Request::builder()
                .uri("/admin/review_candidates?status=open")
                .header(header::AUTHORIZATION, basic_auth_header(ADMIN_USER, ADMIN_PASSWORD))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body = http_body_util::BodyExt::collect(response.into_body()).await.unwrap().to_bytes();
    let candidates: Vec<Value> = serde_json::from_slice(&body).unwrap();
    assert!(candidates.iter().any(|c| c["id"].as_str().unwrap() == candidate_id.to_string()));
}

/// TC-REQ-009-1: confirm merges the ambiguous candidate into the proposed
/// project, end to end through the HTTP route — and the timeline reflects
/// it (IMP-REQ-009-10's full candidate → queue → confirm → timeline path).
#[sqlx::test(migrations = "./migrations")]
async fn tc_req_009_1_confirm_route_merges_candidate_and_updates_timeline(pool: PgPool) {
    ensure_env();
    let (candidate_id, mention_id) = seed_ambiguous_candidate(&pool).await;
    let target_project_id = sqlx::query_scalar!(
        "INSERT INTO projects (civic_address_normalized, project_type) \
         VALUES ('99 ambiguous street proposed', 'residential') RETURNING id"
    )
    .fetch_one(&pool)
    .await
    .unwrap();

    let router = app(test_state(pool.clone()).await);
    let response = router
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/admin/review_candidates/{candidate_id}/confirm"))
                .header(header::AUTHORIZATION, basic_auth_header(ADMIN_USER, ADMIN_PASSWORD))
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    json!({ "version": 1, "project_id": target_project_id }).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);

    let linked_project: Option<Uuid> =
        sqlx::query_scalar!("SELECT project_id FROM project_mentions WHERE id = $1", mention_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(linked_project, Some(target_project_id));

    let timeline_count: i64 = sqlx::query_scalar!(
        "SELECT count(*) FROM project_timeline_events WHERE project_id = $1",
        target_project_id
    )
    .fetch_one(&pool)
    .await
    .unwrap()
    .unwrap();
    assert_eq!(timeline_count, 1);
}

/// TC-REQ-009-3: stale version on confirm returns 409, no changes.
#[sqlx::test(migrations = "./migrations")]
async fn tc_req_009_3_stale_version_returns_409(pool: PgPool) {
    ensure_env();
    let (candidate_id, _) = seed_ambiguous_candidate(&pool).await;
    let target_project_id = sqlx::query_scalar!(
        "INSERT INTO projects (civic_address_normalized, project_type) \
         VALUES ('1 stale ave', 'residential') RETURNING id"
    )
    .fetch_one(&pool)
    .await
    .unwrap();

    let router = app(test_state(pool.clone()).await);
    let response = router
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/admin/review_candidates/{candidate_id}/confirm"))
                .header(header::AUTHORIZATION, basic_auth_header(ADMIN_USER, ADMIN_PASSWORD))
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    json!({ "version": 999, "project_id": target_project_id }).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::CONFLICT);

    let status: String =
        sqlx::query_scalar!("SELECT status FROM review_candidates WHERE id = $1", candidate_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(status, "open");
}

/// TC-REQ-009-5: DB failure during confirm leaves the candidate
/// unresolved, returns 503.
#[sqlx::test(migrations = "./migrations")]
async fn tc_req_009_5_db_failure_during_confirm_returns_503(pool: PgPool) {
    ensure_env();
    let (candidate_id, _) = seed_ambiguous_candidate(&pool).await;
    let project_id = Uuid::new_v4();

    pool.close().await;
    let router = app(test_state(pool).await);
    let response = router
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/admin/review_candidates/{candidate_id}/confirm"))
                .header(header::AUTHORIZATION, basic_auth_header(ADMIN_USER, ADMIN_PASSWORD))
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(json!({ "version": 1, "project_id": project_id }).to_string()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
}

/// Unauthenticated requests are rejected before any handler logic runs
/// (reusing `admin_auth::require_admin`, IMP-REQ-009-05).
#[sqlx::test(migrations = "./migrations")]
async fn unauthenticated_request_is_forbidden(pool: PgPool) {
    ensure_env();
    let router = app(test_state(pool).await);
    let response = router
        .oneshot(
            Request::builder()
                .uri("/admin/review_candidates")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

/// IMP-REQ-009-06: the server-rendered review-queue page renders the
/// candidate created by the resolver, including its Confirm/Reject form.
#[sqlx::test(migrations = "./migrations")]
async fn get_review_queue_page_renders_open_candidate(pool: PgPool) {
    ensure_env();
    let (candidate_id, _) = seed_ambiguous_candidate(&pool).await;
    let router = app(test_state(pool).await);

    let response = router
        .oneshot(
            Request::builder()
                .uri("/admin/review_queue")
                .header(header::AUTHORIZATION, basic_auth_header(ADMIN_USER, ADMIN_PASSWORD))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body = http_body_util::BodyExt::collect(response.into_body()).await.unwrap().to_bytes();
    let html = String::from_utf8(body.to_vec()).unwrap();
    assert!(html.contains(&candidate_id.to_string()));
    assert!(html.contains("data-action=\"confirm\""));
    assert!(html.contains("role=\"alert\""));
}
