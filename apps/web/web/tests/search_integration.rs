//! IMP-REQ-008-06: automates TC-REQ-008-1..4 against the real
//! `GET /api/v1/projects/search` handler and `public_search_documents`
//! index (IMP-REQ-008-01/-02).

use axum::body::Body;
use axum::extract::connect_info::MockConnectInfo;
use axum::http::{Request, StatusCode};
use minijinja::{context, path_loader, Environment};
use serde_json::Value;
use shovelsup_web::jobs::public_search_refresh::refresh_public_search_index;
use shovelsup_web::{app, AppState};
use sqlx::PgPool;
use std::net::SocketAddr;
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

/// Like `seed_searchable_project`, but sets `document_chunks.language`
/// explicitly (TC-003-5). `seed_searchable_project` leaves it `NULL` (no
/// default on that column), so `public_search_documents.source_language`
/// stays `NULL` after any refresh — which is fine for tests that don't care
/// about it, but TC-003-5 specifically needs a document with a KNOWN,
/// asserted language (independent of the municipality's own name/locale) to
/// verify the per-result badge.
async fn seed_searchable_project_with_language(
    pool: &PgPool,
    civic_address_normalized: &str,
    municipality_name: &str,
    language: &str,
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
        "INSERT INTO document_chunks (source_document_id, chunk_index, content, language) \
         VALUES ($1, 0, 'chunk text', $2) RETURNING id",
        doc_id,
        language,
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

/// Like `seed_searchable_project`, but attaches the project to one of the
/// REAL, pre-seeded launch municipalities (migration 002: slugs
/// `montreal`/`toronto`/`vancouver`) instead of creating a fresh
/// randomly-slugged municipality row.
///
/// `seed_searchable_project` can't be used for `municipality_slug`-filter
/// tests (TC-002-1/-3/-6): it always inserts a brand-new municipality with
/// slug `slug-{uuid}`, so a query for the real slug `montreal` would never
/// match anything it seeds — the query and the fixture were talking about
/// two different municipality rows entirely. This helper looks up the real
/// municipality by its real slug (seeded by migration 002, present in every
/// test's migrated DB) and attaches the new project's document chain to it.
async fn seed_searchable_project_for_municipality_slug(
    pool: &PgPool,
    civic_address_normalized: &str,
    municipality_slug: &str,
) -> Uuid {
    let project_id = sqlx::query_scalar!(
        "INSERT INTO projects (civic_address_normalized, project_type) VALUES ($1, 'residential') RETURNING id",
        civic_address_normalized,
    )
    .fetch_one(pool)
    .await
    .unwrap();

    let municipality_id = sqlx::query_scalar!(
        "SELECT id FROM municipalities WHERE slug = $1",
        municipality_slug
    )
    .fetch_one(pool)
    .await
    .unwrap_or_else(|_| panic!("expected migration 002 to have seeded municipality slug '{municipality_slug}'"));

    // Real municipalities (unlike the fresh, always-unique ones
    // `seed_searchable_project` creates) are shared across multiple calls
    // within the same test, so `source_documents`'s
    // `(municipality_id, checksum)` UNIQUE constraint requires a distinct
    // checksum per call, not the literal `'chk'` constant.
    let suffix = Uuid::new_v4();
    let doc_id = sqlx::query_scalar!(
        "INSERT INTO source_documents (municipality_id, source_url, checksum, content, content_type) \
         VALUES ($1, $2, $3, ''::bytea, 'text/html') RETURNING id",
        municipality_id,
        format!("https://{suffix}.example/doc"),
        suffix.to_string(),
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

/// Loop A helper for TC-008-1/-2: like `seed_searchable_project` but lets the
/// caller set `project_type` (including `None`), used as a stand-in signal
/// for the future `category_code` column (TODO(IMP-REQ-008-02): once
/// `projects.category_code` exists, seed/query that column directly instead).
async fn seed_searchable_project_with_type(
    pool: &PgPool,
    civic_address_normalized: &str,
    municipality_name: &str,
    project_type: Option<&str>,
) -> Uuid {
    let project_id = sqlx::query_scalar!(
        "INSERT INTO projects (civic_address_normalized, project_type) VALUES ($1, $2) RETURNING id",
        civic_address_normalized,
        project_type,
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
         VALUES ($1, $2, true, $3, $4, 1, 'approved')",
        chunk_id,
        project_id,
        civic_address_normalized,
        project_type,
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
    let envelope: Value = serde_json::from_slice(&body).unwrap();
    let results = envelope["results"]
        .as_array()
        .unwrap_or_else(|| panic!("expected a 'results' array field, got: {envelope:?}"));
    assert_eq!(results.len(), 1);
    assert_eq!(
        results[0]["project_id"].as_str().unwrap(),
        project_id.to_string()
    );
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
    let envelope: Value = serde_json::from_slice(&body).unwrap();
    let results = envelope["results"]
        .as_array()
        .unwrap_or_else(|| panic!("expected a 'results' array field, got: {envelope:?}"));
    assert_eq!(
        results.len(),
        1,
        "must match on municipality_name even though the keyword isn't in the civic address"
    );
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

/// Builds a unique loopback `SocketAddr` per test run so parallel
/// `#[sqlx::test]` cases (which share one Redis instance) don't collide on
/// the same rate-limit bucket. Since IMP-REQ-001-05, this — not
/// `X-Forwarded-For` — is what actually determines the bucket, so tests
/// simulate distinct clients by layering `MockConnectInfo` with distinct
/// addresses rather than by varying a header.
fn unique_peer_addr() -> SocketAddr {
    SocketAddr::from(([203, 0, 113, rand_octet()], 12345))
}

/// IMP-REQ-008-05: the 61st request within the rate-limit window from the
/// same IP returns 429.
#[sqlx::test(migrations = "./migrations")]
async fn imp_req_008_05_sixty_first_request_in_window_is_rate_limited(pool: PgPool) {
    std::env::set_var("RATE_LIMIT_SEARCH_RPM", "60");
    let app = app(test_state(pool).await).layer(MockConnectInfo(unique_peer_addr()));

    let mut last_status = StatusCode::OK;
    for _ in 0..61 {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/v1/projects/search?q=test")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        last_status = response.status();
    }

    assert_eq!(last_status, StatusCode::TOO_MANY_REQUESTS);
}

/// IMP-REQ-001-05: a client cannot evade the rate limit by forging a fresh
/// `X-Forwarded-For` value on every request. All 61 requests share the same
/// real connection (`ConnectInfo`) but each carries a distinct, forged
/// `X-Forwarded-For` header; if that header were still consulted, every
/// request would land in a different bucket and none would ever be
/// throttled. The 61st must still be rejected.
#[sqlx::test(migrations = "./migrations")]
async fn imp_req_001_05_forged_x_forwarded_for_does_not_bypass_the_limit(pool: PgPool) {
    std::env::set_var("RATE_LIMIT_SEARCH_RPM", "60");
    let app = app(test_state(pool).await).layer(MockConnectInfo(unique_peer_addr()));

    let mut last_status = StatusCode::OK;
    for i in 0..61 {
        let forged_ip = format!("198.51.100.{}", (i % 254) + 1);
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/v1/projects/search?q=test")
                    .header("x-forwarded-for", forged_ip)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        last_status = response.status();
    }

    assert_eq!(
        last_status,
        StatusCode::TOO_MANY_REQUESTS,
        "a distinct forged X-Forwarded-For per request must not let a single \
         real client evade the rate limit"
    );
}

/// IMP-REQ-001-05: a client cannot frame another IP by forging its address
/// in `X-Forwarded-For`. Two distinct real connections (`ConnectInfo`) both
/// send the same forged `X-Forwarded-For` value; if that header were
/// trusted, "victim"'s requests would count against "attacker"'s bucket (or
/// vice versa). Each real connection must get its own, independent budget.
#[sqlx::test(migrations = "./migrations")]
async fn imp_req_001_05_forged_x_forwarded_for_does_not_frame_another_ip(pool: PgPool) {
    std::env::set_var("RATE_LIMIT_SEARCH_RPM", "60");
    let state = test_state(pool).await;
    let shared_forged_ip = "192.0.2.99";

    let attacker_app = app(state.clone()).layer(MockConnectInfo(unique_peer_addr()));
    for _ in 0..60 {
        let response = attacker_app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/v1/projects/search?q=test")
                    .header("x-forwarded-for", shared_forged_ip)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }

    // "Victim" is a different real connection but sends the exact same
    // (forged) X-Forwarded-For the attacker just exhausted the limit under.
    let victim_app = app(state).layer(MockConnectInfo(unique_peer_addr()));
    let victim_response = victim_app
        .oneshot(
            Request::builder()
                .uri("/api/v1/projects/search?q=test")
                .header("x-forwarded-for", shared_forged_ip)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(
        victim_response.status(),
        StatusCode::OK,
        "a different real connection sharing a forged X-Forwarded-For value \
         must not inherit another client's exhausted rate-limit bucket"
    );
}

/// TC-001-6: `GET /search` with `Accept-Language: fr` renders French labels
/// and result content, and the server-rendered page never contains a
/// client-side modal/dialog element — this is a plain no-JS page regardless
/// of locale.
#[sqlx::test(migrations = "./migrations")]
async fn tc_001_6_french_locale_no_modal(pool: PgPool) {
    seed_searchable_project(&pool, "77 rue principale", "Ville de Rivière").await;
    refresh_public_search_index(&pool).await.unwrap();

    let app = app(test_state(pool).await);
    let response = app
        .oneshot(
            Request::builder()
                .uri("/search?q=principale")
                .header("accept-language", "fr")
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
        html.contains("Rechercher un projet"),
        "expected French heading in body, got: {html}"
    );
    assert!(
        html.contains("Adresse civique ou municipalité"),
        "expected French search label in body, got: {html}"
    );
    assert!(
        html.contains("77 rue principale"),
        "expected the matching result's civic address in body, got: {html}"
    );

    assert!(
        !html.contains("<dialog"),
        "no-JS search page must never contain a <dialog> element"
    );
    assert!(
        !html.contains("role=\"dialog\""),
        "no-JS search page must never contain a role=\"dialog\" element"
    );
    assert!(
        !html.contains("class=\"modal\""),
        "no-JS search page must never contain a class=\"modal\" element"
    );
}

/// TC-002-1: searching with a valid `municipality_slug` (`montreal`) returns
/// only that municipality's projects, excluding a project seeded under a
/// different municipality.
///
/// TODO(IMP-REQ-002-01): once `public_search_documents.municipality_slug`
/// exists, seed/query by the real column. For now this seeds the existing
/// free-text `municipality_name` column (using fixture names distinct from
/// the `Montreal`/`Toronto` rows migration 002 already seeds into
/// `municipalities`, since `seed_searchable_project` inserts its own
/// `municipalities` row per call and `name` is `UNIQUE`) and expects
/// `run_search` to apply a `municipality_slug=montreal` filter — which
/// IMP-REQ-002-04 has not wired up yet, so this currently fails (both
/// projects come back).
#[sqlx::test(migrations = "./migrations")]
async fn tc_002_1_valid_municipality_slug_returns_only_that_municipality(pool: PgPool) {
    let montreal_project = seed_searchable_project_for_municipality_slug(
        &pool,
        "1000 rue sainte-catherine",
        "montreal",
    )
    .await;
    seed_searchable_project_for_municipality_slug(&pool, "1000 yonge street", "toronto").await;
    refresh_public_search_index(&pool).await.unwrap();

    let app = app(test_state(pool).await);
    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/projects/search?q=1000&municipality_slug=montreal")
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
    let envelope: Value = serde_json::from_slice(&body).unwrap();
    let results = envelope["results"]
        .as_array()
        .unwrap_or_else(|| panic!("expected a 'results' array field, got: {envelope:?}"));
    assert_eq!(
        results.len(),
        1,
        "municipality_slug=montreal must exclude the Toronto project, got: {results:?}"
    );
    assert_eq!(
        results[0]["project_id"].as_str().unwrap(),
        montreal_project.to_string()
    );
}

/// TC-002-2: an unknown `municipality_slug` (not present in the
/// `municipalities` table) is rejected with 400, before any project query
/// runs.
///
/// This can't be expressed via `municipality_name` (there's no table to
/// validate against on that column) so it's written directly against the
/// `municipality_slug` stub field on `SearchParams`. It fails today because
/// IMP-REQ-002-03/-04 haven't added validation yet, so the handler currently
/// returns 200 for any slug value.
#[sqlx::test(migrations = "./migrations")]
async fn tc_002_2_unknown_municipality_slug_rejected_before_query(pool: PgPool) {
    let app = app(test_state(pool).await);
    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/projects/search?q=test&municipality_slug=nonexistent-city")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(
        response.status(),
        StatusCode::BAD_REQUEST,
        "an unknown municipality_slug must be rejected before any project query runs"
    );
}

/// TC-002-3: combining `q` (keyword) with `municipality_slug` applies both
/// filters (AND), narrowing results further than either alone.
///
/// TODO(IMP-REQ-002-01): seed/query by the real `municipality_slug` column
/// once it exists. This seeds two Montreal-municipality projects with
/// different civic addresses plus one Toronto project sharing the same
/// keyword, and expects `q=principale&municipality_slug=montreal` to return
/// only the single matching Montreal project — narrower than `q=principale`
/// alone (which would also match the Toronto project) and narrower than
/// `municipality_slug=montreal` alone (which would match both Montreal
/// projects). Fails today: no keyword+slug narrowing exists yet.
#[sqlx::test(migrations = "./migrations")]
async fn tc_002_3_keyword_and_municipality_slug_combine_with_and(pool: PgPool) {
    let matching =
        seed_searchable_project_for_municipality_slug(&pool, "500 rue principale", "montreal")
            .await;
    seed_searchable_project_for_municipality_slug(&pool, "600 avenue du parc", "montreal").await;
    seed_searchable_project_for_municipality_slug(&pool, "700 principale road", "toronto").await;
    refresh_public_search_index(&pool).await.unwrap();

    let app = app(test_state(pool).await);
    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/projects/search?q=principale&municipality_slug=montreal")
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
    let envelope: Value = serde_json::from_slice(&body).unwrap();
    let results = envelope["results"]
        .as_array()
        .unwrap_or_else(|| panic!("expected a 'results' array field, got: {envelope:?}"));
    assert_eq!(
        results.len(),
        1,
        "q + municipality_slug together must narrow to the single Montreal \
         project matching both filters, got: {results:?}"
    );
    assert_eq!(
        results[0]["project_id"].as_str().unwrap(),
        matching.to_string()
    );
}

/// TC-002-4: `municipality_slug` omitted entirely still returns all
/// municipalities' results — backward compatible with REQ-001/REQ-008's
/// plain keyword search.
#[sqlx::test(migrations = "./migrations")]
async fn tc_002_4_omitted_municipality_slug_is_backward_compatible(pool: PgPool) {
    seed_searchable_project(&pool, "12 rue de la gare", "Montreal District X").await;
    seed_searchable_project(&pool, "34 station street", "Toronto District X").await;
    refresh_public_search_index(&pool).await.unwrap();

    let app = app(test_state(pool).await);
    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/projects/search?q=gare")
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
    let envelope: Value = serde_json::from_slice(&body).unwrap();
    let results = envelope["results"]
        .as_array()
        .unwrap_or_else(|| panic!("expected a 'results' array field, got: {envelope:?}"));
    assert_eq!(
        results.len(),
        1,
        "plain keyword search without municipality_slug must keep matching \
         across all municipalities, got: {results:?}"
    );
}

/// TC-002-5: a valid municipality_slug with zero matching projects returns
/// an empty list, not an error.
///
/// TODO(IMP-REQ-002-01): seed/query by the real `municipality_slug` column
/// once it exists. Seeds a project under Toronto whose civic address matches
/// `q`, then filters by `municipality_slug=montreal`: the correct behavior
/// is an empty 200 response (no Montreal project matches), not an error.
/// Fails today because the slug filter isn't applied, so the Toronto
/// project is returned instead of an empty list.
#[sqlx::test(migrations = "./migrations")]
async fn tc_002_5_valid_slug_with_no_matches_returns_empty_list(pool: PgPool) {
    seed_searchable_project(&pool, "88 bay street", "Toronto Only District").await;
    refresh_public_search_index(&pool).await.unwrap();

    let app = app(test_state(pool).await);
    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/projects/search?q=bay+street&municipality_slug=montreal")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(
        response.status(),
        StatusCode::OK,
        "an empty-result municipality filter must not be an error"
    );
    let body = http_body_util::BodyExt::collect(response.into_body())
        .await
        .unwrap()
        .to_bytes();
    let envelope: Value = serde_json::from_slice(&body).unwrap();
    let results = envelope["results"]
        .as_array()
        .unwrap_or_else(|| panic!("expected a 'results' array field, got: {envelope:?}"));
    assert!(
        results.is_empty(),
        "montreal filter must exclude the Toronto-only project, got: {results:?}"
    );
}

/// TC-002-6: slug normalization — `Montreal` and `MONTREAL` are normalized
/// to match the canonical `montreal` slug (the chosen behavior; the
/// alternative of rejecting mixed-case input was considered and rejected
/// because slugs arrive from an HTML `<select>` value attribute the server
/// itself renders lowercase, so any mixed case seen server-side indicates a
/// client normalization bug, not a plausible legitimate lookup — better to
/// still resolve it than to bounce the user with a 400).
///
/// This is written directly against the `municipality_slug` stub field: it
/// can't be expressed via `municipality_name` because normalization is a
/// property of slug validation (IMP-REQ-002-03), which doesn't exist yet.
/// Currently fails because no normalization/validation exists yet: the
/// request goes through as an unfiltered search, so the assertion on
/// filtered result count fails.
#[sqlx::test(migrations = "./migrations")]
async fn tc_002_6_municipality_slug_is_case_normalized(pool: PgPool) {
    let montreal_project = seed_searchable_project_for_municipality_slug(
        &pool,
        "9 place jacques-cartier",
        "montreal",
    )
    .await;
    seed_searchable_project_for_municipality_slug(&pool, "9 dundas square", "toronto").await;
    refresh_public_search_index(&pool).await.unwrap();

    let app = app(test_state(pool).await);
    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/projects/search?q=9&municipality_slug=MONTREAL")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(
        response.status(),
        StatusCode::OK,
        "an uppercase municipality_slug must be normalized and accepted, not rejected"
    );
    let body = http_body_util::BodyExt::collect(response.into_body())
        .await
        .unwrap()
        .to_bytes();
    let envelope: Value = serde_json::from_slice(&body).unwrap();
    let results = envelope["results"]
        .as_array()
        .unwrap_or_else(|| panic!("expected a 'results' array field, got: {envelope:?}"));
    assert_eq!(
        results.len(),
        1,
        "MONTREAL must normalize to montreal and exclude the Toronto project, got: {results:?}"
    );
    assert_eq!(
        results[0]["project_id"].as_str().unwrap(),
        montreal_project.to_string()
    );
}

/// TC-003-1: a French-stemmed query ("démolition") should match a document
/// whose text contains a different inflected form ("démolir") via
/// `search_vector_fr` (IMP-REQ-003-01/-02, French `unaccent`+`french`
/// tsvector config) — something a plain ILIKE substring match cannot do.
/// The `search_vector_fr` column doesn't exist yet (this requirement's own
/// migration hasn't landed), so today's `ILIKE` matching in `run_search`
/// documents the gap: it currently returns zero results for this stemmed
/// query. Once IMP-REQ-003-02 (migration) and IMP-REQ-003-04 (route wiring
/// to `search_vector_fr`) land, this must be updated to assert the project
/// IS returned.
#[sqlx::test(migrations = "./migrations")]
async fn tc_003_1_french_stemmed_query_matches_document_ilike_cannot(pool: PgPool) {
    seed_searchable_project(
        &pool,
        "avis de démolir - 10 rue cherrier",
        "Ville de Montréal",
    )
    .await;
    refresh_public_search_index(&pool).await.unwrap();

    let app = app(test_state(pool).await);
    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/projects/search?q=d%C3%A9molition")
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
    let envelope: Value = serde_json::from_slice(&body).unwrap();
    let results = envelope["results"]
        .as_array()
        .unwrap_or_else(|| panic!("expected a 'results' array field, got: {envelope:?}"));
    assert!(
        results.is_empty(),
        "documents today's gap: plain ILIKE cannot match 'démolition' against \
         'démolir' via French stemming (fixed once search_vector_fr lands), got: {results:?}"
    );
}

/// TC-003-2: an explicit `?lang=fr` query param must override both a
/// conflicting `lang=en` cookie and a conflicting `Accept-Language: en`
/// header (precedence: explicit param > cookie > Accept-Language > default
/// `en`). There is no `lang` query param or cookie-reading in
/// `get_search_page` yet (that's IMP-REQ-003-04), so this currently fails:
/// the handler only consults `Accept-Language`, which says `en`, and renders
/// the English page.
#[sqlx::test(migrations = "./migrations")]
async fn tc_003_2_explicit_lang_param_overrides_cookie_and_header(pool: PgPool) {
    seed_searchable_project(&pool, "20 rue saint-denis", "Ville de Montréal").await;
    refresh_public_search_index(&pool).await.unwrap();

    let app = app(test_state(pool).await);
    let response = app
        .oneshot(
            Request::builder()
                .uri("/search?q=saint-denis&lang=fr")
                .header("accept-language", "en")
                .header("cookie", "lang=en")
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
        html.contains("Rechercher un projet"),
        "explicit ?lang=fr must render French regardless of a conflicting \
         Accept-Language: en header and lang=en cookie, got: {html}"
    );
}

/// TC-003-3: with no explicit `lang` param, a `lang=fr` cookie must win over
/// a conflicting `Accept-Language: en` header (precedence: cookie >
/// Accept-Language). Cookie-reading doesn't exist yet in `get_search_page`
/// (IMP-REQ-003-04), so this currently fails: the handler falls back to
/// `Accept-Language`, which says `en`.
#[sqlx::test(migrations = "./migrations")]
async fn tc_003_3_lang_cookie_overrides_accept_language_header(pool: PgPool) {
    seed_searchable_project(&pool, "30 rue ontario", "Ville de Montréal").await;
    refresh_public_search_index(&pool).await.unwrap();

    let app = app(test_state(pool).await);
    let response = app
        .oneshot(
            Request::builder()
                .uri("/search?q=ontario")
                .header("accept-language", "en")
                .header("cookie", "lang=fr")
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
        html.contains("Rechercher un projet"),
        "a lang=fr cookie must override Accept-Language: en, got: {html}"
    );
}

/// TC-003-4: with neither an explicit `lang` param nor a cookie present,
/// locale resolution falls back to `Accept-Language` exactly as REQ-001
/// established (regression guard: this must keep passing as REQ-003 adds
/// param/cookie precedence on top).
#[sqlx::test(migrations = "./migrations")]
async fn tc_003_4_no_param_or_cookie_falls_back_to_accept_language(pool: PgPool) {
    seed_searchable_project(&pool, "40 boulevard rené-lévesque", "Ville de Montréal").await;
    refresh_public_search_index(&pool).await.unwrap();

    let app = app(test_state(pool).await);
    let response = app
        .oneshot(
            Request::builder()
                .uri("/search?q=ren%C3%A9")
                .header("accept-language", "fr")
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
        html.contains("Rechercher un projet"),
        "with no lang param/cookie, Accept-Language: fr must still render \
         French (pre-existing REQ-001 behavior), got: {html}"
    );
}

/// TC-003-5: each search result row displays a per-result source-language
/// badge (`[EN]`/`[FR]`) that reflects that individual result's own
/// `source_language`, independent of the page's overall rendering language.
/// Neither the `public_search_documents.source_language` column
/// (IMP-REQ-003-02) nor the template badge markup (IMP-REQ-003-06) exist
/// yet, and `SearchResult.source_language` is a Loop A stub hard-coded to
/// `None` — so this documents the gap: even with a French-rendered page
/// (`Accept-Language: fr`), the badge for this English-sourced result is
/// absent today. Once wired, the assertion should look for `[EN]` in the
/// result's markup regardless of the French page-level rendering.
#[sqlx::test(migrations = "./migrations")]
async fn tc_003_5_per_result_source_language_badge_independent_of_ui_lang(pool: PgPool) {
    seed_searchable_project_with_language(&pool, "5 avenue du parc", "Ville de Montréal", "en")
        .await;
    refresh_public_search_index(&pool).await.unwrap();

    let app = app(test_state(pool).await);
    let response = app
        .oneshot(
            Request::builder()
                .uri("/search?q=parc")
                .header("accept-language", "fr")
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
        html.contains("[EN]"),
        "expected an [EN] source-language badge on the English-sourced result \
         even though the page itself renders in French; source_language isn't \
         wired yet (Loop A stub), got: {html}"
    );
}

/// TC-004-1: the JSON API returns a paginated envelope
/// (`{results, total, page, per_page, has_more}`), not a bare array
/// (IMP-REQ-004-04). Seeds 3 matching projects and requests `per_page=2`
/// (fewer than the total match count) to verify `results` is capped to the
/// page window while `total` still reflects the full, unpaginated match
/// count, and `has_more` is `true` since a further page remains.
#[sqlx::test(migrations = "./migrations")]
async fn tc_004_1_json_api_returns_paginated_envelope(pool: PgPool) {
    seed_searchable_project(&pool, "1 rue paginate alpha", "Ville de Pagination Alpha").await;
    seed_searchable_project(&pool, "2 rue paginate beta", "Ville de Pagination Beta").await;
    seed_searchable_project(&pool, "3 rue paginate gamma", "Ville de Pagination Gamma").await;
    refresh_public_search_index(&pool).await.unwrap();

    let app = app(test_state(pool).await);
    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/projects/search?q=paginate&per_page=2")
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
    let value: Value = serde_json::from_slice(&body).unwrap();

    assert!(
        value.is_object(),
        "expected a {{results, total, page, per_page, has_more}} envelope \
         object, not a bare array, got: {value:?}"
    );
    let results = value["results"]
        .as_array()
        .unwrap_or_else(|| panic!("expected a 'results' array field, got: {value:?}"));
    assert_eq!(
        results.len(),
        2,
        "per_page=2 should cap this page's results, got: {results:?}"
    );
    assert_eq!(
        value["total"], 3,
        "total must reflect the full unpaginated match count, got: {value:?}"
    );
    assert_eq!(value["page"], 1, "page defaults to 1 when absent, got: {value:?}");
    assert_eq!(value["per_page"], 2, "got: {value:?}");
    assert_eq!(
        value["has_more"], true,
        "3 total matches with per_page=2 means a second page remains, got: {value:?}"
    );
}

/// TC-004-2: an HTMX request (`HX-Request: true`) to `GET /search` receives
/// only the results fragment (no `<html>`/`<head>` page chrome), while a
/// plain browser request (no `HX-Request` header) still receives the full
/// page. This is IMP-REQ-004-05's real behavior, wired in `get_search_page`
/// via an `HX-Request` header check that swaps which template
/// (`results_fragment.html` vs `search.html`) gets rendered.
///
/// This assertion was FLIPPED from the previous "gap" version (which
/// asserted the HTMX-header response STILL got full page chrome, i.e. the
/// bug) now that IMP-REQ-004-05 has closed that gap — the fragment response
/// should contain the results markup but NOT the `<html>`/`<head>` chrome,
/// and a plain request should still get the full page.
#[sqlx::test(migrations = "./migrations")]
async fn tc_004_2_htmx_request_returns_fragment_not_full_page(pool: PgPool) {
    seed_searchable_project(&pool, "8 rue htmx fragment", "Ville de Fragments").await;
    refresh_public_search_index(&pool).await.unwrap();

    let app = app(test_state(pool).await);
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/search?q=fragment")
                .header("HX-Request", "true")
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
        !html.contains("<html") && !html.contains("<head"),
        "an HX-Request: true request must receive only the results \
         fragment, without full page chrome, got: {html}"
    );
    assert!(
        html.contains("rue htmx fragment"),
        "the fragment must still contain the actual result content, got: {html}"
    );

    // A plain browser request (no HX-Request header) still gets the full
    // page, unaffected by the branching above.
    let full_page_response = app
        .oneshot(
            Request::builder()
                .uri("/search?q=fragment")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(full_page_response.status(), StatusCode::OK);
    let full_page_body = http_body_util::BodyExt::collect(full_page_response.into_body())
        .await
        .unwrap()
        .to_bytes();
    let full_page_html = String::from_utf8(full_page_body.to_vec()).unwrap();

    assert!(
        full_page_html.contains("<html") && full_page_html.contains("<head"),
        "a plain request without HX-Request must still get the full page \
         chrome, got: {full_page_html}"
    );
}

/// TC-004-3: each result should eventually carry a synthesized display name
/// derived from civic address + project type (e.g. "Demolition — 123 Main
/// St"), but no such synthesis logic exists yet — `SearchResult::display_name`
/// is a Loop A stub hard-coded to `None`. This asserts the field is present
/// in the JSON body but currently null, documenting the gap.
#[sqlx::test(migrations = "./migrations")]
async fn tc_004_3_display_name_is_synthesized_from_civic_address_and_project_type(pool: PgPool) {
    // `seed_searchable_project` always inserts `project_type = 'residential'`
    // (see its body above), so the synthesized name is expected to be
    // "Residential — 123 main street".
    seed_searchable_project(&pool, "123 main street", "Ville de Synthese").await;
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
    let envelope: Value = serde_json::from_slice(&body).unwrap();
    let results = envelope["results"]
        .as_array()
        .unwrap_or_else(|| panic!("expected a 'results' array field, got: {envelope:?}"));
    assert_eq!(results.len(), 1);
    assert_eq!(
        results[0]["display_name"], "Residential — 123 main street",
        "display_name must be synthesized from civic_address_normalized + \
         project_type via core::synthesize_display_name, got: {:?}",
        results[0]
    );
}

/// TC-004-4: `first_surfaced_at` must be set once on first insert into
/// `public_search_documents` and never change on subsequent refresh-job
/// upserts of the same project. IMP-REQ-004-01 added the column and
/// IMP-REQ-004-02 made `refresh_public_search_index`'s `INSERT ...
/// ON CONFLICT DO UPDATE` (apps/web/web/src/jobs/public_search_refresh.rs)
/// set `first_surfaced_at` on insert while omitting it from the update
/// clause entirely, so it is never touched again once set.
#[sqlx::test(migrations = "./migrations")]
async fn tc_004_4_first_surfaced_at_is_immutable_across_refreshes(pool: PgPool) {
    let project_id =
        seed_searchable_project(&pool, "99 rue immutable", "Ville de Immutabilite").await;

    refresh_public_search_index(&pool).await.unwrap();
    // NOTE: `first_surfaced_at` doesn't exist yet, so this deliberately uses
    // the runtime-checked `sqlx::query_scalar` (not the `query_scalar!`
    // compile-time-checked macro, which would fail `cargo build` today
    // against a schema that lacks the column). Loop B can switch this to the
    // macro form once IMP-REQ-004-01 lands, if desired.
    let first_surfaced_at_initial: chrono::DateTime<chrono::Utc> = sqlx::query_scalar(
        "SELECT first_surfaced_at FROM public_search_documents WHERE project_id = $1",
    )
    .bind(project_id)
    .fetch_one(&pool)
    .await
    .unwrap();

    // Simulate a second refresh-job run on the same project (e.g. its
    // mention's status changed in between).
    sqlx::query!(
        "UPDATE project_mentions SET normalized_status = 'approved' WHERE project_id = $1",
        project_id
    )
    .execute(&pool)
    .await
    .unwrap();
    refresh_public_search_index(&pool).await.unwrap();

    let first_surfaced_at_after_second_refresh: chrono::DateTime<chrono::Utc> =
        sqlx::query_scalar(
            "SELECT first_surfaced_at FROM public_search_documents WHERE project_id = $1",
        )
        .bind(project_id)
        .fetch_one(&pool)
        .await
        .unwrap();

    assert_eq!(
        first_surfaced_at_initial, first_surfaced_at_after_second_refresh,
        "first_surfaced_at must not change across a second refresh-job upsert \
         of the same project"
    );
}

/// TC-004-5: requesting a `page` beyond the last page returns an empty
/// `results` array with `has_more: false`, not an error.
///
/// Note on scope: this test previously deserialized the response as a bare
/// `Vec<Value>` and documented `page` as silently ignored — that was written
/// against the pre-IMP-REQ-004-04 API shape. IMP-REQ-004-04 already wired
/// `page` into `run_search` (via `core::paginate`) and changed
/// `search_projects` to return the `SearchResultsEnvelope` object
/// (`{results, total, page, per_page, has_more}`) rather than a bare array,
/// so the OLD assertions here would panic on `serde_json::from_slice` before
/// ever reaching the pagination assertion. IMP-REQ-004-05 (this task) is
/// scoped to `get_search_page`'s HTMX branching, not the JSON API's
/// pagination wiring, which was already real by the time this task started
/// — so this update only brings the test's assertions in line with the
/// envelope shape/behavior that already exists; it is not new route logic.
#[sqlx::test(migrations = "./migrations")]
async fn tc_004_5_out_of_range_page_returns_empty_results_not_an_error(pool: PgPool) {
    seed_searchable_project(&pool, "1 rue boundary", "Ville de Frontiere").await;
    refresh_public_search_index(&pool).await.unwrap();

    let app = app(test_state(pool).await);
    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/projects/search?q=boundary&page=999")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(
        response.status(),
        StatusCode::OK,
        "an out-of-range page must not be an error"
    );
    let body = http_body_util::BodyExt::collect(response.into_body())
        .await
        .unwrap()
        .to_bytes();
    let value: Value = serde_json::from_slice(&body).unwrap();
    let results = value["results"]
        .as_array()
        .unwrap_or_else(|| panic!("expected a 'results' array field, got: {value:?}"));
    assert_eq!(
        results.len(),
        0,
        "page=999 is far beyond the single match's page, so this page's \
         results window must be empty, got: {value:?}"
    );
    assert_eq!(
        value["total"], 1,
        "total must still reflect the full unpaginated match count \
         regardless of which page was requested, got: {value:?}"
    );
    assert_eq!(
        value["has_more"], false,
        "there is no further page beyond an already-out-of-range page, got: {value:?}"
    );
}

/// TC-007-1: `date_preset=last_7_days` returns only projects surfaced within
/// the last 7 UTC days, excluding an older fixture.
///
/// Was blocked on IMP-REQ-004-01 (the migration adding
/// `public_search_documents.first_surfaced_at`) and IMP-REQ-007-05 (wiring
/// `date_preset` into `run_search`'s query); both have since landed. Written
/// directly against `first_surfaced_at` via runtime-checked `sqlx::query`
/// (not the compile-time-checked macros, which would fail `cargo build`
/// against an older schema lacking the column), per the same pattern as
/// TC-004-4.
#[sqlx::test(migrations = "./migrations")]
async fn tc_007_1_last_7_days_preset_excludes_older_fixture(pool: PgPool) {
    let recent_project =
        seed_searchable_project(&pool, "1 rue recente", "Ville de Recente").await;
    let old_project = seed_searchable_project(&pool, "2 rue ancienne", "Ville de Ancienne").await;
    refresh_public_search_index(&pool).await.unwrap();

    let now = chrono::Utc::now();
    let recent_surfaced_at = now - chrono::Duration::days(2);
    let old_surfaced_at = now - chrono::Duration::days(10);

    sqlx::query("UPDATE public_search_documents SET first_surfaced_at = $1 WHERE project_id = $2")
        .bind(recent_surfaced_at)
        .bind(recent_project)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("UPDATE public_search_documents SET first_surfaced_at = $1 WHERE project_id = $2")
        .bind(old_surfaced_at)
        .bind(old_project)
        .execute(&pool)
        .await
        .unwrap();

    let app = app(test_state(pool).await);
    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/projects/search?q=rue&date_preset=last_7_days")
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
    let envelope: Value = serde_json::from_slice(&body).unwrap();
    let results = envelope["results"]
        .as_array()
        .unwrap_or_else(|| panic!("expected a 'results' array field, got: {envelope:?}"));
    assert_eq!(
        results.len(),
        1,
        "date_preset=last_7_days must exclude the 10-day-old fixture, got: {results:?}"
    );
    assert_eq!(
        results[0]["project_id"].as_str().unwrap(),
        recent_project.to_string()
    );
}

/// TC-007-2: custom range `date_from=YYYY-MM-DD&date_to=YYYY-MM-DD` returns
/// only projects within that inclusive range.
///
/// Was blocked on IMP-REQ-004-01 (first_surfaced_at) and IMP-REQ-007-05
/// (date filter wiring), same as TC-007-1; both have since landed.
#[sqlx::test(migrations = "./migrations")]
async fn tc_007_2_custom_range_returns_only_projects_within_range(pool: PgPool) {
    let in_range_project =
        seed_searchable_project(&pool, "1 rue dans la plage", "Ville de Dansplage").await;
    let out_of_range_project =
        seed_searchable_project(&pool, "2 rue hors plage", "Ville de Horsplage").await;
    refresh_public_search_index(&pool).await.unwrap();

    let in_range_surfaced_at = chrono::DateTime::parse_from_rfc3339("2026-06-15T12:00:00Z")
        .unwrap()
        .with_timezone(&chrono::Utc);
    let out_of_range_surfaced_at = chrono::DateTime::parse_from_rfc3339("2026-05-01T12:00:00Z")
        .unwrap()
        .with_timezone(&chrono::Utc);

    sqlx::query("UPDATE public_search_documents SET first_surfaced_at = $1 WHERE project_id = $2")
        .bind(in_range_surfaced_at)
        .bind(in_range_project)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("UPDATE public_search_documents SET first_surfaced_at = $1 WHERE project_id = $2")
        .bind(out_of_range_surfaced_at)
        .bind(out_of_range_project)
        .execute(&pool)
        .await
        .unwrap();

    let app = app(test_state(pool).await);
    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/projects/search?q=plage&date_from=2026-06-01&date_to=2026-06-30")
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
    let envelope: Value = serde_json::from_slice(&body).unwrap();
    let results = envelope["results"]
        .as_array()
        .unwrap_or_else(|| panic!("expected a 'results' array field, got: {envelope:?}"));
    assert_eq!(
        results.len(),
        1,
        "date_from/date_to range must exclude the out-of-range fixture, got: {results:?}"
    );
    assert_eq!(
        results[0]["project_id"].as_str().unwrap(),
        in_range_project.to_string()
    );
}

/// TC-007-3: an invalid range (`date_from` after `date_to`) is rejected with
/// 400/409 before any query runs, via `core::parse_date_filter`
/// (IMP-REQ-007-03) wired into `run_search` (IMP-REQ-007-05).
#[sqlx::test(migrations = "./migrations")]
async fn tc_007_3_date_from_after_date_to_rejected(pool: PgPool) {
    let app = app(test_state(pool).await);
    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/projects/search?q=test&date_from=2026-07-20&date_to=2026-07-10")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert!(
        response.status() == StatusCode::BAD_REQUEST
            || response.status() == StatusCode::CONFLICT,
        "date_from after date_to must be rejected with 400/409 before any \
         query runs; documents today's gap (no validation exists yet), got: {}",
        response.status()
    );
}

/// TC-007-4: a malformed date string (not `YYYY-MM-DD`) is rejected with
/// 400, not a 500/panic, via `core::parse_date_filter` (IMP-REQ-007-03).
#[sqlx::test(migrations = "./migrations")]
async fn tc_007_4_malformed_date_string_rejected_with_400(pool: PgPool) {
    let app = app(test_state(pool).await);
    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/projects/search?q=test&date_from=not-a-date")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(
        response.status(),
        StatusCode::BAD_REQUEST,
        "a malformed date_from must be rejected with 400, not a 500/panic; \
         documents today's gap (no parsing exists yet), got: {}",
        response.status()
    );
}

