//! JetStream wakeups resolved through the existing authoritative job lifecycle.
use std::{collections::HashMap, sync::Arc, time::Duration};

use anyhow::{Context, Result};
use deepref_app::{
    PostgresClaimTransport,
    work_publication::{self, WorkResolution},
};
use deepref_application::{
    jobs::{
        ClaimedJob, DurableExecutionStore, IntakeControl, JobLease, WorkRequest, WorkTransport,
    },
    work_delivery::EventEnvelope,
};
use deepref_config::NatsConfig;
use deepref_domain::{
    WorkloadClass,
    events::{DELIVERY_BACKOFF_SECONDS, MAX_DELIVERIES},
};
use deepref_integrations::jetstream::{
    ClaimOutcome, Consumer, Delivery, JetStream, TransportObservation,
};
use sqlx::PgPool;
use tokio::sync::{Mutex, watch};
use tracing::Instrument;

pub enum RuntimeTransport {
    Postgres(PostgresClaimTransport),
    JetStream(Box<JetStreamWorkTransport>),
}
impl WorkTransport for RuntimeTransport {
    fn recover_before_poll(&self) -> bool {
        matches!(self, Self::Postgres(_))
    }
    async fn next_work(&self, request: WorkRequest<'_>) -> Result<Vec<ClaimedJob>> {
        match self {
            Self::Postgres(t) => t.next_work(request).await,
            Self::JetStream(t) => t.next_work(request).await,
        }
    }
    async fn next_work_controlled(
        &self,
        request: WorkRequest<'_>,
        control: &IntakeControl,
        budget: Duration,
    ) -> Result<Vec<ClaimedJob>> {
        match self {
            Self::Postgres(t) => t.next_work_controlled(request, control, budget).await,
            Self::JetStream(t) => t.next_work_controlled(request, control, budget).await,
        }
    }
    async fn settled(&self, lease: &JobLease) -> Result<()> {
        match self {
            Self::Postgres(t) => t.settled(lease).await,
            Self::JetStream(t) => t.settled(lease).await,
        }
    }
}

