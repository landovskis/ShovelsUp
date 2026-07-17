//! REQ-011 Loop A: TC-011-1..5 — mobile-responsive search experience.
//!
//! TC-011-1, TC-011-4, and TC-011-5 are real, runnable static-HTML/HTTP
//! assertions (no real browser needed) and are wired up against the actual
//! `app()` router via `tower::ServiceExt::oneshot`, the same technique used
//! throughout `search_integration.rs` / `timeline_resolver.rs` /
//! `no_account_required.rs`.
//!
//! TC-011-2 and TC-011-3 require a real headless browser (to measure
//! `document.documentElement.scrollWidth`/`clientWidth` and to drive focus
//! management), and this repo has no headless-browser/WebDriver tooling yet
//! — evaluating and wiring up `fantoccini` (falling back to a Playwright CI
//! stage if unreliable) is Loop B's IMP-REQ-011-13 task, not this pass's.
//! They are written here `#[ignore]`d, with the full intended assertion body
//! sketched as comments against the plausible `fantoccini::Client` API shape
//! (verify the exact API against the crate's docs once it's added — this is
//! a sketch, not a verified call signature), so Loop B need only:
//!   1. add `fantoccini` (or equivalent) to `[dev-dependencies]`,
//!   2. replace the sketched comment block with real calls,
//!   3. remove the `#[ignore]` attribute.
//!
//! Kept in this new, separate file (rather than folding into
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

/// TC-011-5: a test-only fault-injection hook (IMP-REQ-011-08) should let a
/// test force a 503 on `/search` to verify its error state renders through
/// the responsive shell, without needing a real DB outage (unlike
/// `pool.close()`, which works but can't be scoped to a single request/test
/// alongside other assertions in the same suite). Grepped `web/src` for
/// "fault"/"inject" (case-insensitive): no such mechanism exists anywhere in
/// this codebase today. This test sends the two most plausible signals for
/// such a hook — an `X-Force-Fault: 503` header and a `force_fault=503`
/// query param — and documents that neither currently has any effect:
/// `/search` still returns a normal 200. Once IMP-REQ-011-08 adds a real,
/// test-only-gated hook, this test should be rewritten to assert the
/// induced 503 renders through the responsive shell (per TC-011-4).
#[sqlx::test(migrations = "./migrations")]
async fn tc_011_5_no_fault_injection_hook_exists_yet(pool: PgPool) {
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
        StatusCode::OK,
        "documents today's gap: no fault-injection hook exists yet (IMP-REQ-011-08), so \
         neither the x-force-fault header nor the force_fault query param can force a 503; \
         got: {}",
        response.status()
    );
}

/// TC-011-2: at a 320px viewport, no element on the search results page
/// causes horizontal scroll
/// (`document.documentElement.scrollWidth <= document.documentElement.clientWidth`).
/// Requires a real headless browser to measure actual rendered layout —
/// blocked on IMP-REQ-011-13 (harness evaluation/setup). The server is
/// spun up as a real listening HTTP server (not just a `tower::Service` via
/// `.oneshot()`) using only crates already available to this crate
/// (`axum`, `tokio`), since a real browser needs an actual URL to navigate
/// to — this part is left in place, uncommented, for Loop B to reuse.
#[ignore = "blocked on IMP-REQ-011-13 headless-browser harness setup"]
#[sqlx::test(migrations = "./migrations")]
async fn tc_011_2_no_horizontal_scroll_at_320px_viewport(pool: PgPool) {
    let project_id = seed_project(&pool, "1 no scroll lane").await;
    let app = app(test_state(pool).await);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let search_url = format!("http://{addr}/search?q=no+scroll");
    let detail_url = format!("http://{addr}/projects/{project_id}");

    // ---- Loop B (IMP-REQ-011-13): replace everything below this line with
    // real fantoccini calls once the crate is added to [dev-dependencies].
    // Sketched against the plausible fantoccini::Client API shape (verify
    // exact method names/signatures against the crate's docs when wiring
    // this up for real):
    //
    // let client = fantoccini::ClientBuilder::native()
    //     .connect("http://localhost:9515") // local chromedriver/geckodriver
    //     .await
    //     .expect("connect to WebDriver session");
    //
    // for url in [&search_url, &detail_url] {
    //     client
    //         .set_window_size(320, 640)
    //         .await
    //         .expect("set 320px-wide viewport");
    //     client.goto(url).await.expect("navigate to page");
    //
    //     let scroll_width: f64 = client
    //         .execute("return document.documentElement.scrollWidth", vec![])
    //         .await
    //         .expect("read scrollWidth")
    //         .as_f64()
    //         .expect("scrollWidth is numeric");
    //     let client_width: f64 = client
    //         .execute("return document.documentElement.clientWidth", vec![])
    //         .await
    //         .expect("read clientWidth")
    //         .as_f64()
    //         .expect("clientWidth is numeric");
    //
    //     assert!(
    //         scroll_width <= client_width,
    //         "{url} must not cause horizontal scroll at 320px \
    //          (scrollWidth {scroll_width} > clientWidth {client_width})"
    //     );
    // }
    //
    // client.close().await.ok();

    let _ = (&search_url, &detail_url);
    unimplemented!(
        "blocked on IMP-REQ-011-13: replace this body with the fantoccini client \
         calls sketched in the comment block above once the headless-browser \
         harness exists"
    );
}

