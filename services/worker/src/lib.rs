use std::{future::Future, pin::Pin, sync::Arc, time::Duration};

use deepref_ai::{AiGateway, RoutedGateway};
use deepref_app::{claim_job, complete_job, fail_job, renew_job};
use deepref_application::jobs::{ClaimedJob, JobLease, JobRetry};
use deepref_documents::DocumentStore;
use delivery::DeliveryAction;
use sqlx::postgres::PgPoolOptions;
use tokio::{
    sync::{Semaphore, watch},
    task::JoinSet,
};
use tracing::Instrument;

pub mod assistant;
pub mod config;
pub mod delivery;
pub mod enrichment;
pub mod heartbeat;
pub mod inbox_processor;
pub mod limiter;
pub mod metadata;
pub mod outbox_dispatch;
pub mod pmid_import;
pub mod processor;
pub mod reconciler;
pub mod second_review;
pub mod shutdown;
pub mod store;
pub mod workflows;

/// Metered provider gateway for the configured AI provider (OpenCode Go by
/// default) when its key is set, otherwise an empty router so AI jobs fail with
/// a clear "no adapter" error instead of a crash.
async fn build_ai_gateway(
    pool: &sqlx::PgPool,
    ai_config: &deepref_config::AiProviderConfig,
) -> anyhow::Result<Arc<dyn AiGateway>> {
    for notice in ai_config.deprecations() {
        tracing::warn!("{notice}");
    }
    let provider = ai_config.provider;
    let Some(api_key) = ai_config.api_key() else {
        tracing::warn!(
            provider = provider.id(),
            "{} is not set; AI jobs are disabled",
            provider.api_key_variable()
        );
        return Ok(Arc::new(RoutedGateway::default()));
    };
    let prices = deepref_ai::PriceBook::new(ai_config.price_overrides.iter().map(|price| {
        (
            price.provider.clone(),
            price.model.clone(),
            deepref_ai::ModelPrice {
                input_micros_per_million_tokens: price.input_micros_per_million_tokens,
                output_micros_per_million_tokens: price.output_micros_per_million_tokens,
            },
        )
    }));
    let routes =
        deepref_app::ensure_default_model_routes(pool, provider.id(), &ai_config.default_model)
            .await?;
    tracing::info!(
        provider = provider.id(),
        routes_created = routes.created,
        routes_repointed = routes.repointed,
        "AI provider configured for jobs"
    );
    let gateway = deepref_ai::build_metered_provider(
        provider.id(),
        &ai_config.base_url,
        api_key,
        Arc::new(deepref_app::PostgresUsageLedger::new(pool)),
        prices,
    )?;
    Ok(gateway)
}

pub async fn run(config: config::WorkerConfig) -> anyhow::Result<()> {
    run_with_shutdown(config, shutdown::wait_for_signal()).await
}

/// Runs one claimed job and says what the queue should do with it. The
/// production handler dispatches by job kind (`processor`); tests substitute
/// their own to exercise the claim loop. The `JobLease` is the claim token
/// every lease-checked write of the job must present.
pub type JobHandler = Arc<
    dyn Fn(
            ClaimedJob,
            JobLease,
        ) -> Pin<Box<dyn Future<Output = anyhow::Result<DeliveryAction>> + Send>>
        + Send
        + Sync,
>;

/// Settings of the claim loop, separate from process wiring.
#[derive(Debug, Clone)]
pub struct ClaimLoopConfig {
    pub concurrency: usize,
    pub claim_lease: Duration,
    pub drain_deadline: Duration,
}

/// Checks the OpenAlex key once, when the deployment chain names OpenAlex. A key that
/// fails only downgrades this process to keyless, so the probe never stops boot.
async fn probe_openalex(pool: &sqlx::PgPool, config: &config::WorkerConfig) {
    let Some(openalex) = &config.metadata.openalex else {
        return;
    };
    if !config
        .runtime
        .integrations
        .enabled_metadata_providers()
        .contains(&deepref_domain::ProviderKind::OpenAlex)
    {
        return;
    }
    let settings = match store::load_runtime_settings(pool).await {
        Ok(settings) => settings,
        Err(error) => {
            tracing::warn!(%error, "OpenAlex boot probe skipped; the runtime settings could not be read");
            return;
        }
    };
    let permit =
        limiter::ProviderLimiter::new(pool, settings.rate_limit_per_second, Some(openalex));
    let key_state = openalex.probe_key(&permit).await;
    tracing::info!(
        provider = deepref_domain::ProviderKind::OpenAlex.as_str(),
        mode = key_state.as_str(),
        rate_per_second = openalex.rate_per_second(),
        "OpenAlex boot probe finished"
    );
}

