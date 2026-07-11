//! IMP-REQ-008-06: automates TC-REQ-008-1..4 against the real
//! `GET /api/v1/projects/search` handler and `public_search_documents`
//! index (IMP-REQ-008-01/-02).

use axum::body::Body;
use axum::http::{Request, StatusCode};
use minijinja::{path_loader, Environment};
use serde_json::Value;
use shovelsup_web::jobs::public_search_refresh::refresh_public_search_index;
use shovelsup_web::{app, AppState};
use sqlx::PgPool;
use tower::ServiceExt;
use uuid::Uuid;

async fn test_state(pool: PgPool) -> AppState {
    let mut env = Environment::new();
    env.set_loader(path_loader("templates"));
    let redis_client = redis::Client::open("redis://localhost:6380").unwrap();
    let redis = redis::aio::ConnectionManager::new(redis_client).await.unwrap();
    AppState {
        env: std::sync::Arc::new(env),
        db: pool,
        redis,
    }
}

async fn seed_searchable_project(
    pool: &PgPool,
    civic_address_normalized: &str,
    municipality_name: &str,
) -> Uuid {
    let project_id = sqlx::query_scalar!(
        "INSERT INTO projects (civic_address_normalized, project_type) VALUES ($1, 'residential') RETURNING id",
        civic_address_normalized,
    )
    .fetch_one(pool)
    .await
    .unwrap();

    let suffix = Uuid::new_v4();
    let municipality_id = sqlx::query_scalar!(
        "INSERT INTO municipalities (name, slug, domain_allowlist) VALUES ($1, $2, ARRAY[$3]) RETURNING id",
        municipality_name,
        format!("slug-{suffix}"),
        format!("{suffix}.example"),
    )
    .fetch_one(pool)
    .await
    .unwrap();
    let doc_id = sqlx::query_scalar!(
        "INSERT INTO source_documents (municipality_id, source_url, checksum, content, content_type) \
         VALUES ($1, $2, 'chk', ''::bytea, 'text/html') RETURNING id",
        municipality_id,
        format!("https://{suffix}.example/doc"),
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
    sqlx::query!(
        "INSERT INTO project_mentions \
         (document_chunk_id, project_id, physical_work, civic_address, project_type, scale_units, normalized_status) \
         VALUES ($1, $2, true, $3, 'residential', 1, 'approved')",
        chunk_id,
        project_id,
        civic_address_normalized,
    )
    .execute(pool)
    .await
    .unwrap();

    project_id
}

/// TC-REQ-008-1: anonymous search by civic address returns matching project.
#[sqlx::test(migrations = "./migrations")]
async fn tc_req_008_1_search_by_civic_address_returns_matching_project(pool: PgPool) {
    let project_id = seed_searchable_project(&pool, "123 main street", "Test City").await;
    refresh_public_search_index(&pool).await.unwrap();

    let app = app(test_state(pool).await);
    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/projects/search?q=main+street")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body = http_body_util::BodyExt::collect(response.into_body())
        .await
        .unwrap()
        .to_bytes();
    let results: Vec<Value> = serde_json::from_slice(&body).unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0]["project_id"].as_str().unwrap(), project_id.to_string());
}

/// TC-REQ-008-2: search by municipality name — a query that matches only
/// the municipality field, with no overlap in the civic address, still
/// returns the result (the OR-match boundary between the two columns).
#[sqlx::test(migrations = "./migrations")]
async fn tc_req_008_2_search_by_municipality_name_matches_via_or_boundary(pool: PgPool) {
    seed_searchable_project(&pool, "42 elm crescent", "Riverside Heights").await;
    refresh_public_search_index(&pool).await.unwrap();

    let app = app(test_state(pool).await);
    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/projects/search?q=Riverside")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body = http_body_util::BodyExt::collect(response.into_body())
        .await
        .unwrap()
        .to_bytes();
    let results: Vec<Value> = serde_json::from_slice(&body).unwrap();
    assert_eq!(results.len(), 1, "must match on municipality_name even though the keyword isn't in the civic address");
}

/// TC-REQ-008-3: invalid `per_page` rejected with 400 before any DB query.
#[sqlx::test(migrations = "./migrations")]
async fn tc_req_008_3_invalid_per_page_rejected_without_db_query(pool: PgPool) {
    let app = app(test_state(pool).await);
    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/projects/search?q=test&per_page=0")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

#[sqlx::test(migrations = "./migrations")]
async fn tc_req_008_3_per_page_over_max_rejected(pool: PgPool) {
    let app = app(test_state(pool).await);
    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/projects/search?q=test&per_page=101")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

/// TC-REQ-008-4: 503 when the search connection pool is exhausted/closed.
#[sqlx::test(migrations = "./migrations")]
async fn tc_req_008_4_returns_503_when_pool_unavailable(pool: PgPool) {
    pool.close().await;
    let app = app(test_state(pool).await);
    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/projects/search?q=test")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
}

/// IMP-REQ-008-05: the 61st request within the rate-limit window from the
/// same IP returns 429.
#[sqlx::test(migrations = "./migrations")]
async fn imp_req_008_05_sixty_first_request_in_window_is_rate_limited(pool: PgPool) {
    std::env::set_var("RATE_LIMIT_SEARCH_RPM", "60");
    let unique_ip = format!("203.0.113.{}", rand_octet());
    let app = app(test_state(pool).await);

    let mut last_status = StatusCode::OK;
    for _ in 0..61 {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/v1/projects/search?q=test")
                    .header("x-forwarded-for", &unique_ip)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        last_status = response.status();
    }

    assert_eq!(last_status, StatusCode::TOO_MANY_REQUESTS);
}

fn rand_octet() -> u8 {
    use std::time::{SystemTime, UNIX_EPOCH};
    (SystemTime::now().duration_since(UNIX_EPOCH).unwrap().subsec_nanos() % 254) as u8 + 1
}
