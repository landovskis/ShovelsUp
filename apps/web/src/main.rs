use axum::{
    middleware as axum_middleware,
    routing::{get, post},
    Router,
};
use minijinja::{path_loader, Environment};
use sqlx::postgres::PgPoolOptions;
use std::{net::SocketAddr, sync::Arc};
use tower_http::{services::ServeDir, trace::TraceLayer};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

use shovelsup_web::{middleware, routes, AppState};

#[tokio::main]
async fn main() {
    tracing_subscriber::registry()
        .with(tracing_subscriber::EnvFilter::new(
            std::env::var("RUST_LOG")
                .unwrap_or_else(|_| "shovelsup_web=debug,tower_http=debug".into()),
        ))
        .with(tracing_subscriber::fmt::layer())
        .init();

    let mut env = Environment::new();
    env.set_loader(path_loader("templates"));

    let database_url =
        std::env::var("DATABASE_URL").expect("DATABASE_URL must be set (see .env.example)");
    let db = PgPoolOptions::new()
        .max_connections(10)
        .connect(&database_url)
        .await
        .expect("failed to connect to database");
    sqlx::migrate!("./migrations")
        .run(&db)
        .await
        .expect("failed to run migrations");

    let state = AppState {
        env: Arc::new(env),
        db,
    };

    let admin_routes = Router::new()
        .route(
            "/admin/fetch_jobs/:id/reprocess",
            post(routes::admin::reprocess_fetch_job),
        )
        .layer(axum_middleware::from_fn(
            middleware::admin_auth::require_admin,
        ));

    let app = Router::new()
        .route("/", get(routes::index))
        .merge(admin_routes)
        .nest_service("/static", ServeDir::new("static"))
        .layer(TraceLayer::new_for_http())
        .with_state(state);

    let addr = SocketAddr::from(([0, 0, 0, 0], 3000));
    tracing::info!("listening on {addr}");
    let listener = tokio::net::TcpListener::bind(addr).await.unwrap();
    axum::serve(listener, app).await.unwrap();
}