pub async fn run_with_shutdown(
    config: config::WorkerConfig,
    shutdown_signal: impl Future<Output = ()> + Send + 'static,
) -> anyhow::Result<()> {
    let database = &config.runtime.database;
    let pool = PgPoolOptions::new()
        .min_connections(database.pool_min)
        .max_connections(database.pool_max)
        .acquire_timeout(database.acquire_timeout)
        .idle_timeout(Some(database.idle_timeout))
        .max_lifetime(Some(database.max_lifetime))
        .connect(database.url())
        .await?;
    let owner = format!("deepref-worker-{}", uuid::Uuid::new_v4());
    let document_store = Arc::new(DocumentStore::from_config(&config.documents)?);
    let ai_gateway = build_ai_gateway(&pool, &config.runtime.ai).await?;
    // Workflow blocks that change review state follow the project's AI
    // autonomy settings.
    workflows::set_autonomy_gate(Arc::new(deepref_app::ProjectAutonomyGate::new(&pool)));
    anyhow::ensure!(
        deepref_app::configure_workflow_runtime(config.runtime.workflow.niche_nodes_enabled),
        "API and worker workflow feature configuration must agree"
    );
    let (reconciler_shutdown, reconciler_signal) = watch::channel(false);
    if config.runtime.integrations.contact_email.is_empty() {
        tracing::warn!(
            "DEEPREF_CONTACT_EMAIL is not set; DOI imports, PubMed imports and open-access lookups will be refused"
        );
    }
    tracing::info!(
        metadata_providers = %config.runtime.integrations.metadata_chain_label(),
        "metadata provider chain configured"
    );
    probe_openalex(&pool, &config).await;
    tokio::spawn(workflows::run_scheduler(
        pool.clone(),
        Duration::from_secs(20),
        config.runtime.integrations.contact_email.clone(),
        reconciler_signal.clone(),
    ));
    let provider_reconcilers =
        Arc::new(deepref_application::inbox::ProviderReconcilerRegistry::new());
    let reconciler_task = tokio::spawn(reconciler::run_with_provider_reconcilers(
        pool.clone(),
        config.reconciler_interval,
        reconciler_signal.clone(),
        provider_reconcilers,
    ));
    // The in-process outbox dispatcher (04a) is the outbox recipient until the
    // trigger-gated JetStream relay (04b) replaces it on the same rows. Its
    // registry is empty in 04a; production publication starts with 04b.
    // The inbox processor shell claims nothing
    // until provider adapters register handlers in 02/08.
    let outbox_task = tokio::spawn(supervise_processor(
        "outbox dispatcher",
        reconciler_shutdown.clone(),
        outbox_dispatch::run(
            pool.clone(),
            Arc::new(deepref_application::outbox::OutboxRegistry::new()),
            owner.clone(),
            outbox_dispatch::OutboxDispatchConfig {
                batch: 16,
                concurrency: config.concurrency,
                claim_lease: config.claim_lease,
                poll_interval: Duration::from_millis(250),
                drain_deadline: config.runtime.telemetry.shutdown_deadline,
            },
            shutdown::on_signal(reconciler_signal.clone()),
        ),
    ));
    let inbox_task = tokio::spawn(supervise_processor(
        "inbox processor",
        reconciler_shutdown.clone(),
        inbox_processor::run(
            pool.clone(),
            Arc::new(deepref_application::inbox::InboxRegistry::new()),
            owner.clone(),
            inbox_processor::InboxProcessorConfig {
                batch: 16,
                concurrency: config.concurrency,
                claim_lease: config.claim_lease,
                poll_interval: Duration::from_millis(250),
                drain_deadline: config.runtime.telemetry.shutdown_deadline,
            },
            shutdown::on_signal(reconciler_signal.clone()),
        ),
    ));
    // The heartbeat outlives the drain: the worker is alive until its last
    // in-flight job has been settled.
    let (heartbeat_stop, heartbeat_signal) = watch::channel(false);
    let heartbeat_task = tokio::spawn(heartbeat::run(
        pool.clone(),
        owner.clone(),
        heartbeat::HEARTBEAT_INTERVAL,
        heartbeat_signal,
    ));
    let loop_config = ClaimLoopConfig {
        concurrency: config.concurrency,
        claim_lease: config.claim_lease,
        drain_deadline: config.runtime.telemetry.shutdown_deadline,
    };
    let handler = production_handler(
        pool.clone(),
        document_store,
        ai_gateway,
        Arc::new(config.clone()),
    );
    let loop_shutdown = {
        let reconciler_shutdown = reconciler_shutdown.clone();
        let processor_signal = reconciler_signal.clone();
        async move {
            tokio::select! {
                _ = shutdown_signal => {},
                _ = shutdown::on_signal(processor_signal) => {},
            }
            reconciler_shutdown.send_replace(true);
        }
    };
    let loop_result = run_claim_loop(pool, owner, loop_config, handler, loop_shutdown).await;

    heartbeat_stop.send_replace(true);
    if tokio::time::timeout(Duration::from_secs(5), heartbeat_task)
        .await
        .is_err()
    {
        tracing::warn!("worker heartbeat did not stop in time; its row will age out");
    }
    reconciler_shutdown.send_replace(true);
    let reconciler_drained =
        tokio::time::timeout(config.runtime.telemetry.shutdown_deadline, reconciler_task).await;
    // These processors enforce their own drain deadline, then abort and await
    // every owned event. Await the processors themselves instead of dropping
    // a JoinHandle on an outer timeout and detaching cleanup.
    let outbox_drained = outbox_task.await;
    let inbox_drained = inbox_task.await;
    loop_result?;
    match reconciler_drained {
        Ok(result) => result?,
        Err(_) => anyhow::bail!("worker reconciler drain deadline exceeded"),
    }
    outbox_drained??;
    inbox_drained??;
    Ok(())
}

