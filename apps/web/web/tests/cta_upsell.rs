//! REQ-014: originally the non-modal, collapsible "Get alerts — sign up"
//! upsell CTA card on the anonymous project-detail page, plus
//! fire-and-forget `POST /api/v1/cta-events` telemetry.
//!
//! The CTA card was subsequently removed from `project_detail.html` (it no
//! longer renders on the page at all). The backend it was built on
//! (`cta_signup_url`/`cta_labels` in `web/src/routes/projects.rs`, the
//! `POST /api/v1/cta-events` route, and the `cta_events` table) was left in
//! place, so TC-014-1/-3/-4/-6 were flipped to assert the card's markup is
//! now ABSENT rather than present, and TC-014-2/-5 (which never asserted
//! presence in the first place) are unchanged.
//!
//! Kept in its own file rather than folding into `timeline_resolver.rs`
//! (scoped to timeline/descriptive-field requirements) or `shareable_url.rs`
//! (scoped to canonical URLs/redirects/404s) — the CTA telemetry endpoint
//! and its now-removed card are still a distinct subsystem (upsell/growth,
//! not project-data rendering), matching the precedent both of those files
//! already set of one file per cross-cutting concern.

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

/// TC-014-1 (reversed): the "Get alerts" upsell card was removed from the
/// project-detail page (see project_detail.html); the backend support
/// (cta_signup_url/cta_labels, `/api/v1/cta-events`) is intentionally left
/// in place, but nothing renders it into the page anymore. Asserts the
/// card container and its copy are both absent.
#[sqlx::test(migrations = "./migrations")]
async fn tc_014_1_anonymous_detail_view_renders_cta_card(pool: PgPool) {
    let project_id = seed_project(&pool, "800 upsell blvd", "residential").await;

    let (status, html) = get_detail_page_html(pool, &format!("/projects/{project_id}")).await;

    assert_eq!(status, StatusCode::OK);
    assert!(
        !html.contains(r#"id="cta-upsell""#),
        "the CTA card was removed from project_detail.html, got: {html}"
    );
    assert!(
        !html.contains("Get alerts") && !html.contains("Recevez des alertes"),
        "the CTA's upsell copy was removed from project_detail.html, got: {html}"
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

/// TC-014-3 (reversed): the CTA signup link was removed along with the
/// rest of the card. Retained as a regression guard for the injection-
/// safety property (a hostile query param must never be reflected back
/// into the page) even though the CTA link it originally targeted no
/// longer exists.
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
        !html.contains(r#"id="cta-signup-link""#),
        "the CTA signup link was removed from project_detail.html, got: {html}"
    );
    assert!(
        !html.contains(hostile_marker),
        "raw user-supplied query input must never be reflected into the page, got: {html}"
    );
}

/// TC-014-4 (reversed): the CTA card's collapse/expand toggle was removed
/// along with the rest of the card.
#[sqlx::test(migrations = "./migrations")]
async fn tc_014_4_cta_card_has_collapse_expand_toggle(pool: PgPool) {
    let project_id = seed_project(&pool, "803 collapsible way", "residential").await;

    let (status, html) = get_detail_page_html(pool, &format!("/projects/{project_id}")).await;

    assert_eq!(status, StatusCode::OK);
    assert!(
        !html.contains(r#"id="cta-collapse-toggle""#),
        "the CTA collapse/expand toggle was removed from project_detail.html, got: {html}"
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

/// TC-014-6 (reversed): the CTA card's collapse-persistence script (the
/// `cta-upsell-collapsed` localStorage key) was removed along with the
/// rest of the card. Other unrelated inline scripts (e.g. the copy-link
/// button) remain on the page, so this only asserts the CTA-specific key
/// is gone, not the absence of `<script>` entirely.
#[sqlx::test(migrations = "./migrations")]
async fn tc_014_6_cta_collapse_state_persistence_contract(pool: PgPool) {
    let project_id = seed_project(&pool, "805 persistence place", "residential").await;

    let (status, html) = get_detail_page_html(pool, &format!("/projects/{project_id}")).await;

    assert_eq!(status, StatusCode::OK);
    assert!(
        !html.contains("cta-upsell-collapsed"),
        "the CTA collapse-persistence script was removed from project_detail.html, got: {html}"
    );
}
