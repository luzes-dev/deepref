use std::{future::Future, sync::Arc, time::Duration};

use deepref_ai::{AiGateway, RoutedGateway};
use deepref_application::jobs::ClaimedJob;
use deepref_documents::DocumentStore;
use deepref_postgres::{claim_job, complete_job, fail_job, renew_job};
use sqlx::postgres::PgPoolOptions;
use tokio::{
    sync::{Semaphore, watch},
    task::JoinSet,
};

pub mod assistant;
pub mod config;
pub mod delivery;
pub mod enrichment;
pub mod limiter;
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
async fn build_ai_gateway(pool: &sqlx::PgPool) -> anyhow::Result<Arc<dyn AiGateway>> {
    let ai_config = deepref_config::AiProviderConfig::from_env()?;
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
    let routes = deepref_postgres::ensure_default_model_routes(
        pool,
        provider.id(),
        &ai_config.default_model,
    )
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
        Arc::new(deepref_postgres::PostgresUsageLedger::new(pool)),
        prices,
    )?;
    Ok(gateway)
}

pub async fn run(config: config::WorkerConfig) -> anyhow::Result<()> {
    run_with_shutdown(config, shutdown::wait_for_signal()).await
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
        .connect(&database.url)
        .await?;
    let owner = format!("deepref-worker-{}", uuid::Uuid::new_v4());
    processor::validate_pdf_parse_concurrency()?;
    let document_store = Arc::new(DocumentStore::from_env()?);
    let ai_gateway = build_ai_gateway(&pool).await?;
    // Workflow blocks that change review state follow the project's AI
    // autonomy settings.
    workflows::set_autonomy_gate(Arc::new(deepref_postgres::ProjectAutonomyGate::new(&pool)));
    let semaphore = Arc::new(Semaphore::new(config.concurrency));
    let mut active = JoinSet::new();
    let (reconciler_shutdown, reconciler_signal) = watch::channel(false);
    tokio::spawn(workflows::run_scheduler(
        pool.clone(),
        Duration::from_secs(20),
        reconciler_signal.clone(),
    ));
    let reconciler_task = tokio::spawn(reconciler::run(
        pool.clone(),
        config.reconciler_interval,
        reconciler_signal,
    ));
    let mut shutdown_signal = Box::pin(shutdown_signal);
    let mut loop_error = None;
    let mut claim_tick = tokio::time::interval(Duration::from_millis(100));
    claim_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

    loop {
        tokio::select! {
            _ = &mut shutdown_signal => {
                tracing::info!(active=active.len(), "worker drain started");
                reconciler_shutdown.send_replace(true);
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
                        loop_error = Some(error.into());
                        break;
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
                        let owner = owner.clone();
                        let document_store = Arc::clone(&document_store);
                        let ai_gateway = Arc::clone(&ai_gateway);
                        let lease = config.claim_lease;
                        active.spawn(async move {
                            let _permit = permit;
                            process_claimed_job(
                                pool,
                                owner,
                                job,
                                lease,
                                document_store,
                                ai_gateway,
                            )
                            .await
                        });
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

    reconciler_shutdown.send_replace(true);
    let active_drained = tokio::time::timeout(config.runtime.telemetry.shutdown_deadline, async {
        while let Some(result) = active.join_next().await {
            result??;
        }
        Ok::<(), anyhow::Error>(())
    })
    .await;
    let reconciler_drained =
        tokio::time::timeout(config.runtime.telemetry.shutdown_deadline, reconciler_task).await;
    match active_drained {
        Ok(result) => result?,
        Err(_) => anyhow::bail!(
            "worker drain deadline exceeded with {} active tasks",
            active.len()
        ),
    }
    match reconciler_drained {
        Ok(result) => result?,
        Err(_) => anyhow::bail!("worker reconciler drain deadline exceeded"),
    }
    if let Some(error) = loop_error {
        return Err(error);
    }
    Ok(())
}

async fn process_claimed_job(
    pool: sqlx::PgPool,
    owner: String,
    job: ClaimedJob,
    lease: Duration,
    document_store: Arc<DocumentStore>,
    ai_gateway: Arc<dyn AiGateway>,
) -> anyhow::Result<()> {
    let result = run_with_lease_renewal(
        pool.clone(),
        owner.clone(),
        job.id,
        lease,
        processor::handle_job_with_documents_owned_and_ai(
            pool.clone(),
            &job,
            &owner,
            lease,
            Some(document_store),
            None,
            ai_gateway,
        ),
    )
    .await;
    match result {
        Ok(delivery::DeliveryAction::Ack) => {
            if !complete_job(&pool, &owner, job.id).await? {
                tracing::warn!(job_id=%job.id, "job completion rejected because the lease is no longer owned");
            }
        }
        Ok(delivery::DeliveryAction::Nak(delay)) => {
            if !fail_job(&pool, &owner, &job, "retry requested", delay).await? {
                tracing::warn!(job_id=%job.id, "job retry update rejected because the lease is no longer owned");
            }
        }
        Ok(delivery::DeliveryAction::Terminate) => {
            let job_id = job.id;
            let terminal = ClaimedJob {
                attempts: job.max_attempts,
                ..job
            };
            if !fail_job(
                &pool,
                &owner,
                &terminal,
                "terminal job failure",
                Duration::ZERO,
            )
            .await?
            {
                tracing::warn!(job_id=%job_id, "terminal job update rejected because the lease is no longer owned");
            }
        }
        Err(error) => {
            // A job failure is durable work state, not a worker-process failure. The
            // queue update decides whether this attempt is retried or dead-lettered;
            // only an error while persisting that decision should stop the worker.
            if !fail_job(
                &pool,
                &owner,
                &job,
                &error.to_string(),
                Duration::from_secs(10),
            )
            .await?
            {
                tracing::warn!(job_id=%job.id, "job failure update rejected because the lease is no longer owned");
            }
        }
    }
    Ok(())
}

async fn run_with_lease_renewal<F>(
    pool: sqlx::PgPool,
    owner: String,
    job_id: uuid::Uuid,
    lease: Duration,
    work: F,
) -> anyhow::Result<delivery::DeliveryAction>
where
    F: Future<Output = anyhow::Result<delivery::DeliveryAction>>,
{
    let interval = (lease / 3).max(Duration::from_millis(100));
    tokio::pin!(work);
    let mut renewal = tokio::time::interval(interval);
    renewal.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    renewal.tick().await;
    loop {
        tokio::select! {
            result = &mut work => return result,
            _ = renewal.tick() => {
                if !renew_job(&pool, &owner, job_id, lease).await? {
                    anyhow::bail!("job lease was lost before completion");
                }
            }
        }
    }
}