pub struct JetStreamWorkTransport {
    pool: PgPool,
    broker: JetStream,
    consumers: HashMap<WorkloadClass, Consumer>,
    active: Mutex<HashMap<(uuid::Uuid, i64), (EventEnvelope, Delivery)>>,
}
impl JetStreamWorkTransport {
    pub async fn new(pool: PgPool, broker: JetStream, pools: &[WorkloadClass]) -> Result<Self> {
        let mut consumers = HashMap::new();
        for class in pools {
            consumers.insert(
                *class,
                broker
                    .consumer(&format!("work-{}-v1", class.as_str()), Some(*class))
                    .await?,
            );
        }
        Ok(Self {
            pool,
            broker,
            consumers,
            active: Mutex::new(HashMap::new()),
        })
    }
}
impl WorkTransport for JetStreamWorkTransport {
    fn recover_before_poll(&self) -> bool {
        false
    }
    async fn next_work(&self, request: WorkRequest<'_>) -> Result<Vec<ClaimedJob>> {
        self.next_work_controlled(request, &IntakeControl::default(), Duration::from_secs(1))
            .await
    }
    async fn next_work_controlled(
        &self,
        request: WorkRequest<'_>,
        control: &IntakeControl,
        budget: Duration,
    ) -> Result<Vec<ClaimedJob>> {
        let consumer = self
            .consumers
            .get(&request.pool)
            .context("unconfigured workload consumer")?;
        let deadline = tokio::time::Instant::now() + budget;
        let messages =
            match intake_boundary(control, deadline, consumer.fetch(request.max_jobs)).await {
                Ok(messages) => messages,
                Err(_) => return Ok(Vec::new()),
            };
        let mut jobs = Vec::new();
        for message in messages {
            if control.is_stopped() || tokio::time::Instant::now() >= deadline {
                break;
            }
            let envelope = match message.envelope() {
                Ok(envelope) => envelope,
                Err(error) => {
                    // This branch owns no DB claim. Cancellation drops only the
                    // malformed broker delivery, never an earlier accepted job.
                    let _ = intake_boundary(
                        control,
                        deadline,
                        reject(
                            &self.pool,
                            &self.broker,
                            "durable-work",
                            message,
                            None,
                            &error.to_string(),
                        ),
                    )
                    .await;
                    continue;
                }
            };
            self.broker.observe(TransportObservation::ConsumeLatency {
                elapsed: (chrono::Utc::now() - envelope.occurred_at)
                    .to_std()
                    .unwrap_or_default(),
            });
            let claimed_at = std::time::Instant::now();
            let mut prepared = None;
            let resolution = intake_boundary(
                control,
                deadline,
                work_publication::resolve_work_observed(
                    &self.pool,
                    &envelope,
                    request.owner,
                    request.lease,
                    request.pool,
                    |job| prepared = Some(job.clone()),
                ),
            )
            .await;
            self.broker.observe(TransportObservation::Claim {
                pool: request.pool,
                elapsed: claimed_at.elapsed(),
                outcome: match &resolution {
                    Ok(WorkResolution::Claimed(_)) => ClaimOutcome::Claimed,
                    Ok(WorkResolution::Ack | WorkResolution::Terminate) => ClaimOutcome::Noop,
                    Ok(WorkResolution::Retry) => ClaimOutcome::Retry,
                    Err(_) => ClaimOutcome::Failed,
                },
            });
            match resolution {
                Ok(WorkResolution::Claimed(job)) => {
                    self.active
                        .lock()
                        .await
                        .insert((job.id, job.claim_seq), (envelope, message));
                    jobs.push(job);
                }
                Ok(WorkResolution::Ack) => {
                    let _ = intake_boundary(control, deadline, message.ack()).await;
                }
                Ok(WorkResolution::Terminate) => {
                    let _ = intake_boundary(control, deadline, message.terminate()).await;
                }
                Ok(WorkResolution::Retry) => {
                    let _ = intake_boundary(
                        control,
                        deadline,
                        retry_or_reject(
                            &self.pool,
                            &self.broker,
                            "durable-work",
                            message,
                            Some(&envelope),
                            "work_not_claimable",
                        ),
                    )
                    .await;
                }
                Err(_) => {
                    // A COMMIT response can be ambiguous. The prepared token is
                    // retained until exact unstarted release has been attempted;
                    // a failed release remains owned by its bounded DB lease.
                    if let Some(job) = prepared {
                        let store = deepref_app::PostgresDurableExecutionStore {
                            pool: self.pool.clone(),
                        };
                        let fallback =
                            tokio::time::Instant::now() + budget.min(Duration::from_millis(50));
                        let release_deadline = control
                            .shutdown_deadline()
                            .map(tokio::time::Instant::from_std)
                            .unwrap_or(fallback)
                            .min(fallback);
                        let result = tokio::time::timeout_at(
                            release_deadline,
                            store.release_until(
                                &job.lease(request.owner),
                                release_deadline.into_std(),
                            ),
                        )
                        .await;
                        if !matches!(result, Ok(Ok(true))) {
                            tracing::warn!(job_id=%job.id,claim_seq=job.claim_seq,"unstarted claim release unconfirmed; any committed fence retains DB lease recovery");
                        }
                    }
                    // Closed reason code: database decoder errors may include
                    // private values too. Never persist their Display output.
                    let _ = intake_boundary(
                        control,
                        deadline,
                        retry_or_reject(
                            &self.pool,
                            &self.broker,
                            "durable-work",
                            message,
                            Some(&envelope),
                            "work_resolution_failed",
                        ),
                    )
                    .await;
                }
            }
        }
        if let Ok((pending, in_flight, redeliveries)) =
            intake_boundary(control, deadline, consumer.visibility()).await
        {
            self.broker.observe(TransportObservation::Visibility {
                pool: request.pool,
                pending,
                in_flight,
                redeliveries,
            });
            tracing::info!(pool=%request.pool,pending,in_flight,redeliveries,"JetStream pool visibility");
        }
        Ok(jobs)
    }
    async fn settled(&self, lease: &JobLease) -> Result<()> {
        let Some((envelope, delivery)) = self
            .active
            .lock()
            .await
            .remove(&(lease.id, lease.claim_seq))
        else {
            return Ok(());
        };
        // The DB lifecycle trigger commits the receipt with completion/retry.
        // No ACK if completion failed or a running fence still owns this work.
        if work_publication::event_delivery_state(&self.pool, envelope.event_id)
            .await?
            .as_deref()
            == Some("delivered")
        {
            delivery.ack().await?;
        }
        Ok(())
    }
}

/// Cancellation is scoped to a single boundary, not the whole batch. The
/// caller retains its partial result and the prepared-claim callback above.
async fn intake_boundary<T>(
    control: &IntakeControl,
    deadline: tokio::time::Instant,
    operation: impl std::future::Future<Output = Result<T>>,
) -> Result<T> {
    tokio::select! {
        biased;
        _=control.cancelled()=>anyhow::bail!("intake_stopped"),
        result=tokio::time::timeout_at(deadline,operation)=>result.context("intake_deadline")?,
    }
}

