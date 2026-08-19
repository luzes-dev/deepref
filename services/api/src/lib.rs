pub mod config;
pub mod error;
mod jobs;
mod nats;
mod outbox;
pub mod routes;
pub mod state;

use std::future::Future;
use std::sync::Arc;

use deepref_graph::GraphRepository;
use sqlx::postgres::PgPoolOptions;

use crate::{config::ApiConfig, state::AppState};

pub async fn migrate(config: &ApiConfig) -> anyhow::Result<()> {
    let database = &config.runtime.database;
    let pool = PgPoolOptions::new()
        .min_connections(database.pool_min)
        .max_connections(database.pool_max)
        .acquire_timeout(database.acquire_timeout)
        .idle_timeout(Some(database.idle_timeout))
        .max_lifetime(Some(database.max_lifetime))
        .connect(&database.url)
        .await?;
    sqlx::migrate!("./migrations").run(&pool).await?;
    tracing::info!("database migrations completed");
    Ok(())
}

pub async fn serve(config: ApiConfig) -> anyhow::Result<()> {
    serve_with_shutdown(config, shutdown_signal()).await
}

pub async fn serve_with_shutdown(
    config: ApiConfig,
    shutdown: impl Future<Output = ()> + Send + 'static,
) -> anyhow::Result<()> {
    let database = &config.runtime.database;
    let pool = PgPoolOptions::new()
        .min_connections(database.pool_min)
        .max_connections(database.pool_max)
        .acquire_timeout(database.acquire_timeout)
        .idle_timeout(Some(database.idle_timeout))
        .max_lifetime(Some(database.max_lifetime))
        .connect(&database.url)
        .await?;

    let jetstream = match nats::connect_jetstream(&config.runtime.nats).await {
        Ok(context) => Some(context),
        Err(error) => {
            tracing::warn!(%error, "NATS unavailable; API starts degraded");
            None
        }
    };
    let neo4j = &config.runtime.neo4j;
    let graph = match GraphRepository::connect(
        &neo4j.uri,
        &neo4j.user,
        &neo4j.password,
        neo4j.query_timeout,
    )
    .await
    {
        Ok(repository) => Some(Arc::new(repository)),
        Err(error) => {
            tracing::warn!(%error, "Neo4j unavailable; graph routes start degraded");
            None
        }
    };
    let state = AppState::new(
        pool.clone(),
        jetstream.clone(),
        graph,
        config.graph_retry_after,
    );
    if let Some(context) = jetstream {
        tokio::spawn(outbox::run_publisher(pool.clone(), context));
    }
    tokio::spawn(jobs::run(pool.clone()));
    let listener = tokio::net::TcpListener::bind(config.bind_addr).await?;
    tracing::info!(address = %config.bind_addr, "API listening");
    axum::serve(listener, routes::router(state, &config))
        .with_graceful_shutdown(shutdown)
        .await?;
    Ok(())
}

async fn shutdown_signal() {
    #[cfg(unix)]
    {
        let mut terminate =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
                .expect("SIGTERM handler must install");
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {},
            _ = terminate.recv() => {},
        }
    }
    #[cfg(not(unix))]
    let _ = tokio::signal::ctrl_c().await;
}
