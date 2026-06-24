mod config;
mod error;
mod nats;
mod outbox;
mod routes;
mod state;

use sqlx::postgres::PgPoolOptions;
use tokio::net::TcpListener;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

use crate::{config::ApiConfig, state::AppState};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    if std::env::args().any(|argument| argument == "--print-openapi") {
        println!(
            "{}",
            serde_json::to_string_pretty(&routes::openapi_document())?
        );
        return Ok(());
    }

    tracing_subscriber::registry()
        .with(tracing_subscriber::EnvFilter::from_default_env())
        .with(tracing_subscriber::fmt::layer())
        .init();

    let config = ApiConfig::from_env();
    let pool = PgPoolOptions::new()
        .max_connections(10)
        .connect(&config.database_url)
        .await?;
    sqlx::migrate!("./migrations").run(&pool).await?;

    let jetstream = nats::connect_jetstream(&config.nats_url).await?;
    if let Some(jetstream) = jetstream.clone() {
        tokio::spawn(outbox::run_publisher(pool.clone(), jetstream));
    }
    let app = routes::router(AppState { pool });
    let listener = TcpListener::bind(&config.bind_addr).await?;
    tracing::info!(bind_addr = %config.bind_addr, "deepref API listening");
    axum::serve(listener, app).await?;
    Ok(())
}
