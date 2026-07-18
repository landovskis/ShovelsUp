pub mod jobs;
pub mod middleware;
pub mod routes;

use axum::{
    middleware as axum_middleware,
    routing::{get, post},
    Router,
};
use minijinja::Environment;
use redis::aio::ConnectionManager;
use sqlx::PgPool;
use std::sync::Arc;

#[derive(Clone)]
pub struct AppState {
    pub env: Arc<Environment<'static>>,
    pub db: PgPool,
    /// Backs the per-IP search rate limiter (IMP-REQ-008-05). `redis` was
    /// already provisioned in docker-compose/.env but unused by any prior
    /// requirement — this is its first real caller.
    pub redis: ConnectionManager,
    /// Test-only fault-injection hook (IMP-REQ-006-06). When `Some`, the
    /// project-detail page's citation lookup (`fetch_primary_citation`)
    /// uses this pool instead of `db`, allowing a test to simulate a DB
    /// outage isolated to *just* the citation query (e.g. by calling
    /// `.close()` on a second, independently-connected pool to the same
    /// database) without closing `db` itself and taking the whole page
    /// down. Production (`main.rs`) always leaves this `None`, so the
    /// citation query uses the same `db` pool as every other query there —
    /// this field changes no production behavior.
    pub citation_db_override: Option<PgPool>,
}

/// Public, unauthenticated routes (IMP-REQ-010-02). Every route registered
/// here MUST remain reachable without any credential — this function must
/// NEVER have `middleware::admin_auth::require_admin` (or any other
/// auth-challenge-issuing layer) applied to it, anywhere, by construction.
/// Rate limiting (`rate_limit_search`, IMP-REQ-008-05/IMP-REQ-010-07) is
/// layered on the `/search`/`/api/v1/projects/search` pair only, matching
/// the pre-split scope exactly — it degrades silently to a plain 429 with
/// no interactive challenge, so it does not compromise this function's "no
/// account required" guarantee.
///
/// `tests/no_account_required.rs`'s `tc_010_08_public_router_has_no_auth_layer`
/// builds this function directly (merged with nothing else) and asserts
/// every one of its routes never returns 401/403 even when sent a forged
/// `Authorization` header that `require_admin` would reject — the strongest
/// available proof, short of private-field introspection, that this
/// function's own construction never pulls in that layer.
pub fn public_router(state: AppState) -> Router<AppState> {
    let rate_limited_search_routes = Router::new()
        .route("/search", get(routes::search::get_search_page))
        .route(
            "/api/v1/projects/search",
            get(routes::search::search_projects),
        )
        .layer(axum_middleware::from_fn_with_state(
            state,
            middleware::rate_limit::rate_limit_search,
        ));

    Router::new()
        .route("/", get(routes::index))
        .route(
            "/projects/:id",
            get(routes::projects::get_project_detail_page),
        )
        .route(
            "/api/v1/projects/:id/timeline",
            get(routes::projects::get_project_timeline),
        )
        .route("/categories", get(routes::search::list_categories))
        .merge(rate_limited_search_routes)
}

/// Authenticated (admin-only) routes (IMP-REQ-010-02). Every route
/// registered here has `middleware::admin_auth::require_admin` applied
/// at construction time, before it is ever merged into `app()`'s router —
/// structurally impossible to reach without passing that gate first. Never
/// merge a route intended to be public into this function.
pub fn authenticated_router() -> Router<AppState> {
    Router::new()
        .route(
            "/admin/fetch_jobs/:id/reprocess",
            post(routes::admin::reprocess_fetch_job),
        )
        .route(
            "/admin/source_documents/:id/reprocess",
            post(routes::admin::reprocess_source_document),
        )
        .route(
            "/admin/review_queue",
            get(routes::review_queue::get_review_queue_page),
        )
        .route(
            "/admin/review_candidates",
            get(routes::review_queue::list_review_candidates),
        )
        .route(
            "/admin/review_candidates/:id",
            get(routes::review_queue::get_review_candidate),
        )
        .route(
            "/admin/review_candidates/:id/confirm",
            post(routes::review_queue::confirm_review_candidate),
        )
        .route(
            "/admin/review_candidates/:id/reject",
            post(routes::review_queue::reject_review_candidate),
        )
        .layer(axum_middleware::from_fn(
            middleware::admin_auth::require_admin,
        ))
}

/// Plain 404 for any path unmatched by either `public_router()` or
/// `authenticated_router()` (IMP-REQ-010-02). Set explicitly on the fully
/// merged, outermost `Router` — i.e. AFTER both sub-routers (each already
/// wrapped in their own `.layer(...)`, if any) have been merged in — so it
/// can never end up wrapped by `authenticated_router()`'s `require_admin`
/// layer. Before this fix, an unmatched top-level path (e.g. a typo'd URL)
/// could resolve to whichever sub-router's own implicit fallback the merge
/// happened to promote to the combined router's catch-all; when that
/// sub-router was `authenticated_router()`'s admin group, an unrelated,
/// never-registered path leaked a 403 rather than a normal 404, implying an
/// auth-gated area exists at a URL an anonymous visitor never asked about.
/// An explicit top-level fallback removes any dependence on merge order or
/// layering for this behavior.
async fn not_found() -> axum::http::StatusCode {
    axum::http::StatusCode::NOT_FOUND
}

/// Builds the full application router (routes + middleware), shared by
/// `main.rs` and integration tests so tests exercise the same wiring
/// (including auth middleware) that runs in production. Merges the
/// structurally-separate `public_router()`/`authenticated_router()`
/// (IMP-REQ-010-02) and pins an explicit top-level 404 `fallback` so an
/// unmatched path is never accidentally answered by either sub-router's own
/// fallback (see `not_found`'s doc comment).
pub fn app(state: AppState) -> Router {
    public_router(state.clone())
        .merge(authenticated_router())
        .fallback(not_found)
        .with_state(state)
}
