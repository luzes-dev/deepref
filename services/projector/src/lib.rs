pub mod config;
pub mod consumer;
pub mod metrics;
pub mod projection;
pub mod rebuild;
pub mod shutdown;
pub mod status;

use std::sync::Arc;

use async_nats::jetstream::AckKind;
use deepref_events::{DomainPayload, EventEnvelope};
use deepref_graph::GraphRepository;
use futures::StreamExt;
use sqlx::{PgPool, postgres::PgPoolOptions};

use crate::config::ProjectorConfig;

pub async fn connect_database(config: &ProjectorConfig) -> anyhow::Result<PgPool> {
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

pub async fn connect_graph(config: &ProjectorConfig) -> anyhow::Result<Arc<GraphRepository>> {
    let neo4j = &config.runtime.neo4j;
    Ok(Arc::new(
        GraphRepository::connect(
            &neo4j.uri,
            &neo4j.user,
            &neo4j.password,
            neo4j.query_timeout,
        )
        .await?,
    ))
}

pub async fn run(
    config: &ProjectorConfig,
    pool: PgPool,
    graph: Arc<GraphRepository>,
) -> anyhow::Result<()> {
    graph.apply_migrations().await?;
    let jetstream = consumer::connect(&config.runtime.nats).await?;
    let consumer = consumer::bind(&jetstream, &config.runtime.nats.projector_consumer).await?;
    let mut messages = consumer.messages().await?;
    loop {
        tokio::select! {
            _ = shutdown::wait_for_signal() => { tracing::info!("projector drain started"); break; }
            message = messages.next() => {
                let Some(message) = message else { break; };
                let message = message?;
                let event: EventEnvelope<DomainPayload> = match serde_json::from_slice(&message.payload) {
                    Ok(event) => event,
                    Err(error) => {
                        tracing::error!(%error, "invalid domain event terminated");
                        message.ack_with(AckKind::Term).await.map_err(ack_error)?;
                        continue;
                    }
                };
                match projection::apply(&pool, &graph, &event).await {
                    Ok(outcome) => {
                        tracing::debug!(event_id=%event.event_id, revision=event.revision, ?outcome, "projection delivery completed");
                        message.ack().await.map_err(ack_error)?;
                    }
                    Err(error) => {
                        tracing::error!(%error, event_id=%event.event_id, revision=event.revision, "projection failed");
                        status::record_failure(&pool, &event, &error.to_string()).await?;
                        message.ack_with(AckKind::Nak(Some(std::time::Duration::from_secs(5)))).await.map_err(ack_error)?;
                    }
                }
            }
        }
    }
    Ok(())
}

fn ack_error(error: Box<dyn std::error::Error + Send + Sync>) -> anyhow::Error {
    anyhow::anyhow!(error.to_string())
}
