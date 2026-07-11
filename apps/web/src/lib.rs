pub mod config;
pub mod domain;
pub mod jobs;
pub mod middleware;
pub mod pipeline;
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
        .route("/api/v1/projects/search", get(routes::search::search_projects))
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
        ))
        .layer(axum_middleware::from_fn(
            config::flags::require_review_queue_enabled,
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
        .merge(admin_routes)
        .merge(search_routes)
        .merge(review_queue_routes)
        .with_state(state)
}
