//! Publication/consumer persistence for the existing generic outbox. No NATS SDK.
use std::time::Duration;

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use deepref_application::{
    jobs::ClaimedJob,
    outbox::{OutboxMessage, OutboxRegistry},
    work_delivery::{EventEnvelope, WorkWakeup},
};
use deepref_domain::{JobKind, WorkloadClass};
use sqlx::{PgPool, Row};
use uuid::Uuid;

#[derive(Debug)]
pub struct Publication {
    pub envelope: EventEnvelope,
    pub owner: String,
    pub fence: i64,
    pub attempts: i32,
}

pub async fn claim_publications(
    pool: &PgPool,
    owner: &str,
    lease: Duration,
    limit: usize,
) -> Result<Vec<Publication>> {
    let rows = sqlx::query(
        "WITH candidate AS (
           SELECT event_id FROM outbox_events
           WHERE published_at IS NULL AND delivery_state='pending'
             AND publication_next_at <= now()
             AND (publication_until IS NULL OR publication_until <= now())
           ORDER BY created_at,event_id FOR UPDATE SKIP LOCKED LIMIT $3
         ) UPDATE outbox_events e SET publication_owner=$1,
           publication_until=now()+($2::double precision * interval '1 millisecond'),
           publication_generation=publication_generation+1,publication_attempts=publication_attempts+1
         FROM candidate WHERE e.event_id=candidate.event_id RETURNING e.*")
        .bind(owner).bind(lease.as_millis() as f64).bind(i64::try_from(limit)?).fetch_all(pool).await?;
    rows.iter()
        .map(|row| {
            Ok(Publication {
                envelope: EventEnvelope {
                    envelope_version: 1,
                    event_id: row.try_get("event_id")?,
                    publication_epoch: row.try_get("publication_epoch")?,
                    message_type: row.try_get("message_type")?,
                    message_version: u32::try_from(row.try_get::<i32, _>("message_version")?)?,
                    occurred_at: row.try_get("created_at")?,
                    trace_id: row
                        .try_get::<serde_json::Value, _>("payload")?
                        .get("trace_id")
                        .and_then(|v| v.as_str())
                        .map(str::to_owned),
                    payload: row.try_get("payload")?,
                },
                owner: owner.to_owned(),
                fence: row.try_get("publication_generation")?,
                attempts: row.try_get("publication_attempts")?,
            })
        })
        .collect()
}