/// TC-011-3: the mobile filter sheet (a collapsible filter panel on the
/// search results page, IMP-REQ-011-03) doesn't block page interaction
/// while open (e.g. the rest of the page remains reachable/scrollable, or
/// is correctly `inert`/`aria-hidden` if modal) and returns keyboard focus
/// to the control that opened it once closed. Requires a real headless
/// browser to drive focus/keyboard interaction — blocked on
/// IMP-REQ-011-13, same as TC-011-2.
#[ignore = "blocked on IMP-REQ-011-13 headless-browser harness setup"]
#[sqlx::test(migrations = "./migrations")]
async fn tc_011_3_mobile_filter_sheet_returns_focus_on_close(pool: PgPool) {
    let app = app(test_state(pool).await);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let search_url = format!("http://{addr}/search?q=filter+sheet");

    // ---- Loop B (IMP-REQ-011-13): replace everything below this line with
    // real fantoccini calls once the crate is added to [dev-dependencies].
    // Sketched against the plausible fantoccini::Client API shape (verify
    // exact method names/signatures against the crate's docs when wiring
    // this up for real). Assumes IMP-REQ-011-03 gives the filter-sheet
    // trigger button a stable id `#filter-sheet-trigger` and the sheet
    // itself `#filter-sheet` with a `#filter-sheet-close` control — Loop B
    // should update these selectors to match whatever markup it actually
    // lands:
    //
    // let client = fantoccini::ClientBuilder::native()
    //     .connect("http://localhost:9515")
    //     .await
    //     .expect("connect to WebDriver session");
    // client
    //     .set_window_size(320, 640)
    //     .await
    //     .expect("set 320px-wide viewport");
    // client.goto(&search_url).await.expect("navigate to search results");
    //
    // let trigger = client
    //     .find(fantoccini::Locator::Css("#filter-sheet-trigger"))
    //     .await
    //     .expect("find filter-sheet trigger button");
    // trigger.click().await.expect("open the filter sheet");
    //
    // // The rest of the page must still be reachable/interactive, or
    // // explicitly `inert`/`aria-hidden="true"` if the sheet is modal —
    // // whichever IMP-REQ-011-03 chooses, it must be intentional, not an
    // // accidental focus trap with no escape.
    // let sheet = client
    //     .find(fantoccini::Locator::Css("#filter-sheet"))
    //     .await
    //     .expect("find open filter sheet");
    // assert!(
    //     sheet.is_displayed().await.expect("check sheet visibility"),
    //     "filter sheet must be visible once opened"
    // );
    //
    // let close_button = client
    //     .find(fantoccini::Locator::Css("#filter-sheet-close"))
    //     .await
    //     .expect("find filter-sheet close control");
    // close_button.click().await.expect("close the filter sheet");
    //
    // let focused_element_id: String = client
    //     .execute("return document.activeElement.id", vec![])
    //     .await
    //     .expect("read document.activeElement.id")
    //     .as_str()
    //     .expect("activeElement.id is a string")
    //     .to_string();
    // assert_eq!(
    //     focused_element_id, "filter-sheet-trigger",
    //     "closing the filter sheet must return keyboard focus to the \
    //      trigger button that opened it, got focus on: {focused_element_id}"
    // );
    //
    // client.close().await.ok();

    let _ = &search_url;
    unimplemented!(
        "blocked on IMP-REQ-011-13: replace this body with the fantoccini client \
         calls sketched in the comment block above once the headless-browser \
         harness exists"
    );
}
