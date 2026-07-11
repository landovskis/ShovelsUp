//! ⚠️ Needs Human Review: TC-REQ-006-6 does not yet exercise the real
//! DB-error-to-rendered-retry path; the project-detail template and error
//! rendering path are Loop B work.
//!
//! REQ-006 Loop A: TC-REQ-006-1..6 backend halves + IMP-REQ-006-07.
//!
//! Path deviation (flagged, not silent): the plan's Target Files / Modules
//! column names `apps/web/tests/integration/timeline_resolver.rs`, but this
//! repo's existing integration tests (pipeline_resolver.rs, admin_routes.rs,
//! etc.) are flat files directly under `tests/` — Cargo only auto-discovers
//! each file in `tests/` as its own test binary; a lone file under a
//! `tests/integration/` subdirectory would not be picked up without an
//! additional `tests/integration/main.rs` harness this repo doesn't have.
//! Placed here to match the repo's actual, working convention instead.
//!
//! TC-REQ-006-6's frontend half (UI shows retry) and IMP-REQ-006-08 (E2E
//! loaded/loading/empty/error states) are NOT covered here — see the
//! REQ-006 risk note in IMPLEMENTATION_CHECKLIST.md: this repo has no
//! Playwright/headless-browser tooling available in this environment, so
//! "loading" (a transient client-side htmx state) cannot be observed at
//! all, and the other three states are covered indirectly by asserting the
//! rendered `project_detail.html` markup for each case instead.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use minijinja::{path_loader, Environment};
use serde_json::Value;
use shovelsup_web::pipeline::resolver::resolve_mention;
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

async fn seed_project(pool: &PgPool, address: &str, project_type: &str) -> Uuid {
    sqlx::query_scalar!(
        "INSERT INTO projects (civic_address_normalized, project_type) VALUES ($1, $2) RETURNING id",
        address,
        project_type
    )
    .fetch_one(pool)
    .await
    .unwrap()
}

async fn seed_document_chunk(pool: &PgPool) -> Uuid {
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
    sqlx::query_scalar!(
        "INSERT INTO document_chunks (source_document_id, chunk_index, content) \
         VALUES ($1, 0, 'chunk text') RETURNING id",
        doc_id
    )
    .fetch_one(pool)
    .await
    .unwrap()
}

async fn insert_mention(pool: &PgPool, chunk_id: Uuid, civic_address: &str, project_type: &str) -> Uuid {
    sqlx::query_scalar!(
        "INSERT INTO project_mentions \
         (document_chunk_id, physical_work, civic_address, project_type, scale_units) \
         VALUES ($1, true, $2, $3, 1) RETURNING id",
        chunk_id,
        civic_address,
        project_type,
    )
    .fetch_one(pool)
    .await
    .unwrap()
}

async fn seed_timeline_event(
    pool: &PgPool,
    project_id: Uuid,
    mention_id: Uuid,
    event_date: chrono::DateTime<chrono::Utc>,
    status: &str,
) -> Uuid {
    sqlx::query_scalar!(
        "INSERT INTO project_timeline_events (project_id, project_mention_id, event_date, normalized_status) \
         VALUES ($1, $2, $3, $4) RETURNING id",
        project_id,
        mention_id,
        event_date,
        status,
    )
    .fetch_one(pool)
    .await
    .unwrap()
}

