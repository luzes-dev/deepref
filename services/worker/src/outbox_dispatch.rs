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
use tokio::{task::JoinSet, time::Instant};
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

/// Counters remain available to the process supervisor and integration tests.
#[derive(Debug, Clone, Copy, Default)]
pub struct OutboxDispatchReport {
    pub claim_errors: u64,
}

/// Claim and dispatch committed outbox work until `shutdown` resolves, then
/// drain in-flight events within the configured deadline.
pub async fn run(
    pool: PgPool,
    registry: Arc<OutboxRegistry>,
    owner: String,
    config: OutboxDispatchConfig,
    shutdown: impl Future<Output = ()> + Send,
) -> anyhow::Result<OutboxDispatchReport> {
    anyhow::ensure!(
        config.batch > 0
            && config.concurrency > 0
            && !config.poll_interval.is_zero()
            && !config.claim_lease.is_zero(),
        "processor limits must be positive"
    );
    let mut active = JoinSet::new();
    let mut shutdown = Box::pin(shutdown);
    let mut report = OutboxDispatchReport::default();
    let mut consecutive_claim_errors: u32 = 0;
    let mut next_claim = Instant::now();

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
                        tracing::error!(%error, "outbox dispatcher event failed; its lease will be recovered");
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
            _ = tokio::time::sleep_until(next_claim), if active.len() < config.concurrency => {
                let free = (config.concurrency - active.len()) as i64;
                let limit = config.batch.min(free.max(1));
                let claimed = tokio::select! {
                    _ = &mut shutdown => break,
                    result = deepref_app::outbox::claim_outbox_batch(&pool, &registry, &owner, config.claim_lease, limit) => result,
                };
                match claimed {
                    Ok(claimed) => {
                        consecutive_claim_errors = 0;
                        next_claim = Instant::now() + config.poll_interval;
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
                        report.claim_errors = report.claim_errors.saturating_add(1);
                        consecutive_claim_errors = consecutive_claim_errors.saturating_add(1);
                        let delay = Duration::from_millis(100)
                            .saturating_mul(2u32.saturating_pow(consecutive_claim_errors.saturating_sub(1).min(6)))
                            .min(Duration::from_secs(5));
                        next_claim = Instant::now() + delay;
                        tracing::warn!(%error, claim_errors=report.claim_errors, consecutive_claim_errors,
                            retry_after_ms=delay.as_millis(), "outbox dispatcher claim failed; retrying");
                    }
                }
            }
        }
    }

    let drained = tokio::time::timeout(config.drain_deadline, async {
        while let Some(result) = active.join_next().await {
            match result {
                Ok(Err(error)) => tracing::warn!(%error, "outbox dispatcher event failed during drain"),
                Ok(Ok(())) => {},
                Err(error) => {
                    tracing::error!(%error, "outbox event task ended abnormally during drain; its lease will be recovered");
                }
            }
        }
        Ok::<(), anyhow::Error>(())
    })
    .await;
    if drained.is_err() {
        // JoinSet owns the actual handler futures. Aborting and awaiting every
        // event drops transactions and locks before this processor returns.
        active.abort_all();
        while active.join_next().await.is_some() {}
        anyhow::bail!("outbox dispatcher drain deadline exceeded");
    }
    Ok(report)
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