/// An unexpected processor exit (including unwind) stops the worker so its
/// process supervisor can restart it. Transient claim errors retry in the loop.
async fn supervise_processor<T>(
    name: &'static str,
    shutdown: watch::Sender<bool>,
    work: impl Future<Output = anyhow::Result<T>>,
) -> anyhow::Result<T> {
    struct StopOnExit {
        name: &'static str,
        shutdown: watch::Sender<bool>,
    }
    impl Drop for StopOnExit {
        fn drop(&mut self) {
            if !*self.shutdown.borrow() {
                tracing::error!(
                    processor = self.name,
                    "processor stopped unexpectedly; stopping worker"
                );
                self.shutdown.send_replace(true);
            }
        }
    }
    let _guard = StopOnExit {
        name,
        shutdown: shutdown.clone(),
    };
    let result = work.await;
    if !*shutdown.borrow() {
        result?;
        anyhow::bail!("{name} stopped unexpectedly");
    }
    result
}

fn production_handler(
    pool: sqlx::PgPool,
    document_store: Arc<DocumentStore>,
    ai_gateway: Arc<dyn AiGateway>,
    config: Arc<config::WorkerConfig>,
) -> JobHandler {
    Arc::new(move |job: ClaimedJob, lease: JobLease| {
        let pool = pool.clone();
        let document_store = Arc::clone(&document_store);
        let ai_gateway = Arc::clone(&ai_gateway);
        let config = Arc::clone(&config);
        Box::pin(async move {
            processor::handle_job_with_documents_owned_and_ai(
                pool,
                &job,
                &lease,
                Some(document_store),
                None,
                ai_gateway,
                &config,
            )
            .await
        })
    })
}