/// TC-007-5: no date filter at all still returns all matching projects —
/// backward-compatible with REQ-001/002's plain search.
#[sqlx::test(migrations = "./migrations")]
async fn tc_007_5_no_date_filter_returns_all_matching_projects(pool: PgPool) {
    seed_searchable_project(&pool, "1 rue sans filtre", "Ville de Sansfiltre Un").await;
    seed_searchable_project(&pool, "2 rue sans filtre bis", "Ville de Sansfiltre Deux").await;
    refresh_public_search_index(&pool).await.unwrap();

    let app = app(test_state(pool).await);
    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/projects/search?q=sans+filtre")
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
    let envelope: Value = serde_json::from_slice(&body).unwrap();
    let results = envelope["results"]
        .as_array()
        .unwrap_or_else(|| panic!("expected a 'results' array field, got: {envelope:?}"));
    assert_eq!(
        results.len(),
        2,
        "no date filter must still return all matching projects, got: {results:?}"
    );
}

/// TC-007-6: a project surfaced exactly at the `date_from` boundary
/// (midnight UTC) is included (inclusive boundary); one exactly one second
/// before is excluded.
///
/// Was blocked on IMP-REQ-004-01 (first_surfaced_at) and IMP-REQ-007-05
/// (date filter wiring), same as TC-007-1/-2; both have since landed.
/// Written directly against `first_surfaced_at` via runtime-checked
/// `sqlx::query`, with the full
/// intended assertion body.
#[sqlx::test(migrations = "./migrations")]
async fn tc_007_6_date_from_boundary_is_inclusive(pool: PgPool) {
    let on_boundary_project =
        seed_searchable_project(&pool, "1 rue frontiere pile", "Ville de Frontierepile").await;
    let before_boundary_project =
        seed_searchable_project(&pool, "2 rue frontiere avant", "Ville de Frontiereavant").await;
    refresh_public_search_index(&pool).await.unwrap();

    let boundary_midnight_utc = chrono::Utc::now()
        .date_naive()
        .and_hms_opt(0, 0, 0)
        .unwrap()
        .and_utc();
    let one_second_before = boundary_midnight_utc - chrono::Duration::seconds(1);

    sqlx::query("UPDATE public_search_documents SET first_surfaced_at = $1 WHERE project_id = $2")
        .bind(boundary_midnight_utc)
        .bind(on_boundary_project)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("UPDATE public_search_documents SET first_surfaced_at = $1 WHERE project_id = $2")
        .bind(one_second_before)
        .bind(before_boundary_project)
        .execute(&pool)
        .await
        .unwrap();

    let date_from = boundary_midnight_utc.format("%Y-%m-%d").to_string();

    let app = app(test_state(pool).await);
    let response = app
        .oneshot(
            Request::builder()
                .uri(format!(
                    "/api/v1/projects/search?q=frontiere&date_from={date_from}"
                ))
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
    let envelope: Value = serde_json::from_slice(&body).unwrap();
    let results = envelope["results"]
        .as_array()
        .unwrap_or_else(|| panic!("expected a 'results' array field, got: {envelope:?}"));
    assert_eq!(
        results.len(),
        1,
        "date_from boundary must be inclusive (midnight UTC included, one \
         second before excluded), got: {results:?}"
    );
    assert_eq!(
        results[0]["project_id"].as_str().unwrap(),
        on_boundary_project.to_string()
    );
}

/// TC-008-1: filtering by a valid category code (`residential`) returns only
/// projects with that category.
///
/// TODO(IMP-REQ-008-02): once `projects.category_code` exists, seed/query
/// that column directly. For now this seeds two projects sharing the same
/// keyword but with different `project_type` values (`residential` vs
/// `commercial`) as a stand-in for the future closed taxonomy, and expects
/// `category=residential` to exclude the commercial project. Fails today
/// because `SearchParams::category` is a Loop A stub `run_search` never
/// reads — both projects come back regardless of `category`.
#[sqlx::test(migrations = "./migrations")]
async fn tc_008_1_valid_category_code_returns_only_matching_category(pool: PgPool) {
    let residential_project = seed_searchable_project_with_type(
        &pool,
        "1 rue categorie residentielle",
        "Ville de Categorie Un",
        Some("residential"),
    )
    .await;
    seed_searchable_project_with_type(
        &pool,
        "2 rue categorie commerciale",
        "Ville de Categorie Deux",
        Some("commercial"),
    )
    .await;
    refresh_public_search_index(&pool).await.unwrap();

    let app = app(test_state(pool).await);
    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/projects/search?q=categorie&category=residential")
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
    assert_eq!(
        results.len(),
        1,
        "category=residential must exclude the commercial project; documents \
         today's gap (category param ignored by run_search), got: {results:?}"
    );
    assert_eq!(
        results[0]["project_id"].as_str().unwrap(),
        residential_project.to_string()
    );
}

/// TC-008-2: filtering by `category=uncategorised` returns only projects
/// with `category_code IS NULL`.
///
/// TODO(IMP-REQ-008-02): once `projects.category_code` exists, seed/query
/// that column directly (a `NULL` `category_code` after the migration is the
/// real "uncategorised" state). For now this seeds one project with a
/// `project_type` set (stand-in for "has been assigned a category") and one
/// with `project_type = NULL` (stand-in for "uncategorised"), and expects
/// `category=uncategorised` to return only the latter. Fails today because
/// `category` isn't read by `run_search` — both projects come back.
#[sqlx::test(migrations = "./migrations")]
async fn tc_008_2_uncategorised_pseudo_category_returns_only_null_category(pool: PgPool) {
    seed_searchable_project_with_type(
        &pool,
        "3 rue avec categorie",
        "Ville de Categorie Trois",
        Some("institutional"),
    )
    .await;
    let uncategorised_project = seed_searchable_project_with_type(
        &pool,
        "4 rue sans categorie",
        "Ville de Categorie Quatre",
        None,
    )
    .await;
    refresh_public_search_index(&pool).await.unwrap();

    let app = app(test_state(pool).await);
    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/projects/search?q=categorie&category=uncategorised")
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
    assert_eq!(
        results.len(),
        1,
        "category=uncategorised must exclude the institutional project; \
         documents today's gap (category param ignored by run_search), \
         got: {results:?}"
    );
    assert_eq!(
        results[0]["project_id"].as_str().unwrap(),
        uncategorised_project.to_string()
    );
}

/// TC-008-3: an invalid/unknown category code (not in the taxonomy) is
/// rejected with 400, before any project query runs. No validation exists
/// yet (IMP-REQ-008-03's `category_taxonomy` table doesn't exist), so this
/// currently fails: the handler ignores the unrecognized value and returns
/// 200.
#[sqlx::test(migrations = "./migrations")]
async fn tc_008_3_invalid_category_code_rejected_before_query(pool: PgPool) {
    let app = app(test_state(pool).await);
    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/projects/search?q=test&category=not-a-real-category")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(
        response.status(),
        StatusCode::BAD_REQUEST,
        "an unknown category code must be rejected before any project query \
         runs; documents today's gap (no category_taxonomy validation exists \
         yet), got: {}",
        response.status()
    );
}