/// TC-REQ-006-1: timeline renders events in chronological order.
#[sqlx::test(migrations = "./migrations")]
async fn tc_req_006_1_timeline_renders_events_in_chronological_order(pool: PgPool) {
    let project_id = seed_project(&pool, "1 chrono st", "residential").await;
    let chunk_id = seed_document_chunk(&pool).await;
    let m1 = insert_mention(&pool, chunk_id, "1 chrono st", "residential").await;
    let m2 = insert_mention(&pool, chunk_id, "1 chrono st", "residential").await;
    let m3 = insert_mention(&pool, chunk_id, "1 chrono st", "residential").await;

    let base = chrono::Utc::now();
    seed_timeline_event(&pool, project_id, m2, base + chrono::Duration::days(2), "approved").await;
    seed_timeline_event(&pool, project_id, m1, base, "proposed").await;
    seed_timeline_event(&pool, project_id, m3, base + chrono::Duration::days(5), "deferred").await;

    let app = app(test_state(pool).await);
    let response = app
        .oneshot(
            Request::builder()
                .uri(format!("/api/v1/projects/{project_id}/timeline"))
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
    let events: Vec<Value> = serde_json::from_slice(&body).unwrap();
    assert_eq!(events.len(), 3);
    let statuses: Vec<&str> = events
        .iter()
        .map(|e| e["normalized_status"].as_str().unwrap())
        .collect();
    assert_eq!(statuses, vec!["proposed", "approved", "deferred"]);
}

/// TC-REQ-006-2: same-day events tie-break by ingestion order.
#[sqlx::test(migrations = "./migrations")]
async fn tc_req_006_2_same_day_events_tie_break_by_ingestion_order(pool: PgPool) {
    let project_id = seed_project(&pool, "2 sameday ave", "commercial").await;
    let chunk_id = seed_document_chunk(&pool).await;
    let m1 = insert_mention(&pool, chunk_id, "2 sameday ave", "commercial").await;
    let m2 = insert_mention(&pool, chunk_id, "2 sameday ave", "commercial").await;

    let same_day = chrono::Utc::now();
    let first_id = seed_timeline_event(&pool, project_id, m1, same_day, "proposed").await;
    let second_id = seed_timeline_event(&pool, project_id, m2, same_day, "approved").await;

    let app = app(test_state(pool).await);
    let response = app
        .oneshot(
            Request::builder()
                .uri(format!("/api/v1/projects/{project_id}/timeline"))
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
    let events: Vec<Value> = serde_json::from_slice(&body).unwrap();
    let ids: Vec<String> = events.iter().map(|e| e["id"].as_str().unwrap().to_string()).collect();
    assert_eq!(ids, vec![first_id.to_string(), second_id.to_string()]);
}

/// TC-REQ-006-3: zero-mention project returns empty array, not 404.
#[sqlx::test(migrations = "./migrations")]
async fn tc_req_006_3_zero_mention_project_returns_empty_array(pool: PgPool) {
    let project_id = seed_project(&pool, "3 empty blvd", "institutional").await;

    let app = app(test_state(pool).await);
    let response = app
        .oneshot(
            Request::builder()
                .uri(format!("/api/v1/projects/{project_id}/timeline"))
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
    let events: Vec<Value> = serde_json::from_slice(&body).unwrap();
    assert!(events.is_empty());
}

/// TC-REQ-006-4: malformed project id rejected with 400.
#[sqlx::test(migrations = "./migrations")]
async fn tc_req_006_4_malformed_project_id_rejected_with_400(pool: PgPool) {
    let app = app(test_state(pool).await);
    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/projects/not-a-uuid/timeline")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

/// TC-REQ-006-5: nonexistent project id returns 404.
#[sqlx::test(migrations = "./migrations")]
async fn tc_req_006_5_nonexistent_project_id_returns_404(pool: PgPool) {
    let app = app(test_state(pool).await);
    let response = app
        .oneshot(
            Request::builder()
                .uri(format!("/api/v1/projects/{}/timeline", Uuid::new_v4()))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

/// TC-REQ-006-6 (backend half): DB unavailability returns 503.
/// `pool.close()` puts the pool into a real closed state — subsequent
/// queries fail immediately with `sqlx::Error::PoolClosed`, no live DB
/// outage needed (same technique used for REQ-005's retry tests).
#[sqlx::test(migrations = "./migrations")]
async fn tc_req_006_6_db_unavailability_returns_503(pool: PgPool) {
    let project_id = seed_project(&pool, "6 outage way", "residential").await;
    pool.close().await;

    let app = app(test_state(pool).await);
    let response = app
        .oneshot(
            Request::builder()
                .uri(format!("/api/v1/projects/{project_id}/timeline"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    let body = http_body_util::BodyExt::collect(response.into_body())
        .await
        .unwrap()
        .to_bytes();
    assert!(body.is_empty());
}

/// TC-REQ-006-6 (UI half): a database outage on the project-detail page
/// (`GET /projects/{id}`) returns 503 and renders an accessible retry state.
/// Exercises the real handler end to end, not just the template in isolation.
#[sqlx::test(migrations = "./migrations")]
async fn tc_req_006_6_db_unavailability_renders_retry_ui(pool: PgPool) {
    let project_id = seed_project(&pool, "6b outage crescent", "residential").await;
    pool.close().await;

    let app = app(test_state(pool).await);
    let response = app
        .oneshot(
            Request::builder()
                .uri(format!("/projects/{project_id}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    let body = http_body_util::BodyExt::collect(response.into_body())
        .await
        .unwrap()
        .to_bytes();
    let html = String::from_utf8(body.to_vec()).unwrap();
    assert!(html.contains("Retry timeline"));
    assert!(html.contains("role=\"alert\""));
}

/// IMP-REQ-006-04/-08: the project-detail page renders the empty state
/// (not the loading or error state) for a project with zero timeline events.
#[sqlx::test(migrations = "./migrations")]
async fn imp_req_006_04_project_detail_page_renders_empty_state(pool: PgPool) {
    let project_id = seed_project(&pool, "8 empty crescent", "residential").await;

    let app = app(test_state(pool).await);
    let response = app
        .oneshot(
            Request::builder()
                .uri(format!("/projects/{project_id}"))
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
    let html = String::from_utf8(body.to_vec()).unwrap();
    assert!(html.contains("No timeline events have been recorded for this project yet."));
    assert!(!html.contains("timeline-error"));
}

/// IMP-REQ-006-04/-08: a project with events renders them in the loaded state.
#[sqlx::test(migrations = "./migrations")]
async fn imp_req_006_04_project_detail_page_renders_loaded_state(pool: PgPool) {
    let project_id = seed_project(&pool, "9 loaded loop", "commercial").await;
    let chunk_id = seed_document_chunk(&pool).await;
    let mention_id = insert_mention(&pool, chunk_id, "9 loaded loop", "commercial").await;
    seed_timeline_event(&pool, project_id, mention_id, chrono::Utc::now(), "approved").await;

    let app = app(test_state(pool).await);
    let response = app
        .oneshot(
            Request::builder()
                .uri(format!("/projects/{project_id}"))
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
    let html = String::from_utf8(body.to_vec()).unwrap();
    assert!(html.contains("approved"));
    assert!(html.contains("timeline-event"));
}

/// IMP-REQ-006-05: FR/EN strings render per `Accept-Language`.
#[sqlx::test(migrations = "./migrations")]
async fn imp_req_006_05_project_detail_page_renders_french_labels(pool: PgPool) {
    let project_id = seed_project(&pool, "10 rue vide", "residential").await;

    let app = app(test_state(pool).await);
    let response = app
        .oneshot(
            Request::builder()
                .uri(format!("/projects/{project_id}"))
                .header("accept-language", "fr-CA,fr;q=0.9")
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
    let html = String::from_utf8(body.to_vec()).unwrap();
    assert!(html.contains("<html lang=\"fr\">"));
    assert!(html.contains("Historique du projet"));
    assert!(html.contains("Aucun événement n’a encore été enregistré pour ce projet."));
}

/// IMP-REQ-006-07: resolver write is immediately visible via the timeline
/// endpoint (end-to-end across REQ-005 and REQ-006).
#[sqlx::test(migrations = "./migrations")]
async fn imp_req_006_07_resolver_write_reflected_in_timeline(pool: PgPool) {
    let chunk_id = seed_document_chunk(&pool).await;
    let mention_id = insert_mention(&pool, chunk_id, "7 wired st", "mixed-use").await;
    let outcome = resolve_mention(&pool, mention_id).await.unwrap();
    let project_id = match outcome {
        shovelsup_web::pipeline::resolver::ResolutionOutcome::NewProject { project_id } => project_id,
        other => panic!("expected NewProject, got {other:?}"),
    };

    let app = app(test_state(pool).await);
    let response = app
        .oneshot(
            Request::builder()
                .uri(format!("/api/v1/projects/{project_id}/timeline"))
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
    let events: Vec<Value> = serde_json::from_slice(&body).unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0]["project_mention_id"].as_str().unwrap(), mention_id.to_string());
}
