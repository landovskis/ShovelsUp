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
use shovelsup_pipeline::resolver::resolve_mention;
use shovelsup_web::{app, AppState};
use sqlx::postgres::PgPoolOptions;
use sqlx::PgPool;
use tower::ServiceExt;
use uuid::Uuid;

async fn test_state(pool: PgPool) -> AppState {
    let mut env = Environment::new();
    env.set_loader(path_loader("../templates"));
    let redis_client = redis::Client::open("redis://localhost:6380").unwrap();
    let redis = redis::aio::ConnectionManager::new(redis_client)
        .await
        .unwrap();
    AppState {
        env: std::sync::Arc::new(env),
        db: pool,
        redis,
        citation_db_override: None,
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
    seed_document_chunk_with_source_url(pool, "https://test-city.example/doc").await
}

/// Same as `seed_document_chunk` but with a caller-supplied `source_url`,
/// so REQ-006 tests can seed a "reliable" direct document URL vs. a
/// Montreal-style session-scoped portal URL (the latter being the
/// motivating example for `citation_url_reliable = false` in TC-006-2).
/// `source_documents` has no `citation_url_reliable`/`meeting_date`
/// columns yet (IMP-REQ-006-05's migration), so this seeds only what
/// today's schema supports.
async fn seed_document_chunk_with_source_url(pool: &PgPool, source_url: &str) -> Uuid {
    let municipality_id = sqlx::query_scalar!(
        "INSERT INTO municipalities (name, slug, domain_allowlist) \
         VALUES ('Test City', 'test-city', ARRAY['test-city.example']) RETURNING id"
    )
    .fetch_one(pool)
    .await
    .unwrap();
    let doc_id = sqlx::query_scalar!(
        "INSERT INTO source_documents (municipality_id, source_url, checksum, content, content_type) \
         VALUES ($1, $2, 'chk', ''::bytea, 'text/html') RETURNING id",
        municipality_id,
        source_url,
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

/// Same as `seed_document_chunk`, but records a caller-supplied
/// `document_chunks.language` (e.g. "fr"). Needed for TC-005-5
/// (description-language-divergence indicator): no code path anywhere
/// backfills `document_chunks.language`, and `seed_document_chunk`/
/// `seed_document_chunk_with_source_url` never set it (it stays NULL), so
/// without this helper TC-005-5's fixture could never produce a genuine
/// mismatch against the resolved UI locale — the same class of
/// test-fixture gap REQ-002/REQ-003 fixed with their own seed-helper
/// additions.
async fn seed_document_chunk_with_language(pool: &PgPool, language: &str) -> Uuid {
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
        municipality_id,
    )
    .fetch_one(pool)
    .await
    .unwrap();
    sqlx::query_scalar!(
        "INSERT INTO document_chunks (source_document_id, chunk_index, content, language) \
         VALUES ($1, 0, 'chunk text', $2) RETURNING id",
        doc_id,
        language,
    )
    .fetch_one(pool)
    .await
    .unwrap()
}

async fn insert_mention(
    pool: &PgPool,
    chunk_id: Uuid,
    civic_address: &str,
    project_type: &str,
) -> Uuid {
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
    seed_timeline_event(
        &pool,
        project_id,
        m2,
        base + chrono::Duration::days(2),
        "approved",
    )
    .await;
    seed_timeline_event(&pool, project_id, m1, base, "proposed").await;
    seed_timeline_event(
        &pool,
        project_id,
        m3,
        base + chrono::Duration::days(5),
        "deferred",
    )
    .await;

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
    let ids: Vec<String> = events
        .iter()
        .map(|e| e["id"].as_str().unwrap().to_string())
        .collect();
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
    seed_timeline_event(
        &pool,
        project_id,
        mention_id,
        chrono::Utc::now(),
        "approved",
    )
    .await;

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
        shovelsup_pipeline::resolver::ResolutionOutcome::NewProject { project_id } => project_id,
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
    assert_eq!(
        events[0]["project_mention_id"].as_str().unwrap(),
        mention_id.to_string()
    );
}

// ---------------------------------------------------------------------
// REQ-005 Loop A: project-detail descriptive fields (description,
// confidence_level, source_document_url, description_lang).
//
// `projects` has no `description_lang`/`confidence_level`/
// `source_document_url` columns yet — that migration is IMP-REQ-005-05's
// job, not this pass's. TC-005-1/-2/-5 below seed only what today's schema
// supports and assert on stable ids (`#project-description`,
// `#confidence-notice`, `#source-document-link`,
// `#description-language-notice`) that `project_detail.html` must render
// once those fields exist. They are EXPECTED TO FAIL right now, for the
// documented reason that the template doesn't render these elements yet —
// see `ProjectDetailContext` in `web/src/routes/projects.rs` for the stub
// Loop B must wire up. TC-005-3/-4 are regression guards against already-
// working behavior and are expected to PASS today.
// ---------------------------------------------------------------------

/// TC-005-1: happy path — a project with a timeline (i.e. as "fully
/// populated" as today's schema allows) must, once IMP-REQ-005-05/-06
/// land, render a description, a confidence notice, and a source-document
/// link, each behind a stable id. Currently FAILS: `project_detail.html`
/// has no such elements yet.
#[sqlx::test(migrations = "./migrations")]
async fn tc_005_1_happy_path_all_optional_fields_render(pool: PgPool) {
    let project_id = seed_project(&pool, "100 fulldata ave", "residential").await;
    let chunk_id = seed_document_chunk(&pool).await;
    let mention_id = insert_mention(&pool, chunk_id, "100 fulldata ave", "residential").await;
    seed_timeline_event(
        &pool,
        project_id,
        mention_id,
        chrono::Utc::now(),
        "approved",
    )
    .await;

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

    assert!(
        html.contains(r#"id="project-description""#),
        "expected a #project-description element once IMP-REQ-005-05/-06 land"
    );
    assert!(
        html.contains(r#"id="confidence-notice""#),
        "expected a #confidence-notice element once IMP-REQ-005-05/-06 land"
    );
    assert!(
        html.contains(r#"id="source-document-link""#),
        "expected a #source-document-link element once IMP-REQ-005-05/-06 land"
    );
}

/// TC-005-2: graceful degradation — a project with no document chunk /
/// mention / timeline event seeded (today's closest available proxy for
/// "description and source_document_url absent", since those columns
/// don't exist yet to null out independently) must still return 200 OK,
/// must not leak raw Jinja syntax or a literal "None" for the missing
/// pieces, must omit the description/source-document elements entirely,
/// yet must still render the confidence notice (per IMP-REQ-005-05,
/// confidence_level is independent of description/source_document_url).
/// Independent seed data from TC-005-1. Currently FAILS: the
/// confidence-notice assertion fails because the element doesn't exist
/// yet (the negative assertions already hold today, trivially, since none
/// of the new elements exist regardless of data).
#[sqlx::test(migrations = "./migrations")]
async fn tc_005_2_graceful_degradation_missing_fields(pool: PgPool) {
    let project_id = seed_project(&pool, "200 partialdata rd", "commercial").await;

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

    assert!(
        !html.contains("{{"),
        "template must not leak raw Jinja syntax when optional fields are missing"
    );
    assert!(
        !html.contains(">None<"),
        "missing optional fields must be omitted entirely, not rendered as literal None"
    );
    assert!(
        !html.contains(r#"id="project-description""#),
        "description element must be omitted entirely when no description is available"
    );
    assert!(
        !html.contains(r#"id="source-document-link""#),
        "source-document-link element must be omitted entirely when no source url is available"
    );
    assert!(
        html.contains(r#"id="confidence-notice""#),
        "expected a #confidence-notice element once IMP-REQ-005-05/-06 land, \
         even when description/source_document_url are absent"
    );
}

/// TC-005-3 (regression guard): a malformed UUID in the `/projects/{id}`
/// path is rejected with 400 by Axum's `Path<Uuid>` extractor before the
/// handler runs. Locks in already-working behavior.
#[sqlx::test(migrations = "./migrations")]
async fn tc_005_3_malformed_uuid_returns_400(pool: PgPool) {
    let app = app(test_state(pool).await);
    let response = app
        .oneshot(
            Request::builder()
                .uri("/projects/not-a-uuid")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

/// TC-005-4 (regression guard): a well-formed but nonexistent project id
/// returns 404. Locks in already-working behavior.
#[sqlx::test(migrations = "./migrations")]
async fn tc_005_4_nonexistent_project_returns_404(pool: PgPool) {
    let app = app(test_state(pool).await);
    let response = app
        .oneshot(
            Request::builder()
                .uri(format!("/projects/{}", Uuid::new_v4()))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

/// TC-005-4 (regression guard): DB unavailability on `/projects/{id}`
/// returns 503 with the rendered error page body (retry affordance).
/// Locks in already-working behavior (same technique as
/// `tc_req_006_6_db_unavailability_renders_retry_ui`: `pool.close()`
/// forces subsequent queries to fail immediately with `PoolClosed`).
#[sqlx::test(migrations = "./migrations")]
async fn tc_005_4_db_unavailable_returns_503_with_error_page(pool: PgPool) {
    let project_id = seed_project(&pool, "400 outage terrace", "residential").await;
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

/// TC-005-5: locale/source-language divergence — a project rendered under
/// the default (English, no `Accept-Language` header) UI locale must,
/// once IMP-REQ-005-05/-06 land and `description_lang` is populated and
/// differs from the resolved UI `lang`, surface an explicit
/// `#description-language-notice` indicator rather than silently
/// displaying foreign-language text with no cue. Currently FAILS: no such
/// element exists yet.
#[sqlx::test(migrations = "./migrations")]
async fn tc_005_5_description_language_divergence_indicator(pool: PgPool) {
    let project_id = seed_project(&pool, "500 languedivergente st", "institutional").await;
    // "fr" so it diverges from the resolved UI locale below ("en", no
    // Accept-Language header) — see `seed_document_chunk_with_language`'s
    // doc comment for why this dedicated helper exists.
    let chunk_id = seed_document_chunk_with_language(&pool, "fr").await;
    let mention_id =
        insert_mention(&pool, chunk_id, "500 languedivergente st", "institutional").await;
    seed_timeline_event(
        &pool,
        project_id,
        mention_id,
        chrono::Utc::now(),
        "proposed",
    )
    .await;

    let app = app(test_state(pool).await);
    let response = app
        .oneshot(
            Request::builder()
                .uri(format!("/projects/{project_id}"))
                // No accept-language header: resolves to the "en" default
                // per `detect_lang`, against a (future) French description.
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

    assert!(
        html.contains(r#"id="description-language-notice""#),
        "expected a #description-language-notice element once IMP-REQ-005-05/-06 land \
         and description_lang diverges from the resolved UI locale"
    );
}

/// IMP-REQ-005-09: the four REQ-005 detail fields
/// (`#project-description`, `#confidence-notice`,
/// `#description-language-notice`, `#source-document-link`) must be
/// wrapped in the `.project-detail-fields` container so the responsive
/// CSS in `static/css/main.css` (640px breakpoint, same convention as
/// IMP-REQ-002-07/003-07/004-08) can target and style them as a group
/// instead of leaving them as bare, unstyled paragraphs directly under
/// `.project-detail`.
#[sqlx::test(migrations = "./migrations")]
async fn imp_req_005_09_detail_fields_have_responsive_wrapper_class(pool: PgPool) {
    let project_id = seed_project(&pool, "700 wrapper class blvd", "residential").await;
    let chunk_id = seed_document_chunk_with_language(&pool, "fr").await;
    let mention_id = insert_mention(&pool, chunk_id, "700 wrapper class blvd", "residential").await;
    seed_timeline_event(
        &pool,
        project_id,
        mention_id,
        chrono::Utc::now(),
        "approved",
    )
    .await;

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

    let wrapper_start = html
        .find(r#"<div class="project-detail-fields">"#)
        .expect("expected a .project-detail-fields wrapper div around the REQ-005 detail fields");
    let wrapper_end = html[wrapper_start..]
        .find("</div>")
        .map(|offset| wrapper_start + offset)
        .expect("expected the .project-detail-fields wrapper div to be closed");
    let wrapper_html = &html[wrapper_start..wrapper_end];

    for id in [
        "project-description",
        "confidence-notice",
        "description-language-notice",
        "source-document-link",
    ] {
        assert!(
            wrapper_html.contains(&format!(r#"id="{id}""#)),
            "expected #{id} to be nested inside the .project-detail-fields \
             wrapper (so the responsive CSS rules apply to it), got wrapper: {wrapper_html}"
        );
    }
}

/// IMP-REQ-005-14: manual accessibility/responsive pass, encoded as an
/// assertion. Confirms: (1) `#source-document-link`'s visible text is
/// descriptive ("View source document"), not a non-descriptive phrase
/// like "click here"; (2) `#confidence-notice` and
/// `#description-language-notice` convey their meaning through visible
/// text content, not color alone (no `style="color"` on those elements
/// and no reliance on a bare icon/symbol); (3) the new REQ-005 fields do
/// not introduce a competing `<h1>`/`<h2>`, so `#project-title` remains
/// the page's only `<h1>` and `#timeline-title` remains the only `<h2>`.
#[sqlx::test(migrations = "./migrations")]
async fn imp_req_005_14_detail_fields_are_accessible(pool: PgPool) {
    let project_id = seed_project(&pool, "800 accessible way", "residential").await;
    let chunk_id = seed_document_chunk_with_language(&pool, "fr").await;
    let mention_id = insert_mention(&pool, chunk_id, "800 accessible way", "residential").await;
    seed_timeline_event(
        &pool,
        project_id,
        mention_id,
        chrono::Utc::now(),
        "approved",
    )
    .await;

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

    // (1) descriptive link text, not "click here".
    let link_start = html
        .find(r#"id="source-document-link""#)
        .expect("expected a #source-document-link element");
    let tag_close = html[link_start..]
        .find('>')
        .map(|offset| link_start + offset + 1)
        .expect("expected the source-document-link opening tag to close");
    let text_end = html[tag_close..]
        .find("</a>")
        .map(|offset| tag_close + offset)
        .expect("expected a closing </a> for source-document-link");
    let link_text = html[tag_close..text_end].trim();
    assert!(
        !link_text.is_empty(),
        "source-document-link must have non-empty visible link text"
    );
    assert!(
        !link_text.to_lowercase().contains("click here"),
        "source-document-link text must be descriptive, not 'click here', got: {link_text}"
    );

    // (2) confidence-notice / description-language-notice convey meaning
    // via text, not color alone: no inline color styling on either
    // element, and each renders non-empty text content.
    for id in ["confidence-notice", "description-language-notice"] {
        let start = html
            .find(&format!(r#"id="{id}""#))
            .unwrap_or_else(|| panic!("expected a #{id} element"));
        let tag_close = html[start..]
            .find('>')
            .map(|offset| start + offset + 1)
            .unwrap_or_else(|| panic!("expected the #{id} opening tag to close"));
        let text_end = html[tag_close..]
            .find("</p>")
            .map(|offset| tag_close + offset)
            .unwrap_or_else(|| panic!("expected a closing </p> for #{id}"));
        let element_text = html[tag_close..text_end].trim();
        assert!(
            !element_text.is_empty(),
            "#{id} must convey its meaning through visible text content"
        );

        let tag_start = html[..start].rfind('<').unwrap();
        let opening_tag = &html[tag_start..tag_close];
        assert!(
            !opening_tag.contains("style=") || !opening_tag.contains("color"),
            "#{id} must not rely on inline color styling as the sole cue, got tag: {opening_tag}"
        );
    }

    // (3) heading hierarchy: exactly one <h1> (#project-title) and one
    // <h2> (#timeline-title); the new paragraph-level REQ-005 fields must
    // not introduce a competing heading.
    let h1_count = html.matches("<h1").count();
    let h2_count = html.matches("<h2").count();
    assert_eq!(
        h1_count, 1,
        "expected exactly one <h1> (#project-title); REQ-005 fields must not add another, got html: {html}"
    );
    assert_eq!(
        h2_count, 1,
        "expected exactly one <h2> (#timeline-title); REQ-005 fields must not add another, got html: {html}"
    );
    assert!(
        html.contains(r#"<h1 id="project-title""#),
        "expected the sole <h1> to remain #project-title"
    );
    assert!(
        html.contains(r#"<h2 id="timeline-title""#),
        "expected the sole <h2> to remain #timeline-title"
    );
}

// ---------------------------------------------------------------------
// REQ-006: source transparency notice (citation section).
//
// IMP-REQ-006-02/-03/-04/-05/-06 (done): migration 021 added
// `source_documents.meeting_date`; `routes/projects.rs`'s `core` module has
// a pure `resolve_citation_view` decision (URL-shape reliability heuristic,
// "Document retrieved" fallback, "no source document" omission);
// `fetch_primary_citation` joins through to the primary source document;
// and `get_project_detail_page` wires all of it into `project_detail.html`,
// rendering a `#project-source` section. TC-006-1/-2/-3/-4 all pass.
// TC-006-5 required a test-design fix — see its own doc comment for why
// `pool.close()` (this suite's usual full-outage technique) can't express
// an isolated single-query failure, and what was used instead.
// ---------------------------------------------------------------------

/// TC-006-1: a project whose source document has a reliable, real
/// `source_url` renders a clickable hyperlink citation (including the
/// `meeting_date`, once present) inside a `#project-source` element.
#[sqlx::test(migrations = "./migrations")]
async fn tc_006_1_reliable_source_renders_hyperlink_citation(pool: PgPool) {
    let project_id = seed_project(&pool, "600 reliable source ave", "residential").await;
    let chunk_id =
        seed_document_chunk_with_source_url(&pool, "https://test-city.example/reliable-doc")
            .await;
    let mention_id = insert_mention(&pool, chunk_id, "600 reliable source ave", "residential")
        .await;
    seed_timeline_event(
        &pool,
        project_id,
        mention_id,
        chrono::Utc::now(),
        "approved",
    )
    .await;

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

    assert!(
        html.contains(r#"id="project-source""#),
        "expected a #project-source citation element once IMP-REQ-006-05/-06/-08 land"
    );
    assert!(
        html.contains(r#"href="https://test-city.example/reliable-doc""#),
        "expected a clickable hyperlink to the reliable source_url once wired up"
    );
}

/// TC-006-2: a project whose source document's URL is unreliable by shape
/// (e.g. Montreal's session-scoped portal URL pattern, where the URL is
/// only valid for the duration of the browsing session and would mislead
/// readers if hyperlinked) renders citation-only text (municipality +
/// meeting date) and NOT a hyperlink, even though a `source_url` value
/// exists in the row. Independent seed data from TC-006-1 (distinct
/// address, distinct session-scoped URL shape).
#[sqlx::test(migrations = "./migrations")]
async fn tc_006_2_unreliable_source_renders_citation_text_only(pool: PgPool) {
    let project_id = seed_project(&pool, "601 session scoped blvd", "commercial").await;
    let chunk_id = seed_document_chunk_with_source_url(
        &pool,
        "https://montreal.ca/portal/session/8f3c1?token=ephemeral",
    )
    .await;
    let mention_id = insert_mention(&pool, chunk_id, "601 session scoped blvd", "commercial")
        .await;
    seed_timeline_event(
        &pool,
        project_id,
        mention_id,
        chrono::Utc::now(),
        "proposed",
    )
    .await;

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

    assert!(
        html.contains(r#"id="project-source""#),
        "expected a #project-source citation element once IMP-REQ-006-05/-06/-08 land"
    );
    assert!(
        html.contains("Test City"),
        "expected the municipality name to appear in the citation-only text"
    );
    assert!(
        !html.contains("https://montreal.ca/portal/session/8f3c1?token=ephemeral"),
        "an unreliable source_url must never be rendered as a hyperlink target, \
         even as citation text"
    );
}

/// TC-006-3: a project whose source document has no `meeting_date` (the
/// column is nullable with no backfill, per migration 021) falls back to a
/// "Document retrieved" message instead of a date, without breaking the
/// rest of the page. Independent seed data from TC-006-1/-2.
#[sqlx::test(migrations = "./migrations")]
async fn tc_006_3_missing_meeting_date_falls_back_to_document_retrieved(pool: PgPool) {
    let project_id = seed_project(&pool, "602 no meeting date way", "institutional").await;
    let chunk_id =
        seed_document_chunk_with_source_url(&pool, "https://test-city.example/undated-doc").await;
    let mention_id =
        insert_mention(&pool, chunk_id, "602 no meeting date way", "institutional").await;
    seed_timeline_event(
        &pool,
        project_id,
        mention_id,
        chrono::Utc::now(),
        "deferred",
    )
    .await;

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

    assert!(
        html.contains(r#"id="project-source""#),
        "expected a #project-source citation element once IMP-REQ-006-05/-06/-08 land"
    );
    assert!(
        html.contains("Document retrieved"),
        "expected a 'Document retrieved' fallback when meeting_date is absent"
    );
}

/// TC-006-4: a project with NO associated source document at all (no
/// document chunk, no mention — the edge case) must omit the citation
/// section entirely rather than erroring; the page must still render 200.
/// The assertions below are written to be meaningful (a real 200 response,
/// and an explicit absence of citation-related error text) rather than a
/// no-op that would pass regardless of behavior.
#[sqlx::test(migrations = "./migrations")]
async fn tc_006_4_no_source_document_omits_citation_section(pool: PgPool) {
    let project_id = seed_project(&pool, "603 no source document cres", "residential").await;

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

    assert!(
        !html.contains(r#"id="project-source""#),
        "a project with no source document must not render a #project-source element"
    );
    assert!(
        !html.contains("citation-error"),
        "a project with no source document must not surface a citation error state"
    );
}

/// TC-006-5: a DB failure specifically on the citation lookup query must
/// be isolated from the rest of the project detail page — the citation
/// query failing must NOT take down the whole page with a 503.
///
/// ⚠️ Test-design conflict, found and fixed (IMP-REQ-006-06): the plan's
/// original version of this test used the same `pool.close()` technique as
/// `tc_req_006_6_db_unavailability_renders_retry_ui`/`tc_005_4_...` to
/// simulate a DB outage. That technique is structurally incompatible with
/// what this test is trying to prove: `pool.close()` closes the *entire*
/// pool, so EVERY query issued against it fails — including
/// `get_project_detail_page`'s project-existence check, which runs before
/// the citation query and, correctly, already 503s the whole page on a
/// full outage (that's exactly what TC-REQ-006-6/TC-005-4 pin down, and
/// must keep doing). There is no way to make a single shared `PgPool`
/// simultaneously "down for the citation query" and "up for everything
/// else" — closing it is an all-or-nothing operation. Satisfying the
/// original assertion (`StatusCode::OK` after `pool.close()`) would have
/// required either not closing the pool at all (testing nothing) or making
/// `get_project_detail_page` swallow full-outage errors on the
/// project-existence query too (breaking the correct, already-tested
/// TC-REQ-006-6/TC-005-4 full-outage-503 behavior). Neither is acceptable,
/// so this test is rewritten to inject the fault differently: a *second*,
/// independently-connected `PgPool` to the same test database is created
/// (via `pool.connect_options()`, reusing the same connection string the
/// `sqlx::test` fixture already established) and set as
/// `AppState::citation_db_override` — a test-only hook
/// (`web/src/lib.rs`/`web/src/routes/projects.rs`) that the citation query
/// alone uses in place of `db` when present. Closing *that* second pool
/// fails only the citation query; `pool` (and therefore every other query
/// the handler runs) stays live, so this test now genuinely exercises
/// "citation query down, rest of the page up" instead of "everything down".
#[sqlx::test(migrations = "./migrations")]
async fn tc_006_5_citation_query_failure_isolated_from_page(pool: PgPool) {
    let project_id = seed_project(&pool, "604 isolated failure pl", "commercial").await;
    let chunk_id =
        seed_document_chunk_with_source_url(&pool, "https://test-city.example/doomed-doc").await;
    let mention_id = insert_mention(&pool, chunk_id, "604 isolated failure pl", "commercial")
        .await;
    seed_timeline_event(
        &pool,
        project_id,
        mention_id,
        chrono::Utc::now(),
        "approved",
    )
    .await;

    // Second, independent pool to the same database — closing it fails only
    // the citation query, unlike `pool.close()` which would fail every
    // query (see the doc comment above for why that's the wrong tool here).
    let citation_pool = PgPoolOptions::new()
        .max_connections(1)
        .connect_with((*pool.connect_options()).clone())
        .await
        .unwrap();
    citation_pool.close().await;

    let mut state = test_state(pool).await;
    state.citation_db_override = Some(citation_pool);

    let app = app(state);
    let response = app
        .oneshot(
            Request::builder()
                .uri(format!("/projects/{project_id}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(
        response.status(),
        StatusCode::OK,
        "a citation-query-only DB failure must not 503 the whole page"
    );
    let body = http_body_util::BodyExt::collect(response.into_body())
        .await
        .unwrap()
        .to_bytes();
    let html = String::from_utf8(body.to_vec()).unwrap();
    assert!(
        html.contains("timeline-event") || html.contains("citation-error"),
        "expected the rest of the page to render normally with a graceful citation \
         fallback/error state, got: {html}"
    );
    assert!(
        !html.contains(r#"id="project-source""#),
        "the citation section must be omitted (not rendered with stale/error data) \
         when the isolated citation query fails, got: {html}"
    );
}

/// IMP-REQ-006-09: accessibility pass on the citation section. The
/// distinction between "reliable" and "unreliable" sources must be
/// structural (`<a>` vs. plain `<span>`), never conveyed by color alone,
/// and the hyperlink's link text must be meaningful (the municipality
/// name) rather than a generic "click here"/"link" placeholder. This test
/// seeds one reliable-source project (TC-006-1-style) and one
/// unreliable-source project (TC-006-2-style) and asserts both properties
/// concretely against the rendered HTML.
#[sqlx::test(migrations = "./migrations")]
async fn tc_006_6_citation_link_text_is_meaningful_and_unreliable_is_not_a_link(pool: PgPool) {
    // Reliable citation: expect a real `<a>` whose visible text is the
    // municipality name, not placeholder text like "click here"/"link".
    let reliable_project_id =
        seed_project(&pool, "605 accessible reliable ave", "residential").await;
    let reliable_chunk_id = seed_document_chunk_with_source_url(
        &pool,
        "https://test-city.example/accessible-reliable-doc",
    )
    .await;
    let reliable_mention_id = insert_mention(
        &pool,
        reliable_chunk_id,
        "605 accessible reliable ave",
        "residential",
    )
    .await;
    seed_timeline_event(
        &pool,
        reliable_project_id,
        reliable_mention_id,
        chrono::Utc::now(),
        "approved",
    )
    .await;

    let reliable_app = app(test_state(pool.clone()).await);
    let response = reliable_app
        .oneshot(
            Request::builder()
                .uri(format!("/projects/{reliable_project_id}"))
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

    let link_open = r#"<a id="citation-link" href="https://test-city.example/accessible-reliable-doc">"#;
    let link_start = html
        .find(link_open)
        .unwrap_or_else(|| panic!("expected a citation hyperlink in: {html}"));
    let after_open = &html[link_start + link_open.len()..];
    let link_close = after_open
        .find("</a>")
        .unwrap_or_else(|| panic!("expected a closing </a> after the citation link in: {html}"));
    let link_text = &after_open[..link_close];

    assert_eq!(
        link_text, "Test City",
        "citation link text must be the real municipality name, not a generic placeholder"
    );
    let lowercase_link_text = link_text.to_lowercase();
    assert!(
        !lowercase_link_text.contains("click here") && !lowercase_link_text.contains("link"),
        "citation link text must not be generic \"click here\"/\"link\" placeholder text, \
         got: {link_text}"
    );

    // Unreliable citation: expect a plain, non-clickable element — no
    // `<a href>` wrapping the unreliable URL anywhere on the page, so the
    // reliable/unreliable distinction stays structural rather than a
    // color-only cue on an otherwise-identical-looking link. Seeded with a
    // distinct municipality name (rather than reusing
    // `seed_document_chunk_with_source_url`'s hardcoded "Test City") since
    // `municipalities.name` is unique and the reliable case above already
    // inserted "Test City" into this same pool/database.
    let unreliable_project_id =
        seed_project(&pool, "606 accessible unreliable blvd", "commercial").await;
    let unreliable_municipality_id = sqlx::query_scalar!(
        "INSERT INTO municipalities (name, slug, domain_allowlist) \
         VALUES ('Test City Two', 'test-city-two', ARRAY['test-city-two.example']) RETURNING id"
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    let unreliable_doc_id = sqlx::query_scalar!(
        "INSERT INTO source_documents (municipality_id, source_url, checksum, content, content_type) \
         VALUES ($1, $2, 'chk', ''::bytea, 'text/html') RETURNING id",
        unreliable_municipality_id,
        "https://montreal.ca/portal/session/9a2b7?token=ephemeral",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    let unreliable_chunk_id = sqlx::query_scalar!(
        "INSERT INTO document_chunks (source_document_id, chunk_index, content) \
         VALUES ($1, 0, 'chunk text') RETURNING id",
        unreliable_doc_id
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    let unreliable_mention_id = insert_mention(
        &pool,
        unreliable_chunk_id,
        "606 accessible unreliable blvd",
        "commercial",
    )
    .await;
    seed_timeline_event(
        &pool,
        unreliable_project_id,
        unreliable_mention_id,
        chrono::Utc::now(),
        "proposed",
    )
    .await;

    let unreliable_app = app(test_state(pool).await);
    let response = unreliable_app
        .oneshot(
            Request::builder()
                .uri(format!("/projects/{unreliable_project_id}"))
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

    assert!(
        html.contains(r#"<span id="citation-text">Test City Two</span>"#),
        "unreliable citation must render as a plain, non-clickable <span>, got: {html}"
    );
    assert!(
        !html.contains("<a") || !html.contains("montreal.ca/portal/session/9a2b7"),
        "the unreliable source URL must never be wrapped in an <a href>, got: {html}"
    );
}

// ---------------------------------------------------------------------
// REQ-015 Loop A (detail-page half): search-result confidence indicator
// ("Detected N days ago from M council source(s)"), also required on the
// project-detail page per TC-015-2 (not just the search card — see
// `tests/search_integration.rs`'s `tc_015_*` tests for the search-card
// halves of TC-015-1/-3/-4/-5/-6).
//
// `public_search_documents` has neither `first_detected_at` nor
// `source_count` yet (IMP-REQ-015-02/03/04's migration), and
// `ProjectDetailContext` in `web/src/routes/projects.rs` carries them as
// Loop A stub fields that `get_project_detail_page` never populates or
// threads into `project_detail.html` — so this is EXPECTED TO FAIL today.
// ---------------------------------------------------------------------

/// TC-015-2: the confidence indicator ("Detected N days ago from M council
/// source(s)") renders on the project-detail page, not just the search
/// results card. Seeds a project with two distinct source documents/chunks
/// (the future "distinct council source" signal), documenting the target
/// N=5/M=2 contract used consistently with `tc_015_1_...` in
/// `search_integration.rs`. Currently FAILS: `get_project_detail_page` has
/// no query populating `first_detected_at`/`source_count`, and
/// `project_detail.html` has no rendering for the indicator at all yet.
#[sqlx::test(migrations = "./migrations")]
async fn tc_015_2_project_detail_page_shows_days_and_source_count(pool: PgPool) {
    let project_id = seed_project(&pool, "700 rue detection detail", "residential").await;
    let chunk_id_one =
        seed_document_chunk_with_source_url(&pool, "https://test-city.example/detail-doc-one")
            .await;
    let mention_id =
        insert_mention(&pool, chunk_id_one, "700 rue detection detail", "residential").await;
    seed_timeline_event(
        &pool,
        project_id,
        mention_id,
        chrono::Utc::now(),
        "approved",
    )
    .await;

    // Second distinct source document, feeding the same project via a
    // second project_mention — the future "distinct council source" signal
    // that would drive source_count = 2 once IMP-REQ-015 lands.
    let chunk_id_two =
        seed_document_chunk_with_source_url(&pool, "https://test-city.example/detail-doc-two")
            .await;
    insert_mention(&pool, chunk_id_two, "700 rue detection detail", "residential").await;

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

    assert!(
        html.contains("Detected 5 days ago from 2 council sources"),
        "expected the confidence indicator sentence on the project-detail \
         page once IMP-REQ-015-02/03/04/07/12 land and \
         first_detected_at/source_count are populated, got: {html}"
    );
}
