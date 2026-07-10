pub mod middleware;
pub mod pipeline;
pub mod routes;

use minijinja::Environment;
use sqlx::PgPool;
use std::sync::Arc;

#[derive(Clone)]
pub struct AppState {
    pub env: Arc<Environment<'static>>,
    pub db: PgPool,
}
