use chrono::Utc;
use minijinja::{path_loader, Environment};
use shovelsup_pipeline::extractor::llm::AnthropicProvider;
use shovelsup_pipeline::parser::ocr::TesseractOcrProvider;
use shovelsup_pipeline::scheduler::Scheduler;
use shovelsup_pipeline::worker;
use sqlx::postgres::PgPoolOptions;
use std::time::Duration;
use std::{net::SocketAddr, sync::Arc};
use tower_http::{services::ServeDir, trace::TraceLayer};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

use shovelsup_web::AppState;

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

    let redis_url = std::env::var("REDIS_URL").expect("REDIS_URL must be set (see .env.example)");
    let redis_client = redis::Client::open(redis_url).expect("failed to build redis client");
    let redis = redis::aio::ConnectionManager::new(redis_client)
        .await
        .expect("failed to connect to redis");

    let state = AppState {
        env: Arc::new(env),
        db,
        redis,
    };

    let ocr = TesseractOcrProvider;
    let llm = AnthropicProvider::from_env();

    let pipeline_db = state.db.clone();
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(3600));
        loop {
            interval.tick().await;
            if let Err(e) = Scheduler::enqueue_due_fetches(&pipeline_db, Utc::now()).await {
                tracing::error!(error = %e, "enqueue_due_fetches failed");
            }
            match worker::run_due_fetch_jobs(&pipeline_db, &ocr, &llm).await {
                Ok(summary) => tracing::info!(?summary, "pipeline tick complete"),
                Err(e) => tracing::error!(error = %e, "run_due_fetch_jobs failed"),
            }
        }
    });

    let app = shovelsup_web::app(state)
        .nest_service("/static", ServeDir::new("static"))
        .layer(TraceLayer::new_for_http());

    let addr = SocketAddr::from(([0, 0, 0, 0], 3000));
    tracing::info!("listening on {addr}");
    let listener = tokio::net::TcpListener::bind(addr).await.unwrap();
    axum::serve(listener, app).await.unwrap();
}
