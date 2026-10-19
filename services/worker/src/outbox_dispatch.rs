//! In-process outbox dispatcher for the worker process (handoff 04a).
//!
//! Until the trigger-gated JetStream relay (04b) exists, this loop is the
//! outbox recipient: it claims a bounded batch with a lease, invokes the typed
//! handlers registered in [`OutboxRegistry`], and settles each round. Claiming
//! never marks a row delivered — only the consumers' committed receipts do.
//! The loop follows the job claim loop's shape: bounded work, per-event tasks,
//! responsive shutdown with a drain deadline.

use std::{future::Future, sync::Arc, time::Duration};

use deepref_app::outbox::{ClaimedOutboxEvent, dispatch_outbox_event};
use deepref_application::outbox::OutboxRegistry;
use sqlx::PgPool;
use tokio::task::JoinSet;
use tracing::Instrument;

/// Settings of the dispatch loop, separate from process wiring.
#[derive(Debug, Clone)]
pub struct OutboxDispatchConfig {
    /// Most rows claimed per tick.
    pub batch: i64,
    /// Most events dispatched at once.
    pub concurrency: usize,
    /// Claim lease; a crashed dispatcher's rows are reclaimable after it.
    pub claim_lease: Duration,
    /// Idle poll interval between claim ticks.
    pub poll_interval: Duration,
    /// How long in-flight events may settle after shutdown.
    pub drain_deadline: Duration,
}

/// Claim and dispatch committed outbox work until `shutdown` resolves, then
/// drain in-flight events within the configured deadline.
pub async fn run(
    pool: PgPool,
    registry: Arc<OutboxRegistry>,
    owner: String,
    config: OutboxDispatchConfig,
    shutdown: impl Future<Output = ()> + Send,
) -> anyhow::Result<()> {
    let mut active = JoinSet::new();
    let mut shutdown = Box::pin(shutdown);
    let mut loop_error = None;
    let mut tick = tokio::time::interval(config.poll_interval);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

    loop {
        tokio::select! {
            _ = &mut shutdown => {
                tracing::info!(active = active.len(), "outbox dispatcher drain started");
                break;
            }
            Some(joined) = active.join_next(), if !active.is_empty() => {
                match joined {
                    Ok(Ok(())) => {}
                    Ok(Err(error)) => {
                        loop_error = Some(error);
                        break;
                    }
                    // Handler panics are isolated inside dispatch; anything
                    // reaching here panicked in the dispatch bookkeeping
                    // itself. Lease recovery spends the claim attempt, so one
                    // event never takes the dispatcher down.
                    Err(error) => {
                        tracing::error!(%error, "outbox event task ended abnormally; its lease will be recovered");
                    }
                }
            }
            _ = tick.tick(), if active.len() < config.concurrency => {
                let free = (config.concurrency - active.len()) as i64;
                let limit = config.batch.min(free.max(1));
                match deepref_app::outbox::claim_outbox_batch(&pool, &owner, config.claim_lease, limit).await {
                    Ok(claimed) => {
                        for event in claimed {
                            let pool = pool.clone();
                            let registry = Arc::clone(&registry);
                            let lease = config.claim_lease;
                            let span = tracing::info_span!(
                                "outbox_event",
                                event_id = %event.event_id,
                                message_type = %event.message_type,
                                message_version = event.message_version,
                                attempt = event.attempts,
                            );
                            active.spawn(
                                async move { process_event(pool, registry, lease, event).await }
                                    .instrument(span),
                            );
                        }
                    }
                    Err(error) => {
                        loop_error = Some(error);
                        break;
                    }
                }
            }
        }
    }

    let drained = tokio::time::timeout(config.drain_deadline, async {
        while let Some(result) = active.join_next().await {
            match result {
                Ok(result) => result?,
                Err(error) => {
                    tracing::error!(%error, "outbox event task ended abnormally during drain; its lease will be recovered");
                }
            }
        }
        Ok::<(), anyhow::Error>(())
    })
    .await;
    match drained {
        Ok(result) => result?,
        Err(_) => anyhow::bail!("outbox dispatcher drain deadline exceeded"),
    }
    if let Some(error) = loop_error {
        return Err(error);
    }
    Ok(())
}

/// One claimed event on its own task: a failure is durable delivery state (the
/// row retries or quarantines), never a dispatcher-process failure.
async fn process_event(
    pool: PgPool,
    registry: Arc<OutboxRegistry>,
    lease: Duration,
    event: ClaimedOutboxEvent,
) -> anyhow::Result<()> {
    match dispatch_outbox_event(&pool, &registry, lease, &event).await {
        Ok(()) => Ok(()),
        Err(error) => {
            tracing::warn!(
                event_id = %event.event_id,
                message_type = %event.message_type,
                %error,
                "outbox dispatch bookkeeping failed; the row will be recovered by its lease"
            );
            Ok(())
        }
    }
}