/// TC-008-4: the category facet endpoint (`GET /categories`) returns the
/// full list of valid categories.
///
/// Chosen approach: a real HTTP request against the not-yet-routed
/// `/categories` path (rather than calling the `list_categories` stub
/// function directly), matching this file's existing HTTP-request idiom for
/// every other test. `list_categories` exists in `search.rs` as a Loop A
/// stub but IMP-REQ-008-04 has not wired it into the router yet, so this
/// currently fails: the request lands on `admin_routes`' 404 fallback
/// wrapped by `require_admin` (an existing `Router::merge` quirk where
/// `.layer()` on a sub-router wraps its default fallback too), which returns
/// 403 (no `ADMIN_USER`/`ADMIN_PASSWORD_HASH` set in tests) instead of the
/// eventual 200 + taxonomy list.
#[sqlx::test(migrations = "./migrations")]
async fn tc_008_4_categories_facet_endpoint_returns_full_taxonomy(pool: PgPool) {
    let app = app(test_state(pool).await);
    let response = app
        .oneshot(
            Request::builder()
                .uri("/categories")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(
        response.status(),
        StatusCode::OK,
        "documents today's gap: GET /categories isn't routed yet \
         (IMP-REQ-008-04), got: {}",
        response.status()
    );
    let body = http_body_util::BodyExt::collect(response.into_body())
        .await
        .unwrap()
        .to_bytes();
    let categories: Vec<String> = serde_json::from_slice(&body).unwrap();
    let expected = vec![
        "residential",
        "commercial",
        "institutional",
        "infrastructure",
        "other",
    ];
    assert_eq!(
        categories, expected,
        "the facet endpoint must return exactly the assumed initial \
         taxonomy, got: {categories:?}"
    );
}

/// TC-008-5: facet-endpoint degradation — if the underlying facet query
/// fails (e.g. DB unavailable), `GET /categories` degrades gracefully (this
/// plan chooses 503, mirroring `run_search`'s existing pool-unavailable
/// behavior in TC-REQ-008-4) rather than crashing the whole search page.
///
/// Chosen approach: same real-HTTP-request idiom as TC-008-4, with the pool
/// closed beforehand to simulate DB unavailability. `list_categories` isn't
/// routed yet and takes no `State<AppState>`/pool argument at all — it's a
/// parameterless stub always returning 501 — so this currently fails: the
/// request lands on `admin_routes`' `require_admin`-wrapped 404 fallback
/// (see TC-008-4's comment) and gets 403, rather than 503 (graceful
/// degradation), documenting that IMP-REQ-008-04/-13 must both route it and
/// wire in the pool-failure fallback.
#[sqlx::test(migrations = "./migrations")]
async fn tc_008_5_categories_facet_degrades_gracefully_on_db_failure(pool: PgPool) {
    pool.close().await;
    let app = app(test_state(pool).await);
    let response = app
        .oneshot(
            Request::builder()
                .uri("/categories")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(
        response.status(),
        StatusCode::SERVICE_UNAVAILABLE,
        "the facet endpoint must degrade to 503 when its underlying query \
         fails, not crash or silently succeed; documents today's gap (route \
         doesn't exist yet, so this hits admin_routes' require_admin-wrapped \
         fallback and gets 403 instead), got: {}",
        response.status()
    );
}

/// TC-009-1: `sort=date` orders results by `latest_meeting_date` descending
/// (newest first).
///
/// Blocked on IMP-REQ-009-01 (the migration adding
/// `public_search_documents.latest_meeting_date`, backfilled from
/// `MAX(project_timeline_events.event_date)`) and IMP-REQ-009-06 (wiring
/// `sort=date` into `run_search`'s `ORDER BY`). Written directly against
/// `latest_meeting_date` via runtime-checked `sqlx::query` (not the
/// compile-time-checked macros, which would fail `cargo build` today against
/// a schema lacking the column), with the full intended assertion body, per
/// the same pattern as TC-004-4/TC-007-1.
#[ignore = "blocked on IMP-REQ-009-01 migration (latest_meeting_date)"]
#[sqlx::test(migrations = "./migrations")]
async fn tc_009_1_sort_date_orders_by_latest_meeting_date_descending(pool: PgPool) {
    let newer_project =
        seed_searchable_project(&pool, "1 rue chronologie recente", "Ville de Chronologie Un")
            .await;
    let older_project = seed_searchable_project(
        &pool,
        "2 rue chronologie ancienne",
        "Ville de Chronologie Deux",
    )
    .await;
    refresh_public_search_index(&pool).await.unwrap();

    let newer_date = chrono::Utc::now() - chrono::Duration::days(1);
    let older_date = chrono::Utc::now() - chrono::Duration::days(30);

    sqlx::query(
        "UPDATE public_search_documents SET latest_meeting_date = $1 WHERE project_id = $2",
    )
    .bind(newer_date)
    .bind(newer_project)
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "UPDATE public_search_documents SET latest_meeting_date = $1 WHERE project_id = $2",
    )
    .bind(older_date)
    .bind(older_project)
    .execute(&pool)
    .await
    .unwrap();

    let app = app(test_state(pool).await);
    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/projects/search?q=chronologie&sort=date")
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
    assert_eq!(results.len(), 2, "both fixtures must match, got: {results:?}");
    assert_eq!(
        results[0]["project_id"].as_str().unwrap(),
        newer_project.to_string(),
        "sort=date must place the newer latest_meeting_date first, got: {results:?}"
    );
    assert_eq!(
        results[1]["project_id"].as_str().unwrap(),
        older_project.to_string(),
        "sort=date must place the older latest_meeting_date last, got: {results:?}"
    );
}

/// TC-009-2: `sort=relevance` (or omitted) preserves the existing default
/// ordering (`civic_address_normalized` ASC, per today's `run_search`) —
/// regression guard. This does NOT depend on `latest_meeting_date`, so it's
/// written directly and expected to PASS today.
#[sqlx::test(migrations = "./migrations")]
async fn tc_009_2_sort_relevance_preserves_default_civic_address_order(pool: PgPool) {
    seed_searchable_project(&pool, "9 rue relevance zebra", "Ville de Relevance Un").await;
    seed_searchable_project(&pool, "1 rue relevance alpha", "Ville de Relevance Deux").await;
    refresh_public_search_index(&pool).await.unwrap();

    let app = app(test_state(pool).await);
    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/projects/search?q=relevance&sort=relevance")
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
    assert_eq!(results.len(), 2, "both fixtures must match, got: {results:?}");
    assert_eq!(
        results[0]["civic_address_normalized"].as_str().unwrap(),
        "1 rue relevance alpha",
        "sort=relevance must preserve today's civic_address_normalized ASC \
         ordering (regression guard), got: {results:?}"
    );
    assert_eq!(
        results[1]["civic_address_normalized"].as_str().unwrap(),
        "9 rue relevance zebra",
        "sort=relevance must preserve today's civic_address_normalized ASC \
         ordering (regression guard), got: {results:?}"
    );
}

/// TC-009-3: projects with NULL `latest_meeting_date` sort last regardless of
/// `sort=date` direction (NULLS LAST).
///
/// Blocked on IMP-REQ-009-01/-06, same as TC-009-1.
#[ignore = "blocked on IMP-REQ-009-01 migration (latest_meeting_date)"]
#[sqlx::test(migrations = "./migrations")]
async fn tc_009_3_null_latest_meeting_date_sorts_last(pool: PgPool) {
    let dated_project =
        seed_searchable_project(&pool, "1 rue avec date", "Ville de Nulltri Un").await;
    let undated_project =
        seed_searchable_project(&pool, "2 rue sans date", "Ville de Nulltri Deux").await;
    refresh_public_search_index(&pool).await.unwrap();

    // dated_project gets a real latest_meeting_date; undated_project is left
    // NULL (the refresh job's default for a project with no timeline events).
    sqlx::query(
        "UPDATE public_search_documents SET latest_meeting_date = $1 WHERE project_id = $2",
    )
    .bind(chrono::Utc::now())
    .bind(dated_project)
    .execute(&pool)
    .await
    .unwrap();

    let app = app(test_state(pool).await);
    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/projects/search?q=nulltri&sort=date")
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
    assert_eq!(results.len(), 2, "both fixtures must match, got: {results:?}");
    assert_eq!(
        results[0]["project_id"].as_str().unwrap(),
        dated_project.to_string(),
        "the project with a non-NULL latest_meeting_date must sort first, \
         got: {results:?}"
    );
    assert_eq!(
        results[1]["project_id"].as_str().unwrap(),
        undated_project.to_string(),
        "the project with a NULL latest_meeting_date must sort last \
         (NULLS LAST), got: {results:?}"
    );
}

/// TC-009-4: two projects with an identical `latest_meeting_date` tie-break
/// stably by a secondary key (`civic_address_normalized` ASC) — assert
/// deterministic order across repeated calls.
///
/// Blocked on IMP-REQ-009-01/-06, same as TC-009-1.
#[ignore = "blocked on IMP-REQ-009-01 migration (latest_meeting_date)"]
#[sqlx::test(migrations = "./migrations")]
async fn tc_009_4_identical_latest_meeting_date_ties_break_stably(pool: PgPool) {
    let second_alphabetically =
        seed_searchable_project(&pool, "9 rue egalite zebra", "Ville de Egalite Un").await;
    let first_alphabetically =
        seed_searchable_project(&pool, "1 rue egalite alpha", "Ville de Egalite Deux").await;
    refresh_public_search_index(&pool).await.unwrap();

    let shared_date = chrono::Utc::now() - chrono::Duration::days(5);
    sqlx::query(
        "UPDATE public_search_documents SET latest_meeting_date = $1 WHERE project_id = $2",
    )
    .bind(shared_date)
    .bind(second_alphabetically)
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "UPDATE public_search_documents SET latest_meeting_date = $1 WHERE project_id = $2",
    )
    .bind(shared_date)
    .bind(first_alphabetically)
    .execute(&pool)
    .await
    .unwrap();

    let app = app(test_state(pool).await);

    for attempt in 0..2 {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/v1/projects/search?q=egalite&sort=date")
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
        assert_eq!(
            results.len(),
            2,
            "both fixtures must match (attempt {attempt}), got: {results:?}"
        );
        assert_eq!(
            results[0]["project_id"].as_str().unwrap(),
            first_alphabetically.to_string(),
            "identical latest_meeting_date must tie-break by \
             civic_address_normalized ASC (attempt {attempt}), got: {results:?}"
        );
        assert_eq!(
            results[1]["project_id"].as_str().unwrap(),
            second_alphabetically.to_string(),
            "identical latest_meeting_date must tie-break by \
             civic_address_normalized ASC (attempt {attempt}), got: {results:?}"
        );
    }
}