/// Claim jobs and run them through `handler` until `shutdown` resolves, then
/// drain in-flight jobs within the configured deadline.
pub async fn run_claim_loop(
    pool: sqlx::PgPool,
    owner: String,
    config: ClaimLoopConfig,
    handler: JobHandler,
    shutdown: impl Future<Output = ()> + Send,
) -> anyhow::Result<()> {
    let semaphore = Arc::new(Semaphore::new(config.concurrency));
    let mut active = JoinSet::new();
    let mut shutdown = Box::pin(shutdown);
    let mut loop_error = None;
    let mut claim_tick = tokio::time::interval(Duration::from_millis(100));
    claim_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

    loop {
        tokio::select! {
            _ = &mut shutdown => {
                tracing::info!(active=active.len(), "worker drain started");
                break;
            }
            Some(joined) = active.join_next(), if !active.is_empty() => {
                match joined {
                    Ok(Ok(())) => {}
                    Ok(Err(error)) => {
                        loop_error = Some(error);
                        break;
                    }
                    // Handler panics are caught inside `process_claimed_job`;
                    // anything reaching here panicked in the queue bookkeeping
                    // itself. Lease recovery spends the attempt, so one job
                    // never takes the worker down.
                    Err(error) => {
                        tracing::error!(%error, "job task ended abnormally; its lease will be recovered");
                    }
                }
            }
            _ = claim_tick.tick(), if active.len() < config.concurrency => {
                match claim_job(&pool, &owner, config.claim_lease).await {
                    Ok(Some(job)) => {
                        let permit = match semaphore.clone().acquire_owned().await {
                            Ok(permit) => permit,
                            Err(error) => {
                                loop_error = Some(error.into());
                                break;
                            }
                        };
                        let pool = pool.clone();
                        let lease = job.lease(owner.clone());
                        let handler = Arc::clone(&handler);
                        let duration = config.claim_lease;
                        // Every line logged for this job, including inside its
                        // handler task, carries the job and project it serves.
                        let span = tracing::info_span!(
                            "job",
                            job_id = %job.id,
                            kind = %job.kind,
                            project_id = %job.project_id.as_uuid(),
                            attempt = job.attempts,
                        );
                        active.spawn(
                            async move {
                                let _permit = permit;
                                process_claimed_job(pool, job, lease, duration, handler).await
                            }
                            .instrument(span),
                        );
                    }
                    Ok(None) => {}
                    Err(error) => {
                        loop_error = Some(error);
                        break;
                    }
                }
            }
        }
    }

    let active_drained = tokio::time::timeout(config.drain_deadline, async {
        while let Some(result) = active.join_next().await {
            match result {
                Ok(result) => result?,
                Err(error) => {
                    tracing::error!(%error, "job task ended abnormally during drain; its lease will be recovered");
                }
            }
        }
        Ok::<(), anyhow::Error>(())
    })
    .await;
    match active_drained {
        Ok(result) => result?,
        Err(_) => anyhow::bail!(
            "worker drain deadline exceeded with {} active tasks",
            active.len()
        ),
    }
    if let Some(error) = loop_error {
        return Err(error);
    }
    Ok(())
}

async fn process_claimed_job(
    pool: sqlx::PgPool,
    job: ClaimedJob,
    lease: JobLease,
    duration: Duration,
    handler: JobHandler,
) -> anyhow::Result<()> {
    let result = {
        let renew_pool = pool.clone();
        let renew_lease = lease.clone();
        let handler_job = job.clone();
        let handler_lease = lease.clone();
        run_with_lease_renewal(
            duration,
            move || {
                let pool = renew_pool.clone();
                let lease = renew_lease.clone();
                async move { renew_job(&pool, &lease, duration).await }
            },
            run_isolated(async move { handler(handler_job, handler_lease).await }),
        )
        .await
    };
    match result {
        Ok(DeliveryAction::Ack) => {
            if !complete_job(&pool, &lease).await? {
                tracing::warn!(job_id=%job.id, kind=%job.kind, attempts=job.attempts, "job completion rejected because the lease is no longer owned");
            }
        }
        Ok(DeliveryAction::Nak(delay)) => {
            record_failure(
                &pool,
                &job,
                &lease,
                "retry requested",
                JobRetry::After(delay),
            )
            .await?;
        }
        Ok(DeliveryAction::Terminate) => {
            record_failure(
                &pool,
                &job,
                &lease,
                "terminal job failure",
                JobRetry::DeadLetter,
            )
            .await?;
        }
        Err(error) => {
            // A job failure is durable work state, not a worker-process failure. The
            // queue update decides whether this attempt is retried or dead-lettered;
            // only an error while persisting that decision should stop the worker.
            record_failure(
                &pool,
                &job,
                &lease,
                &error.to_string(),
                JobRetry::After(Duration::from_secs(10)),
            )
            .await?;
        }
    }
    Ok(())
}

