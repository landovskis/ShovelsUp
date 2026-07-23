//! REQ-013 Loop A: TC-013-1..5 — shareable project URL (redirect on merge,
//! canonical/OG metadata, friendly bilingual 404/400, copy-link control).
//!
//! Kept in this new, separate file rather than folding into
//! `timeline_resolver.rs` (already 990 lines and scoped to timeline/detail
//! descriptive-field requirements) or `responsive_e2e.rs` (scoped to
//! mobile-responsive layout). REQ-013 is its own cross-cutting concern —
//! canonical URLs, redirects, and error-page copy — so it gets its own file,
//! matching the precedent `responsive_e2e.rs` itself set for REQ-011.
//!
//! TC-013-2, TC-013-4, and TC-013-5 are real, runnable HTTP assertions (no
//! browser needed) via `tower::ServiceExt::oneshot`, same technique as
//! `timeline_resolver.rs`/`responsive_e2e.rs`.
//!
//! TC-013-1 requires a `merged_into_id` column that does not exist yet
//! (`projects` migration 011 has no such column) — IMP-REQ-013-01's job, not
//! this pass's — so it is `#[ignore]`d with a fully written assertion body
//! using `sqlx::query` (runtime-checked, not the `query!` macro, since the
//! macro's compile-time schema check would fail to compile against a column
//! that doesn't exist in the migrated test DB yet). Once IMP-REQ-013-01
//! lands the migration, this test needs no code changes to become
//! executable — only removing the `#[ignore]`.
//!
//! TC-013-3 needs a real headless browser (to click a button and read
//! `navigator.clipboard`) — IMP-REQ-011-13 built a Playwright harness for
//! exactly this class of test (see `responsive_e2e.rs`'s module doc for why
//! `fantoccini` was rejected in this environment). Its real implementation
//! lives in `apps/web/e2e/tests/copy-link.spec.ts`; no Rust-side placeholder
//! is kept here, matching the precedent set for TC-011-2/TC-011-3.

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

