use std::{
    collections::HashMap,
    future::Future,
    pin::Pin,
    sync::Arc,
    time::{Duration, Instant},
};

use deepref_ai::{AiGateway, RoutedGateway};
use deepref_app::{PostgresClaimTransport, PostgresDurableExecutionStore};
use deepref_application::jobs::{
    ClaimedJob, DurableExecutionStore, JobLease, JobRetry, WorkRequest, WorkTransport,
};
use deepref_documents::DocumentStore;
use deepref_domain::WorkloadClass;
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
pub mod metrics;
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
    let prices =
        deepref_ai::PriceBook::new(ai_config.model_price_table().into_iter().map(|price| {
            (
                price.provider,
                price.model,
                deepref_ai::ModelPrice {
                    input_micros_per_million_tokens: price.input_micros_per_million_tokens,
                    output_micros_per_million_tokens: price.output_micros_per_million_tokens,
                    reasoning_micros_per_million_tokens: price.reasoning_micros_per_million_tokens,
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
        Arc::new(deepref_app::PostgresAiConcurrencyAdmission::new(pool)),
        concurrency_limits(ai_config),
    )?;
    Ok(gateway)
}

/// The simultaneous-call ceilings one process admits its AI calls under.
pub(crate) fn concurrency_limits(
    ai_config: &deepref_config::AiProviderConfig,
) -> deepref_ai::AiConcurrencyLimits {
    deepref_ai::AiConcurrencyLimits {
        workspace: ai_config.max_concurrent_per_workspace,
        route: ai_config.max_concurrent_per_route,
    }
}

pub async fn run(config: config::WorkerConfig, pools: Vec<WorkloadClass>) -> anyhow::Result<()> {
    run_with_shutdown(config, pools, shutdown::wait_for_signal()).await
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
    /// Workload pools this worker serves: one per `--pool` class, or every
    /// class when the process was started without a pool selection.
    pub pools: Vec<WorkloadClass>,
    /// Concurrency per served pool. A saturated pool claims nothing and never
    /// drains or starves another pool.
    pub pool_concurrency: HashMap<WorkloadClass, usize>,
    /// Largest batch one claim statement may return. A tick claims at most
    /// `min(free pool capacity, max_batch)` jobs; there is no inner claim loop.
    pub max_batch: usize,
    pub claim_lease: Duration,
    pub drain_deadline: Duration,
}

/// Base claim interval and idle backoff ceiling: a tick that finds nothing
/// claimable backs off up to here instead of polling hot.
const CLAIM_TICK: Duration = Duration::from_millis(100);
const CLAIM_IDLE_MAX: Duration = Duration::from_secs(1);

fn lease_control_pool(business: &sqlx::PgPool) -> sqlx::PgPool {
    // Long business transactions (notably streaming export uploads) may use
    // every configured business connection. One additional, bounded control
    // connection keeps job claims/renewals/settlement and liveness independent.
    business
        .options()
        .clone()
        .min_connections(0)
        .max_connections(1)
        .connect_lazy_with(business.connect_options().as_ref().clone())
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
    pools: Vec<WorkloadClass>,
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
    let control_pool = lease_control_pool(&pool);
    // Validate the reserved connection at boot rather than discovering a
    // missing database connection allowance after consequential work starts.
    drop(control_pool.acquire().await?);
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
    let mut provider_reconcilers = deepref_application::inbox::ProviderReconcilerRegistry::new();
    let billing = if let Some(config) = config.runtime.dodo.clone() {
        Some(deepref_app::billing::BillingService::new(
            pool.clone(),
            Arc::new(deepref_integrations::dodo::DodoBilling::new(config)?),
        ))
    } else {
        None
    };
    let billing_inbox = if let Some(service) = billing {
        let registry = deepref_app::billing::registry(service.clone())?;
        provider_reconcilers.register(Arc::new(deepref_app::billing::BillingReconciler::new(
            service,
        )))?;
        registry
    } else {
        deepref_application::inbox::InboxRegistry::new()
    };
    let provider_reconcilers = Arc::new(provider_reconcilers);
    let reconciler_task = tokio::spawn(reconciler::run_with_provider_reconcilers(
        pool.clone(),
        config.reconciler_interval,
        reconciler_signal.clone(),
        provider_reconcilers,
        config.runtime.usage_export.clone(),
    ));
    // The in-process outbox dispatcher (04a) is the outbox recipient until the
    // trigger-gated JetStream relay (04b) replaces it on the same rows. Usage
    // export (09 slice B) registers the first production contract; while the
    // export switch is off no consumer is registered, so nothing is claimed
    // and no delivery attempt is spent waiting for a billing provider.
    let usage_export_registry = if config.runtime.usage_export.enabled {
        let sink: Arc<dyn deepref_application::usage_export::UsageExportSink> =
            // No billing provider is chosen yet (BUSINESS_DECISIONS.md). The
            // disabled sink never claims success, so exported events stay
            // pending until handoff 08 supplies an adapter.
            Arc::new(deepref_application::usage_export::DisabledUsageExportSink);
        deepref_app::usage_export_registry(
            &pool,
            sink,
            Arc::new(deepref_application::usage_export::UsageExportCounters::default()),
        )?
    } else {
        tracing::info!("AI usage export is disabled; retained usage facts wait for a later replay");
        deepref_application::outbox::OutboxRegistry::new()
    };
    let outbox_task = tokio::spawn(supervise_processor(
        "outbox dispatcher",
        reconciler_shutdown.clone(),
        outbox_dispatch::run(
            pool.clone(),
            Arc::new(usage_export_registry),
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
            Arc::new(billing_inbox),
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
    let pool_capacities = pools
        .iter()
        .map(|pool| {
            (
                *pool,
                config.pool_concurrency.get(pool).copied().unwrap_or(1),
            )
        })
        .collect::<Vec<_>>();
    let heartbeat_task = tokio::spawn(heartbeat::run(
        control_pool.clone(),
        owner.clone(),
        heartbeat::HEARTBEAT_INTERVAL,
        heartbeat_signal,
        pool_capacities,
    ));
    // Sampling is opt-in with the configured OTLP pipeline; no collector
    // means no additional operational queries on the application database.
    let metrics_task = config
        .runtime
        .telemetry
        .otlp_endpoint
        .as_ref()
        .map(|_| tokio::spawn(metrics::run(pool.clone(), reconciler_signal.clone())));
    let loop_config = ClaimLoopConfig {
        pools,
        pool_concurrency: config.pool_concurrency.clone(),
        max_batch: config.claim_batch_max,
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
    // The durable store owns lifecycle writes; the transport delivers work.
    // Under 05a both are the PostgreSQL queue and the claim *is* the delivery.
    let store = PostgresDurableExecutionStore {
        pool: control_pool.clone(),
    };
    let transport = PostgresClaimTransport {
        store: store.clone(),
    };
    let loop_result =
        run_claim_loop(transport, store, owner, loop_config, handler, loop_shutdown).await;

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
    if let Some(mut task) = metrics_task
        && tokio::time::timeout(Duration::from_secs(5), &mut task)
            .await
            .is_err()
    {
        task.abort();
        let _ = task.await;
        tracing::warn!("operational metric sampler was aborted during shutdown");
    }
    let outbox_drained = outbox_task.await;
    let inbox_drained = inbox_task.await;
    loop_result?;
    match reconciler_drained {
        Ok(result) => result?,
        Err(_) => anyhow::bail!("worker reconciler drain deadline exceeded"),
    }
    outbox_drained??;
    inbox_drained??;
    control_pool.close().await;
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

/// Claim work and run it through `handler` until `shutdown` resolves, then
/// drain in-flight jobs within one absolute deadline. Shutdown releases for
/// unstarted claims consume that same budget before active handlers drain.
///
/// Claiming is per workload pool and bounded: each tick claims at most
/// `min(free pool capacity, config.max_batch)` jobs for a pool, in one
/// statement, and never loops for more. A saturated pool claims nothing while
/// its siblings keep draining. Shutdown is observed between the claims of a
/// batch and between batches: a partially dispatched batch is abandoned at
/// once and its unstarted claims are released back to the queue (queued and
/// immediately claimable, attempt refunded) while the shutdown budget remains;
/// any release that times out is left for lease recovery.
pub async fn run_claim_loop<T, S>(
    transport: T,
    store: S,
    owner: String,
    config: ClaimLoopConfig,
    handler: JobHandler,
    shutdown: impl Future<Output = ()> + Send,
) -> anyhow::Result<()>
where
    T: WorkTransport + Send + Sync,
    S: DurableExecutionStore + Clone + Send + Sync + 'static,
{
    // One semaphore per served pool: its permits are the pool's in-flight
    // handlers, so free capacity is `available_permits` and a finished (or
    // panicked) task returns its capacity by dropping its permit.
    let semaphores: HashMap<WorkloadClass, Arc<Semaphore>> = config
        .pools
        .iter()
        .map(|pool| {
            (
                *pool,
                Arc::new(Semaphore::new(
                    config.pool_concurrency.get(pool).copied().unwrap_or(1),
                )),
            )
        })
        .collect();
    let mut active: JoinSet<anyhow::Result<()>> = JoinSet::new();
    let mut shutdown = Box::pin(shutdown);
    let mut loop_error = None;
    let mut idle = CLAIM_TICK;
    let mut shutdown_deadline = None;
    // Persistent claim deadline: completion events pull it in, they never
    // restart it, so steady handler churn cannot starve new claims.
    let mut next_claim = tokio::time::Instant::now();
    // Shutdown already observed inside a tick body; the shutdown future has
    // resolved and must not be polled again.
    let mut draining = false;

    while !draining {
        tokio::select! {
            _ = &mut shutdown => {
                establish_shutdown_deadline(&mut shutdown_deadline, config.drain_deadline);
                tracing::info!(active=active.len(), "worker drain started");
                break;
            }
            Some(joined) = active.join_next(), if !active.is_empty() => {
                match joined {
                    Ok(Ok(())) => {
                        // Capacity was just returned to a pool; claim promptly.
                        idle = CLAIM_TICK;
                        next_claim = next_claim.min(tokio::time::Instant::now() + CLAIM_TICK);
                    }
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
            _ = tokio::time::sleep_until(next_claim) => {
                // Crash recovery before selecting new work: a crashed worker's
                // jobs requeue with backoff (or dead-letter at the attempt
                // budget) instead of waiting for the reconciler.
                if let Err(error) = store.recover_expired().await {
                    loop_error = Some(error);
                    break;
                }
                let mut claimed_any = false;
                for pool in &config.pools {
                    if shutdown_peek(&mut shutdown).await {
                        establish_shutdown_deadline(&mut shutdown_deadline, config.drain_deadline);
                        tracing::info!(active=active.len(), "worker drain started");
                        draining = true;
                        break;
                    }
                    let semaphore = &semaphores[pool];
                    let free = semaphore.available_permits();
                    let capacity = config.pool_concurrency.get(pool).copied().unwrap_or(1);
                    let in_flight = capacity.saturating_sub(free);
                    let saturation = (capacity > 0)
                        .then(|| in_flight as f64 / capacity as f64);
                    tracing::debug!(
                        pool = pool.as_str(),
                        in_flight,
                        capacity,
                        saturation = ?saturation,
                        "pool capacity tick"
                    );
                    if free == 0 {
                        continue;
                    }
                    let batch = free.min(config.max_batch);
                    let claimed_at = Instant::now();
                    let jobs = transport
                        .next_work(WorkRequest {
                            owner: &owner,
                            lease: config.claim_lease,
                            pool: *pool,
                            max_jobs: batch,
                        })
                        .await;
                    let jobs = match jobs {
                        Ok(jobs) => jobs,
                        Err(error) => {
                            loop_error = Some(error);
                            draining = true;
                            break;
                        }
                    };
                    let claimed_total = config
                        .pool_concurrency
                        .get(pool)
                        .copied()
                        .unwrap_or(free);
                    tracing::debug!(
                        pool = pool.as_str(),
                        claimed = jobs.len(),
                        batch_cap = batch,
                        free_capacity = free,
                        claim_latency_ms = claimed_at.elapsed().as_millis() as u64,
                        in_flight,
                        capacity = claimed_total,
                        "pool claim batch"
                    );
                    claimed_any = claimed_any || !jobs.is_empty();
                    for (index, job) in jobs.iter().enumerate() {
                        if shutdown_peek(&mut shutdown).await {
                            // Mid-batch shutdown: release this claim and every
                            // claim after it. None of them started, so no
                            // business attempt is spent and no lease is left
                            // behind to expire.
                            let deadline = establish_shutdown_deadline(
                                &mut shutdown_deadline,
                                config.drain_deadline,
                            );
                            let released =
                                release_unstarted(&store, &owner, &jobs[index..], deadline).await;
                            tracing::info!(
                                released,
                                active = active.len(),
                                "shutdown during batch claim; unstarted claims released"
                            );
                            draining = true;
                            break;
                        }
                        let permit = match semaphore.clone().try_acquire_owned() {
                            Ok(permit) => permit,
                            Err(error) => {
                                // Free capacity was accounted before the
                                // claim, so this cannot happen; if it ever
                                // does, the claim must not be lost silently.
                                let deadline = establish_shutdown_deadline(
                                    &mut shutdown_deadline,
                                    config.drain_deadline,
                                );
                                let released = release_unstarted(
                                    &store,
                                    &owner,
                                    &jobs[index..],
                                    deadline,
                                )
                                .await;
                                loop_error = Some(anyhow::anyhow!(
                                    "pool capacity vanished after a batch claim ({error}); released {released} unstarted claims"
                                ));
                                draining = true;
                                break;
                            }
                        };
                        let store = store.clone();
                        let job = job.clone();
                        let lease = job.lease(owner.clone());
                        let handler = Arc::clone(&handler);
                        let duration = config.claim_lease;
                        // Every line logged for this job, including inside its
                        // handler task, carries the job and the tenant scope it
                        // serves.
                        let span = tracing::info_span!(
                            "job",
                            job_id = %job.id,
                            kind = %job.kind,
                            project_id = %job.project_id().map(|id| id.as_uuid().to_string()).unwrap_or_default(),
                            scope = %job.scope,
                            attempt = job.attempts,
                        );
                        active.spawn(
                            async move {
                                let _permit = permit;
                                process_claimed_job(store, job, lease, duration, handler).await
                            }
                            .instrument(span),
                        );
                    }
                }
                if draining {
                    break;
                }
                idle = if claimed_any {
                    CLAIM_TICK
                } else {
                    // Nothing was claimable: back off instead of polling hot.
                    (idle * 2).min(CLAIM_IDLE_MAX)
                };
                next_claim = tokio::time::Instant::now() + idle;
            }
        }
    }

    let deadline = establish_shutdown_deadline(&mut shutdown_deadline, config.drain_deadline);
    let active_drained = tokio::time::timeout_at(deadline, async {
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

fn establish_shutdown_deadline(
    deadline: &mut Option<tokio::time::Instant>,
    budget: Duration,
) -> tokio::time::Instant {
    *deadline.get_or_insert_with(|| tokio::time::Instant::now() + budget)
}

/// Observe shutdown once, without consuming the future: the caller stops
/// claiming and drains when it has resolved.
async fn shutdown_peek(shutdown: &mut Pin<Box<impl Future<Output = ()> + Send>>) -> bool {
    tokio::select! {
        biased;
        _ = &mut *shutdown => true,
        _ = std::future::ready(()) => false,
    }
}

/// Release claims that were accepted in a batch but never started: each row
/// returns to `queued`, immediately claimable, with its attempt refunded.
/// Failures are logged and the remaining claims are still released.
async fn release_unstarted<S: DurableExecutionStore>(
    store: &S,
    owner: &str,
    jobs: &[ClaimedJob],
    deadline: tokio::time::Instant,
) -> usize {
    let mut released = 0;
    for (index, job) in jobs.iter().enumerate() {
        match tokio::time::timeout_at(
            deadline,
            store.release_until(&job.lease(owner), deadline.into_std()),
        )
        .await
        {
            Err(_) => {
                tracing::warn!(
                    remaining = jobs.len() - index,
                    "shutdown release deadline reached; remaining claims will be left for lease recovery"
                );
                break;
            }
            Ok(Ok(true)) => released += 1,
            Ok(Ok(false)) => tracing::warn!(
                job_id = %job.id,
                kind = %job.kind,
                "unstarted claim could not be released; its lease will be recovered"
            ),
            Ok(Err(error)) => tracing::error!(
                job_id = %job.id,
                kind = %job.kind,
                %error,
                "unstarted claim could not be released; its lease will be recovered"
            ),
        }
    }
    released
}

async fn process_claimed_job<S>(
    store: S,
    job: ClaimedJob,
    lease: JobLease,
    duration: Duration,
    handler: JobHandler,
) -> anyhow::Result<()>
where
    S: DurableExecutionStore + Clone + Send + Sync,
{
    let result = {
        let renew_store = store.clone();
        let renew_lease = lease.clone();
        let handler_job = job.clone();
        let handler_lease = lease.clone();
        run_with_lease_renewal(
            duration,
            move || {
                let store = renew_store.clone();
                let lease = renew_lease.clone();
                async move { store.renew(&lease, duration).await }
            },
            run_isolated(async move { handler(handler_job, handler_lease).await }),
        )
        .await
    };
    match result {
        Ok(DeliveryAction::Ack) => {
            if !store.complete(&lease).await? {
                tracing::warn!(job_id=%job.id, kind=%job.kind, attempts=job.attempts, "job completion rejected because the lease is no longer owned");
            }
        }
        Ok(DeliveryAction::Nak(delay)) => {
            record_failure(
                &store,
                &job,
                &lease,
                "retry requested",
                JobRetry::After(delay),
            )
            .await?;
        }
        Ok(DeliveryAction::Terminate) => {
            record_failure(
                &store,
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
                &store,
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
/// retried, `error!` when this attempt dead-letters it. The retry delay is
/// persisted as the job's due time; the claim loop picks it up when due.
async fn record_failure<S>(
    store: &S,
    job: &ClaimedJob,
    lease: &JobLease,
    error: &str,
    retry: JobRetry,
) -> anyhow::Result<()>
where
    S: DurableExecutionStore + Send + Sync,
{
    let persisted = store.fail(lease, error, retry).await?;
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

    #[tokio::test]
    #[allow(clippy::panic_in_result_fn)]
    async fn lease_control_survives_max_pool_one_long_business_transaction() -> anyhow::Result<()> {
        let Some(business) =
            deepref_app::test_isolation::isolated_pool_from_env("DATABASE_URL", 1).await
        else {
            return Ok(());
        };
        let lease_duration = Duration::from_secs(1);
        let job_id = uuid::Uuid::new_v4();
        deepref_app::enqueue_job_pool(
            &business,
            &deepref_app::scoped_job(
                job_id,
                deepref_application::jobs::JobScope::Platform,
                deepref_domain::JobKind::AccountDataExport,
                serde_json::json!({"export_id":uuid::Uuid::new_v4()}),
                format!("lease-pool:{job_id}"),
            ),
        )
        .await?;
        let claimed = deepref_app::claim_job(&business, "pool-reservation-test", lease_duration)
            .await?
            .ok_or_else(|| anyhow::anyhow!("expected claimed export job"))?;
        let lease = claimed.lease("pool-reservation-test");
        let control = lease_control_pool(&business);
        let control_store = PostgresDurableExecutionStore {
            pool: control.clone(),
        };
        let renewals = Arc::new(AtomicU32::new(0));
        let counter = renewals.clone();
        let work_pool = business.clone();
        let result = run_with_lease_renewal(
            lease_duration,
            move || {
                let store = control_store.clone();
                let lease = lease.clone();
                let counter = counter.clone();
                async move {
                    let renewed = store.renew(&lease, lease_duration).await?;
                    if renewed {
                        counter.fetch_add(1, Ordering::SeqCst);
                    }
                    Ok(renewed)
                }
            },
            run_isolated(async move {
                let mut tx = work_pool.begin().await?;
                sqlx::query("SELECT 1").execute(&mut *tx).await?;
                tokio::time::sleep(Duration::from_millis(2400)).await;
                tx.rollback().await?;
                Ok(DeliveryAction::Ack)
            }),
        )
        .await;
        assert!(
            matches!(result, Ok(DeliveryAction::Ack)),
            "long held upload connection must not consume lease renewal capacity: {result:?}"
        );
        assert!(
            renewals.load(Ordering::SeqCst) >= 2,
            "real worker renewals must progress beyond the initial lease"
        );
        let still_live: bool =
            sqlx::query_scalar("SELECT leased_until>clock_timestamp() FROM jobs WHERE id=$1")
                .bind(job_id)
                .fetch_one(&business)
                .await?;
        assert!(still_live);
        control.close().await;
        Ok(())
    }

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
            scope: deepref_application::jobs::JobScope::Project(deepref_domain::ProjectId::new(
                uuid::Uuid::new_v4(),
            )),
            kind: deepref_domain::JobKind::RecomputeMetrics,
            payload: serde_json::json!({}),
            claim_seq: 0,
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