async fn retry_or_reject(
    pool: &PgPool,
    broker: &JetStream,
    consumer: &str,
    delivery: Delivery,
    envelope: Option<&EventEnvelope>,
    reason: &str,
) -> Result<()> {
    let count = delivery.deliveries()?;
    if count >= MAX_DELIVERIES {
        reject(pool, broker, consumer, delivery, envelope, reason).await
    } else {
        delivery
            .retry(Duration::from_secs(
                DELIVERY_BACKOFF_SECONDS
                    [(count.saturating_sub(1) as usize).min(DELIVERY_BACKOFF_SECONDS.len() - 1)],
            ))
            .await
    }
}

async fn reject(
    pool: &PgPool,
    broker: &JetStream,
    consumer: &str,
    delivery: Delivery,
    envelope: Option<&EventEnvelope>,
    reason: &str,
) -> Result<()> {
    let identity = delivery.identity()?;
    work_publication::record_dead_letter(
        pool,
        &identity,
        envelope,
        consumer,
        delivery.deliveries()?,
        reason,
    )
    .await?;
    broker.dead_letter(consumer, &delivery, reason).await?;
    delivery.terminate().await
}

/// Explicit operator maintenance for an exhausted retained sequence. This is
/// diagnostic recovery only; pending business events require separate reviewed
/// publication replay. A failed step is safe to rerun with the same identity.
pub async fn recover_terminal_sequence(
    pool: &PgPool,
    broker: &JetStream,
    consumer: &str,
    sequence: u64,
) -> Result<()> {
    let envelope = broker.retained_sequence(sequence).await?;
    let identity = broker.sequence_identity(consumer, sequence);
    work_publication::record_dead_letter(
        pool,
        &identity,
        envelope.as_ref(),
        consumer,
        MAX_DELIVERIES,
        "terminal_sequence_recovered",
    )
    .await?;
    // Preserve the already-committed logical consumer/reason if normal
    // terminalization reached DB evidence before its broker publish failed.
    let (logical_consumer, reason, deliveries): (String, String, i64) = sqlx::query_as(
        "SELECT consumer,reason,deliveries FROM transport_dead_letters WHERE delivery_id=$1",
    )
    .bind(&identity)
    .fetch_one(pool)
    .await?;
    broker
        .dead_letter_sequence(
            &logical_consumer,
            sequence,
            u64::try_from(deliveries)?,
            &reason,
        )
        .await
}

/// Publication runtime settings, already validated at the config boundary.
pub struct PublicationConfig {
    pub nats: NatsConfig,
    pub claim_lease: Duration,
    pub drain_deadline: Duration,
}

