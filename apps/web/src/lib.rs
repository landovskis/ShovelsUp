pub mod middleware;
pub mod pipeline;
pub mod routes;

use axum::{
    middleware as axum_middleware,
    routing::{get, post},
    Router,
};
use minijinja::Environment;
use sqlx::PgPool;
use std::sync::Arc;

#[derive(Clone)]
pub struct AppState {
    pub env: Arc<Environment<'static>>,
    pub db: PgPool,
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
        .with_state(state)
}
