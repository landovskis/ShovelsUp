//! REQ-011: TC-011-1..5 — mobile-responsive search experience.
//!
//! TC-011-1, TC-011-4, and TC-011-5 are real, runnable static-HTML/HTTP
//! assertions (no real browser needed) and are wired up against the actual
//! `app()` router via `tower::ServiceExt::oneshot`, the same technique used
//! throughout `search_integration.rs` / `timeline_resolver.rs` /
//! `no_account_required.rs`.
//!
//! TC-011-2 and TC-011-3 need a real headless browser (to measure
//! `document.documentElement.scrollWidth`/`clientWidth` and to drive focus
//! management) — IMP-REQ-011-13 evaluated `fantoccini` first, per the plan's
//! own contract, and it was NOT usable in the target environment:
//! `fantoccini` requires `chromedriver`, and the only available
//! `chromedriver` build was unsigned/non-notarized and rejected outright by
//! macOS Gatekeeper (`spctl -a -vv` => "rejected"), with no interactive way
//! to approve it. The fallback the plan names for exactly this case — a
//! Node/Playwright system-test stage — is what's actually implemented:
//! `apps/web/e2e/` is a standalone Playwright project (Playwright manages
//! its own signed/notarized browser binary via `npx playwright install`,
//! sidestepping the Gatekeeper problem entirely). System-test coverage for
//! TC-011-2 (`apps/web/e2e/tests/no-horizontal-scroll.spec.ts`), TC-011-3
//! (`apps/web/e2e/tests/filter-sheet.spec.ts`), plus the remaining
//! REQ-011 integration/accessibility coverage (IMP-REQ-011-14..17) now
//! lives entirely in that project — see `apps/web/e2e/playwright.config.ts`
//! for exactly how to run it. No Rust-side placeholder is kept for these
//! two cases: the real assertions exist and pass in the Playwright suite,
//! so an `#[ignore]`d Rust stub here would be dead, misleading weight.
//!
//! Kept in this separate file (rather than folding into
//! `search_integration.rs`/`timeline_resolver.rs`) because REQ-011 is a
//! distinct, browser-driven subsystem cutting across both the search and
//! project-detail pages, matching the precedent set by
//! `no_account_required.rs` for other cross-cutting requirements.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use minijinja::{path_loader, Environment};
use shovelsup_web::{app, AppState};
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

async fn seed_project(pool: &PgPool, address: &str) -> Uuid {
    sqlx::query_scalar!(
        "INSERT INTO projects (civic_address_normalized, project_type) VALUES ($1, 'residential') RETURNING id",
        address,
    )
    .fetch_one(pool)
    .await
    .unwrap()
}

/// TC-011-1: rendered HTML for `GET /search` includes a
/// `<meta name="viewport" content="width=device-width, initial-scale=1">`
/// tag (or equivalent) so mobile browsers don't apply desktop-width
/// zoomed-out layout. `base.html` (extended by `search.html`) already
/// declares this meta tag, so this is a regression guard as much as a
/// scaffolding test: it locks in behavior IMP-REQ-011-03 must not remove.
#[sqlx::test(migrations = "./migrations")]
async fn tc_011_1_search_page_has_viewport_meta_tag(pool: PgPool) {
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
        html.contains(r#"name="viewport""#),
        "expected a viewport meta tag in /search's rendered HTML, got: {html}"
    );
    assert!(
        html.contains("width=device-width"),
        "expected the viewport meta tag to declare width=device-width, got: {html}"
    );
}

