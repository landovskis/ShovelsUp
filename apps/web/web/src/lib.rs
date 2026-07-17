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

/// Builds the full application router (routes + middleware), shared by
/// `main.rs` and integration tests so tests exercise the same wiring
/// (including auth middleware) that runs in production.
pub fn app(state: AppState) -> Router {
    let admin_routes = Router::new()
        .route(
            "/admin/fetch_jobs/:id/reprocess",
            post(routes::admin::reprocess_fetch_job),
        )
        .route(
            "/admin/source_documents/:id/reprocess",
            post(routes::admin::reprocess_source_document),
        )
        .layer(axum_middleware::from_fn(
            middleware::admin_auth::require_admin,
        ));

    let search_routes = Router::new()
        .route("/search", get(routes::search::get_search_page))
        .route(
            "/api/v1/projects/search",
            get(routes::search::search_projects),
        )
        .layer(axum_middleware::from_fn_with_state(
            state.clone(),
            middleware::rate_limit::rate_limit_search,
        ));

    let review_queue_routes = Router::new()
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
        // IMP-REQ-008-04: public, unauthenticated category facet endpoint.
        // Deliberately routed here rather than under `admin_routes` — that
        // sub-router's `.layer(require_admin)` wraps its own 404 fallback,
        // which becomes the merged app's catch-all for any unmatched path
        // (see this module's own doc comment/CLAUDE.md notes on that
        // quirk); routing `/categories` on the top-level `Router` instead
        // keeps it outside admin auth entirely.
        .route("/categories", get(routes::search::list_categories))
        .merge(admin_routes)
        .merge(search_routes)
        .merge(review_queue_routes)
        .with_state(state)
}