/// TC-009-5: an invalid `sort` value (not `relevance` or `date`) is rejected
/// with 400, before any query runs. No such validation exists yet
/// (`SearchParams::sort` is a Loop A stub `run_search` never reads), so this
/// currently fails: the handler ignores the unrecognized value and returns
/// 200.
#[sqlx::test(migrations = "./migrations")]
async fn tc_009_5_invalid_sort_value_rejected_before_query(pool: PgPool) {
    let app = app(test_state(pool).await);
    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/projects/search?q=test&sort=alphabetical")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(
        response.status(),
        StatusCode::BAD_REQUEST,
        "an unrecognized sort value must be rejected before any query runs; \
         documents today's gap (no sort validation exists yet), got: {}",
        response.status()
    );
}

/// TC-012-1: a zero-result search must render more than today's single
/// `empty_message` string — a distinct headline element and body text.
///
/// Target markup contract (documented here for Loop B to implement):
/// `<h2 class="search-empty-heading">` + `<p class="search-empty-body">`,
/// alongside (not replacing) today's `<p class="search-empty">`. Currently
/// fails: `templates/search.html`'s empty branch only renders
/// `<p class="search-empty">{{ empty_message }}</p>` — no heading element,
/// no separate body element exist yet.
#[sqlx::test(migrations = "./migrations")]
async fn tc_012_1_zero_results_renders_headline_and_body(pool: PgPool) {
    seed_searchable_project(&pool, "1 unrelated avenue", "Some Other Town 012a").await;
    refresh_public_search_index(&pool).await.unwrap();

    let app = app(test_state(pool).await);
    let response = app
        .oneshot(
            Request::builder()
                .uri("/search?q=zzz-no-such-project-012")
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
        html.contains("class=\"search-empty-heading\""),
        "expected a distinct empty-state headline element, got: {html}"
    );
    assert!(
        html.contains("No matches for your search"),
        "expected the target English empty-state headline text, got: {html}"
    );
    assert!(
        html.contains("class=\"search-empty-body\""),
        "expected a distinct empty-state body element, got: {html}"
    );
    assert!(
        html.contains("Try adjusting your filters below, or explore these suggestions."),
        "expected the target English empty-state body text, got: {html}"
    );
}

/// TC-012-2: the same empty state under `Accept-Language: fr` renders
/// headline, body, suggestions, and action links all in French — no mixed
/// English/French copy. Currently fails: none of this markup exists yet
/// (today's `search_labels("fr")` only supplies `empty_message`).
#[sqlx::test(migrations = "./migrations")]
async fn tc_012_2_zero_results_french_locale_is_fully_french(pool: PgPool) {
    seed_searchable_project(&pool, "1 avenue non lie", "Une Autre Ville 012b").await;
    refresh_public_search_index(&pool).await.unwrap();

    let app = app(test_state(pool).await);
    let response = app
        .oneshot(
            Request::builder()
                .uri("/search?q=zzz-aucun-projet-012")
                .header("accept-language", "fr")
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
        html.contains("Aucun résultat pour votre recherche"),
        "expected the target French empty-state headline text, got: {html}"
    );
    assert!(
        html.contains("Essayez d'ajuster vos filtres ci-dessous, ou explorez ces suggestions."),
        "expected the target French empty-state body text, got: {html}"
    );
    assert!(
        html.contains("Essayez une autre municipalité"),
        "expected a French refinement suggestion, got: {html}"
    );
    assert!(
        html.contains("Effacer les filtres"),
        "expected the French 'clear filters' action link, got: {html}"
    );
    assert!(
        html.contains("Parcourir tous les projets"),
        "expected the French 'browse all projects' action link, got: {html}"
    );
    assert!(
        !html.contains("No matches for your search"),
        "French empty state must not contain leftover English headline, got: {html}"
    );
    assert!(
        !html.contains("Clear filters"),
        "French empty state must not contain leftover English action-link text, got: {html}"
    );
}

/// TC-012-3: exactly 4 refinement suggestions are rendered — not more, not
/// fewer. Target markup contract: each suggestion is an
/// `<li class="search-empty-suggestion">`. Currently fails: no such elements
/// exist yet (count is 0).
#[sqlx::test(migrations = "./migrations")]
async fn tc_012_3_exactly_four_refinement_suggestions(pool: PgPool) {
    seed_searchable_project(&pool, "1 unrelated boulevard", "Some Other Town 012c").await;
    refresh_public_search_index(&pool).await.unwrap();

    let app = app(test_state(pool).await);
    let response = app
        .oneshot(
            Request::builder()
                .uri("/search?q=zzz-no-such-project-012c")
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

    let suggestion_count = html.matches("class=\"search-empty-suggestion\"").count();
    assert_eq!(
        suggestion_count, 4,
        "expected exactly 4 refinement suggestions, got {suggestion_count} in: {html}"
    );
}

/// TC-012-4: two action links ("clear filters" -> `/search`, "browse all
/// projects" -> `/`) are rendered, and both hrefs resolve to working routes
/// (not dead links) when followed. Currently fails: no
/// `search-empty-action` links exist yet.
#[sqlx::test(migrations = "./migrations")]
async fn tc_012_4_two_action_links_resolve_to_working_routes(pool: PgPool) {
    seed_searchable_project(&pool, "1 unrelated crescent", "Some Other Town 012d").await;
    refresh_public_search_index(&pool).await.unwrap();

    let app = app(test_state(pool).await);
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/search?q=zzz-no-such-project-012d")
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

    let action_link_count = html.matches("class=\"search-empty-action\"").count();
    assert_eq!(
        action_link_count, 2,
        "expected exactly 2 action links, got {action_link_count} in: {html}"
    );
    assert!(
        html.contains("class=\"search-empty-action\" href=\"/search\""),
        "expected a 'clear filters' action link targeting /search, got: {html}"
    );
    assert!(
        html.contains("class=\"search-empty-action\" href=\"/\""),
        "expected a 'browse all projects' action link targeting /, got: {html}"
    );

    // Both targets must be live routes, not dead links.
    let clear_filters_response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/search")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert!(
        clear_filters_response.status().is_success(),
        "the 'clear filters' target /search must resolve successfully, got: {}",
        clear_filters_response.status()
    );

    let browse_all_response = app
        .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert!(
        browse_all_response.status().is_success(),
        "the 'browse all projects' target / must resolve successfully, got: {}",
        browse_all_response.status()
    );
}