pub async fn settle_publication(
    pool: &PgPool,
    publication: &Publication,
    error: Option<&str>,
) -> Result<bool> {
    let delay = deepref_application::outbox::retry_backoff(publication.attempts);
    Ok(sqlx::query("UPDATE outbox_events SET published_at=CASE WHEN $4::text IS NULL THEN now() ELSE published_at END,
        publication_error=$4, publication_next_at=now()+($5::double precision * interval '1 millisecond'),
        publication_owner=NULL,publication_until=NULL
        WHERE event_id=$1 AND publication_owner=$2 AND publication_generation=$3 AND publication_until>now()")
        .bind(publication.envelope.event_id).bind(&publication.owner).bind(publication.fence).bind(error)
        .bind(delay.as_millis() as f64).execute(pool).await?.rows_affected()==1)
}

/// Deliberate replay with stable business identity and a fresh deduplication
/// epoch. Ordinary relay selection never touches already-published rows.
pub async fn replay_publication(pool: &PgPool, event_id: Uuid) -> Result<bool> {
    Ok(sqlx::query("UPDATE outbox_events SET published_at=NULL,publication_epoch=publication_epoch+1,
        publication_owner=NULL,publication_until=NULL,publication_next_at=now(),publication_error=NULL
        WHERE event_id=$1 AND delivery_state='pending'")
        .bind(event_id).execute(pool).await?.rows_affected()==1)
}

/// Normal due scheduling, separate from slow repair. Updating the emitted
/// marker invokes the same trigger as N-1 immediate enqueues and commits one
/// outbox event per persisted generation. Workers never hot-poll for claims.
pub async fn publish_due_work(pool: &PgPool, limit: usize) -> Result<u64> {
    Ok(sqlx::query("WITH due AS (
        SELECT id FROM jobs WHERE state IN ('queued','capability_wait') AND available_at<=now()
          AND attempts<max_attempts AND wakeup_emitted_generation<wakeup_generation
        ORDER BY available_at,id FOR UPDATE SKIP LOCKED LIMIT $1
      ) UPDATE jobs SET wakeup_emitted_generation=jobs.wakeup_emitted_generation FROM due WHERE jobs.id=due.id")
        .bind(i64::try_from(limit)?).execute(pool).await?.rows_affected())
}

/// Repair only: a published wakeup that failed to acquire authority for long
/// enough is republished explicitly. Never reset attempts or business state.
pub async fn repair_stranded_work(pool: &PgPool, age: Duration, limit: usize) -> Result<u64> {
    Ok(sqlx::query("WITH stranded AS (
        SELECT e.event_id FROM jobs j JOIN outbox_events e
          ON e.event_id=job_wakeup_event_id(j.id,j.wakeup_generation)
        WHERE j.state IN ('queued','capability_wait') AND j.available_at<=now() AND j.attempts<j.max_attempts
          AND e.delivery_state='pending' AND e.published_at < now()-($1::double precision * interval '1 millisecond')
        ORDER BY e.published_at FOR UPDATE OF e SKIP LOCKED LIMIT $2
      ) UPDATE outbox_events e SET published_at=NULL,publication_epoch=publication_epoch+1,
        publication_next_at=now(),publication_error='repair: queued work without timely delivery',
        publication_owner=NULL,publication_until=NULL FROM stranded WHERE e.event_id=stranded.event_id")
        .bind(age.as_millis() as f64).bind(i64::try_from(limit)?).execute(pool).await?.rows_affected())
}

#[derive(Debug)]
pub enum WorkResolution {
    Claimed(ClaimedJob),
    Ack,
    Terminate,
    /// Unsupported JobKind, live lock contention or uncommitted recovery.
    Retry,
}

/// A broker hint grants no authority. Lock the one indicated job, validate
/// persisted generation/pool/kind and perform the same capability/fence claim.
pub async fn resolve_work(
    pool: &PgPool,
    envelope: &EventEnvelope,
    owner: &str,
    lease: Duration,
    class: WorkloadClass,
) -> Result<WorkResolution> {
    let wakeup = envelope.work()?;
    anyhow::ensure!(
        wakeup.workload_class == class,
        "wakeup delivered to wrong workload pool"
    );
    let mut tx = pool.begin().await?;
    let stored = sqlx::query("SELECT payload,message_type,message_version,publication_epoch FROM outbox_events WHERE event_id=$1")
        .bind(envelope.event_id).fetch_optional(&mut *tx).await?.context("unknown outbox event")?;
    anyhow::ensure!(
        stored.try_get::<serde_json::Value, _>("payload")? == envelope.payload
            && stored.try_get::<String, _>("message_type")? == WorkWakeup::TYPE
            && stored.try_get::<i32, _>("message_version")? == WorkWakeup::VERSION as i32,
        "wakeup does not match authoritative outbox"
    );
    let row = sqlx::query("SELECT * FROM jobs WHERE id=$1 FOR UPDATE SKIP LOCKED")
        .bind(wakeup.job_id)
        .fetch_optional(&mut *tx)
        .await?;
    let Some(row) = row else {
        return Ok(WorkResolution::Retry);
    };
    let generation: i64 = row.try_get("wakeup_generation")?;
    let state: String = row.try_get("state")?;
    let available: DateTime<Utc> = row.try_get("available_at")?;
    let current_time: DateTime<Utc> = sqlx::query_scalar("SELECT now()")
        .fetch_one(&mut *tx)
        .await?;
    let result = if generation != wakeup.generation {
        WorkResolution::Terminate
    } else if matches!(state.as_str(), "completed" | "dead" | "failed") {
        if state == "completed" {
            WorkResolution::Ack
        } else {
            WorkResolution::Terminate
        }
    } else if state == "running" {
        let until: Option<DateTime<Utc>> = row.try_get("leased_until")?;
        if until.is_some_and(|until| until > current_time)
            || crate::jobs::recover_expired_job(&mut tx, wakeup.job_id).await?
        {
            WorkResolution::Ack
        } else {
            WorkResolution::Retry
        }
    } else if available > current_time {
        // An early notification must leave a durable scheduling obligation,
        // even if it already carried the current generation.
        sqlx::query("UPDATE jobs SET wakeup_emitted_generation=least(wakeup_emitted_generation,wakeup_generation-1) WHERE id=$1")
            .bind(wakeup.job_id).execute(&mut *tx).await?;
        WorkResolution::Ack
    } else if !matches!(state.as_str(), "queued" | "capability_wait") {
        WorkResolution::Terminate
    } else {
        let kind: String = row.try_get("kind")?;
        let known = kind.parse::<JobKind>().ok();
        if known.is_none() {
            WorkResolution::Retry
        } else {
            anyhow::ensure!(
                row.try_get::<String, _>("workload_class")? == class.as_str(),
                "persisted workload mismatch"
            );
            // Consequential handlers still recheck run/step fences. This
            // preflight avoids spending a claim on already-terminal work.
            if kind == "durable_run_step" {
                let payload: serde_json::Value = row.try_get("payload")?;
                let runnable: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM durable_steps s JOIN durable_runs r ON r.id=s.run_id WHERE s.id::text=$1 AND r.id::text=$2 AND s.status IN ('pending','queued','running','waiting') AND r.status IN ('queued','running','waiting'))")
                    .bind(payload.get("step_id").and_then(|v|v.as_str()))
                    .bind(payload.get("run_id").and_then(|v|v.as_str())).fetch_one(&mut *tx).await?;
                if !runnable {
                    sqlx::query("UPDATE jobs SET state='failed',last_error='terminal durable run/step notification' WHERE id=$1")
                        .bind(wakeup.job_id).execute(&mut *tx).await?;
                    tx.commit().await?;
                    return Ok(WorkResolution::Terminate);
                }
            }
            let kinds = JobKind::known_kinds();
            sqlx::query("SELECT set_config('deepref.job_kinds',$1,true)")
                .bind(kinds.join(","))
                .execute(&mut *tx)
                .await?;
            let claimed = sqlx::query("UPDATE jobs SET state='running',lease_owner=$2,
                leased_until=now()+($3::double precision * interval '1 millisecond'),lease_renewed_at=now(),attempts=attempts+1
                WHERE id=$1 AND attempts<max_attempts RETURNING *")
                .bind(wakeup.job_id).bind(owner).bind(lease.as_millis() as f64).fetch_optional(&mut *tx).await?;
            match claimed {
                Some(claimed) => WorkResolution::Claimed(crate::jobs::row_job(&claimed)?),
                None => WorkResolution::Terminate,
            }
        }
    };
    tx.commit().await?;
    Ok(result)
}

/// Poison/exhausted delivery is transport evidence, never a business failure.
/// Commit before publishing the DLQ and before terminal acknowledgement.
pub async fn record_dead_letter(
    pool: &PgPool,
    identity: &str,
    envelope: Option<&EventEnvelope>,
    consumer: &str,
    deliveries: u64,
    reason: &str,
) -> Result<()> {
    sqlx::query("INSERT INTO transport_dead_letters(delivery_id,event_id,consumer,publication_epoch,reason,deliveries)
        VALUES($1,(SELECT event_id FROM outbox_events WHERE event_id=$2),$3,$4,$5,$6) ON CONFLICT DO NOTHING")
        .bind(identity).bind(envelope.map(|e| e.event_id)).bind(consumer).bind(envelope.map(|e| e.publication_epoch))
        .bind(reason.chars().take(1024).collect::<String>()).bind(i64::try_from(deliveries)?).execute(pool).await?;
    Ok(())
}

/// Resolve a generic event using the existing 04a claim/receipt generation.
/// Only this exact event is eligible; missing consumers retry independently.
pub async fn claim_event_delivery(
    pool: &PgPool,
    registry: &OutboxRegistry,
    envelope: &EventEnvelope,
    owner: &str,
    lease: Duration,
) -> Result<Option<crate::outbox::ClaimedOutboxEvent>> {
    let Some(expected) =
        registry.expected_consumers(&envelope.message_type, envelope.message_version)
    else {
        return Ok(None);
    };
    let supported: Vec<_> = expected.iter().collect();
    let mut tx = pool.begin().await?;
    let row = sqlx::query(
        "UPDATE outbox_events SET lease_owner=$2,
        lease_expires_at=clock_timestamp()+($3::double precision * interval '1 millisecond'),
        attempts=attempts+1,claim_generation=claim_generation+1
        WHERE event_id=$1 AND message_type=$4 AND message_version=$5 AND payload=$6
          AND delivery_state='pending' AND next_attempt_at<=clock_timestamp()
          AND expected_consumers <@ $7::text[]
          AND (lease_owner IS NULL OR lease_expires_at<=clock_timestamp()) RETURNING *",
    )
    .bind(envelope.event_id)
    .bind(owner)
    .bind(lease.as_millis() as f64)
    .bind(&envelope.message_type)
    .bind(i32::try_from(envelope.message_version)?)
    .bind(&envelope.payload)
    .bind(&supported)
    .fetch_optional(&mut *tx)
    .await?;
    let Some(row) = row else {
        return Ok(None);
    };
    let generation: i64 = row.try_get("claim_generation")?;
    let consumers: Vec<String> = row.try_get("expected_consumers")?;
    sqlx::query("INSERT INTO outbox_receipts AS r(event_id,consumer,state,attempts,attempt_generation)
        SELECT $1,consumer,'retrying',1,$3 FROM unnest($2::text[]) AS expected(consumer)
        ON CONFLICT(event_id,consumer) DO UPDATE SET attempts=r.attempts+1,attempt_generation=$3
        WHERE r.state='retrying' AND r.next_attempt_at<=clock_timestamp() AND r.attempt_generation<$3")
        .bind(envelope.event_id).bind(&consumers).bind(generation).execute(&mut *tx).await?;
    tx.commit().await?;
    Ok(Some(crate::outbox::ClaimedOutboxEvent {
        event_id: envelope.event_id,
        message_type: envelope.message_type.clone(),
        message_version: envelope.message_version,
        payload: envelope.payload.clone(),
        expected_consumers: consumers,
        attempts: row.try_get("attempts")?,
        max_attempts: row.try_get("max_attempts")?,
        claim_fence: deepref_application::outbox::OutboxClaimFence::new(owner, generation),
    }))
}

pub async fn event_delivery_state(pool: &PgPool, event_id: Uuid) -> Result<Option<String>> {
    Ok(
        sqlx::query_scalar("SELECT delivery_state FROM outbox_events WHERE event_id=$1")
            .bind(event_id)
            .fetch_optional(pool)
            .await?,
    )
}
