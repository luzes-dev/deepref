mod config;
mod limiter;
mod nats;
mod outbox;
mod processor;
mod store;

use async_nats::jetstream;
use futures::StreamExt;
use sqlx::postgres::PgPoolOptions;
use std::sync::Arc;
use tokio::sync::Semaphore;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

use crate::config::WorkerConfig;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::registry()
        .with(tracing_subscriber::EnvFilter::from_default_env())
        .with(tracing_subscriber::fmt::layer())
        .init();

    let config = WorkerConfig::from_env()?;
    let pool = PgPoolOptions::new()
        .max_connections(10)
        .connect(&config.database_url)
        .await?;
    let nats_client = async_nats::connect(config.nats_url).await?;
    let jetstream = jetstream::new(nats_client);
    let consumer = nats::ensure_stream_and_consumer(&jetstream).await?;
    let mut messages = consumer.messages().await?;
    tokio::spawn(outbox::run_publisher(pool.clone(), jetstream.clone()));

    let semaphore = Arc::new(Semaphore::new(config.concurrency));
    let settings = store::load_runtime_settings(&pool).await?;
    let limiter = Arc::new(limiter::per_second(settings.rate_limit_per_second));

    tracing::info!(
        concurrency = config.concurrency,
        rate_limit_per_second = settings.rate_limit_per_second,
        "citation worker started"
    );

    while let Some(message) = messages.next().await {
        let message = message?;
        let permit = semaphore.clone().acquire_owned().await?;
        let pool = pool.clone();
        let limiter = limiter.clone();
        tokio::spawn(async move {
            let _permit = permit;
            match processor::handle_message(pool, limiter, message.payload.to_vec()).await {
                Ok(()) => {
                    if let Err(error) = message.ack().await {
                        tracing::error!(%error, "failed to ack message");
                    }
                }
                Err(error) => tracing::error!(%error, "failed to process message"),
            }
        });
    }

    Ok(())
}
