//! REQ-014 Loop A: TC-014-1..6 — non-modal, collapsible "Get alerts —
//! sign up" upsell CTA card on the anonymous project-detail page, plus
//! fire-and-forget `POST /api/v1/cta-events` telemetry.
//!
//! Kept in its own file rather than folding into `timeline_resolver.rs`
//! (scoped to timeline/descriptive-field requirements) or `shareable_url.rs`
//! (scoped to canonical URLs/redirects/404s) — the CTA card, its collapse
//! toggle, its telemetry endpoint, and its client-side persistence
//! contract are a genuinely distinct subsystem (upsell/growth, not
//! project-data rendering), matching the precedent both of those files
//! already set of one file per cross-cutting concern.
//!
//! Nothing in this requirement exists yet as of this pass: no CTA markup
//! in `project_detail.html`, no `cta_events` table/migration, no
//! `POST /api/v1/cta-events` route. All six tests are written against
//! *today's* actual behavior:
//!   - TC-014-1, -3, -4, -6 currently FAIL (the asserted markup/contract
//!     doesn't exist yet) — that's IMP-014-05/-06/-07/-08/-09's job.
//!   - TC-014-2 currently PASSES, but not vacuously: it asserts both the
//!     absence of modal markers AND that the page still renders normally
//!     (200, real body), so it keeps meaning as a regression guard once
//!     Loop B adds the CTA card — if a future change makes the card a
//!     `<dialog>`/modal, this test starts failing.
//!   - TC-014-5 currently PASSES because there is structurally no
//!     `cta-events` route to fail into the detail page's own handler (they
//!     are different Axum routes entirely) — written to actually exercise
//!     both endpoints in one test (POST then GET) rather than just
//!     asserting an architectural claim, so it's a real assertion of
//!     to-day's separation, not a tautology.
//!
//! None of the six are `#[ignore]`d: unlike TC-013-3's clipboard test
//! (which needs a real browser to read `navigator.clipboard`), every
//! assertion here — including TC-014-6's persistence contract — can be
//! checked from the rendered HTTP response body alone (presence of a
//! `<script>`/data-attribute wiring up `localStorage`), so no headless
//! browser harness is required for this requirement's Loop A pass.

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

async fn seed_project(pool: &PgPool, address: &str, project_type: &str) -> Uuid {
    sqlx::query_scalar!(
        "INSERT INTO projects (civic_address_normalized, project_type) VALUES ($1, $2) RETURNING id",
        address,
        project_type,
    )
    .fetch_one(pool)
    .await
    .unwrap()
}

async fn get_detail_page_html(pool: PgPool, uri: &str) -> (StatusCode, String) {
    let app = app(test_state(pool).await);
    let response = app
        .oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let body = http_body_util::BodyExt::collect(response.into_body())
        .await
        .unwrap()
        .to_bytes();
    (status, String::from_utf8(body.to_vec()).unwrap())
}

