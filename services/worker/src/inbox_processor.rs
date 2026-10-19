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
use tokio::{task::JoinSet, time::Instant};
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

/// Counters remain available to the process supervisor and integration tests.
#[derive(Debug, Clone, Copy, Default)]
pub struct InboxProcessorReport {
    pub claim_errors: u64,
}

/// Process admitted provider events until `shutdown` resolves, then drain
/// in-flight events within the configured deadline.
pub async fn run(
    pool: PgPool,
    registry: Arc<InboxRegistry>,
    owner: String,
    config: InboxProcessorConfig,
    shutdown: impl Future<Output = ()> + Send,
) -> anyhow::Result<InboxProcessorReport> {
    anyhow::ensure!(
        config.batch > 0
            && config.concurrency > 0
            && !config.poll_interval.is_zero()
            && !config.claim_lease.is_zero(),
        "processor limits must be positive"
    );
    let mut active = JoinSet::new();
    let mut shutdown = Box::pin(shutdown);
    let mut report = InboxProcessorReport::default();
    let mut consecutive_claim_errors: u32 = 0;
    let mut next_claim = Instant::now();

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
                        tracing::error!(%error, "inbox processor event failed; its lease will be recovered");
                    }
                    Err(error) => {
                        tracing::error!(%error, "inbox event task ended abnormally; its lease will be recovered");
                    }
                }
            }
            _ = tokio::time::sleep_until(next_claim), if active.len() < config.concurrency => {
                let free = (config.concurrency - active.len()) as i64;
                let limit = config.batch.min(free.max(1));
                let claimed = tokio::select! {
                    _ = &mut shutdown => break,
                    result = deepref_app::inbox::claim_inbox_batch(&pool, &registry, &owner, config.claim_lease, limit) => result,
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
                        report.claim_errors = report.claim_errors.saturating_add(1);
                        consecutive_claim_errors = consecutive_claim_errors.saturating_add(1);
                        let delay = Duration::from_millis(100)
                            .saturating_mul(2u32.saturating_pow(consecutive_claim_errors.saturating_sub(1).min(6)))
                            .min(Duration::from_secs(5));
                        next_claim = Instant::now() + delay;
                        tracing::warn!(%error, claim_errors=report.claim_errors, consecutive_claim_errors,
                            retry_after_ms=delay.as_millis(), "inbox processor claim failed; retrying");
                    }
                }
            }
        }
    }

    let drained = tokio::time::timeout(config.drain_deadline, async {
        while let Some(result) = active.join_next().await {
            match result {
                Ok(Err(error)) => tracing::warn!(%error, "inbox processor event failed during drain"),
                Ok(Ok(())) => {},
                Err(error) => {
                    tracing::error!(%error, "inbox event task ended abnormally during drain; its lease will be recovered");
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
        anyhow::bail!("inbox processor drain deadline exceeded");
    }
    Ok(report)
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