/// Persist a failed attempt and say so in the log: `warn!` when the job will be
/// retried, `error!` when this attempt dead-letters it.
async fn record_failure(
    pool: &sqlx::PgPool,
    job: &ClaimedJob,
    lease: &JobLease,
    error: &str,
    retry: JobRetry,
) -> anyhow::Result<()> {
    let persisted = fail_job(pool, lease, error, retry).await?;
    let dead_letter = match retry {
        JobRetry::DeadLetter => true,
        JobRetry::After(_) => job.attempts >= job.max_attempts,
    };
    if !persisted {
        tracing::warn!(
            job_id = %job.id,
            kind = %job.kind,
            attempts = job.attempts,
            error,
            "job failure update rejected because the lease is no longer owned"
        );
    } else if dead_letter {
        tracing::error!(
            job_id = %job.id,
            kind = %job.kind,
            attempts = job.attempts,
            max_attempts = job.max_attempts,
            error,
            "job dead-lettered"
        );
    } else {
        let retry_after_ms = match retry {
            JobRetry::After(delay) => delay.as_millis() as u64,
            JobRetry::DeadLetter => 0,
        };
        tracing::warn!(
            job_id = %job.id,
            kind = %job.kind,
            attempts = job.attempts,
            max_attempts = job.max_attempts,
            retry_after_ms,
            error,
            "job failed; will retry"
        );
    }
    Ok(())
}

/// Run a handler future on its own task so a panic becomes this job's failure
/// instead of tearing down the claim loop. The task is aborted if the caller
/// stops waiting (for example when the lease is lost).
async fn run_isolated<F>(work: F) -> anyhow::Result<DeliveryAction>
where
    F: Future<Output = anyhow::Result<DeliveryAction>> + Send + 'static,
{
    struct AbortOnDrop(tokio::task::JoinHandle<anyhow::Result<DeliveryAction>>);
    impl Drop for AbortOnDrop {
        fn drop(&mut self) {
            self.0.abort();
        }
    }
    let mut task = AbortOnDrop(tokio::spawn(work.in_current_span()));
    match (&mut task.0).await {
        Ok(result) => result,
        Err(error) if error.is_panic() => {
            let payload = error.into_panic();
            let message = payload
                .downcast_ref::<&str>()
                .map(|message| (*message).to_owned())
                .or_else(|| payload.downcast_ref::<String>().cloned())
                .unwrap_or_else(|| "non-string panic payload".to_owned());
            Err(anyhow::anyhow!("job handler panicked: {message}"))
        }
        Err(error) => Err(anyhow::anyhow!("job handler was cancelled: {error}")),
    }
}

