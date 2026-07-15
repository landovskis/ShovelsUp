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
    let results: Vec<Value> = serde_json::from_slice(&body).unwrap();
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
    let results: Vec<Value> = serde_json::from_slice(&body).unwrap();
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
    let montreal_project =
        seed_searchable_project(&pool, "1000 rue sainte-catherine", "Montreal Borough One").await;
    seed_searchable_project(&pool, "1000 yonge street", "Toronto Borough One").await;
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
    let results: Vec<Value> = serde_json::from_slice(&body).unwrap();
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
        seed_searchable_project(&pool, "500 rue principale", "Montreal Ward Alpha").await;
    seed_searchable_project(&pool, "600 avenue du parc", "Montreal Ward Beta").await;
    seed_searchable_project(&pool, "700 principale road", "Toronto Ward Alpha").await;
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
    let results: Vec<Value> = serde_json::from_slice(&body).unwrap();
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
    let results: Vec<Value> = serde_json::from_slice(&body).unwrap();
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
    let results: Vec<Value> = serde_json::from_slice(&body).unwrap();
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
    let montreal_project =
        seed_searchable_project(&pool, "9 place jacques-cartier", "Montreal District Y").await;
    seed_searchable_project(&pool, "9 dundas square", "Toronto District Y").await;
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
    let results: Vec<Value> = serde_json::from_slice(&body).unwrap();
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
    let results: Vec<Value> = serde_json::from_slice(&body).unwrap();
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
    seed_searchable_project(&pool, "5 avenue du parc", "Ville de Montréal").await;
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

/// TC-004-1: the JSON API is documented to eventually return a paginated
/// envelope (`{results, total, page, per_page, has_more}`), but
/// `search_projects` still returns a bare array today. Seeds 3 matching
/// projects with `per_page=2` (fewer than total matches) and asserts
/// today's actual response is a bare JSON array of length 2 — NOT an
/// envelope object — documenting the gap IMP-REQ-004-04 must close by
/// wiring `SearchResultsEnvelope` (added as a dead-code stub in
/// `search.rs`) into the handler's real return type.
#[sqlx::test(migrations = "./migrations")]
async fn tc_004_1_json_api_returns_bare_array_not_paginated_envelope(pool: PgPool) {
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
        value.is_array(),
        "documents today's gap: the handler still returns a bare JSON array, \
         not a {{results, total, page, per_page, has_more}} envelope, got: {value:?}"
    );
    let results = value.as_array().unwrap();
    assert_eq!(
        results.len(),
        2,
        "per_page=2 should still cap today's bare-array response, got: {results:?}"
    );
    assert!(
        value.get("total").is_none() && value.get("has_more").is_none(),
        "no envelope fields exist yet on the bare-array response today, got: {value:?}"
    );
}

/// TC-004-2: an HTMX request (`HX-Request: true`) to `GET /search` should
/// eventually receive only the results fragment, while a plain browser
/// request receives the full page. This branching doesn't exist yet in
/// `get_search_page`, so both requests currently return identical full-page
/// HTML (with `<html>`/`<head>` chrome) — this test documents that gap by
/// asserting the HTMX-header response STILL contains full page chrome today.
#[sqlx::test(migrations = "./migrations")]
async fn tc_004_2_htmx_request_still_returns_full_page_not_fragment(pool: PgPool) {
    seed_searchable_project(&pool, "8 rue htmx fragment", "Ville de Fragments").await;
    refresh_public_search_index(&pool).await.unwrap();

    let app = app(test_state(pool).await);
    let response = app
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
        html.contains("<html") && html.contains("<head"),
        "documents today's gap: an HX-Request: true request still gets full \
         page chrome instead of a results-only fragment (fixed by \
         IMP-REQ-004-05), got: {html}"
    );
}

/// TC-004-3: each result should eventually carry a synthesized display name
/// derived from civic address + project type (e.g. "Demolition — 123 Main
/// St"), but no such synthesis logic exists yet — `SearchResult::display_name`
/// is a Loop A stub hard-coded to `None`. This asserts the field is present
/// in the JSON body but currently null, documenting the gap.
#[sqlx::test(migrations = "./migrations")]
async fn tc_004_3_display_name_is_absent_pending_synthesis(pool: PgPool) {
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
    let results: Vec<Value> = serde_json::from_slice(&body).unwrap();
    assert_eq!(results.len(), 1);
    assert!(
        results[0].get("display_name").is_some(),
        "the display_name key must be present in the JSON body, got: {:?}",
        results[0]
    );
    assert!(
        results[0]["display_name"].is_null(),
        "documents today's gap: no synthesis logic exists yet, so \
         display_name is still null (fixed by IMP-REQ-004-09, e.g. \
         'Demolition — 123 Main St'), got: {:?}",
        results[0]
    );
}

