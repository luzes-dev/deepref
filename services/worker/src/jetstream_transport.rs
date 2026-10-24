//! JetStream wakeups resolved through the existing authoritative job lifecycle.
use std::{collections::HashMap, sync::Arc, time::Duration};

use anyhow::{Context, Result};
use deepref_app::{
    PostgresClaimTransport,
    work_publication::{self, WorkResolution},
};
use deepref_application::{
    jobs::{ClaimedJob, JobLease, WorkRequest, WorkTransport},
    work_delivery::EventEnvelope,
};
use deepref_config::NatsConfig;
use deepref_domain::{
    WorkloadClass,
    events::{DELIVERY_BACKOFF_SECONDS, MAX_DELIVERIES},
};
use deepref_integrations::jetstream::{Consumer, Delivery, JetStream};
use sqlx::PgPool;
use tokio::sync::{Mutex, watch};
use tracing::Instrument;

pub enum RuntimeTransport {
    Postgres(PostgresClaimTransport),
    JetStream(JetStreamWorkTransport),
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
        let consumer = self
            .consumers
            .get(&request.pool)
            .context("unconfigured workload consumer")?;
        let messages = match consumer.fetch(request.max_jobs).await {
            Ok(messages) => messages,
            Err(error) => {
                tracing::warn!(%error,pool=%request.pool,"transport unavailable; DB work retained");
                return Ok(Vec::new());
            }
        };
        let mut jobs = Vec::new();
        for message in messages {
            let envelope = match message.envelope() {
                Ok(envelope) => envelope,
                Err(error) => {
                    reject(
                        &self.pool,
                        &self.broker,
                        "durable-work",
                        message,
                        None,
                        &error.to_string(),
                    )
                    .await?;
                    continue;
                }
            };
            let span = tracing::info_span!("work_delivery",event_id=%envelope.event_id,trace_id=?envelope.trace_id,pool=%request.pool,epoch=envelope.publication_epoch);
            match work_publication::resolve_work(
                &self.pool,
                &envelope,
                request.owner,
                request.lease,
                request.pool,
            )
            .instrument(span)
            .await
            {
                Ok(WorkResolution::Claimed(job)) => {
                    self.active
                        .lock()
                        .await
                        .insert((job.id, job.claim_seq), (envelope, message));
                    jobs.push(job);
                }
                Ok(WorkResolution::Ack) => message.ack().await?,
                Ok(WorkResolution::Terminate) => message.terminate().await?,
                Ok(WorkResolution::Retry) => {
                    retry_or_reject(
                        &self.pool,
                        &self.broker,
                        "durable-work",
                        message,
                        Some(&envelope),
                        "work not claimable by this delivery",
                    )
                    .await?
                }
                Err(error) => {
                    // DB/validation failures consume no business attempt. The
                    // transport budget is separate and repair retains DB work.
                    retry_or_reject(
                        &self.pool,
                        &self.broker,
                        "durable-work",
                        message,
                        Some(&envelope),
                        &format!("delivery resolution failed: {error}"),
                    )
                    .await?;
                }
            }
        }
        if let Ok((pending, in_flight, redeliveries)) = consumer.visibility().await {
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

/// Relay, due scheduling and generic typed-consumer shell. Work delivery is
/// pulled by run_claim_loop and remains bounded by each pool's free capacity.
pub async fn run_publication(
    pool: PgPool,
    broker: JetStream,
    registry: Arc<deepref_application::outbox::OutboxRegistry>,
    owner: String,
    config: NatsConfig,
    lease: Duration,
    drain_deadline: Duration,
    mut shutdown: watch::Receiver<bool>,
) -> Result<crate::outbox_dispatch::OutboxDispatchReport> {
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
                match work_publication::repair_stranded_work(&pool,config.repair_after,64).await {
                    Ok(republished) => tracing::info!(republished,"transport reconciliation repair"),
                    Err(error) => tracing::warn!(%error,"transport repair failed"),
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
