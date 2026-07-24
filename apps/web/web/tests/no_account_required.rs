//! REQ-010 Loop A: TC-010-01..05 — structural + HTTP-level regression guards
//! that no account/session/auth is ever required to search or view public
//! project data.
//!
//! Chosen as a new, separate file (rather than folding into
//! `search_integration.rs` / `timeline_resolver.rs`) because this
//! requirement is cross-cutting across the whole search -> results ->
//! detail journey and across the admin/public boundary, rather than
//! belonging to either single route module.
//!
//! TC-010-04 is written as an HTTP-level black-box check (assert the
//! unauthenticated response for each public path is never 401/403) rather
//! than a router-construction-level test, because `web/src/lib.rs`'s
//! `app()` has no named `public_router()`/`authenticated_router()` function
//! yet to introspect directly — routes are wired via inline `.merge()`
//! calls. This is a deliberate proxy until IMP-REQ-010-02 introduces those
//! named functions, which Loop B can then test directly against (e.g. by
//! asserting `public_router().layers().is_empty()` or similar router
//! introspection).

use axum::body::Body;
use axum::extract::connect_info::MockConnectInfo;
use axum::http::{Request, StatusCode};
use minijinja::{path_loader, Environment};
use shovelsup_web::{app, AppState};
use sqlx::PgPool;
use std::net::SocketAddr;
use tower::ServiceExt;
use uuid::Uuid;

/// Builds a unique loopback `SocketAddr` per call so any test below that
/// exercises a rate-limited path (`/projects`, `/api/v1/projects/search`)
/// gets its own Redis rate-limit bucket via `MockConnectInfo`, rather than
/// sharing the fallback `"unknown"` bucket every plain (non-`ConnectInfo`)
/// `oneshot()` call in this crate's OTHER test files lands in. Mirrors
/// `search_integration.rs`'s own `unique_peer_addr` helper (duplicated
/// rather than shared, since integration test binaries in this crate don't
/// share a common test-support module) — this is required to keep this
/// file's own new IMP-REQ-010-08/-09 tests from contributing extra requests
/// to that shared bucket and destabilizing unrelated rate-limit-boundary
/// tests elsewhere in the suite (`imp_req_008_05`'s exact 61st-request
/// assertion, in particular).
fn unique_peer_addr() -> SocketAddr {
    use std::time::{SystemTime, UNIX_EPOCH};
    let octet = (SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .subsec_nanos()
        % 254) as u8
        + 1;
    SocketAddr::from(([203, 0, 113, octet], 12345))
}

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

async fn seed_project(pool: &PgPool, address: &str) -> Uuid {
    sqlx::query_scalar!(
        "INSERT INTO projects (civic_address_normalized, project_type) VALUES ($1, 'residential') RETURNING id",
        address,
    )
    .fetch_one(pool)
    .await
    .unwrap()
}

/// Asserts no AUTH-shaped `Set-Cookie` header is present. `GET /projects`
/// legitimately sets a `lang=...` cookie (IMP-REQ-003-08, UI-locale
/// persistence — not an auth mechanism), so a blanket "no Set-Cookie at
/// all" assertion is no longer correct for that route; this narrows the
/// check back to this test's actual intent (no auth-challenge cookie)
/// while still failing if anything OTHER than the known `lang=` cookie
/// ever appears.
fn assert_no_auth_cookie_set(headers: &axum::http::HeaderMap, context: &str) {
    for value in headers.get_all("set-cookie") {
        let value = value.to_str().unwrap_or_default();
        assert!(
            value.starts_with("lang="),
            "{context}: no auth-challenge cookie expected, but got Set-Cookie: {value}"
        );
    }
}