/// TC-011-4 (503 case): a DB-outage error page on `/projects/{id}` renders
/// through the same responsive shell as normal pages — it extends
/// `base.html` (via `project_detail.html`'s `render_error_page` closure in
/// `web/src/routes/projects.rs`), so it carries the same viewport meta tag
/// and the same `main.css` stylesheet link as a normal 200 response. This
/// passes today: `render_error_page` already renders the full template
/// rather than returning a bare status code.
#[sqlx::test(migrations = "./migrations")]
async fn tc_011_4_1_service_unavailable_error_page_uses_responsive_shell(pool: PgPool) {
    let project_id = seed_project(&pool, "1 responsive outage way").await;
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

    assert!(
        html.contains(r#"name="viewport""#),
        "the 503 error page must route through the responsive shell (viewport meta), got: {html}"
    );
    assert!(
        html.contains("/static/css/main.css"),
        "the 503 error page must route through the responsive shell (main.css link), got: {html}"
    );
}

/// TC-011-4 (404 case): documents today's gap — a nonexistent-project 404
/// on `/projects/{id}` returns a bare `StatusCode::NOT_FOUND` with an empty
/// body (`get_project_detail_page`'s `project_exists.ok_or(StatusCode::NOT_FOUND)?`
/// propagates directly as an Axum rejection, never touching
/// `project_detail.html`), so it carries no viewport meta tag and no
/// stylesheet link at all — the opposite of the 503 case above. This is
/// exactly the "bare unstyled error" REQ-011 says must not happen; fixing
/// it (rendering a styled 404 page through the same shell) is Loop B's
/// IMP-REQ-011-07 job. Currently FAILS, for this documented reason.
#[sqlx::test(migrations = "./migrations")]
async fn tc_011_4_2_not_found_error_page_should_use_responsive_shell(pool: PgPool) {
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
    let body = http_body_util::BodyExt::collect(response.into_body())
        .await
        .unwrap()
        .to_bytes();
    let html = String::from_utf8(body.to_vec()).unwrap();

    assert!(
        html.contains(r#"name="viewport""#),
        "documents today's gap: the 404 page is a bare empty-body response, not routed \
         through the responsive shell (fixed by IMP-REQ-011-07); got body: {html:?}"
    );
}

/// TC-011-4 (400 case): documents today's gap — a malformed project id on
/// `/projects/{id}` is rejected by Axum's `Path<Uuid>` extractor before
/// the handler ever runs, so the response is a bare `StatusCode::BAD_REQUEST`
/// with an empty body, never touching `project_detail.html`. Same gap as
/// the 404 case above; fixing it (a styled 400 page through the same shell)
/// is Loop B's IMP-REQ-011-07 job. Currently FAILS, for this documented
/// reason.
#[sqlx::test(migrations = "./migrations")]
async fn tc_011_4_3_bad_request_error_page_should_use_responsive_shell(pool: PgPool) {
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
    let body = http_body_util::BodyExt::collect(response.into_body())
        .await
        .unwrap()
        .to_bytes();
    let html = String::from_utf8(body.to_vec()).unwrap();

    assert!(
        html.contains(r#"name="viewport""#),
        "documents today's gap: the 400 page is a bare empty-body response, not routed \
         through the responsive shell (fixed by IMP-REQ-011-07); got body: {html:?}"
    );
}

/// TC-011-5: a test-only fault-injection hook (IMP-REQ-011-08) lets a test
/// force a 503 on `/search` to verify its error state renders through the
/// responsive shell, without needing a real DB outage (unlike
/// `pool.close()`, which works but can't be scoped to a single request/test
/// alongside other assertions in the same suite). `core::should_force_fault`
/// (`web/src/routes/search.rs`) recognizes either an `X-Force-Fault: 503`
/// header or a `force_fault=503` query param (either alone is sufficient);
/// `get_search_page` honors it only in debug builds (`cfg(debug_assertions)`)
/// — a release build never even contains the code path that reads these
/// signals, so this hook can't be used to force an outage against a live
/// deployment. This test integration-tests the debug-build behavior (the
/// binary under test here is always built without `--release`); the
/// underlying pure decision function has its own unit tests in
/// `search.rs`'s `core::tests` module.
#[sqlx::test(migrations = "./migrations")]
async fn tc_011_5_fault_injection_hook_forces_503_through_responsive_shell(pool: PgPool) {
    let app = app(test_state(pool).await);
    let response = app
        .oneshot(
            Request::builder()
                .uri("/search?q=test&force_fault=503")
                .header("x-force-fault", "503")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(
        response.status(),
        StatusCode::SERVICE_UNAVAILABLE,
        "IMP-REQ-011-08's fault-injection hook should force a 503 when either the \
         x-force-fault header or the force_fault query param is set to \"503\""
    );
    let body = http_body_util::BodyExt::collect(response.into_body())
        .await
        .unwrap()
        .to_bytes();
    let html = String::from_utf8(body.to_vec()).unwrap();

    assert!(
        html.contains(r#"name="viewport""#),
        "the induced 503 must route through the responsive shell (viewport meta), got: {html}"
    );
    assert!(
        html.contains("/static/css/main.css"),
        "the induced 503 must route through the responsive shell (main.css link), got: {html}"
    );
}

// TC-011-2 (320px no-horizontal-scroll sweep) and TC-011-3 (mobile filter
// sheet non-blocking + focus-return) both need a real headless browser and
// now have real, passing coverage in the Playwright harness:
//   - TC-011-2 / IMP-REQ-011-14: apps/web/e2e/tests/no-horizontal-scroll.spec.ts
//   - TC-011-3 / IMP-REQ-011-15: apps/web/e2e/tests/filter-sheet.spec.ts
// See apps/web/e2e/playwright.config.ts for how to run that suite (it
// expects `cargo run -p shovelsup-web` already running and reachable). No
// `#[ignore]`d placeholder is kept here for either case — see this file's
// module doc comment for why.