/// Drive `work` while renewing its lease every third of `lease`. `renew`
/// returns whether the lease is still held.
async fn run_with_lease_renewal<F, R, RF>(
    lease: Duration,
    mut renew: R,
    work: F,
) -> anyhow::Result<DeliveryAction>
where
    F: Future<Output = anyhow::Result<DeliveryAction>>,
    R: FnMut() -> RF,
    RF: Future<Output = anyhow::Result<bool>>,
{
    use tokio::time::Instant;

    let interval = (lease / 3).max(Duration::from_millis(100));
    tokio::pin!(work);
    // The lease is certainly held until `held_until`: it is measured from
    // before the last successful renewal was sent, so it never overestimates.
    let mut held_until = Instant::now() + lease;
    let mut next_renewal = Instant::now() + interval;
    let mut failures: u32 = 0;
    loop {
        tokio::select! {
            result = &mut work => return result,
            _ = tokio::time::sleep_until(next_renewal) => {
                let sent_at = Instant::now();
                let renewal = tokio::time::timeout_at(held_until, renew())
                    .await
                    .map_err(|_| anyhow::anyhow!("job lease renewal timed out before expiry"))?;
                match renewal {
                    Ok(true) => {
                        held_until = sent_at + lease;
                        failures = 0;
                        next_renewal = Instant::now() + interval;
                    }
                    // Another claim owns the job now: stop at once.
                    Ok(false) => anyhow::bail!("job lease was lost before completion"),
                    // A failed UPDATE says nothing about the lease, which may
                    // still be valid: retry with backoff while it lasts instead
                    // of cancelling work that may already have side effects.
                    Err(error) => {
                        failures += 1;
                        let backoff = Duration::from_millis(100)
                            .saturating_mul(2_u32.saturating_pow(failures - 1))
                            .min(interval);
                        let retry_at = Instant::now() + backoff;
                        if retry_at >= held_until {
                            anyhow::bail!(
                                "job lease could not be renewed before it expired: {error:#}"
                            );
                        }
                        tracing::warn!(%error, failures, retry_in_ms = backoff.as_millis() as u64, "job lease renewal failed; retrying");
                        next_renewal = retry_at;
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU32, Ordering};

    use super::*;

    /// One failed renewal UPDATE (a dropped connection, a pool timeout) must
    /// not cancel work that still holds a valid lease.
    #[tokio::test]
    async fn a_transient_renewal_error_does_not_cancel_the_work() {
        let calls = Arc::new(AtomicU32::new(0));
        let renew = {
            let calls = Arc::clone(&calls);
            move || {
                let call = calls.fetch_add(1, Ordering::SeqCst);
                async move {
                    if call == 0 {
                        anyhow::bail!("connection reset by peer");
                    }
                    Ok(true)
                }
            }
        };
        let work = async {
            tokio::time::sleep(Duration::from_millis(700)).await;
            Ok(DeliveryAction::Ack)
        };
        let result = run_with_lease_renewal(Duration::from_millis(600), renew, work).await;
        assert!(
            matches!(result, Ok(DeliveryAction::Ack)),
            "work was cancelled: {result:?}"
        );
        assert!(calls.load(Ordering::SeqCst) >= 2, "renewal was retried");
    }

    /// A lease that is definitely gone (another claim holds the job) still
    /// stops the work at once.
    #[tokio::test]
    async fn a_lost_lease_stops_the_work() {
        let work = async {
            tokio::time::sleep(Duration::from_secs(5)).await;
            Ok(DeliveryAction::Ack)
        };
        let result =
            run_with_lease_renewal(Duration::from_millis(300), || async { Ok(false) }, work).await;
        assert!(result.is_err());
    }

    /// Renewal that keeps failing gives up once the lease must have expired,
    /// instead of running unfenced forever.
    #[tokio::test]
    async fn renewal_gives_up_once_the_lease_has_expired() {
        let work = async {
            tokio::time::sleep(Duration::from_secs(5)).await;
            Ok(DeliveryAction::Ack)
        };
        let started = tokio::time::Instant::now();
        let result = run_with_lease_renewal(
            Duration::from_millis(300),
            || async { anyhow::bail!("database unavailable") },
            work,
        )
        .await;
        assert!(result.is_err());
        assert!(started.elapsed() < Duration::from_secs(2));
    }
}

#[allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
#[cfg(test)]
mod lease_deadline_tests {
    use super::*;

    #[tokio::test]
    async fn a_stalled_renewal_cannot_outlive_the_lease() {
        let started = tokio::time::Instant::now();
        let result = run_with_lease_renewal(
            Duration::from_millis(300),
            std::future::pending::<anyhow::Result<bool>>,
            std::future::pending::<anyhow::Result<DeliveryAction>>(),
        )
        .await;
        assert!(result.is_err());
        assert!(started.elapsed() < Duration::from_secs(2));
    }

    #[tokio::test]
    async fn a_panic_while_constructing_the_handler_future_is_isolated() {
        let handler: JobHandler = Arc::new(|_, _| panic!("handler construction failed"));
        let job = ClaimedJob {
            id: uuid::Uuid::new_v4(),
            project_id: deepref_domain::ProjectId::new(uuid::Uuid::new_v4()),
            kind: deepref_domain::JobKind::RecomputeMetrics,
            payload: serde_json::json!({}),
            attempts: 1,
            max_attempts: 2,
        };
        let lease = job.lease("test-worker".to_owned());
        let result = run_isolated(async move { handler(job, lease).await }).await;
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("handler construction failed")
        );
    }
}
