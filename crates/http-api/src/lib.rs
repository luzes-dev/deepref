pub mod config;
pub mod error;
pub mod routes;
pub mod state;

use sqlx::{PgPool, postgres::PgPoolOptions};
use std::future::Future;

use crate::{config::ApiConfig, state::AppState};

pub async fn migrate(config: &ApiConfig) -> anyhow::Result<()> {
    let pool = database_pool(config).await?;
    deepref_postgres::migrate(&pool).await?;
    tracing::info!("database migrations completed");
    Ok(())
}

pub async fn import_legacy(
    config: &ApiConfig,
) -> anyhow::Result<deepref_postgres::LegacyImportCounts> {
    let pool = database_pool(config).await?;
    deepref_postgres::import_legacy(&pool).await
}

pub async fn serve(config: ApiConfig) -> anyhow::Result<()> {
    serve_with_shutdown(config, shutdown_signal()).await
}

pub async fn serve_with_shutdown(
    config: ApiConfig,
    shutdown: impl Future<Output = ()> + Send + 'static,
) -> anyhow::Result<()> {
    deepref_application::validate_shipped_appraisal_definitions()
        .map_err(|error| anyhow::anyhow!("invalid shipped appraisal definition: {error}"))?;
    let pool = database_pool(&config).await?;

    let document_store = deepref_documents::DocumentStore::from_env()?;
    let state = AppState::new(pool).with_document_store(document_store);
    let listener = tokio::net::TcpListener::bind(config.bind_addr).await?;
    tracing::info!(address = %config.bind_addr, "API listening");
    axum::serve(listener, routes::router(state, &config))
        .with_graceful_shutdown(shutdown)
        .await?;
    Ok(())
}

async fn database_pool(config: &ApiConfig) -> anyhow::Result<PgPool> {
    let database = &config.runtime.database;
    Ok(PgPoolOptions::new()
        .min_connections(database.pool_min)
        .max_connections(database.pool_max)
        .acquire_timeout(database.acquire_timeout)
        .idle_timeout(Some(database.idle_timeout))
        .max_lifetime(Some(database.max_lifetime))
        .connect(&database.url)
        .await?)
}

async fn shutdown_signal() {
    #[cfg(unix)]
    {
        let mut terminate =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()).ok();
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {},
            _ = async {
                if let Some(stream) = terminate.as_mut() {
                    stream.recv().await;
                } else {
                    std::future::pending::<()>().await;
                }
            } => {},
        }
    }
    #[cfg(not(unix))]
    let _ = tokio::signal::ctrl_c().await;
}