/// TC-012-5: the empty state and the error state are structurally mutually
/// exclusive — a DB-failure response on `/search` must show the error
/// markup and never the "no results" empty-state copy at the same time.
/// This documents that `get_search_page`'s `match search_outcome` already
/// makes `search_results`/`search_error` mutually exclusive branches
/// (`Some(Err(_)) => (Vec::new(), true)` sets `search_error = true`, and the
/// template's `{% if search_error %} ... {% elif has_searched %}` means the
/// empty branch can't render when `search_error` is true) — expected to
/// PASS today, since there is no empty-state markup yet to conflict with.
/// Written as: (a) a positive assertion that today's distinct error-state
/// markup renders, and (b) a negative assertion that the empty-state
/// copy/text is absent from the same response.
#[sqlx::test(migrations = "./migrations")]
async fn tc_012_5_error_state_and_empty_state_are_mutually_exclusive(pool: PgPool) {
    pool.close().await;
    let app = app(test_state(pool).await);
    let response = app
        .oneshot(
            Request::builder()
                .uri("/search?q=whatever-012e")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(
        response.status(),
        StatusCode::OK,
        "a DB failure on /search must still render the page (with an error banner), not a raw HTTP error"
    );
    let body = http_body_util::BodyExt::collect(response.into_body())
        .await
        .unwrap()
        .to_bytes();
    let html = String::from_utf8(body.to_vec()).unwrap();

    assert!(
        html.contains("class=\"search-error\""),
        "expected today's distinct error-state markup to render on DB failure, got: {html}"
    );
    assert!(
        html.contains("role=\"alert\""),
        "expected the error-state markup to carry role=\"alert\", got: {html}"
    );
    assert!(
        !html.contains("class=\"search-empty\""),
        "a DB-failure response must never simultaneously render the empty-state markup, got: {html}"
    );
    assert!(
        !html.contains("No projects match your search."),
        "a DB-failure response must never simultaneously show the 'no results' empty-state copy, got: {html}"
    );
}

/// IMP-REQ-001-06: zero-result search renders a distinct guidance line
/// (beyond today's single `empty_message`) suggesting the user broaden or
/// adjust their query, in both EN and FR. Deliberately scoped to just this
/// one line — the richer headline/body/suggestions/action-links empty state
/// is REQ-012's later job (see `tc_012_*` above); this test's markup
/// (`class="search-empty-guidance"`) is intentionally distinct from
/// REQ-012's future `search-empty-heading` / `search-empty-body` /
/// `search-empty-suggestion` / `search-empty-action` element classes.
#[sqlx::test(migrations = "./migrations")]
async fn imp_req_001_06_empty_state_shows_guidance_line(pool: PgPool) {
    seed_searchable_project(&pool, "1 unrelated street", "Some Other Town 00106").await;
    refresh_public_search_index(&pool).await.unwrap();

    let app = app(test_state(pool).await);

    let en_response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/search?q=zzz-no-such-project-00106")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(en_response.status(), StatusCode::OK);
    let en_body = http_body_util::BodyExt::collect(en_response.into_body())
        .await
        .unwrap()
        .to_bytes();
    let en_html = String::from_utf8(en_body.to_vec()).unwrap();

    assert!(
        en_html.contains("class=\"search-empty-guidance\""),
        "expected a distinct empty-state guidance element, got: {en_html}"
    );
    assert!(
        en_html.contains("Try broadening your search"),
        "expected English guidance copy suggesting the user broaden their query, got: {en_html}"
    );

    let fr_response = app
        .oneshot(
            Request::builder()
                .uri("/search?q=zzz-aucun-projet-00106")
                .header("accept-language", "fr")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(fr_response.status(), StatusCode::OK);
    let fr_body = http_body_util::BodyExt::collect(fr_response.into_body())
        .await
        .unwrap()
        .to_bytes();
    let fr_html = String::from_utf8(fr_body.to_vec()).unwrap();

    assert!(
        fr_html.contains("class=\"search-empty-guidance\""),
        "expected a distinct empty-state guidance element in French render, got: {fr_html}"
    );
    assert!(
        fr_html.contains("Essayez une recherche plus large"),
        "expected French guidance copy suggesting the user broaden their query, got: {fr_html}"
    );
    assert!(
        !fr_html.contains("Try broadening your search"),
        "French empty state must not contain leftover English guidance copy, got: {fr_html}"
    );
}

/// IMP-REQ-001-07: the per-result `[EN]`/`[FR]` source-language badge markup
/// added to `search.html` must gracefully omit itself when
/// `SearchResult.source_language` is `None` — which is 100% of the time
/// today, since neither the `public_search_documents.source_language` column
/// nor its backfill exist yet (that's IMP-REQ-003-02/03/04's job; see the
/// Loop A stub comment on `SearchResult::source_language` in `search.rs`).
/// This test only covers the omission case. It deliberately does NOT assert
/// the positive `[EN]`/`[FR]` rendering case, since there is no real
/// `source_language` data to seed yet — REQ-003's own `tc_003_5` test
/// documents that gap and will exercise the positive case once REQ-003's
/// later Loop B tasks populate real data through this same badge markup.
#[sqlx::test(migrations = "./migrations")]
async fn imp_req_001_07_lang_badge_omitted_when_source_language_unknown(pool: PgPool) {
    seed_searchable_project(&pool, "10 rue badge test 00107", "Ville Badge Test 00107").await;
    refresh_public_search_index(&pool).await.unwrap();

    let app = app(test_state(pool).await);
    let response = app
        .oneshot(
            Request::builder()
                .uri("/search?q=badge+test+00107")
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
        html.contains("10 rue badge test 00107"),
        "sanity check that the result actually rendered, got: {html}"
    );
    assert!(
        !html.contains("result-lang-badge"),
        "no badge markup should render while source_language is None (today's \
         only real case — the column/backfill are IMP-REQ-003-02/03/04's job), \
         got: {html}"
    );
    assert!(
        !html.contains("[EN]") && !html.contains("[FR]"),
        "no [EN]/[FR] badge text should appear anywhere in the response while \
         source_language is unpopulated, got: {html}"
    );
}

// ---------------------------------------------------------------------
// REQ-015 Loop A: search-result confidence indicator ("Detected N days ago
// from M council source(s)").
//
// `public_search_documents` has neither `first_detected_at` nor
// `source_count` columns yet — that migration is IMP-REQ-015-02/03/04's job,
// out of scope for this pass. `SearchResult::first_detected_at`/`source_count`
// are Loop A stubs hard-coded to `None` in `run_search` (see `search.rs`), and
// `templates/search.html` has no rendering for the indicator at all yet. So
// every test below that asserts the indicator IS rendered (TC-015-1/-3/-4/-6)
// is EXPECTED TO FAIL today, each for that same documented reason. TC-015-5
// (missing field(s) omit the whole sentence) legitimately PASSES today, since
// nothing renders regardless of data — it is written to assert something
// meaningful (200 OK, plus explicit absence of a partial/broken fragment)
// rather than a vacuous no-op.
// ---------------------------------------------------------------------

/// TC-015-1: a search result with both `first_detected_at` and
/// `source_count` populated renders "Detected N days ago from M council
/// source(s)" with the correct day count and source count on the search
/// results card. Seeds a project with 2 distinct source documents/chunks
/// feeding into it via `project_mentions` (the intended "distinct council
/// source" signal per IMP-REQ-015's discovery rule), documenting the target
/// N=5/M=2 contract. Currently FAILS: neither field is backed by real data
/// (`public_search_documents` has no columns for them yet), so the indicator
/// never renders regardless of how many source documents are seeded.
#[sqlx::test(migrations = "./migrations")]
async fn tc_015_1_search_card_shows_days_and_source_count(pool: PgPool) {
    let project_id =
        seed_searchable_project(&pool, "1 rue detection alpha", "Ville de Detection Un").await;

    // Second distinct source document/chunk mentioning the same project, so
    // that once IMP-REQ-015 lands, "distinct council source" discovery
    // (COUNT(DISTINCT source document) or its document_chunk_id fallback)
    // would compute source_count = 2 for this project.
    let suffix = Uuid::new_v4();
    let municipality_id = sqlx::query_scalar!(
        "INSERT INTO municipalities (name, slug, domain_allowlist) VALUES ($1, $2, ARRAY[$3]) RETURNING id",
        format!("Second Source Municipality {suffix}"),
        format!("slug-{suffix}"),
        format!("{suffix}.example"),
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    let doc_id = sqlx::query_scalar!(
        "INSERT INTO source_documents (municipality_id, source_url, checksum, content, content_type) \
         VALUES ($1, $2, 'chk2', ''::bytea, 'text/html') RETURNING id",
        municipality_id,
        format!("https://{suffix}.example/second-doc"),
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    let chunk_id = sqlx::query_scalar!(
        "INSERT INTO document_chunks (source_document_id, chunk_index, content) \
         VALUES ($1, 0, 'second chunk text') RETURNING id",
        doc_id
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    sqlx::query!(
        "INSERT INTO project_mentions \
         (document_chunk_id, project_id, physical_work, civic_address, project_type, scale_units, normalized_status) \
         VALUES ($1, $2, true, $3, 'residential', 1, 'approved')",
        chunk_id,
        project_id,
        "1 rue detection alpha",
    )
    .execute(&pool)
    .await
    .unwrap();

    refresh_public_search_index(&pool).await.unwrap();

    let app = app(test_state(pool).await);
    let response = app
        .oneshot(
            Request::builder()
                .uri("/search?q=detection+alpha")
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
        "expected the confidence indicator sentence once IMP-REQ-015-02/03/04/06/11 \
         land and first_detected_at/source_count are populated, got: {html}"
    );
}

/// TC-015-2 (search-card half): see `tc_015_2_project_detail_page_shows_days_and_source_count`
/// in `timeline_resolver.rs` for the detail-page assertion.
///
/// TC-015-3: day-count boundary — a project detected exactly 1 day ago
/// renders singular "1 day ago" (not "1 days ago"), and a project detected
/// 0 days ago (today) renders "today" (the chosen phrasing for the N=0 case,
/// asserted explicitly). Both fail today for the same documented reason as
/// TC-015-1: the indicator never renders.
#[sqlx::test(migrations = "./migrations")]
async fn tc_015_3_day_count_boundary_singular_and_today(pool: PgPool) {
    seed_searchable_project(&pool, "2 rue frontiere un jour", "Ville de Frontierejour Un").await;
    seed_searchable_project(
        &pool,
        "3 rue frontiere aujourdhui",
        "Ville de Frontierejour Deux",
    )
    .await;
    refresh_public_search_index(&pool).await.unwrap();

    let app = app(test_state(pool).await);

    let response_one_day = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/search?q=frontiere+un+jour")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response_one_day.status(), StatusCode::OK);
    let body_one_day = http_body_util::BodyExt::collect(response_one_day.into_body())
        .await
        .unwrap()
        .to_bytes();
    let html_one_day = String::from_utf8(body_one_day.to_vec()).unwrap();
    assert!(
        html_one_day.contains("Detected 1 day ago"),
        "expected the singular '1 day ago' phrasing (not '1 days ago') once \
         the indicator is wired up, got: {html_one_day}"
    );
    assert!(
        !html_one_day.contains("1 days ago"),
        "must never render the incorrect plural '1 days ago', got: {html_one_day}"
    );

    let response_today = app
        .oneshot(
            Request::builder()
                .uri("/search?q=frontiere+aujourdhui")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response_today.status(), StatusCode::OK);
    let body_today = http_body_util::BodyExt::collect(response_today.into_body())
        .await
        .unwrap()
        .to_bytes();
    let html_today = String::from_utf8(body_today.to_vec()).unwrap();
    assert!(
        html_today.contains("Detected today"),
        "expected the explicit 'Detected today' phrasing for a project \
         detected 0 days ago, got: {html_today}"
    );
}

/// TC-015-4: source-count pluralization — `source_count=1` renders singular
/// "1 council source" vs. `source_count=2` renders plural "2 council
/// sources". Independent seed data (two distinct projects) from TC-015-1/-3.
/// Fails today for the same documented reason: the indicator never renders.
#[sqlx::test(migrations = "./migrations")]
async fn tc_015_4_source_count_pluralization(pool: PgPool) {
    // Single-source project.
    seed_searchable_project(&pool, "4 rue source unique", "Ville de Sourceunique").await;

    // Two-source project.
    let two_source_project =
        seed_searchable_project(&pool, "5 rue deux sources", "Ville de Deuxsources").await;
    let suffix = Uuid::new_v4();
    let municipality_id = sqlx::query_scalar!(
        "INSERT INTO municipalities (name, slug, domain_allowlist) VALUES ($1, $2, ARRAY[$3]) RETURNING id",
        format!("Second Source Municipality {suffix}"),
        format!("slug-{suffix}"),
        format!("{suffix}.example"),
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    let doc_id = sqlx::query_scalar!(
        "INSERT INTO source_documents (municipality_id, source_url, checksum, content, content_type) \
         VALUES ($1, $2, 'chk3', ''::bytea, 'text/html') RETURNING id",
        municipality_id,
        format!("https://{suffix}.example/second-doc"),
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    let chunk_id = sqlx::query_scalar!(
        "INSERT INTO document_chunks (source_document_id, chunk_index, content) \
         VALUES ($1, 0, 'second chunk text') RETURNING id",
        doc_id
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    sqlx::query!(
        "INSERT INTO project_mentions \
         (document_chunk_id, project_id, physical_work, civic_address, project_type, scale_units, normalized_status) \
         VALUES ($1, $2, true, $3, 'residential', 1, 'approved')",
        chunk_id,
        two_source_project,
        "5 rue deux sources",
    )
    .execute(&pool)
    .await
    .unwrap();

    refresh_public_search_index(&pool).await.unwrap();

    let app = app(test_state(pool).await);

    let response_single = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/search?q=source+unique")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response_single.status(), StatusCode::OK);
    let body_single = http_body_util::BodyExt::collect(response_single.into_body())
        .await
        .unwrap()
        .to_bytes();
    let html_single = String::from_utf8(body_single.to_vec()).unwrap();
    assert!(
        html_single.contains("1 council source") && !html_single.contains("1 council sources"),
        "expected singular '1 council source' for source_count=1, got: {html_single}"
    );

    let response_plural = app
        .oneshot(
            Request::builder()
                .uri("/search?q=deux+sources")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response_plural.status(), StatusCode::OK);
    let body_plural = http_body_util::BodyExt::collect(response_plural.into_body())
        .await
        .unwrap()
        .to_bytes();
    let html_plural = String::from_utf8(body_plural.to_vec()).unwrap();
    assert!(
        html_plural.contains("2 council sources"),
        "expected plural '2 council sources' for source_count=2, got: {html_plural}"
    );
}

/// TC-015-5: either field NULL (missing) omits the ENTIRE indicator
/// sentence, not a partial/broken sentence with a missing number. This
/// legitimately PASSES today, since neither field is backed by real data
/// (both are always `None`), so nothing renders regardless — written to
/// assert something meaningful (200 OK, plus explicit absence of a
/// partial/broken fragment or a literal "None") rather than a vacuous no-op.
#[sqlx::test(migrations = "./migrations")]
async fn tc_015_5_missing_field_omits_entire_indicator(pool: PgPool) {
    seed_searchable_project(&pool, "6 rue indicateur absent", "Ville de Indicateurabsent").await;
    refresh_public_search_index(&pool).await.unwrap();

    let app = app(test_state(pool).await);
    let response = app
        .oneshot(
            Request::builder()
                .uri("/search?q=indicateur+absent")
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
        !html.contains("Detected"),
        "the indicator sentence must be omitted entirely when first_detected_at/\
         source_count are missing, not a partial fragment, got: {html}"
    );
    assert!(
        !html.contains("council source"),
        "no partial 'council source(s)' fragment must leak into the markup \
         when the fields are missing, got: {html}"
    );
    assert!(
        !html.contains(">None<"),
        "missing fields must never render as a literal 'None' in the markup, got: {html}"
    );
    assert!(
        !html.contains("{{"),
        "template must not leak raw Jinja syntax when the indicator fields are missing, got: {html}"
    );
}

/// TC-015-6: EN/FR — the indicator sentence is correctly localized in both
/// languages (the whole sentence structure, not just the count numerals).
/// Fails today for the same documented reason as TC-015-1: the indicator
/// never renders in either language.
#[sqlx::test(migrations = "./migrations")]
async fn tc_015_6_indicator_localized_en_fr(pool: PgPool) {
    seed_searchable_project(&pool, "7 rue localisation", "Ville de Localisation").await;
    refresh_public_search_index(&pool).await.unwrap();

    let app = app(test_state(pool).await);

    let response_en = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/search?q=localisation")
                .header("accept-language", "en")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response_en.status(), StatusCode::OK);
    let body_en = http_body_util::BodyExt::collect(response_en.into_body())
        .await
        .unwrap()
        .to_bytes();
    let html_en = String::from_utf8(body_en.to_vec()).unwrap();
    assert!(
        html_en.contains("Detected") && html_en.contains("days ago from") && html_en.contains("council source"),
        "expected the full English indicator sentence structure once wired up, got: {html_en}"
    );

    let response_fr = app
        .oneshot(
            Request::builder()
                .uri("/search?q=localisation")
                .header("accept-language", "fr")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response_fr.status(), StatusCode::OK);
    let body_fr = http_body_util::BodyExt::collect(response_fr.into_body())
        .await
        .unwrap()
        .to_bytes();
    let html_fr = String::from_utf8(body_fr.to_vec()).unwrap();
    assert!(
        html_fr.contains("Détecté") && html_fr.contains("jour") && html_fr.contains("source"),
        "expected the full French indicator sentence structure (not just \
         translated numerals) once wired up, got: {html_fr}"
    );
    assert!(
        !html_fr.contains("Detected"),
        "the French-rendered page must not contain leftover English indicator text, got: {html_fr}"
    );
}

/// IMP-REQ-001-08: the search results page shows a result-count header
/// ("N results found" / EN, "N résultats trouvés" / FR) computed from the
/// actual number of results, with correct EN/FR singular/plural wording for
/// a single result and for several results.
#[sqlx::test(migrations = "./migrations")]
async fn imp_req_001_08_result_count_header_pluralization(pool: PgPool) {
    seed_searchable_project(&pool, "1 rue singulier compte", "Ville de Comptesingulier").await;
    seed_searchable_project(&pool, "1 rue pluriel compte alpha", "Ville de Comptepluriel Un").await;
    seed_searchable_project(&pool, "2 rue pluriel compte beta", "Ville de Comptepluriel Deux").await;
    seed_searchable_project(&pool, "3 rue pluriel compte gamma", "Ville de Comptepluriel Trois").await;
    refresh_public_search_index(&pool).await.unwrap();

    let app = app(test_state(pool).await);

    // Singular, English: exactly one match.
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/search?q=singulier+compte")
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
        html.contains("1 result found"),
        "expected singular EN result-count header, got: {html}"
    );

    // Plural, English: three matches.
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/search?q=pluriel+compte")
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
        html.contains("3 results found"),
        "expected plural EN result-count header with the actual count, got: {html}"
    );

    // Singular, French: exactly one match.
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/search?q=singulier+compte")
                .header("accept-language", "fr")
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
        html.contains("1 résultat trouvé"),
        "expected singular FR result-count header, got: {html}"
    );

    // Plural, French: three matches.
    let response = app
        .oneshot(
            Request::builder()
                .uri("/search?q=pluriel+compte")
                .header("accept-language", "fr")
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
        html.contains("3 résultats trouvés"),
        "expected plural FR result-count header with the actual count, got: {html}"
    );
}

/// IMP-REQ-001-08 gap: `get_search_page` gates `result_count_label` on
/// `!search_results.is_empty()` (search.rs), so the zero-results path must
/// never render the "N results found" / "N résultats trouvés" header —
/// that state is exclusively owned by `empty_message`/`empty_guidance`
/// (IMP-REQ-001-06). Verifies the negative directly, in both languages,
/// rather than just trusting the code comment.
#[sqlx::test(migrations = "./migrations")]
async fn imp_req_001_08_zero_results_omits_count_header(pool: PgPool) {
    seed_searchable_project(&pool, "1 unrelated way", "Some Other Town 00108z").await;
    refresh_public_search_index(&pool).await.unwrap();

    let app = app(test_state(pool).await);

    let en_response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/search?q=zzz-no-such-project-00108z")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(en_response.status(), StatusCode::OK);
    let en_body = http_body_util::BodyExt::collect(en_response.into_body())
        .await
        .unwrap()
        .to_bytes();
    let en_html = String::from_utf8(en_body.to_vec()).unwrap();
    assert!(
        !en_html.contains("class=\"search-result-count\""),
        "zero-results EN response must not render the result-count element, got: {en_html}"
    );
    assert!(
        !en_html.contains("result found"),
        "zero-results EN response must not render 'result found' text, got: {en_html}"
    );

    let fr_response = app
        .oneshot(
            Request::builder()
                .uri("/search?q=zzz-no-such-project-00108z")
                .header("accept-language", "fr")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(fr_response.status(), StatusCode::OK);
    let fr_body = http_body_util::BodyExt::collect(fr_response.into_body())
        .await
        .unwrap()
        .to_bytes();
    let fr_html = String::from_utf8(fr_body.to_vec()).unwrap();
    assert!(
        !fr_html.contains("class=\"search-result-count\""),
        "zero-results FR response must not render the result-count element, got: {fr_html}"
    );
    assert!(
        !fr_html.contains("résultat"),
        "zero-results FR response must not render any 'résultat' text, got: {fr_html}"
    );
}

/// IMP-REQ-001-08 gap: `result_count_label` is gated on `!search_error &&
/// !search_results.is_empty()`, so a DB-failure response (which forces
/// `search_results` to `Vec::new()` and `search_error` to `true`, per
/// `get_search_page`'s `match search_outcome`) must never render the
/// result-count header alongside the error banner. Uses the same
/// `pool.close()` fault-injection pattern as
/// `tc_012_5_error_state_and_empty_state_are_mutually_exclusive` and
/// `tc_req_008_4_returns_503_when_pool_unavailable`.
#[sqlx::test(migrations = "./migrations")]
async fn imp_req_001_08_db_error_omits_count_header(pool: PgPool) {
    pool.close().await;
    let app = app(test_state(pool).await);
    let response = app
        .oneshot(
            Request::builder()
                .uri("/search?q=whatever-00108e")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(
        response.status(),
        StatusCode::OK,
        "a DB failure on /search must still render the page with an error banner"
    );
    let body = http_body_util::BodyExt::collect(response.into_body())
        .await
        .unwrap()
        .to_bytes();
    let html = String::from_utf8(body.to_vec()).unwrap();

    assert!(
        html.contains("class=\"search-error\""),
        "expected the error-state markup to render on DB failure, got: {html}"
    );
    assert!(
        !html.contains("class=\"search-result-count\""),
        "a DB-failure response must never render the result-count header, got: {html}"
    );
    assert!(
        !html.contains("result found") && !html.contains("résultat"),
        "a DB-failure response must never render 'result found'/'résultat' text, got: {html}"
    );
}

/// IMP-REQ-001-08 / per_page interaction gap: `run_search` caps the returned
/// `Vec<SearchResult>` at `per_page` via `LIMIT $2` in the SQL query
/// (search.rs), and `format_result_count_label` is called with
/// `search_results.len()` — the length of that already-truncated Vec, not
/// the total number of matching rows in the database. Seeds 5 matching
/// rows and requests `per_page=2`: this test confirms the rendered header
/// says "2 results found" (the truncated/returned count), not "5 results
/// found" (the total match count). This is flagged in the task report as
/// potentially misleading: a user sees "2 results found" with no
/// indication 3 more matches exist and were dropped, rather than a
/// "showing 2 of 5" style label — but per instructions this test documents
/// today's actual behavior rather than redesigning the label.
#[sqlx::test(migrations = "./migrations")]
async fn imp_req_001_08_count_label_reflects_per_page_truncated_count(pool: PgPool) {
    seed_searchable_project(&pool, "1 rue troncature alpha", "Ville de Troncature Un").await;
    seed_searchable_project(&pool, "2 rue troncature beta", "Ville de Troncature Deux").await;
    seed_searchable_project(&pool, "3 rue troncature gamma", "Ville de Troncature Trois").await;
    seed_searchable_project(&pool, "4 rue troncature delta", "Ville de Troncature Quatre").await;
    seed_searchable_project(&pool, "5 rue troncature epsilon", "Ville de Troncature Cinq").await;
    refresh_public_search_index(&pool).await.unwrap();

    let app = app(test_state(pool).await);
    let response = app
        .oneshot(
            Request::builder()
                .uri("/search?q=troncature&per_page=2")
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
        html.contains("2 results found"),
        "expected the count header to reflect the per_page-truncated (returned) \
         count of 2, got: {html}"
    );
    assert!(
        !html.contains("5 results found"),
        "the count header must not reflect the total match count of 5 when \
         per_page truncated the returned Vec to 2, got: {html}"
    );
}

/// IMP-REQ-001-10: manual WCAG AA review of `search.html` (form, results
/// list, error state, lang badges — the markup added by
/// IMP-REQ-001-06/07/08). Renders the template directly (rather than going
/// through `/search` + `run_search`) so the assertions exercise the actual
/// markup contract in isolation from unrelated, already-documented data-layer
/// gaps (e.g. `SearchResult.source_language` being a stub the DB path doesn't
/// populate yet, per TC-003-5 above). Asserts:
///   - the search `<input>` has a `<label for="search-q">` matching its
///     `id="search-q"` (not just a placeholder);
///   - the submit button carries plain, non-icon-only text;
///   - a successful search renders results as a semantic `<ul>`/`<li>` list,
///     not bare `<div>`s;
///   - the per-result source-language badge is real text (`[EN]`), not a
///     color-only indicator;
///   - exactly one `<h1>` establishes the page's heading hierarchy;
///   - the error state carries `role="alert"` so assistive tech announces it.
#[test]
fn imp_req_001_10_search_page_meets_basic_accessibility_requirements() {
    let mut env = Environment::new();
    env.set_loader(path_loader("../templates"));
    let tmpl = env.get_template("search.html").unwrap();

    // --- Results state ---------------------------------------------------
    let html = tmpl
        .render(context! {
            lang => "en",
            nav_permits => "Permits",
            nav_council => "Council",
            page_title => "Search projects",
            heading => "Search for a project",
            search_label => "Civic address or municipality",
            submit_label => "Search",
            empty_message => "No projects match your search.",
            empty_guidance => "Try broadening your search: use a more general keyword, or double-check the spelling of the address or municipality.",
            query => "accessible",
            has_searched => true,
            search_results => vec![minijinja::value::Value::from_serialize(
                serde_json::json!({
                    "project_id": "11111111-1111-1111-1111-111111111111",
                    "civic_address_normalized": "15 rue accessible",
                    "municipality_name": "Ville de Accessibilite",
                    "normalized_status": "approved",
                    "source_language": "en",
                }),
            )],
            search_error => false,
            result_count_label => "1 result found",
        })
        .unwrap();

    assert!(
        html.contains(r#"<label for="search-q">"#) && html.contains(r#"id="search-q""#),
        "search input must have a <label for> matching its id, got: {html}"
    );
    assert!(
        html.contains("type=\"submit\"") && html.contains(">Search</button>"),
        "submit button must carry plain, non-icon-only text, got: {html}"
    );
    assert!(
        html.contains(r#"<ul class="search-results-list">"#)
            && html.contains(r#"<li class="search-result">"#),
        "search results must be rendered as a semantic ul/li list, not bare divs, got: {html}"
    );
    assert!(
        html.contains("[EN]"),
        "the source-language badge must be real text, not color-only, got: {html}"
    );
    assert_eq!(
        html.matches("<h1").count(),
        1,
        "the page must establish a single, unambiguous <h1> heading, got: {html}"
    );

    // --- Error state -------------------------------------------------------
    let error_html = tmpl
        .render(context! {
            lang => "en",
            nav_permits => "Permits",
            nav_council => "Council",
            page_title => "Search projects",
            heading => "Search for a project",
            search_label => "Civic address or municipality",
            submit_label => "Search",
            empty_message => "No projects match your search.",
            empty_guidance => "Try broadening your search: use a more general keyword, or double-check the spelling of the address or municipality.",
            query => "accessible",
            has_searched => true,
            search_results => Vec::<minijinja::value::Value>::new(),
            search_error => true,
            search_error_message => "We couldn't complete that search.",
        })
        .unwrap();

    assert!(
        error_html.contains(r#"role="alert""#),
        "the error state must carry role=\"alert\" so assistive tech announces it, got: {error_html}"
    );
}

/// IMP-REQ-002-10: the municipality `<select>` control added by
/// IMP-REQ-002-06/07/08 must follow the same accessible-labeling pattern as
/// the search `<input>` verified in `imp_req_001_10` above — a `<label for>`
/// whose target matches the select's `id`, not a placeholder or bare text.
/// Renders the template directly (no DB round-trip needed for a markup-only
/// assertion).
#[test]
fn imp_req_002_10_municipality_select_meets_basic_accessibility_requirements() {
    let mut env = Environment::new();
    env.set_loader(path_loader("../templates"));
    let tmpl = env.get_template("search.html").unwrap();

    let html = tmpl
        .render(context! {
            lang => "en",
            nav_permits => "Permits",
            nav_council => "Council",
            page_title => "Search projects",
            heading => "Search for a project",
            search_label => "Civic address or municipality",
            municipality_select_label => "Municipality",
            municipality_all_option => "All municipalities",
            submit_label => "Search",
            empty_message => "No projects match your search.",
            empty_guidance => "Try broadening your search: use a more general keyword, or double-check the spelling of the address or municipality.",
            query => "",
            has_searched => false,
            search_results => Vec::<minijinja::value::Value>::new(),
            search_error => false,
            municipalities => vec![minijinja::value::Value::from_serialize(
                serde_json::json!({"slug": "montreal", "display_name": "Montreal"}),
            )],
        })
        .unwrap();

    assert!(
        html.contains(r#"<label for="search-municipality-slug">"#)
            && html.contains(r#"id="search-municipality-slug""#),
        "municipality select must have a <label for> matching its id, got: {html}"
    );
    assert!(
        !html.contains("tabindex=\"-1\""),
        "municipality select must remain keyboard-operable (no tabindex=-1), got: {html}"
    );
    assert!(
        !html.contains("aria-hidden") && !html.contains("disabled"),
        "municipality select and its label must not be hidden from assistive tech \
         or disabled, got: {html}"
    );
}

/// IMP-REQ-002-06: the search form's `<select name="municipality_slug">`
/// must be populated from the live `municipalities` table (migration 002:
/// `montreal`/`toronto`/`vancouver`), not a hardcoded list, and must
/// preserve the currently-selected municipality across a form re-submission
/// rather than resetting to blank.
#[sqlx::test(migrations = "./migrations")]
async fn imp_req_002_06_search_form_has_municipality_select_with_preserved_selection(
    pool: PgPool,
) {
    let app = app(test_state(pool).await);

    // No municipality_slug in the query string: the select must still render
    // with options for all three launch municipalities, localized EN display
    // names by default, and none of them marked selected.
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/search")
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
        html.contains(r#"<select id="search-municipality-slug" name="municipality_slug">"#),
        "expected the municipality select control to render, got: {html}"
    );
    assert!(
        html.contains(r#"<option value="montreal">Montreal</option>"#),
        "expected an unselected Montreal option (EN display name), got: {html}"
    );
    assert!(
        html.contains(r#"<option value="toronto">Toronto</option>"#),
        "expected an unselected Toronto option, got: {html}"
    );
    assert!(
        html.contains(r#"<option value="vancouver">Vancouver</option>"#),
        "expected an unselected Vancouver option, got: {html}"
    );
    assert!(
        !html.contains(r#"value="montreal" selected"#)
            && !html.contains(r#"value="toronto" selected"#)
            && !html.contains(r#"value="vancouver" selected"#),
        "with no municipality_slug in the query string, no municipality option \
         should be marked selected, got: {html}"
    );

    // With municipality_slug=montreal in the query string, the Montreal
    // option must come back marked selected, and the others must not.
    let response = app
        .oneshot(
            Request::builder()
                .uri("/search?municipality_slug=montreal")
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
        html.contains(r#"<option value="montreal" selected>Montreal</option>"#),
        "expected the montreal option to be preserved as selected after \
         re-submitting with municipality_slug=montreal, got: {html}"
    );
    assert!(
        html.contains(r#"<option value="toronto">Toronto</option>"#),
        "toronto option must render without selected, got: {html}"
    );
    assert!(
        html.contains(r#"<option value="vancouver">Vancouver</option>"#),
        "vancouver option must render without selected, got: {html}"
    );
    assert!(
        !html.contains(r#"value="toronto" selected"#),
        "toronto must not be marked selected, got: {html}"
    );
    assert!(
        !html.contains(r#"value="vancouver" selected"#),
        "vancouver must not be marked selected, got: {html}"
    );
}

/// IMP-REQ-002-07: the search form wraps its input/select/submit controls in
/// a `.search-filter-bar` layout (each field further wrapped in
/// `.search-filter-field`, and the submit button carrying
/// `.search-filter-submit`) so the municipality `<select>` added in
/// IMP-REQ-002-06 sits alongside the existing controls without overflowing,
/// per the CSS rules in `static/css/main.css`. This is a static-HTML
/// assertion only (no headless-browser tooling exists yet); the wrapping
/// behavior itself is verified by static review of the CSS.
#[sqlx::test(migrations = "./migrations")]
async fn imp_req_002_07_search_form_has_responsive_filter_bar_wrapper(pool: PgPool) {
    let app = app(test_state(pool).await);

    let response = app
        .oneshot(
            Request::builder()
                .uri("/search")
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
        html.contains(r#"<form role="search" method="get" action="/search" class="search-filter-bar">"#),
        "expected the search form itself to carry the search-filter-bar wrapper class, got: {html}"
    );
    assert_eq!(
        html.matches(r#"class="search-filter-field""#).count(),
        5,
        "expected the address input, municipality select, date-preset select, \
         and date_from/date_to inputs to each be wrapped in a \
         search-filter-field container (IMP-REQ-007-09 added the date-filter \
         fields alongside the pre-existing address/municipality ones), got: {html}"
    );
    assert!(
        html.contains(r#"<button type="submit" class="search-filter-submit">"#),
        "expected the submit button to carry the search-filter-submit class \
         so it can be targeted by the narrow-viewport CSS rules, got: {html}"
    );
}

/// IMP-REQ-002-08: a municipality-scoped search that returns zero matches
/// must render a message naming that specific municipality (e.g. "No
/// projects found in Montreal matching your search"), not the generic
/// `empty_message` from IMP-REQ-001-06. Seeds a project under Toronto only,
/// then searches with `municipality_slug=montreal`: the montreal filter
/// excludes the Toronto project, so the zero-results branch renders — and it
/// must name Montreal specifically, in both English and French.
#[sqlx::test(migrations = "./migrations")]
async fn imp_req_002_08_municipality_scoped_empty_state_names_the_municipality(pool: PgPool) {
    seed_searchable_project_for_municipality_slug(&pool, "88 bay street unique002 08", "toronto")
        .await;
    refresh_public_search_index(&pool).await.unwrap();

    let app = app(test_state(pool).await);

    let en_response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/search?q=unique002+08&municipality_slug=montreal")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(en_response.status(), StatusCode::OK);
    let en_body = http_body_util::BodyExt::collect(en_response.into_body())
        .await
        .unwrap()
        .to_bytes();
    let en_html = String::from_utf8(en_body.to_vec()).unwrap();

    assert!(
        en_html.contains("No projects found in Montreal matching your search."),
        "expected the English empty state to name Montreal specifically, got: {en_html}"
    );
    assert!(
        !en_html.contains("No projects match your search."),
        "the generic IMP-REQ-001-06 empty message must not also render \
         alongside the municipality-specific one, got: {en_html}"
    );

    let fr_response = app
        .oneshot(
            Request::builder()
                .uri("/search?q=unique002+08&municipality_slug=montreal")
                .header("accept-language", "fr")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(fr_response.status(), StatusCode::OK);
    let fr_body = http_body_util::BodyExt::collect(fr_response.into_body())
        .await
        .unwrap()
        .to_bytes();
    let fr_html = String::from_utf8(fr_body.to_vec()).unwrap();

    assert!(
        fr_html.contains("Aucun projet trouvé à Montréal correspondant à votre recherche."),
        "expected the French empty state to name Montréal specifically, got: {fr_html}"
    );
    assert!(
        !fr_html.contains("Aucun projet ne correspond à votre recherche."),
        "the generic IMP-REQ-001-06 French empty message must not also render \
         alongside the municipality-specific one, got: {fr_html}"
    );
}

/// IMP-REQ-002-08 regression guard: a zero-results search with NO
/// municipality filter applied must keep showing the generic IMP-REQ-001-06
/// `empty_message` — the municipality-specific message must only replace it
/// when a municipality filter was actually active.
#[sqlx::test(migrations = "./migrations")]
async fn imp_req_002_08_generic_empty_message_unaffected_when_no_municipality_filter(
    pool: PgPool,
) {
    seed_searchable_project(&pool, "1 unrelated street 00208", "Some Other Town 00208").await;
    refresh_public_search_index(&pool).await.unwrap();

    let app = app(test_state(pool).await);
    let response = app
        .oneshot(
            Request::builder()
                .uri("/search?q=zzz-no-such-project-00208")
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
        html.contains("No projects match your search."),
        "the generic empty message must still render when no municipality \
         filter is active, got: {html}"
    );
    assert!(
        !html.contains("No projects found in"),
        "the municipality-specific phrasing must not appear when no \
         municipality filter is active, got: {html}"
    );
}

/// IMP-REQ-002-08: documents the current (intentional) behavior of the
/// server-rendered `/search` page when `municipality_slug` is syntactically
/// valid but doesn't match any row in `municipalities`. `run_search` (used
/// by both the JSON API and this page) rejects it with
/// `StatusCode::BAD_REQUEST`, but `get_search_page` never propagates that
/// status out of the handler — it's caught by the `Some(Err(_)) => (Vec::new(),
/// true)` arm and rendered as the existing friendly `search-error` state
/// (`role="alert"`, "We couldn't complete that search.") with an HTTP 200.
/// This is already a rendered, user-friendly message rather than a bare
/// status code, so this task leaves it as-is; this test pins that decision
/// rather than silently relying on undocumented behavior.
#[sqlx::test(migrations = "./migrations")]
async fn imp_req_002_08_invalid_municipality_slug_renders_friendly_error_not_bare_status(
    pool: PgPool,
) {
    let app = app(test_state(pool).await);
    let response = app
        .oneshot(
            Request::builder()
                .uri("/search?q=test&municipality_slug=nonexistent-city-00208")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(
        response.status(),
        StatusCode::OK,
        "the HTML page path must render a friendly page, not propagate the \
         underlying 400 as the HTTP response status"
    );
    let body = http_body_util::BodyExt::collect(response.into_body())
        .await
        .unwrap()
        .to_bytes();
    let html = String::from_utf8(body.to_vec()).unwrap();

    assert!(
        html.contains(r#"role="alert""#) && html.contains("We couldn’t complete that search."),
        "expected the existing friendly search-error state to render for an \
         invalid municipality_slug, got: {html}"
    );
}

/// IMP-REQ-003-06: with TWO documents of different `source_language` values
/// both matching the same query in a single result set, each row's
/// `[EN]`/`[FR]` badge must be attributed to that row's own result, not
/// (e.g.) both showing the first result's language. TC-003-5 only ever
/// seeded a single English-language result, so it could not catch a bug
/// where the badge failed to vary per-row within a shared `{% for %}` loop
/// (e.g. a template accidentally closing over the first iteration's
/// variable, or asserting only "the string [FR] appears somewhere in the
/// page" without checking which row it's attached to).
#[sqlx::test(migrations = "./migrations")]
async fn imp_req_003_06_mixed_language_result_set_shows_correct_badges_per_row(pool: PgPool) {
    seed_searchable_project_with_language(
        &pool,
        "15 avenue bilingue english",
        "Ville de Mixed Alpha",
        "en",
    )
    .await;
    seed_searchable_project_with_language(
        &pool,
        "25 avenue bilingue french",
        "Ville de Mixed Beta",
        "fr",
    )
    .await;
    refresh_public_search_index(&pool).await.unwrap();

    let app = app(test_state(pool).await);
    let response = app
        .oneshot(
            Request::builder()
                .uri("/search?q=bilingue")
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

    // Split the results list into individual `<li class="search-result">`
    // rows so each badge can be checked against ITS OWN row's civic address,
    // rather than just confirming "[EN]" and "[FR]" both appear somewhere in
    // the page (which would also pass if both badges were wrongly attached
    // to the same row).
    let rows: Vec<&str> = html.split(r#"<li class="search-result">"#).collect();
    assert_eq!(
        rows.len(),
        3, // rows[0] is everything before the first <li>, then 2 result rows
        "expected exactly 2 search-result rows in the mixed-language result \
         set, got HTML: {html}"
    );

    let english_row = rows
        .iter()
        .find(|row| row.contains("15 avenue bilingue english"))
        .unwrap_or_else(|| panic!("expected a row for the English result, got: {html}"));
    let french_row = rows
        .iter()
        .find(|row| row.contains("25 avenue bilingue french"))
        .unwrap_or_else(|| panic!("expected a row for the French result, got: {html}"));

    assert!(
        english_row.contains("[EN]"),
        "the English-sourced row must carry the [EN] badge, got row: {english_row}"
    );
    assert!(
        !english_row.contains("[FR]"),
        "the English-sourced row must NOT carry the [FR] badge, got row: {english_row}"
    );
    assert!(
        french_row.contains("[FR]"),
        "the French-sourced row must carry the [FR] badge, got row: {french_row}"
    );
    assert!(
        !french_row.contains("[EN]"),
        "the French-sourced row must NOT carry the [EN] badge, got row: {french_row}"
    );
}

/// IMP-REQ-003-07: the search results list wraps each row in
/// `.search-results-list` / `.search-result` and the EN/FR toggle link (added
/// in IMP-REQ-003-08) sits inside a `.lang-toggle` wrapper, so both the
/// per-row `.result-lang-badge` (IMP-REQ-003-06) and the toggle link can be
/// targeted by the narrow-viewport (`max-width: 640px`) rules in
/// `static/css/main.css` that stack the result row and drop the badge's
/// auto-margin instead of overflowing. This is a static-HTML assertion only
/// (no headless-browser tooling exists yet, per the IMP-REQ-002-07
/// precedent); the wrapping behavior itself is verified by static review of
/// the CSS.
#[sqlx::test(migrations = "./migrations")]
async fn imp_req_003_07_search_results_have_responsive_wrapper_classes(pool: PgPool) {
    seed_searchable_project_with_language(&pool, "80 rue responsive", "Ville de Montréal", "en")
        .await;
    refresh_public_search_index(&pool).await.unwrap();

    let app = app(test_state(pool).await);
    let response = app
        .oneshot(
            Request::builder()
                .uri("/search?q=responsive")
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
        html.contains(r#"<p class="lang-toggle">"#),
        "expected the EN/FR toggle link to be wrapped in a lang-toggle \
         container so it can be targeted by the narrow-viewport CSS rules, \
         got: {html}"
    );
    assert!(
        html.contains(r#"<ul class="search-results-list">"#),
        "expected the results list to carry the search-results-list \
         wrapper class, got: {html}"
    );
    assert!(
        html.contains(r#"<li class="search-result">"#),
        "expected each result row to carry the search-result class so the \
         narrow-viewport CSS can stack it instead of letting the badge \
         overflow, got: {html}"
    );
    assert!(
        html.contains(r#"<span class="result-lang-badge">[EN]</span>"#),
        "expected the per-row language badge to carry the result-lang-badge \
         class targeted by the narrow-viewport CSS, got: {html}"
    );
}

/// IMP-REQ-003-08 (a): the search page's EN/FR toggle link renders with the
/// correct target language and `href` on both the English and French
/// renderings of the page, and preserves the current `q`/`municipality_slug`
/// filters in that `href` rather than losing them when the link is
/// followed.
#[sqlx::test(migrations = "./migrations")]
async fn imp_req_003_08_lang_toggle_link_renders_correct_target_and_href(pool: PgPool) {
    seed_searchable_project(&pool, "60 rue toggle", "Ville de Montréal").await;
    refresh_public_search_index(&pool).await.unwrap();

    let app = app(test_state(pool).await);

    // English page (default): toggle must point to French, preserving `q`
    // and `municipality_slug`.
    let en_response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/search?q=toggle&municipality_slug=montreal")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(en_response.status(), StatusCode::OK);
    let en_body = http_body_util::BodyExt::collect(en_response.into_body())
        .await
        .unwrap()
        .to_bytes();
    let en_html = String::from_utf8(en_body.to_vec()).unwrap();

    assert!(
        en_html.contains(r#"id="lang-toggle-link""#),
        "expected a lang-toggle link on the English page, got: {en_html}"
    );
    assert!(
        en_html.contains(">Français<"),
        "the English page's toggle link text must name French (\"Français\"), got: {en_html}"
    );
    // minijinja's default HTML auto-escaping (this template has a `.html`
    // extension) also escapes `/` to `&#x2f;`, not just `&` to `&amp;` — so
    // the expected attribute value below matches that actual escaping, not
    // a literal unescaped URL.
    assert!(
        en_html.contains(
            r#"href="&#x2f;search?q=toggle&amp;municipality_slug=montreal&amp;lang=fr""#
        ),
        "the English page's toggle link must target ?lang=fr while preserving \
         q and municipality_slug, got: {en_html}"
    );

    // French page (`?lang=fr`): toggle must point to English, preserving
    // the same filters.
    let fr_response = app
        .oneshot(
            Request::builder()
                .uri("/search?q=toggle&municipality_slug=montreal&lang=fr")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(fr_response.status(), StatusCode::OK);
    let fr_body = http_body_util::BodyExt::collect(fr_response.into_body())
        .await
        .unwrap()
        .to_bytes();
    let fr_html = String::from_utf8(fr_body.to_vec()).unwrap();

    assert!(
        fr_html.contains(r#"id="lang-toggle-link""#),
        "expected a lang-toggle link on the French page, got: {fr_html}"
    );
    assert!(
        fr_html.contains(">English<"),
        "the French page's toggle link text must name English, got: {fr_html}"
    );
    assert!(
        fr_html.contains(
            r#"href="&#x2f;search?q=toggle&amp;municipality_slug=montreal&amp;lang=en""#
        ),
        "the French page's toggle link must target ?lang=en while preserving \
         q and municipality_slug, got: {fr_html}"
    );
}

/// IMP-REQ-003-08 (b): a request with an explicit `?lang=fr` param must
/// persist that resolved locale as a `lang=fr` Set-Cookie response header —
/// this is the actual gap this task adds on top of TC-003-3's existing
/// coverage (which already proves a `lang=fr` COOKIE sent on a request wins
/// resolution; it does not prove the server ever WRITES that cookie in the
/// first place). A separate follow-up request carrying that cookie but no
/// `?lang=` param must still render French.
#[sqlx::test(migrations = "./migrations")]
async fn imp_req_003_08_lang_param_sets_cookie_and_cookie_persists_across_requests(
    pool: PgPool,
) {
    seed_searchable_project(&pool, "70 rue cookie", "Ville de Montréal").await;
    refresh_public_search_index(&pool).await.unwrap();

    let app = app(test_state(pool).await);

    // First request: explicit ?lang=fr must produce a Set-Cookie: lang=fr
    // response header.
    let first_response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/search?q=cookie&lang=fr")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(first_response.status(), StatusCode::OK);
    let set_cookie = first_response
        .headers()
        .get("set-cookie")
        .unwrap_or_else(|| panic!("expected a Set-Cookie header on the ?lang=fr response"))
        .to_str()
        .unwrap();
    assert!(
        set_cookie.starts_with("lang=fr"),
        "expected Set-Cookie: lang=fr..., got: {set_cookie}"
    );

    // Second, separate request: no ?lang= param, but the lang=fr cookie
    // from the first response is attached — must still render French via
    // cookie-precedence resolution.
    let second_response = app
        .oneshot(
            Request::builder()
                .uri("/search?q=cookie")
                .header("cookie", "lang=fr")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(second_response.status(), StatusCode::OK);
    let second_body = http_body_util::BodyExt::collect(second_response.into_body())
        .await
        .unwrap()
        .to_bytes();
    let second_html = String::from_utf8(second_body.to_vec()).unwrap();
    assert!(
        second_html.contains("Rechercher un projet"),
        "a follow-up request with no ?lang= param but a lang=fr cookie must \
         still render French, got: {second_html}"
    );
}

/// IMP-REQ-003-11: the REQ-003 elements added by IMP-REQ-003-06/07/08 — the
/// per-result `[EN]`/`[FR]` source-language badge and the EN/FR toggle link
/// — must meet basic accessibility requirements: the badge must be plain
/// text content (not conveyed by color/icon alone), the toggle link must
/// carry clear, non-ambiguous link text naming its OWN target language
/// (not "here", not an icon, not empty), and neither element may be pulled
/// out of the keyboard/assistive-tech tree via `tabindex="-1"` or
/// `aria-hidden`. Renders the template directly (no DB round-trip needed
/// for a markup-only assertion), covering both the EN and FR renderings of
/// the toggle link.
#[test]
fn imp_req_003_11_lang_badge_and_toggle_meet_basic_accessibility_requirements() {
    let mut env = Environment::new();
    env.set_loader(path_loader("../templates"));
    let tmpl = env.get_template("search.html").unwrap();

    let base_ctx = serde_json::json!({
        "lang": "en",
        "nav_permits": "Permits",
        "nav_council": "Council",
        "page_title": "Search projects",
        "heading": "Search for a project",
        "search_label": "Civic address or municipality",
        "submit_label": "Search",
        "empty_message": "No projects match your search.",
        "empty_guidance": "Try broadening your search: use a more general keyword, or double-check the spelling of the address or municipality.",
        "query": "accessible",
        "has_searched": true,
        "search_error": false,
        "result_count_label": "2 results found",
        "search_results": [
            {
                "project_id": "11111111-1111-1111-1111-111111111111",
                "civic_address_normalized": "15 rue accessible",
                "municipality_name": "Ville de Accessibilite",
                "normalized_status": "approved",
                "source_language": "en",
            },
            {
                "project_id": "22222222-2222-2222-2222-222222222222",
                "civic_address_normalized": "16 rue accessible",
                "municipality_name": "Ville de Accessibilite",
                "normalized_status": "approved",
                "source_language": "fr",
            },
        ],
    });

    // --- English page: toggle names the target language, "Français" -------
    let mut en_ctx = base_ctx.clone();
    en_ctx["lang_toggle_href"] = serde_json::json!("/search?lang=fr");
    en_ctx["lang_toggle_label"] = serde_json::json!("Français");
    let en_html = tmpl
        .render(minijinja::value::Value::from_serialize(&en_ctx))
        .unwrap();

    assert_lang_badges_and_toggle_are_accessible(&en_html, "Français");

    // --- French page: toggle names the target language, "English" ---------
    let mut fr_ctx = base_ctx;
    fr_ctx["lang_toggle_href"] = serde_json::json!("/search?lang=en");
    fr_ctx["lang_toggle_label"] = serde_json::json!("English");
    let fr_html = tmpl
        .render(minijinja::value::Value::from_serialize(&fr_ctx))
        .unwrap();

    assert_lang_badges_and_toggle_are_accessible(&fr_html, "English");
}

/// Shared assertions for `imp_req_003_11`: given a rendered search page and
/// the expected toggle-link text (the target language's own name), verify
/// the badge and toggle both meet the acceptance criteria.
fn assert_lang_badges_and_toggle_are_accessible(html: &str, expected_toggle_text: &str) {
    // The `[EN]`/`[FR]` badges must be literal text content inside the
    // markup (not a `::before`/`::after` CSS-generated swatch, and not
    // hidden from assistive tech), so a plain substring match on the
    // rendered HTML is sufficient proof they are real, readable text nodes.
    assert!(
        html.contains(r#"<span class="result-lang-badge">[EN]</span>"#),
        "the EN badge must be plain text content, not color/icon-only, got: {html}"
    );
    assert!(
        html.contains(r#"<span class="result-lang-badge">[FR]</span>"#),
        "the FR badge must be plain text content, not color/icon-only, got: {html}"
    );

    // The toggle link's inner text must be its own non-empty target-language
    // name — not "here", not an icon-only glyph. Extract the anchor's exact
    // text content between its opening and closing tags to assert on it
    // concretely, rather than merely checking substring containment.
    let anchor_start = html
        .find(r#"<a id="lang-toggle-link""#)
        .expect("expected a #lang-toggle-link anchor to be present");
    let tag_open_end = html[anchor_start..]
        .find('>')
        .map(|i| anchor_start + i + 1)
        .expect("malformed anchor tag: no closing '>' found");
    let tag_close_start = html[tag_open_end..]
        .find("</a>")
        .map(|i| tag_open_end + i)
        .expect("malformed anchor tag: no closing </a> found");
    let anchor_tag = &html[anchor_start..tag_open_end];
    let link_text = html[tag_open_end..tag_close_start].trim();

    assert_eq!(
        link_text, expected_toggle_text,
        "the toggle link's text content must be exactly its own target \
         language's name, not a generic phrase or icon, got link text: {link_text:?}"
    );
    assert!(
        !link_text.is_empty() && link_text != "here",
        "the toggle link text must not be empty or a bare \"here\", got: {link_text:?}"
    );

    // Neither the toggle link nor the badges may be removed from the
    // keyboard/assistive-tech tree.
    assert!(
        !anchor_tag.contains("tabindex=\"-1\"") && !anchor_tag.contains("aria-hidden"),
        "the toggle link must remain keyboard-reachable and exposed to \
         assistive tech (no tabindex=-1 / aria-hidden), got anchor tag: {anchor_tag}"
    );
    assert!(
        !html.contains(r#"<span class="result-lang-badge" aria-hidden"#)
            && !html.contains(r#"<span class="result-lang-badge" tabindex="-1""#),
        "the language badges must not be hidden from assistive tech or \
         pulled out of tab order, got: {html}"
    );
}

/// IMP-REQ-004-06: `GET /search`'s results fragment must render real
/// pagination controls derived from the actual `PaginationInfo` — a "Next"
/// link present only when a further page remains, absent on the last page,
/// and a "Previous" link present from page 2 onward — plus each result row
/// showing the synthesized `display_name` (e.g. "Residential — 1 rue
/// pagination alpha"), not the bare `civic_address_normalized`.
#[sqlx::test(migrations = "./migrations")]
async fn imp_req_004_06_pagination_controls_reflect_has_more_and_page(pool: PgPool) {
    // `seed_searchable_project` always inserts `project_type = 'residential'`,
    // so each row's synthesized display name is "Residential — <address>".
    seed_searchable_project(&pool, "1 rue pagination alpha", "Ville de Pagination Alpha").await;
    seed_searchable_project(&pool, "2 rue pagination beta", "Ville de Pagination Beta").await;
    seed_searchable_project(&pool, "3 rue pagination gamma", "Ville de Pagination Gamma").await;
    refresh_public_search_index(&pool).await.unwrap();

    let app = app(test_state(pool).await);

    // --- Page 1 of 2 (per_page=2, total=3): Next present, Previous absent ---
    let page_1_response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/search?q=pagination&per_page=2")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(page_1_response.status(), StatusCode::OK);
    let page_1_body = http_body_util::BodyExt::collect(page_1_response.into_body())
        .await
        .unwrap()
        .to_bytes();
    let page_1_html = String::from_utf8(page_1_body.to_vec()).unwrap();

    assert!(
        page_1_html.contains("Residential — 1 rue pagination alpha")
            || page_1_html.contains("Residential — 2 rue pagination beta"),
        "expected the synthesized display_name (not the bare civic address) \
         to be rendered in a result row, got: {page_1_html}"
    );
    assert!(
        page_1_html.contains(r#"class="search-pagination-next""#)
            && page_1_html.contains("page=2"),
        "expected a Next link to page=2 on page 1 of a 2-page result set, \
         got: {page_1_html}"
    );
    assert!(
        !page_1_html.contains(r#"class="search-pagination-prev""#),
        "page 1 must not show a Previous link, got: {page_1_html}"
    );

    // --- Page 2 of 2: Previous present, Next absent (last page) -------------
    let page_2_response = app
        .oneshot(
            Request::builder()
                .uri("/search?q=pagination&per_page=2&page=2")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(page_2_response.status(), StatusCode::OK);
    let page_2_body = http_body_util::BodyExt::collect(page_2_response.into_body())
        .await
        .unwrap()
        .to_bytes();
    let page_2_html = String::from_utf8(page_2_body.to_vec()).unwrap();

    assert!(
        page_2_html.contains("Residential — 3 rue pagination gamma"),
        "expected page 2's single remaining result's synthesized display_name, \
         got: {page_2_html}"
    );
    assert!(
        page_2_html.contains(r#"class="search-pagination-prev""#)
            && page_2_html.contains("page=1"),
        "expected a Previous link to page=1 on page 2, got: {page_2_html}"
    );
    assert!(
        !page_2_html.contains(r#"class="search-pagination-next""#),
        "page 2 (the last page) must not show a Next link, got: {page_2_html}"
    );
}

/// IMP-REQ-004-07: the results fragment's "Next" link must be progressively
/// enhanced with htmx infinite-scroll attributes on top of — not instead of —
/// its plain `href` fallback. `hx-get` must target the exact same URL as
/// `href` (the already-computed `next_page_href`), `hx-trigger="revealed"`
/// fires the fetch when the link scrolls into view (htmx's built-in
/// "revealed" trigger, no custom JS), and `hx-swap="outerHTML"` lets the
/// fetched next page's own fragment (its own results + its own new "Next"
/// link) replace this link in place, continuing the chain. Absent htmx/JS,
/// the plain `href` still navigates normally — this test pins that both
/// coexist on the same element.
#[sqlx::test(migrations = "./migrations")]
async fn imp_req_004_07_next_link_carries_htmx_infinite_scroll_attributes_alongside_href(
    pool: PgPool,
) {
    seed_searchable_project(&pool, "1 rue infinite alpha", "Ville de Infinite Alpha").await;
    seed_searchable_project(&pool, "2 rue infinite beta", "Ville de Infinite Beta").await;
    seed_searchable_project(&pool, "3 rue infinite gamma", "Ville de Infinite Gamma").await;
    refresh_public_search_index(&pool).await.unwrap();

    let app = app(test_state(pool).await);

    let response = app
        .oneshot(
            Request::builder()
                .uri("/search?q=infinite&per_page=2")
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

    let next_link_start = html
        .find(r#"class="search-pagination-next""#)
        .expect("expected a Next link on page 1 of a 2-page result set");
    // The class attribute is one attribute among several on the same `<a>`
    // tag; grab the whole opening tag (from the preceding `<a` to the
    // closing `>`) so all its attributes can be inspected together.
    let tag_start = html[..next_link_start].rfind("<a").unwrap();
    let tag_end = tag_start + html[tag_start..].find('>').unwrap() + 1;
    let next_link_tag = &html[tag_start..tag_end];

    // Minijinja HTML-escapes attribute values by default (`/` -> `&#x2f;`,
    // `&` -> `&amp;`), so the expected href/hx-get value is the escaped form,
    // not the raw URL.
    let expected_href_escaped = "&#x2f;search?q=infinite&amp;page=2&amp;lang=en";
    assert!(
        next_link_tag.contains(&format!("href=\"{expected_href_escaped}\"")),
        "the plain href fallback must still be present on the Next link \
         (progressive enhancement, not a JS-required control), got tag: \
         {next_link_tag}"
    );
    assert!(
        next_link_tag.contains(&format!("hx-get=\"{expected_href_escaped}\"")),
        "hx-get must target the exact same URL as the plain href, got tag: \
         {next_link_tag}"
    );
    assert!(
        next_link_tag.contains(r#"hx-trigger="revealed""#),
        "the Next link must auto-load via htmx's \"revealed\" trigger \
         (scrolled into view), got tag: {next_link_tag}"
    );
    assert!(
        next_link_tag.contains(r#"hx-swap="outerHTML""#),
        "the Next link must swap its own outerHTML with the next page's \
         fragment so the chain continues, got tag: {next_link_tag}"
    );
}

/// IMP-REQ-004-08: the `normalized_status` value rendered in each result row
/// must carry a `status-indicator`/`status-indicator--<status>` class pair
/// so the per-status CSS (main.css) can color-code it, and the class must
/// reflect the row's *actual* `normalized_status` value (migration 008's
/// vocabulary: proposed/approved/deferred/referred/rejected) rather than a
/// hardcoded default.
#[sqlx::test(migrations = "./migrations")]
async fn imp_req_004_08_status_indicator_carries_status_specific_class(pool: PgPool) {
    // `seed_searchable_project` always inserts `normalized_status = 'approved'`.
    seed_searchable_project(&pool, "1 rue status indicator", "Ville de Statut").await;
    refresh_public_search_index(&pool).await.unwrap();

    let app = app(test_state(pool).await);
    let response = app
        .oneshot(
            Request::builder()
                .uri("/search?q=indicator")
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
        html.contains(r#"class="status-indicator status-indicator--approved""#),
        "expected the status span to carry a status-specific \
         status-indicator--approved class reflecting the row's actual \
         normalized_status, got: {html}"
    );
    assert!(
        html.contains(">approved<"),
        "the status class alone is not enough — the actual status word \
         must still be rendered as visible text, got: {html}"
    );
}

/// IMP-REQ-004-11: confirms the accessibility properties the pagination
/// controls and status indicator must have — pagination links carry real,
/// non-empty text (not icon-only), the status indicator's meaning is
/// carried by text and not by color alone, and the htmx-enhanced "Next"
/// link (IMP-REQ-004-07) remains a genuine `<a href>` rather than a
/// `<button>`/`<div>` that would lose native link keyboard/screen-reader
/// semantics.
#[sqlx::test(migrations = "./migrations")]
async fn imp_req_004_11_pagination_and_status_preserve_accessible_semantics(pool: PgPool) {
    seed_searchable_project(&pool, "1 rue a11y alpha", "Ville de Accessible Alpha").await;
    seed_searchable_project(&pool, "2 rue a11y beta", "Ville de Accessible Beta").await;
    seed_searchable_project(&pool, "3 rue a11y gamma", "Ville de Accessible Gamma").await;
    refresh_public_search_index(&pool).await.unwrap();

    let app = app(test_state(pool).await);

    // Page 2 so both Previous and (via page 1) Next links can be inspected
    // in the same fetch pair below.
    let page_1_response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/search?q=a11y&per_page=2")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(page_1_response.status(), StatusCode::OK);
    let page_1_body = http_body_util::BodyExt::collect(page_1_response.into_body())
        .await
        .unwrap()
        .to_bytes();
    let page_1_html = String::from_utf8(page_1_body.to_vec()).unwrap();

    // --- Status is real text, not just a color class ------------------------
    assert!(
        page_1_html.contains(">approved<"),
        "the status indicator must render the status as visible text \
         alongside its color class, got: {page_1_html}"
    );

    // --- Next link is a genuine <a>, carries the exact same href/hx-get, and
    //     has non-empty, non-icon-only link text ----------------------------
    let next_link_start = page_1_html
        .find(r#"class="search-pagination-next""#)
        .expect("expected a Next link on page 1 of a 2-page result set");
    let tag_start = page_1_html[..next_link_start].rfind("<a").unwrap();
    let tag_end = tag_start + page_1_html[tag_start..].find('>').unwrap() + 1;
    let next_link_tag = &page_1_html[tag_start..tag_end];
    assert!(
        next_link_tag.starts_with("<a "),
        "the Next control must be a real <a> element (not a <button> or \
         <div> masquerading as a link), got tag: {next_link_tag}"
    );
    assert!(
        next_link_tag.contains("href=\""),
        "the Next link must keep a plain href so it stays keyboard- and \
         screen-reader-navigable without JS, got tag: {next_link_tag}"
    );
    let close_tag_idx = page_1_html[tag_end..].find("</a>").unwrap();
    let next_link_text = page_1_html[tag_end..tag_end + close_tag_idx].trim();
    assert!(
        !next_link_text.is_empty(),
        "the Next link must have non-empty, human-readable text content \
         (not icon-only), got tag+text: {next_link_tag}{next_link_text}"
    );

    // --- Previous link on page 2: same non-empty-text requirement -----------
    let page_2_response = app
        .oneshot(
            Request::builder()
                .uri("/search?q=a11y&per_page=2&page=2")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(page_2_response.status(), StatusCode::OK);
    let page_2_body = http_body_util::BodyExt::collect(page_2_response.into_body())
        .await
        .unwrap()
        .to_bytes();
    let page_2_html = String::from_utf8(page_2_body.to_vec()).unwrap();

    let prev_link_start = page_2_html
        .find(r#"class="search-pagination-prev""#)
        .expect("expected a Previous link on page 2");
    let prev_tag_start = page_2_html[..prev_link_start].rfind("<a").unwrap();
    let prev_tag_end = prev_tag_start + page_2_html[prev_tag_start..].find('>').unwrap() + 1;
    let prev_link_tag = &page_2_html[prev_tag_start..prev_tag_end];
    assert!(
        prev_link_tag.starts_with("<a "),
        "the Previous control must be a real <a> element, got tag: {prev_link_tag}"
    );
    let prev_close_idx = page_2_html[prev_tag_end..].find("</a>").unwrap();
    let prev_link_text = page_2_html[prev_tag_end..prev_tag_end + prev_close_idx].trim();
    assert!(
        !prev_link_text.is_empty(),
        "the Previous link must have non-empty, human-readable text content, \
         got tag+text: {prev_link_tag}{prev_link_text}"
    );
}

/// IMP-REQ-007-09/-10: the date-filter preset `<select>` must render and
/// must reflect the CURRENT query-string state across a request, the same
/// way the pre-existing municipality `<select>` already does
/// (`imp_req_002_06`). Covers all three presets: no date params (`any_time`
/// selected, custom fields empty), `date_preset=last_7_days` (that option
/// selected), and a custom `date_from`/`date_to` pair (`custom_range`
/// selected AND both date inputs retain their submitted values).
#[sqlx::test(migrations = "./migrations")]
async fn imp_req_007_09_10_date_preset_control_reflects_query_string_state(pool: PgPool) {
    let app = app(test_state(pool).await);

    // No date params at all: "Any time" must be the selected option, and
    // both custom-range inputs must render empty.
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/search")
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
        html.contains(r#"<select id="search-date-preset" name="date_preset">"#),
        "expected the date-preset select control to render, got: {html}"
    );
    assert!(
        html.contains(r#"<option value="" selected>Any time</option>"#),
        "expected 'Any time' selected with no date params present, got: {html}"
    );
    assert!(
        html.contains(r#"<input type="date" id="search-date-from" name="date_from" value="">"#),
        "expected an empty date_from input with no date params present, got: {html}"
    );
    assert!(
        html.contains(r#"<input type="date" id="search-date-to" name="date_to" value="">"#),
        "expected an empty date_to input with no date params present, got: {html}"
    );
    assert!(
        !html.contains(r#"value="last_7_days" selected"#),
        "the last_7_days option must not be selected with no date params, got: {html}"
    );

    // date_preset=last_7_days: that option must come back selected.
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/search?date_preset=last_7_days")
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
        html.contains(r#"<option value="last_7_days" selected>Last 7 days</option>"#),
        "expected the last_7_days option selected after re-submitting with \
         date_preset=last_7_days, got: {html}"
    );

    // A custom date_from/date_to range: "Custom range" must be selected AND
    // both inputs must retain their submitted values across the request.
    let response = app
        .oneshot(
            Request::builder()
                .uri("/search?date_from=2026-06-01&date_to=2026-06-30")
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
        html.matches("selected>Custom range</option>").count() == 1,
        "expected the Custom range option selected when date_from/date_to \
         are present, got: {html}"
    );
    assert!(
        html.contains(
            r#"<input type="date" id="search-date-from" name="date_from" value="2026-06-01">"#
        ),
        "expected date_from's submitted value to be preserved on the input, got: {html}"
    );
    assert!(
        html.contains(
            r#"<input type="date" id="search-date-to" name="date_to" value="2026-06-30">"#
        ),
        "expected date_to's submitted value to be preserved on the input, got: {html}"
    );
}

/// IMP-REQ-007-10/-11: the applied date-filter chip must appear only when a
/// date filter is active, must show clear removable-indicator text, its "x"
/// clear-link must carry a real accessible name (not a bare "x" glyph), and
/// that link must remove ONLY the date params from the query string while
/// preserving `q`/`municipality_slug`/`lang`. Also confirms the chip is
/// entirely absent when no date filter is active.
#[sqlx::test(migrations = "./migrations")]
async fn imp_req_007_10_11_applied_filter_chip_appears_and_clears_only_date_params(
    pool: PgPool,
) {
    seed_searchable_project(&pool, "1 rue chip test", "Ville de Chip").await;
    refresh_public_search_index(&pool).await.unwrap();

    let app = app(test_state(pool).await);

    // No date filter active: the chip must not render at all.
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/search?q=chip")
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
        !html.contains(r#"id="date-filter-chip""#),
        "expected no applied-filter chip when no date filter is active, got: {html}"
    );

    // A date filter (last_7_days) IS active, alongside q/municipality_slug:
    // the chip must render, with a real accessible clear-label (not a bare
    // "x"), and its href must preserve q/municipality_slug/lang while
    // dropping date_preset entirely.
    let response = app
        .oneshot(
            Request::builder()
                .uri("/search?q=chip&municipality_slug=montreal&date_preset=last_7_days&lang=en")
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
        html.contains(r#"id="date-filter-chip""#),
        "expected an applied-filter chip when a date filter is active, got: {html}"
    );
    assert!(
        html.contains("Last 7 days"),
        "expected the chip to show the active preset's label, got: {html}"
    );
    assert!(
        html.contains(r#"aria-label="Clear date filter""#),
        "expected the chip's clear link to carry a real accessible name \
         (aria-label), not a bare '×' glyph, got: {html}"
    );

    let clear_href_start = html
        .find(r#"class="search-filter-chip-clear""#)
        .expect("expected the chip's clear link to carry its own class");
    let tag_start = html[..clear_href_start].rfind("<a").unwrap();
    let href_start = html[tag_start..].find("href=\"").unwrap() + tag_start + 6;
    let href_end = html[href_start..].find('"').unwrap() + href_start;
    let clear_href = &html[href_start..href_end];

    assert!(
        clear_href.contains("q=chip"),
        "expected the clear link to preserve q, got href: {clear_href}"
    );
    assert!(
        clear_href.contains("municipality_slug=montreal"),
        "expected the clear link to preserve municipality_slug, got href: {clear_href}"
    );
    assert!(
        clear_href.contains("lang=en"),
        "expected the clear link to preserve lang, got href: {clear_href}"
    );
    assert!(
        !clear_href.contains("date_preset")
            && !clear_href.contains("date_from")
            && !clear_href.contains("date_to"),
        "expected the clear link to drop every date param, got href: {clear_href}"
    );
}

/// IMP-REQ-007-11: the date-preset select must have a proper `<label for>`
/// association (matching the same pattern already verified for the
/// municipality select in `imp_req_002_10`), and the two custom-range date
/// inputs must each have their own associated `<label for>` too — none of
/// the three controls may rely on a placeholder or bare text instead of a
/// real label. Renders the template directly (no DB round-trip needed for a
/// markup-only assertion), matching `imp_req_002_10`'s approach.
#[test]
fn imp_req_007_11_date_filter_controls_meet_basic_accessibility_requirements() {
    let mut env = Environment::new();
    env.set_loader(path_loader("../templates"));
    let tmpl = env.get_template("search.html").unwrap();

    let html = tmpl
        .render(context! {
            lang => "en",
            nav_permits => "Permits",
            nav_council => "Council",
            page_title => "Search projects",
            heading => "Search for a project",
            search_label => "Civic address or municipality",
            municipality_select_label => "Municipality",
            municipality_all_option => "All municipalities",
            date_filter_label => "Date filter",
            date_preset_any_time_option => "Any time",
            date_preset_last_7_days_option => "Last 7 days",
            date_preset_custom_range_option => "Custom range",
            date_from_label => "From",
            date_to_label => "To",
            date_filter_clear_label => "Clear date filter",
            submit_label => "Search",
            empty_message => "No projects match your search.",
            empty_guidance => "Try broadening your search: use a more general keyword, or double-check the spelling of the address or municipality.",
            query => "",
            has_searched => false,
            search_results => Vec::<minijinja::value::Value>::new(),
            search_error => false,
            municipalities => Vec::<minijinja::value::Value>::new(),
            date_preset_selection => "custom_range",
            date_from_value => "2026-06-01",
            date_to_value => "2026-06-30",
            date_filter_chip_label => "From 2026-06-01 to 2026-06-30",
            clear_date_filter_href => "/search?lang=en",
        })
        .unwrap();

    assert!(
        html.contains(r#"<label for="search-date-preset">"#)
            && html.contains(r#"id="search-date-preset""#),
        "date-preset select must have a <label for> matching its id, got: {html}"
    );
    assert!(
        html.contains(r#"<label for="search-date-from">"#)
            && html.contains(r#"id="search-date-from""#),
        "date_from input must have a <label for> matching its id, got: {html}"
    );
    assert!(
        html.contains(r#"<label for="search-date-to">"#)
            && html.contains(r#"id="search-date-to""#),
        "date_to input must have a <label for> matching its id, got: {html}"
    );
    assert!(
        html.contains(r#"aria-label="Clear date filter""#),
        "the chip's clear link must carry a real accessible name via \
         aria-label, not rely on the bare '×' glyph alone, got: {html}"
    );
    assert!(
        !html.contains("tabindex=\"-1\""),
        "date-filter controls must remain keyboard-operable, got: {html}"
    );
    assert!(
        !html.contains("aria-hidden") && !html.contains("disabled"),
        "date-filter controls and their labels must not be hidden from \
         assistive tech or disabled, got: {html}"
    );
}

/// IMP-REQ-007-13: bilingual QA pass — every new date-filter UI string
/// (preset option labels, custom-range field labels, and the applied chip's
/// label + accessible clear-text) must render correctly in BOTH English and
/// French, matching the wording committed in `search_labels`.
#[sqlx::test(migrations = "./migrations")]
async fn imp_req_007_13_date_filter_ui_is_fully_bilingual(pool: PgPool) {
    seed_searchable_project(&pool, "1 rue bilingue", "Ville Bilingue").await;
    refresh_public_search_index(&pool).await.unwrap();

    let app = app(test_state(pool).await);

    // English.
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/search?q=bilingue&date_preset=last_7_days&lang=en")
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
    let en_html = String::from_utf8(body.to_vec()).unwrap();

    assert!(en_html.contains(">Date filter<"), "got: {en_html}");
    assert!(en_html.contains(">Any time<"), "got: {en_html}");
    assert!(en_html.contains(">Last 7 days<"), "got: {en_html}");
    assert!(en_html.contains(">Custom range<"), "got: {en_html}");
    assert!(en_html.contains(">From<"), "got: {en_html}");
    assert!(en_html.contains(">To<"), "got: {en_html}");
    assert!(
        en_html.contains("aria-label=\"Clear date filter\""),
        "got: {en_html}"
    );

    // French.
    let response = app
        .oneshot(
            Request::builder()
                .uri("/search?q=bilingue&date_preset=last_7_days&lang=fr")
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
    let fr_html = String::from_utf8(body.to_vec()).unwrap();

    assert!(fr_html.contains(">Filtre de date<"), "got: {fr_html}");
    assert!(fr_html.contains(">Toute période<"), "got: {fr_html}");
    assert!(fr_html.contains(">7 derniers jours<"), "got: {fr_html}");
    assert!(fr_html.contains(">Plage personnalisée<"), "got: {fr_html}");
    assert!(fr_html.contains(">Depuis<"), "got: {fr_html}");
    assert!(
        fr_html.contains(">Jusqu&#x27;au<") || fr_html.contains(">Jusqu'au<"),
        "got: {fr_html}"
    );
    assert!(
        fr_html.contains("aria-label=\"Effacer le filtre de date\""),
        "got: {fr_html}"
    );
    assert!(fr_html.contains("7 derniers jours"), "got: {fr_html}");
}

fn rand_octet() -> u8 {
    use std::time::{SystemTime, UNIX_EPOCH};
    (SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .subsec_nanos()
        % 254) as u8
        + 1
}