/// TC-014-1: since this app has no auth at all today, every detail-page
/// view is anonymous — so the CTA card must render unconditionally.
/// Asserts the intended behavior once IMP-014-05/-07 land: the rendered
/// page includes the CTA card container, identified by a stable
/// `id="cta-upsell"` (Loop B's contract to satisfy — see stub notes in
/// `web/src/routes/projects.rs`), plus visible upsell copy. Currently
/// FAILS: `project_detail.html` has no such element at all.
#[sqlx::test(migrations = "./migrations")]
async fn tc_014_1_anonymous_detail_view_renders_cta_card(pool: PgPool) {
    let project_id = seed_project(&pool, "800 upsell blvd", "residential").await;

    let (status, html) = get_detail_page_html(pool, &format!("/projects/{project_id}")).await;

    assert_eq!(status, StatusCode::OK);
    assert!(
        html.contains(r#"id="cta-upsell""#),
        "expected the CTA card container (id=\"cta-upsell\") once IMP-014-05/-07 land, got: {html}"
    );
    assert!(
        html.contains("Get alerts") || html.contains("Recevez des alertes"),
        "expected the CTA's bilingual upsell copy (IMP-014-06), got: {html}"
    );
}

/// TC-014-2 (regression guard): the CTA card must be a plain in-page
/// section, never a modal/dialog. Asserts the absence of `<dialog>`,
/// `role="dialog"`, a scroll-lock class (`scroll-lock`/`modal-open`, the
/// two conventional names for this pattern), and a backdrop element,
/// while also asserting the page renders normally (200, non-empty body) —
/// so this isn't just "nothing exists therefore nothing is a modal", it's
/// "the page renders for real, and none of what it renders is a modal".
/// Passes today (no CTA exists yet at all) and must keep passing after
/// IMP-014-05/-07/-08 land the real card.
#[sqlx::test(migrations = "./migrations")]
async fn tc_014_2_cta_card_is_not_a_modal_dialog(pool: PgPool) {
    let project_id = seed_project(&pool, "801 non modal ave", "commercial").await;

    let (status, html) = get_detail_page_html(pool, &format!("/projects/{project_id}")).await;

    assert_eq!(status, StatusCode::OK);
    assert!(!html.is_empty(), "expected a real rendered page body");
    assert!(
        !html.contains("<dialog"),
        "CTA card must not be a <dialog> element, got: {html}"
    );
    assert!(
        !html.to_lowercase().contains(r#"role="dialog""#),
        "CTA card must not carry role=\"dialog\", got: {html}"
    );
    assert!(
        !html.contains("scroll-lock") && !html.contains("modal-open"),
        "CTA card must not apply a scroll-lock/modal-open class, got: {html}"
    );
    assert!(
        !html.contains("cta-backdrop") && !html.contains("modal-backdrop"),
        "CTA card must not have a backdrop element, got: {html}"
    );
}

/// TC-014-3: the CTA's signup link points at `/signup` in the *same* tab
/// (no `target="_blank"` on that anchor), and its href reflects no raw
/// user input — only server-validated query params, if any. Exercised
/// with a hostile query string on the *project detail* request (a
/// plausible injection vector: a referrer/campaign param some future
/// change might naively forward into the CTA's href) to prove nothing
/// user-controlled leaks into the signup link. The presence assertions
/// (stable id `cta-signup-link`, href starting with `/signup`) currently
/// FAIL — no CTA/signup link exists yet. The injection-safety assertion
/// is currently vacuously true (nothing is reflected because nothing is
/// rendered) but stays meaningful as a regression guard once IMP-014-05/-06
/// land the real link.
#[sqlx::test(migrations = "./migrations")]
async fn tc_014_3_cta_signup_link_is_same_tab_and_has_no_raw_input_reflected(pool: PgPool) {
    let project_id = seed_project(&pool, "802 signup crescent", "residential").await;
    let hostile_marker = "evil-injected-param-xyz123";

    let (status, html) = get_detail_page_html(
        pool,
        &format!("/projects/{project_id}?ref={hostile_marker}"),
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    assert!(
        html.contains(r#"id="cta-signup-link""#),
        "expected a stable-id signup link (IMP-014-05/-07) once implemented, got: {html}"
    );
    assert!(
        html.contains(r#"href="/signup"#),
        "expected the CTA link to point at /signup, got: {html}"
    );
    assert!(
        !html.contains(r#"target="_blank""#),
        "CTA signup link must open in the same tab (no target=\"_blank\"), got: {html}"
    );
    assert!(
        !html.contains(hostile_marker),
        "raw user-supplied query input must never be reflected into the CTA href, got: {html}"
    );
}

/// TC-014-4: the CTA card must be dismissible/collapsible via a UI
/// control. Asserts the intended contract once IMP-014-07/-09 land: a
/// stable-id toggle button (`cta-collapse-toggle`) carrying `aria-expanded`
/// (for accessibility, since this is a disclosure widget, not a modal).
/// Currently FAILS: no such control exists.
#[sqlx::test(migrations = "./migrations")]
async fn tc_014_4_cta_card_has_collapse_expand_toggle(pool: PgPool) {
    let project_id = seed_project(&pool, "803 collapsible way", "residential").await;

    let (status, html) = get_detail_page_html(pool, &format!("/projects/{project_id}")).await;

    assert_eq!(status, StatusCode::OK);
    assert!(
        html.contains(r#"id="cta-collapse-toggle""#),
        "expected a collapse/expand toggle control (IMP-014-07/-09), got: {html}"
    );
    assert!(
        html.contains("aria-expanded="),
        "expected the toggle to expose aria-expanded for accessibility, got: {html}"
    );
}

/// TC-014-5: telemetry-endpoint failure isolation. `POST
/// /api/v1/cta-events` and `GET /projects/{id}` are handled by
/// completely separate Axum route registrations (see `web/src/lib.rs`) —
/// this test proves that structurally by actually calling both against
/// the *same* app/state in sequence and asserting the detail page still
/// renders 200 regardless of what the telemetry endpoint did.
///
/// Empirically (verified by running this test), `/api/v1/cta-events`
/// today returns 403 Forbidden, not a plain 404 — NOT because any route
/// matches it, but because of a pre-existing routing quirk in
/// `web/src/lib.rs`: `admin_routes` has `middleware::admin_auth::require_admin`
/// applied via `.layer()` *before* `.merge()`-ing it into the top-level
/// `Router`. Axum's `Router::merge` composes an unmatched sub-router as a
/// fallback chain, so that layer — which rejects with 403 whenever
/// `ADMIN_USER`/`ADMIN_PASSWORD_HASH` aren't set, as they aren't in this
/// test env — ends up running for *any* unmatched path in the whole app,
/// not just the two `/admin/...` routes it's meant to guard. This is a
/// latent bug in existing, already-merged code, wholly unrelated to
/// REQ-014; fixing it (e.g. swapping `.layer()` for `.route_layer()` on
/// `admin_routes`) is out of scope for this Loop A pass and is flagged
/// here rather than silently worked around. It doesn't weaken this test:
/// the real thing under test — that the *detail page* renders
/// successfully no matter what happened on the telemetry request — still
/// holds and is asserted below regardless of which status the telemetry
/// call returns. Once IMP-014-04/-10 land the real `/api/v1/cta-events`
/// route (backed by the not-yet-created `cta_events` table), the first
/// assertion's expected status should change from 403 to whatever
/// "accepted" status the real endpoint uses (e.g. 202/204), and ideally
/// the merge-order bug above gets fixed at the same time so an unmatched
/// path goes back to a plain 404. The second assertion (detail page still
/// 200) must continue to hold either way, including when the real
/// endpoint fails (e.g. DB unavailable).
#[sqlx::test(migrations = "./migrations")]
async fn tc_014_5_telemetry_endpoint_failure_does_not_affect_detail_page(pool: PgPool) {
    let project_id = seed_project(&pool, "804 telemetry terrace", "commercial").await;
    let app = app(test_state(pool).await);

    let cta_event_response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/cta-events")
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::json!({ "project_id": project_id, "event": "impression" })
                        .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        cta_event_response.status(),
        StatusCode::ACCEPTED,
        "IMP-REQ-014-02/-10 landed the real /api/v1/cta-events route: a \
         well-formed telemetry event with no Origin/Referer header (as this \
         request sends, matching the fallback branch of \
         core::origin_check_passes) is accepted and recorded, returning 202 \
         Accepted for this fire-and-forget beacon"
    );

    let detail_response = app
        .oneshot(
            Request::builder()
                .uri(format!("/projects/{project_id}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        detail_response.status(),
        StatusCode::OK,
        "the detail page must render successfully regardless of the cta-events \
         endpoint's outcome — these are independent routes/handlers"
    );
}

/// TC-014-6 (collapse-persistence contract): the CTA's collapsed/expanded
/// state must persist across page loads for 30 days via client-side
/// storage. A real cross-page-load assertion needs a browser to execute
/// JS and reload — written instead as a CONTRACT test of the rendered
/// HTTP response, checking for an inline `<script>` that wires up
/// `localStorage` under a stable, documented key
/// (`cta-upsell-collapsed`) — the same technique already used by
/// `shareable_url.rs`'s TC-013-2 for asserting a structural HTML contract
/// without a browser. Not `#[ignore]`d: unlike TC-013-3's clipboard-read
/// assertion (which has no non-browser equivalent at all), this contract
/// is fully checkable from the response body, so there's no genuine gap
/// requiring the REQ-011 headless-browser harness — that harness remains
/// the right tool for an actual reload-and-check E2E test, which is
/// explicitly Loop B/QA's follow-up, not this Loop A pass's job. Currently
/// FAILS: no such script exists yet.
#[sqlx::test(migrations = "./migrations")]
async fn tc_014_6_cta_collapse_state_persistence_contract(pool: PgPool) {
    let project_id = seed_project(&pool, "805 persistence place", "residential").await;

    let (status, html) = get_detail_page_html(pool, &format!("/projects/{project_id}")).await;

    assert_eq!(status, StatusCode::OK);
    assert!(
        html.contains("<script"),
        "expected an inline script wiring up collapse persistence (IMP-014-09), got: {html}"
    );
    assert!(
        html.contains("localStorage"),
        "expected the persistence mechanism to use localStorage (IMP-014-09), got: {html}"
    );
    assert!(
        html.contains("cta-upsell-collapsed"),
        "expected the documented localStorage key \"cta-upsell-collapsed\" (IMP-014-09), got: {html}"
    );
}