/// Relay, due scheduling and generic typed-consumer shell. Work delivery is
/// pulled by run_claim_loop and remains bounded by each pool's free capacity.
pub async fn run_publication(
    pool: PgPool,
    broker: JetStream,
    registry: Arc<deepref_application::outbox::OutboxRegistry>,
    owner: String,
    settings: PublicationConfig,
    mut shutdown: watch::Receiver<bool>,
) -> Result<crate::outbox_dispatch::OutboxDispatchReport> {
    let PublicationConfig {
        nats: config,
        claim_lease: lease,
        drain_deadline,
    } = settings;
    let consumer = broker.consumer("outbox-v1", None).await?;
    let mut tick = tokio::time::interval(Duration::from_millis(250));
    let mut repair = tokio::time::interval(Duration::from_secs(30));
    let mut active = tokio::task::JoinSet::new();
    loop {
        tokio::select! {
            biased;
            _ = shutdown.changed() => break,
            Some(joined) = active.join_next(), if !active.is_empty() => {
                match joined {
                    Ok(Ok(())) => {},
                    Ok(Err(error)) => tracing::warn!(%error,"generic delivery failed; awaiting redelivery"),
                    Err(error) => tracing::error!(%error,"generic consumer task failed; awaiting redelivery"),
                }
            }
            _ = repair.tick() => {
                let ready=tokio::select! {
                    _=shutdown.changed()=>break,
                    result=tokio::time::timeout(config.request_timeout,broker.readiness())=>result,
                };
                if !matches!(ready,Ok(Ok(()))) {broker.observe(TransportObservation::Readiness{ready:false});}
                let repaired=tokio::select! {
                    _=shutdown.changed()=>break,
                    result=tokio::time::timeout(config.request_timeout,work_publication::repair_stranded_work(&pool,config.repair_after,64))=>result,
                };
                match repaired {
                    Ok(Ok(republished)) => tracing::info!(republished,"transport reconciliation repair"),
                    Ok(Err(error)) => tracing::warn!(%error,"transport repair failed"),
                    Err(_) => tracing::warn!("transport repair deadline exceeded"),
                }

            }
            _ = tick.tick() => {
                let batch = tokio::select! {
                    _ = shutdown.changed() => break,
                    batch = publication_tick(&pool,&broker,&owner,lease.max(config.request_timeout * 32)) => batch,
                };
                if let Err(error) = batch { tracing::warn!(%error,"publication failed; DB retains pending work"); }
                let capacity = 8_usize.saturating_sub(active.len());
                if capacity==0 {continue;}
                let messages = tokio::select! {
                    _ = shutdown.changed() => break,
                    messages = consumer.fetch(capacity) => messages,
                };
                let messages = match messages { Ok(messages)=>messages, Err(error)=>{ tracing::warn!(%error,"generic consumer unavailable"); continue; } };
                for message in messages {
                    let pool=pool.clone(); let broker=broker.clone();
                    let registry=Arc::clone(&registry); let owner=owner.clone();
                    active.spawn(async move {consume_event(&pool,&broker,&registry,&owner,lease,message).await});
                }
            }
        }
    }
    // Stop intake first; healthy generic handlers retain both progress and DB
    // lease renewal while draining. Timeout aborts and awaits owned tasks.
    if tokio::time::timeout(drain_deadline, async {
        while let Some(result) = active.join_next().await {
            match result {
                Ok(Err(error)) => tracing::warn!(%error,"generic delivery failed while draining"),
                Err(error) => tracing::warn!(%error,"generic consumer task stopped while draining"),
                Ok(Ok(())) => {}
            }
        }
    })
    .await
    .is_err()
    {
        active.abort_all();
        while active.join_next().await.is_some() {}
        anyhow::bail!("generic transport consumer drain deadline exceeded");
    }
    Ok(Default::default())
}

pub async fn publication_tick(
    pool: &PgPool,
    broker: &JetStream,
    owner: &str,
    lease: Duration,
) -> Result<usize> {
    work_publication::publish_due_work(pool, 64).await?;
    let events = work_publication::claim_publications(pool, owner, lease, 16).await?;
    let count = events.len();
    for event in events {
        let published = broker.publish(&event.envelope).await;
        let error = published.err().map(|error| error.to_string());
        work_publication::settle_publication(pool, &event, error.as_deref()).await?;
    }
    Ok(count)
}

pub async fn consume_event(
    pool: &PgPool,
    broker: &JetStream,
    registry: &deepref_application::outbox::OutboxRegistry,
    owner: &str,
    lease: Duration,
    delivery: Delivery,
) -> Result<()> {
    let envelope = match delivery.envelope() {
        Ok(e) => e,
        Err(error) => {
            return reject(pool, broker, "outbox", delivery, None, &error.to_string()).await;
        }
    };
    broker.observe(TransportObservation::ConsumeLatency {
        elapsed: (chrono::Utc::now() - envelope.occurred_at)
            .to_std()
            .unwrap_or_default(),
    });
    let span = tracing::info_span!("outbox_delivery",event_id=%envelope.event_id,trace_id=?envelope.trace_id,message_type=%envelope.message_type,message_version=envelope.message_version);
    consume_valid_event(pool, broker, registry, owner, lease, delivery, envelope)
        .instrument(span)
        .await
}

async fn consume_valid_event(
    pool: &PgPool,
    broker: &JetStream,
    registry: &deepref_application::outbox::OutboxRegistry,
    owner: &str,
    lease: Duration,
    delivery: Delivery,
    envelope: EventEnvelope,
) -> Result<()> {
    if let Some(event) =
        work_publication::claim_event_delivery(pool, registry, &envelope, owner, lease).await?
    {
        deepref_app::outbox::dispatch_outbox_event(pool, registry, lease, &event).await?;
    }
    match work_publication::event_delivery_state(pool, envelope.event_id)
        .await?
        .as_deref()
    {
        Some("delivered") => delivery.ack().await,
        Some("quarantined") => {
            reject(
                pool,
                broker,
                "outbox",
                delivery,
                Some(&envelope),
                "consumer quarantined",
            )
            .await
        }
        _ => {
            retry_or_reject(
                pool,
                broker,
                "outbox",
                delivery,
                Some(&envelope),
                "consumer receipt not committed",
            )
            .await
        }
    }
}