/// TC-013-1: a project whose `merged_into_id` points at another project
/// 301-redirects `/projects/{id}` to the canonical `/projects/{merged_into_id}`.
/// `projects` has no `merged_into_id` column yet (IMP-REQ-013-01's
/// migration) — the `UPDATE` below uses `sqlx::query` (not `query!`) so this
/// file compiles today regardless of the missing column; running the test
/// fails at runtime with an "column does not exist" error until the
/// migration lands, hence `#[ignore]`.
#[sqlx::test(migrations = "./migrations")]
async fn tc_013_1_merged_project_redirects_to_canonical(pool: PgPool) {
    let canonical_id = seed_project(&pool, "700 canonical ave", "residential").await;
    let merged_id = seed_project(&pool, "701 merged away st", "residential").await;

    sqlx::query("UPDATE projects SET merged_into_id = $1 WHERE id = $2")
        .bind(canonical_id)
        .bind(merged_id)
        .execute(&pool)
        .await
        .expect("set merged_into_id (requires IMP-REQ-013-01 migration)");

    let app = app(test_state(pool).await);
    let response = app
        .oneshot(
            Request::builder()
                .uri(format!("/projects/{merged_id}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::MOVED_PERMANENTLY);
    let location = response
        .headers()
        .get(axum::http::header::LOCATION)
        .expect("expected a Location header on the redirect")
        .to_str()
        .unwrap();
    assert_eq!(location, format!("/projects/{canonical_id}"));
}

/// TC-013-2: a plain (non-merged) project's detail page includes a
/// `<link rel="canonical" ...>` tag whose href is absolute and built from a
/// configured base URL — NOT from the request's `Host`/`X-Forwarded-Host`
/// header, which a client fully controls and must never be trusted to build
/// a security/SEO-sensitive canonical URL. Written directly against
/// current behavior: `project_detail.html` renders no canonical tag at all
/// today, so this FAILS until IMP-REQ-013-06 lands. Deliberately does not
/// assert an exact configured base-URL literal (no `PUBLIC_BASE_URL`-shaped
/// config exists yet anywhere in `AppState`/`main.rs`/`.env.example` — that
/// configuration is IMP-REQ-013-01's job to introduce) — instead asserts
/// the structural properties that must hold regardless of whatever base
/// URL IMP-REQ-013-01/-06 settle on: an absolute (`https://`-prefixed)
/// canonical href ending in this project's own path, and the complete
/// absence of either spoofed host value anywhere in the response.
#[sqlx::test(migrations = "./migrations")]
async fn tc_013_2_canonical_url_ignores_spoofed_host_header(pool: PgPool) {
    let project_id = seed_project(&pool, "702 canonical url ave", "commercial").await;

    let app = app(test_state(pool).await);
    let response = app
        .oneshot(
            Request::builder()
                .uri(format!("/projects/{project_id}"))
                .header("host", "evil-spoofed-host.example")
                .header("x-forwarded-host", "also-evil-spoofed-host.example")
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
        html.contains(r#"<link rel="canonical" href="https://"#),
        "expected an absolute https canonical <link> tag once IMP-REQ-013-06 lands, got: {html}"
    );
    assert!(
        html.contains(&format!("/projects/{project_id}\">")),
        "expected the canonical href to end with this project's own path, got: {html}"
    );
    assert!(
        !html.contains("evil-spoofed-host.example"),
        "the canonical URL must never be built from the spoofed Host header, got: {html}"
    );
    assert!(
        !html.contains("also-evil-spoofed-host.example"),
        "the canonical URL must never be built from the spoofed X-Forwarded-Host header, got: {html}"
    );
}

/// TC-013-3: clicking the "Copy link" button (IMP-013-08 markup +
/// IMP-013-09 clipboard JS) writes the canonical URL to the clipboard and
/// shows UI feedback (e.g. a "Copied!" toast/label change). Requires a real
/// browser to exercise `navigator.clipboard.writeText` and observe the
/// resulting UI feedback — implemented for real in
/// `apps/web/e2e/tests/copy-link.spec.ts` (Playwright), which passes
/// against a live server. No Rust-side placeholder is kept here.
///
/// TC-013-4: a malformed UUID in the `/projects/{id}` path renders a
/// friendly, bilingual not-found-style page — not Axum's raw plain-text
/// rejection body (`Path<Uuid>`'s extractor failure, e.g. "Invalid URL: UUID
/// parsing failed: ...") that it produces today (see
/// `tc_011_4_3_bad_request_error_page_should_use_responsive_shell` in
/// `responsive_e2e.rs`, which already documents this same gap from the
/// responsive-shell angle — that test's doc comment describes the body as
/// "empty"; verified here that it is actually Axum's non-empty raw
/// rejection text, not literally empty, but equally not a rendered
/// template). Asserts the INTENDED behavior once IMP-REQ-013-05 lands: the
/// 400 status is kept (a malformed id is a client input error, distinct
/// from a well-formed-but-missing id), but the body becomes the rendered
/// not-found-style template instead of Axum's raw rejection text. If
/// IMP-REQ-013-05 instead unifies malformed and nonexistent ids under a
/// single 404 (to avoid leaking whether an id "looks like" a real UUID),
/// update the expected status here to `NOT_FOUND` to match. Currently
/// FAILS: today's response is a bare 400 with Axum's raw
/// "Invalid URL: UUID parsing failed: ..." rejection text, not any
/// rendered template.
#[sqlx::test(migrations = "./migrations")]
async fn tc_013_4_malformed_uuid_renders_friendly_not_found_page(pool: PgPool) {
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
        !html.contains("UUID parsing failed"),
        "expected a rendered template body once IMP-REQ-013-05 lands, \
         got Axum's raw rejection text instead: {html:?}"
    );
    assert!(
        html.contains(r#"name="viewport""#),
        "expected the friendly not-found page to route through the responsive shell, got: {html:?}"
    );
    assert!(
        html.contains("Project not found") || html.contains("Projet introuvable"),
        "expected friendly bilingual not-found copy, got: {html:?}"
    );
}

/// TC-013-5 (EN case): a well-formed but nonexistent project id renders the
/// friendly bilingual not-found template — replacing today's bare
/// `StatusCode::NOT_FOUND` with an empty body (`get_project_detail_page`'s
/// `project_exists.ok_or(StatusCode::NOT_FOUND)?` short-circuits before any
/// template is rendered). Default locale (no `Accept-Language` header).
/// Currently FAILS: today's body is empty.
#[sqlx::test(migrations = "./migrations")]
async fn tc_013_5_nonexistent_project_renders_not_found_page_en(pool: PgPool) {
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
        html.contains("<html lang=\"en\">"),
        "expected the EN-locale not-found page shell, got: {html:?}"
    );
    assert!(
        html.contains("Project not found"),
        "expected friendly EN not-found copy, got: {html:?}"
    );
    assert!(
        html.contains(r#"name="viewport""#),
        "expected the not-found page to route through the responsive shell, got: {html:?}"
    );
}

/// TC-013-5 (FR case): same as above with `Accept-Language: fr`. Independent
/// request from the EN case above (no seed data needed either way, since a
/// nonexistent id requires none). Currently FAILS: today's body is empty
/// regardless of `Accept-Language`.
#[sqlx::test(migrations = "./migrations")]
async fn tc_013_5_nonexistent_project_renders_not_found_page_fr(pool: PgPool) {
    let app = app(test_state(pool).await);
    let response = app
        .oneshot(
            Request::builder()
                .uri(format!("/projects/{}", Uuid::new_v4()))
                .header("accept-language", "fr-CA,fr;q=0.9")
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
        html.contains("<html lang=\"fr\">"),
        "expected the FR-locale not-found page shell, got: {html:?}"
    );
    assert!(
        html.contains("Projet introuvable"),
        "expected friendly FR not-found copy, got: {html:?}"
    );
    assert!(
        html.contains(r#"name="viewport""#),
        "expected the not-found page to route through the responsive shell, got: {html:?}"
    );
}
