//! Processor shell for the provider integration inbox (handoff 04a).
//!
//! Claims due inbox rows (`received`, retrying `failed`, or `processing` with
//! an expired lease) and runs them through the typed handler registry. No
//! provider handlers are registered yet: WorkOS and Polar adapters (handoffs
//! 02 and 08) register before their routes go live. Runs like the job claim
//! loop: bounded batch, per-event tasks, responsive shutdown with a drain
//! deadline.

use std::{future::Future, sync::Arc, time::Duration};

use deepref_app::inbox::{ClaimedInboxEvent, process_inbox_event};
use deepref_application::inbox::InboxRegistry;
use sqlx::PgPool;
use tokio::task::JoinSet;
use tracing::Instrument;

/// Settings of the inbox processing loop, separate from process wiring.
#[derive(Debug, Clone)]
pub struct InboxProcessorConfig {
    /// Most rows claimed per tick.
    pub batch: i64,
    /// Most events processed at once.
    pub concurrency: usize,
    /// Claim lease; a crashed processor's rows are reclaimable after it.
    pub claim_lease: Duration,
    /// Idle poll interval between claim ticks.
    pub poll_interval: Duration,
    /// How long in-flight events may settle after shutdown.
    pub drain_deadline: Duration,
}

/// Process admitted provider events until `shutdown` resolves, then drain
/// in-flight events within the configured deadline.
pub async fn run(
    pool: PgPool,
    registry: Arc<InboxRegistry>,
    owner: String,
    config: InboxProcessorConfig,
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
                tracing::info!(active = active.len(), "inbox processor drain started");
                break;
            }
            Some(joined) = active.join_next(), if !active.is_empty() => {
                match joined {
                    Ok(Ok(())) => {}
                    Ok(Err(error)) => {
                        loop_error = Some(error);
                        break;
                    }
                    Err(error) => {
                        tracing::error!(%error, "inbox event task ended abnormally; its lease will be recovered");
                    }
                }
            }
            _ = tick.tick(), if active.len() < config.concurrency => {
                let free = (config.concurrency - active.len()) as i64;
                let limit = config.batch.min(free.max(1));
                match deepref_app::inbox::claim_inbox_batch(&pool, &owner, config.claim_lease, limit).await {
                    Ok(claimed) => {
                        for event in claimed {
                            let pool = pool.clone();
                            let registry = Arc::clone(&registry);
                            let lease = config.claim_lease;
                            let span = tracing::info_span!(
                                "inbox_event",
                                inbox_id = event.id,
                                provider = %event.provider,
                                event_type = event.event_type.as_deref().unwrap_or(""),
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
                    tracing::error!(%error, "inbox event task ended abnormally during drain; its lease will be recovered");
                }
            }
        }
        Ok::<(), anyhow::Error>(())
    })
    .await;
    match drained {
        Ok(result) => result?,
        Err(_) => anyhow::bail!("inbox processor drain deadline exceeded"),
    }
    if let Some(error) = loop_error {
        return Err(error);
    }
    Ok(())
}

/// One claimed inbox event on its own task: a failure is durable inbox state
/// (the row retries or quarantines), never a processor-process failure.
async fn process_event(
    pool: PgPool,
    registry: Arc<InboxRegistry>,
    lease: Duration,
    event: ClaimedInboxEvent,
) -> anyhow::Result<()> {
    match process_inbox_event(&pool, &registry, lease, &event).await {
        Ok(()) => Ok(()),
        Err(error) => {
            tracing::warn!(
                inbox_id = event.id,
                provider = %event.provider,
                %error,
                "inbox processing bookkeeping failed; the row will be recovered by its lease"
            );
            Ok(())
        }
    }
}
