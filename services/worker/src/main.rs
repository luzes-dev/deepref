use std::sync::Arc;

use async_nats::jetstream::AckKind;
use deepref_worker::{
    config::WorkerConfig, delivery::DeliveryAction, nats, outbox, processor, reconciler, shutdown,
};
use futures::StreamExt;
use sqlx::postgres::PgPoolOptions;
use tokio::{sync::Semaphore, task::JoinSet};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let config = WorkerConfig::from_env()?;
    let telemetry = deepref_telemetry::init(config.runtime.telemetry.clone())?;
    let database = &config.runtime.database;
    let pool = PgPoolOptions::new()
        .min_connections(database.pool_min)
        .max_connections(database.pool_max)
        .acquire_timeout(database.acquire_timeout)
        .idle_timeout(Some(database.idle_timeout))
        .max_lifetime(Some(database.max_lifetime))
        .connect(&database.url)
        .await?;
    let jetstream = nats::connect(&config.runtime.nats).await?;
    let consumer = nats::bind_consumer(&jetstream, &config.runtime.nats.worker_consumer).await?;
    let mut messages = consumer.messages().await?;
    let outbox_task = tokio::spawn(outbox::run_publisher(pool.clone(), jetstream.clone()));
    let reconciler_task = tokio::spawn(reconciler::run(pool.clone(), config.reconciler_interval));
    let semaphore = Arc::new(Semaphore::new(config.concurrency));
    let mut active = JoinSet::new();

    loop {
        tokio::select! {
            _ = shutdown::wait_for_signal() => {
                tracing::info!(active=active.len(), "worker drain started");
                break;
            }
            Some(joined) = active.join_next(), if !active.is_empty() => {
                if let Err(error) = joined { tracing::error!(%error, "worker task failed to join"); }
            }
            message = messages.next() => {
                let Some(message) = message else { break; };
                let message = message?;
                let permit = semaphore.clone().acquire_owned().await?;
                let pool = pool.clone();
                let claim_lease = config.claim_lease;
                active.spawn(async move {
                    let _permit = permit;
                    let delivered = message.info().map(|info| info.delivered.max(1) as u64).unwrap_or(1);
                    let action = processor::handle_message(pool, message.payload.to_vec(), delivered, claim_lease).await;
                    match action {
                        Ok(DeliveryAction::Ack) => message.ack().await.map_err(ack_error)?,
                        Ok(DeliveryAction::Nak(delay)) => message.ack_with(AckKind::Nak(Some(delay))).await.map_err(ack_error)?,
                        Ok(DeliveryAction::Terminate) => message.ack_with(AckKind::Term).await.map_err(ack_error)?,
                        Err(error) => {
                            tracing::error!(%error, delivered, "processing failed before durable disposition");
                            message.ack_with(AckKind::Nak(None)).await.map_err(ack_error)?;
                        }
                    }
                    Ok::<(), anyhow::Error>(())
                });
            }
        }
    }

    outbox_task.abort();
    reconciler_task.abort();
    let drained = tokio::time::timeout(config.runtime.telemetry.shutdown_deadline, async {
        while let Some(result) = active.join_next().await {
            result??;
        }
        Ok::<(), anyhow::Error>(())
    })
    .await;
    match drained {
        Ok(result) => result?,
        Err(_) => anyhow::bail!(
            "worker drain deadline exceeded with {} active tasks",
            active.len()
        ),
    }
    telemetry.shutdown().await;
    Ok(())
}

fn ack_error(error: Box<dyn std::error::Error + Send + Sync>) -> anyhow::Error {
    anyhow::anyhow!(error.to_string())
}