/// TC-010-01: the full anonymous journey — `GET /projects`, `GET
/// /projects?q=...`, `GET /projects/{id}` — all succeed with no auth
/// challenge (no 401/403), no `Set-Cookie` auth-challenge header, and no
/// `Location` redirect to a login page, all without sending any
/// session/auth cookie in the request.
#[sqlx::test(migrations = "./migrations")]
async fn tc_010_01_full_anonymous_journey_succeeds_without_auth_challenge(pool: PgPool) {
    let project_id = seed_project(&pool, "1 anonymous journey ave").await;
    let app = app(test_state(pool).await);

    // Step 1: bare /projects form load, no query.
    let search_form_response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/projects")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(search_form_response.status(), StatusCode::OK);
    assert_no_auth_cookie_set(
        search_form_response.headers(),
        "GET /projects must not set any auth-challenge cookie for an anonymous request",
    );
    assert!(
        search_form_response.headers().get("location").is_none(),
        "GET /projects must not redirect an anonymous request to a login page"
    );

    // Step 2: /projects?q=... results.
    let search_results_response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/projects?q=anonymous+journey")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(search_results_response.status(), StatusCode::OK);
    assert_no_auth_cookie_set(
        search_results_response.headers(),
        "GET /projects?q=... must not set any auth-challenge cookie for an anonymous request",
    );
    assert!(search_results_response.headers().get("location").is_none());

    // Step 3: /projects/{id} detail view.
    let detail_response = app
        .oneshot(
            Request::builder()
                .uri(format!("/projects/{project_id}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(detail_response.status(), StatusCode::OK);
    assert!(
        detail_response.headers().get("set-cookie").is_none(),
        "GET /projects/{{id}} must not set any auth-challenge cookie for an anonymous request"
    );
    assert!(
        detail_response.headers().get("location").is_none(),
        "GET /projects/{{id}} must not redirect an anonymous request to a login page"
    );
}

/// TC-010-02: an expired/forged auth-shaped cookie sent alongside a request
/// to a public route is simply ignored — the request still succeeds as if
/// anonymous, rather than being rejected. There is no session-cookie
/// concept anywhere in this codebase today (admin auth is HTTP Basic via
/// the `Authorization` header only, per `middleware/admin_auth.rs`), so this
/// asserts the garbage `session` cookie has literally zero effect on the
/// response, which is the strongest form of "ignored" available to test
/// against today's implementation.
#[sqlx::test(migrations = "./migrations")]
async fn tc_010_02_forged_session_cookie_is_ignored_on_public_route(pool: PgPool) {
    let project_id = seed_project(&pool, "2 forged cookie blvd").await;
    let app = app(test_state(pool).await);

    let response = app
        .oneshot(
            Request::builder()
                .uri(format!("/projects/{project_id}"))
                .header("cookie", "session=forged.garbage.token-not-a-real-jwt")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(
        response.status(),
        StatusCode::OK,
        "a forged/garbage session cookie must be ignored, not rejected, on a public route"
    );
}

/// TC-010-03: pagination boundary (`page=2`) on the public search results
/// still succeeds anonymously — pagination introduces no auth requirement.
/// `SearchParams::page` is a Loop A stub `run_search` does not yet read
/// (IMP-REQ-004-03/04), so this only asserts the auth-related guarantee
/// (200, no auth challenge), not pagination's actual filtering behavior,
/// which TC-004-5 in `search_integration.rs` already covers/documents.
#[sqlx::test(migrations = "./migrations")]
async fn tc_010_03_pagination_boundary_succeeds_anonymously(pool: PgPool) {
    let app = app(test_state(pool).await);

    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/projects/search?q=pagination&page=2")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(
        response.status(),
        StatusCode::OK,
        "pagination via ?page=2 must not introduce an auth requirement on the public search API"
    );
}

/// TC-010-04 (regression guard, HTTP-level black-box proxy — see module
/// doc comment): none of the 5 public paths this requirement covers ever
/// return 401/403 for an unauthenticated request. This would catch a
/// regression where a future PR accidentally moved one of these routes
/// under `admin_auth::require_admin` (or any other auth layer): today,
/// `admin_routes`/`review_queue_routes` are the only sub-routers with
/// `.layer(axum_middleware::from_fn(middleware::admin_auth::require_admin))`
/// applied (`web/src/lib.rs`), and none of the 5 paths below are merged
/// into either of those sub-routers.
#[sqlx::test(migrations = "./migrations")]
async fn tc_010_04_public_routes_never_return_auth_challenge_status(pool: PgPool) {
    let project_id = seed_project(&pool, "3 regression guard cres").await;
    let app = app(test_state(pool).await);

    let public_paths = vec![
        "/".to_string(),
        "/projects".to_string(),
        "/api/v1/projects/search?q=test".to_string(),
        format!("/projects/{project_id}"),
        format!("/api/v1/projects/{project_id}/timeline"),
    ];

    for path in public_paths {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(&path)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_ne!(
            response.status(),
            StatusCode::UNAUTHORIZED,
            "public path {path} must never return 401 for an unauthenticated request"
        );
        assert_ne!(
            response.status(),
            StatusCode::FORBIDDEN,
            "public path {path} must never return 403 for an unauthenticated request \
             (this is the exact status require_admin returns; a regression moving this \
             path under require_admin would flip this assertion)"
        );
    }
}

/// TC-010-05: admin routes remain properly gated — this requirement's
/// public guarantee must not accidentally weaken existing admin auth.
/// `middleware/admin_auth::require_admin` rejects with 403 (not 401) when
/// the `Authorization` header is missing/malformed/mismatched, so that is
/// the status asserted here, matching the real handler's documented
/// behavior rather than the generic HTTP convention.
#[sqlx::test(migrations = "./migrations")]
async fn tc_010_05_admin_routes_still_require_auth(pool: PgPool) {
    let app = app(test_state(pool).await);

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/admin/review_queue")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        response.status(),
        StatusCode::FORBIDDEN,
        "GET /admin/review_queue without credentials must still be rejected with 403"
    );

    // Also sanity-check that a garbage Authorization header (the admin-side
    // analogue of TC-010-02's forged cookie) is rejected, not ignored —
    // the opposite guarantee from the public routes, on purpose.
    let forged_auth_response = app
        .oneshot(
            Request::builder()
                .uri("/admin/review_queue")
                .header("authorization", "Basic bm90LWEtcmVhbC11c2VyOm5vdC1hLXJlYWwtcGFzcw==")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        forged_auth_response.status(),
        StatusCode::FORBIDDEN,
        "a forged/incorrect Authorization header on an admin route must still be rejected with 403"
    );
}

/// TC-010-06 (IMP-REQ-010-02 regression guard): a genuinely unmatched path
/// — never registered by `public_router()` OR `authenticated_router()` —
/// must get a plain 404, never the 403 `middleware::admin_auth::require_admin`
/// returns. Before `app()`'s explicit `.fallback(not_found)`, this app's
/// merge order could let `authenticated_router()`'s own implicit fallback
/// (wrapped in `require_admin` by that router's `.layer(...)` call) become
/// the combined router's top-level catch-all, so an anonymous visitor
/// hitting a typo'd/nonexistent URL saw a 403 that implies an auth-gated
/// area exists there, rather than an ordinary "page not found".
#[sqlx::test(migrations = "./migrations")]
async fn tc_010_06_unmatched_path_returns_404_not_403(pool: PgPool) {
    let app = app(test_state(pool).await);

    let response = app
        .oneshot(
            Request::builder()
                .uri("/this/path/was/never/registered")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(
        response.status(),
        StatusCode::NOT_FOUND,
        "an unmatched path must return a plain 404, never the 403 require_admin returns"
    );
}

/// IMP-REQ-010-08: `public_router()` — built and exercised WITHOUT ever
/// merging in `authenticated_router()` — never returns 401/403 for any of
/// its own routes, even when sent a forged `Authorization` header that
/// `middleware::admin_auth::require_admin` would reject with 403
/// (mirroring `tc_010_05_admin_routes_still_require_auth`'s own forged-header
/// assertion, on the opposite router). This is the strongest available proof,
/// short of reaching into axum's private `Router` internals, that
/// `public_router()`'s own construction never pulls in that layer — exactly
/// the router-introspection substitute this module's own doc comment
/// anticipated once IMP-REQ-010-02 landed named `public_router()`/
/// `authenticated_router()` functions.
#[sqlx::test(migrations = "./migrations")]
async fn tc_010_08_public_router_has_no_auth_layer(pool: PgPool) {
    let project_id = seed_project(&pool, "8 public router only ave").await;
    let state = test_state(pool).await;
    let public_only_app = shovelsup_web::public_router(state.clone())
        .with_state(state)
        .layer(MockConnectInfo(unique_peer_addr()));

    let public_paths = vec![
        "/".to_string(),
        "/projects".to_string(),
        "/api/v1/projects/search?q=test".to_string(),
        "/categories".to_string(),
        format!("/projects/{project_id}"),
        format!("/api/v1/projects/{project_id}/timeline"),
    ];

    for path in public_paths {
        let response = public_only_app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(&path)
                    .header(
                        "authorization",
                        "Basic bm90LWEtcmVhbC11c2VyOm5vdC1hLXJlYWwtcGFzcw==",
                    )
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_ne!(
            response.status(),
            StatusCode::UNAUTHORIZED,
            "public_router() alone must never return 401 for {path}, even with a forged Authorization header"
        );
        assert_ne!(
            response.status(),
            StatusCode::FORBIDDEN,
            "public_router() alone must never return 403 for {path} — that would mean require_admin \
             got pulled into this router's own construction"
        );
    }
}

/// IMP-REQ-010-09: the search-results JSON API's error branches (here,
/// `per_page` out of range, TC-REQ-008-3's 400 mapping) behave identically
/// whether the request carries no cookie at all or an anonymous/forged
/// `session` cookie — this route never consults a session, so neither
/// request shape should get treated any differently from the other,
/// including for its error responses (not just its 200 happy path, which
/// TC-010-02 already covers).
#[sqlx::test(migrations = "./migrations")]
async fn imp_req_010_09_results_error_branch_is_identical_with_or_without_cookie(pool: PgPool) {
    let app = app(test_state(pool).await).layer(MockConnectInfo(unique_peer_addr()));

    let no_cookie_response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/v1/projects/search?q=test&per_page=0")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let with_cookie_response = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/projects/search?q=test&per_page=0")
                .header("cookie", "session=forged.garbage.token-not-a-real-jwt")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(
        no_cookie_response.status(),
        StatusCode::BAD_REQUEST,
        "per_page=0 must be rejected with 400 before any query runs"
    );
    assert_eq!(
        no_cookie_response.status(),
        with_cookie_response.status(),
        "an anonymous/forged session cookie must not change this error branch's status code"
    );
}

/// IMP-REQ-010-10: `GET /projects/{id}` renders successfully with no
/// session/cookie of any kind (the plain no-cookie half of TC-010-01's Step
/// 3), and a syntactically valid but never-seeded project id 404s — neither
/// behavior depends on, or is gated by, any session/account concept, since
/// none exists on this route.
#[sqlx::test(migrations = "./migrations")]
async fn imp_req_010_10_detail_renders_without_session_and_404s_on_unknown_id(pool: PgPool) {
    let project_id = seed_project(&pool, "10 detail no session way").await;
    let app = app(test_state(pool).await);

    let known_id_response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("/projects/{project_id}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        known_id_response.status(),
        StatusCode::OK,
        "a known project id must render successfully with no cookie/session sent at all"
    );

    let unknown_id = Uuid::new_v4();
    let unknown_id_response = app
        .oneshot(
            Request::builder()
                .uri(format!("/projects/{unknown_id}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        unknown_id_response.status(),
        StatusCode::NOT_FOUND,
        "a syntactically valid but never-seeded project id must 404, not error/redirect to a login page"
    );
}