/// TC-004-4: `first_surfaced_at` must be set once on first insert into
/// `public_search_documents` and never change on subsequent refresh-job
/// upserts of the same project. Blocked on IMP-REQ-004-01 (the migration
/// adding the column) — `refresh_public_search_index`'s `INSERT ...
/// ON CONFLICT DO UPDATE` (apps/web/web/src/jobs/public_search_refresh.rs)
/// has no `first_surfaced_at` clause at all today, so this can't compile
/// against a real column. Left `#[ignore]`d (not `xfail`) with the full
/// intended assertion body so Loop B need only remove the attribute once
/// the column exists and the refresh job's `ON CONFLICT DO UPDATE` is
/// updated to leave it untouched.
#[ignore = "blocked on IMP-REQ-004-01 migration adding first_surfaced_at"]
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

/// TC-004-5: requesting a `page` beyond the last page should eventually
/// return an empty `results` array with `has_more: false`, not an error.
/// No `page` param is wired into `run_search` yet (`SearchParams::page` is
/// a Loop A stub), so today the handler ignores it entirely and returns the
/// same (non-empty, first-"page") bare array regardless of the `page`
/// value — this documents that gap.
#[sqlx::test(migrations = "./migrations")]
async fn tc_004_5_out_of_range_page_is_ignored_not_empty_today(pool: PgPool) {
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
        "an out-of-range page must not be an error, even before pagination is wired"
    );
    let body = http_body_util::BodyExt::collect(response.into_body())
        .await
        .unwrap()
        .to_bytes();
    let results: Vec<Value> = serde_json::from_slice(&body).unwrap();
    assert_eq!(
        results.len(),
        1,
        "documents today's gap: page=999 is silently ignored (not read by \
         run_search yet), so the single match still comes back instead of an \
         empty page (fixed by IMP-REQ-004-03/04), got: {results:?}"
    );
}

/// TC-007-1: `date_preset=last_7_days` returns only projects surfaced within
/// the last 7 UTC days, excluding an older fixture.
///
/// Blocked on IMP-REQ-004-01 (the migration adding
/// `public_search_documents.first_surfaced_at`) and IMP-REQ-007-05 (wiring
/// `date_preset` into `run_search`'s query). Written directly against
/// `first_surfaced_at` via runtime-checked `sqlx::query` (not the
/// compile-time-checked macros, which would fail `cargo build` today against
/// a schema lacking the column), with the full intended assertion body, per
/// the same pattern as TC-004-4.
#[ignore = "blocked on IMP-REQ-004-01 migration (first_surfaced_at) and IMP-REQ-007-05 (date filter wiring)"]
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
    let results: Vec<Value> = serde_json::from_slice(&body).unwrap();
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
/// Blocked on IMP-REQ-004-01 (first_surfaced_at) and IMP-REQ-007-05 (date
/// filter wiring), same as TC-007-1.
#[ignore = "blocked on IMP-REQ-004-01 migration (first_surfaced_at) and IMP-REQ-007-05 (date filter wiring)"]
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
    let results: Vec<Value> = serde_json::from_slice(&body).unwrap();
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
/// 400/409 before any query runs. No such validation exists yet
/// (IMP-REQ-007-03), so this currently fails: the handler runs the query
/// anyway (ignoring the stub params) and returns 200.
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

/// TC-007-4: a malformed date string (not ISO 8601) is rejected with 400,
/// not a 500/panic. No parsing/validation exists yet (IMP-REQ-007-03), so
/// this currently fails: the handler ignores the unparsed stub field
/// entirely and returns 200.
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
/// backward-compatible with REQ-001/002's plain search. Trivially true today
/// since date filtering isn't wired into `run_search` yet.
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
    let results: Vec<Value> = serde_json::from_slice(&body).unwrap();
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
/// Blocked on IMP-REQ-004-01 (first_surfaced_at) and IMP-REQ-007-05 (date
/// filter wiring), same as TC-007-1/-2. Written directly against
/// `first_surfaced_at` via runtime-checked `sqlx::query`, with the full
/// intended assertion body.
#[ignore = "blocked on IMP-REQ-004-01 migration (first_surfaced_at) and IMP-REQ-007-05 (date filter wiring)"]
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
    let results: Vec<Value> = serde_json::from_slice(&body).unwrap();
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
        html.contains("<button type=\"submit\">Search</button>"),
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

fn rand_octet() -> u8 {
    use std::time::{SystemTime, UNIX_EPOCH};
    (SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .subsec_nanos()
        % 254) as u8
        + 1
}
