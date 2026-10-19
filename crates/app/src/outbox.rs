//! Generic transactional outbox persistence and in-process dispatch
//! (handoff 04a). Broker-independent: the in-process dispatcher is the outbox
//! recipient until the trigger-gated JetStream relay (04b) takes over the same
//! rows.
//!
//! Row contract:
//! - `insert_outbox_event` runs in the caller's transaction, atomically with
//!   the authoritative change that caused the event.
//! - Claiming or leasing a row never marks it delivered. A row becomes
//!   `delivered` only when every expected consumer's receipt has committed,
//!   which `record_outbox_receipt` records inside the consumer's own
//!   transaction together with the consumer's effect.
//! - Failures retry with backoff per consumer; a consumer that exhausts its
//!   attempt budget (or fails terminally) quarantines the row for operational
//!   review and replay instead of being silently discarded.
//! - The expected consumer set is frozen onto each row at insert, so what
//!   `delivered` means never changes for rows already committed. Receipts for
//!   consumers outside that set are kept as evidence but never counted.
//!
//! `ai_autonomy_outbox` is a separate, domain-specific obligation table and is
//! deliberately not consolidated here.

use std::{collections::BTreeMap, time::Duration};

use anyhow::Context;
use chrono::{DateTime, Utc};
use deepref_application::outbox::{
    DeliveryContext, ErasedOutboxHandler, NewOutboxEvent, OutboxClaimFence, OutboxHandlerError,
    OutboxRegistry, OutboxRetry, retry_backoff,
};
use futures::FutureExt;
use serde_json::Value;
use sqlx::{PgPool, Postgres, Transaction};
use std::panic::AssertUnwindSafe;
use uuid::Uuid;

/// Delivery state of one receipt row. `Retrying` is dispatcher bookkeeping for
/// a consumer that has not yet committed its effect; `Delivered` is written by
/// the consumer itself, in the same transaction as its effect.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReceiptState {
    Retrying,
    Delivered,
    Quarantined,
}

impl ReceiptState {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Retrying => "retrying",
            Self::Delivered => "delivered",
            Self::Quarantined => "quarantined",
        }
    }

    fn parse(text: &str) -> anyhow::Result<Self> {
        match text {
            "retrying" => Ok(Self::Retrying),
            "delivered" => Ok(Self::Delivered),
            "quarantined" => Ok(Self::Quarantined),
            other => anyhow::bail!("invalid persisted outbox receipt state {other}"),
        }
    }
}

/// Aggregate delivery state of one outbox row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeliveryState {
    Pending,
    Delivered,
    Quarantined,
}

impl DeliveryState {
    fn parse(text: &str) -> anyhow::Result<Self> {
        match text {
            "pending" => Ok(Self::Pending),
            "delivered" => Ok(Self::Delivered),
            "quarantined" => Ok(Self::Quarantined),
            other => anyhow::bail!("invalid persisted outbox delivery state {other}"),
        }
    }
}

/// One claimed outbox row. This is dispatcher bookkeeping: application
/// handlers never see it (they receive [`DeliveryContext`] and their typed
/// message instead).
#[derive(Debug, Clone, PartialEq)]
pub struct ClaimedOutboxEvent {
    pub event_id: Uuid,
    pub message_type: String,
    pub message_version: u32,
    pub payload: Value,
    pub expected_consumers: Vec<String>,
    pub attempts: i32,
    pub max_attempts: i32,
    pub claim_fence: OutboxClaimFence,
}

/// One per-consumer receipt row.
#[derive(Debug, Clone, PartialEq)]
pub struct OutboxReceiptRecord {
    pub event_id: Uuid,
    pub consumer: String,
    pub state: ReceiptState,
    pub attempts: i32,
    pub attempt_generation: i64,
    pub settled_generation: i64,
    pub next_attempt_at: DateTime<Utc>,
    pub last_error: Option<String>,
    pub delivered_at: Option<DateTime<Utc>>,
}

/// Insert one outbox row in the caller's transaction. Returns `false` when the
/// event id already exists (a replayed business transaction): the outbox row
/// identity is stable, so a duplicate insert is a committed no-op.
pub async fn insert_outbox_event(
    tx: &mut Transaction<'_, Postgres>,
    event: &NewOutboxEvent,
) -> anyhow::Result<bool> {
    anyhow::ensure!(
        !event.expected_consumers.is_empty(),
        "an outbox event must declare at least one expected consumer"
    );
    anyhow::ensure!(
        event
            .expected_consumers
            .iter()
            .all(|consumer| { !consumer.is_empty() && consumer.trim() == consumer }),
        "outbox consumer names must be non-empty and have no surrounding whitespace"
    );
    let unique_consumers: std::collections::BTreeSet<&String> =
        event.expected_consumers.iter().collect();
    anyhow::ensure!(
        unique_consumers.len() == event.expected_consumers.len(),
        "an outbox event cannot declare the same consumer more than once"
    );
    anyhow::ensure!(
        event.max_attempts > 0,
        "an outbox event must allow at least one delivery attempt"
    );
    let message_version = i32::try_from(event.message_version).context("message version fits")?;
    let rows = sqlx::query!(
        r#"INSERT INTO outbox_events
             (event_id, message_type, message_version, payload, expected_consumers, max_attempts)
           VALUES ($1, $2, $3, $4, $5, $6)
           ON CONFLICT (event_id) DO NOTHING"#,
        event.event_id,
        event.message_type,
        message_version,
        event.payload,
        &event.expected_consumers,
        event.max_attempts,
    )
    .execute(&mut **tx)
    .await?;
    Ok(rows.rows_affected() == 1)
}

/// Recover abandoned claims, then reserve one attempt for each due consumer
/// atomically with claiming its event. A generation settles each reservation at
/// most once; consumers still backing off receive no reservation or charge.
pub async fn claim_outbox_batch(
    pool: &PgPool,
    registry: &OutboxRegistry,
    owner: &str,
    lease: Duration,
    limit: i64,
) -> anyhow::Result<Vec<ClaimedOutboxEvent>> {
    anyhow::ensure!(
        !lease.is_zero() && limit > 0,
        "claim lease and limit must be positive"
    );
    let mut contracts = Vec::new();
    for (kind, version) in registry.contracts() {
        let consumers: Vec<_> = registry
            .expected_consumers(kind, version)
            .into_iter()
            .flatten()
            .collect();
        contracts.push(serde_json::json!({
            "message_type": kind,
            "message_version": i32::try_from(version).context("registered message version fits")?,
            "consumers": consumers,
        }));
    }
    if contracts.is_empty() {
        return Ok(Vec::new());
    }
    let contracts = Value::Array(contracts);
    let mut tx = pool.begin().await?;
    park_unsupported_consumers(&mut tx, &contracts, limit).await?;
    recover_abandoned_outbox_claims(&mut tx, &contracts, limit).await?;
    let lease_ms = lease.as_millis() as f64;
    let rows = sqlx::query!(
        r#"WITH candidate AS (
             SELECT e.event_id FROM outbox_events e
             JOIN jsonb_to_recordset($4::jsonb)
               AS contract(message_type text, message_version integer, consumers text[])
               USING (message_type, message_version)
             WHERE e.delivery_state='pending' AND e.lease_owner IS NULL
               AND e.next_attempt_at <= clock_timestamp()
               AND e.expected_consumers <@ contract.consumers
             ORDER BY e.created_at,e.event_id FOR UPDATE OF e SKIP LOCKED LIMIT $3
           )
           UPDATE outbox_events e
           SET lease_owner=$1,
               lease_expires_at=clock_timestamp()+($2::double precision*interval '1 millisecond'),
               attempts=e.attempts+1,claim_generation=e.claim_generation+1
           FROM candidate WHERE e.event_id=candidate.event_id
           RETURNING e.event_id,e.message_type,e.message_version,e.payload,
                     e.expected_consumers,e.attempts,e.max_attempts,e.claim_generation"#,
        owner,
        lease_ms,
        limit,
        contracts,
    )
    .fetch_all(&mut *tx)
    .await?;
    for row in &rows {
        sqlx::query!(
            r#"INSERT INTO outbox_receipts AS receipts
                 (event_id,consumer,state,attempts,attempt_generation)
               SELECT $1,consumer,'retrying',1,$3 FROM unnest($2::text[]) AS expected(consumer)
               ON CONFLICT (event_id,consumer) DO UPDATE
               SET attempts=receipts.attempts+1,attempt_generation=EXCLUDED.attempt_generation
               WHERE receipts.state='retrying' AND receipts.next_attempt_at <= clock_timestamp()
                 AND receipts.attempt_generation < EXCLUDED.attempt_generation"#,
            row.event_id,
            &row.expected_consumers,
            row.claim_generation,
        )
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;
    rows.into_iter()
        .map(|row| {
            let event_id = row.event_id;
            Ok(ClaimedOutboxEvent {
                event_id,
                message_type: row.message_type,
                message_version: u32::try_from(row.message_version)
                    .with_context(|| format!("invalid persisted message version for {event_id}"))?,
                payload: row.payload,
                expected_consumers: row.expected_consumers,
                attempts: row.attempts,
                max_attempts: row.max_attempts,
                claim_fence: OutboxClaimFence::new(owner, row.claim_generation),
            })
        })
        .collect()
}

/// Park a bounded batch for five minutes with a diagnostic. The claim query
/// also excludes every incompatible row, including ones beyond this batch, so
/// an arbitrarily old incompatible backlog cannot starve supported work.
async fn park_unsupported_consumers(
    tx: &mut Transaction<'_, Postgres>,
    contracts: &Value,
    limit: i64,
) -> anyhow::Result<()> {
    sqlx::query!(
        r#"WITH candidate AS (
             SELECT e.event_id,
               'unsupported frozen outbox consumers: ' || array_to_string(ARRAY(
                 SELECT consumer FROM unnest(e.expected_consumers) AS expected(consumer)
                 WHERE NOT consumer=ANY(contract.consumers)), ', ') AS reason
             FROM outbox_events e
             JOIN jsonb_to_recordset($1::jsonb)
               AS contract(message_type text,message_version integer,consumers text[])
               USING (message_type,message_version)
             WHERE e.delivery_state='pending' AND e.next_attempt_at <= clock_timestamp()
               AND (e.lease_owner IS NULL OR e.lease_expires_at <= clock_timestamp())
               AND NOT e.expected_consumers <@ contract.consumers
             ORDER BY e.created_at,e.event_id FOR UPDATE OF e SKIP LOCKED LIMIT $2
           )
           UPDATE outbox_events e SET last_error=candidate.reason,
             next_attempt_at=clock_timestamp()+interval '5 minutes',
             lease_owner=NULL,lease_expires_at=NULL,claim_generation=claim_generation+1
           FROM candidate WHERE e.event_id=candidate.event_id"#,
        contracts,
        limit,
    )
    .execute(&mut **tx)
    .await?;
    Ok(())
}

async fn recover_abandoned_outbox_claims(
    tx: &mut Transaction<'_, Postgres>,
    contracts: &Value,
    limit: i64,
) -> anyhow::Result<()> {
    let expired = sqlx::query!(
        r#"SELECT e.event_id,e.max_attempts,e.claim_generation,e.lease_owner AS "lease_owner!"
           FROM outbox_events e
           JOIN jsonb_to_recordset($1::jsonb)
             AS contract(message_type text,message_version integer,consumers text[])
             USING (message_type,message_version)
           WHERE e.delivery_state='pending' AND e.lease_expires_at <= clock_timestamp()
             AND e.expected_consumers <@ contract.consumers
           ORDER BY e.lease_expires_at,e.event_id FOR UPDATE OF e SKIP LOCKED LIMIT $2"#,
        contracts,
        limit,
    )
    .fetch_all(&mut **tx)
    .await?;
    for event in expired {
        let reason = format!(
            "abandoned outbox claim after lease expiry (generation {})",
            event.claim_generation
        );
        let unfinished = sqlx::query!(
            r#"SELECT consumer,attempts FROM outbox_receipts
               WHERE event_id=$1 AND attempt_generation=$2 AND settled_generation<>$2
                 AND state='retrying'"#,
            event.event_id,
            event.claim_generation,
        )
        .fetch_all(&mut **tx)
        .await?;
        for receipt in unfinished {
            let state = if receipt.attempts >= event.max_attempts {
                "quarantined"
            } else {
                "retrying"
            };
            let delay_ms = retry_backoff(receipt.attempts).as_millis() as f64;
            sqlx::query!(
                r#"UPDATE outbox_receipts SET state=$4,settled_generation=$3,
                     next_attempt_at=clock_timestamp()+($5::double precision*interval '1 millisecond'),last_error=$6
                   WHERE event_id=$1 AND consumer=$2 AND attempt_generation=$3
                     AND settled_generation<>$3 AND state='retrying'"#,
                event.event_id,receipt.consumer,event.claim_generation,state,delay_ms,reason,
            ).execute(&mut **tx).await?;
        }
        let exhausted = sqlx::query_scalar!(
            r#"SELECT EXISTS(SELECT 1 FROM outbox_receipts r JOIN outbox_events e USING(event_id)
                 WHERE r.event_id=$1 AND r.consumer=ANY(e.expected_consumers)
                   AND r.state='quarantined') AS "exhausted!""#,
            event.event_id,
        )
        .fetch_one(&mut **tx)
        .await?;
        let state = if exhausted { "quarantined" } else { "pending" };
        let reason = if exhausted {
            format!("{reason}; attempt budget exhausted")
        } else {
            reason
        };
        sqlx::query!(
            r#"UPDATE outbox_events e SET delivery_state=$4,last_error=$5,
                 next_attempt_at=COALESCE((SELECT min(r.next_attempt_at) FROM outbox_receipts r
                   WHERE r.event_id=e.event_id AND r.consumer=ANY(e.expected_consumers)
                     AND r.state='retrying'),clock_timestamp()),
                 lease_owner=NULL,lease_expires_at=NULL,claim_generation=claim_generation+1
               WHERE e.event_id=$1 AND e.lease_owner=$2 AND e.claim_generation=$3
                 AND e.delivery_state='pending' AND e.lease_expires_at <= clock_timestamp()"#,
            event.event_id,
            event.lease_owner,
            event.claim_generation,
            state,
            reason,
        )
        .execute(&mut **tx)
        .await?;
    }
    Ok(())
}

/// Extend the claim lease while dispatching. `false` when the lease is no
/// longer owned (expired and reclaimed).
pub async fn renew_outbox_lease(
    pool: &PgPool,
    event_id: Uuid,
    fence: &OutboxClaimFence,
    lease: Duration,
) -> anyhow::Result<bool> {
    let lease_ms = lease.as_millis() as f64;
    Ok(sqlx::query!(
        r#"UPDATE outbox_events
           SET lease_expires_at = clock_timestamp() + ($3::double precision * interval '1 millisecond')
           WHERE event_id = $1 AND lease_owner = $2 AND claim_generation = $4
             AND delivery_state = 'pending' AND lease_expires_at > clock_timestamp()"#,
        event_id,
        fence.owner(),
        lease_ms,
        fence.generation(),
    )
    .execute(pool)
    .await?
    .rows_affected()
        == 1)
}

/// Give up the claim lease without touching delivery state (for example on
/// shutdown before every consumer was attempted).
pub async fn release_outbox_lease(
    pool: &PgPool,
    event_id: Uuid,
    fence: &OutboxClaimFence,
) -> anyhow::Result<bool> {
    Ok(sqlx::query!(
        "UPDATE outbox_events SET lease_owner = NULL, lease_expires_at = NULL
         WHERE event_id = $1 AND lease_owner = $2 AND claim_generation = $3
           AND delivery_state = 'pending' AND lease_expires_at > clock_timestamp()",
        event_id,
        fence.owner(),
        fence.generation(),
    )
    .execute(pool)
    .await?
    .rows_affected()
        == 1)
}

/// Record one consumer's receipt in the consumer's transaction, together with
/// the consumer's effect. When this completes the expected set, the outbox row
/// becomes `delivered` in the same commit — never earlier. Returns `true` when
/// this call wrote the delivered receipt (a consumer that already holds one is
/// a no-op).
pub async fn record_outbox_receipt(
    tx: &mut Transaction<'_, Postgres>,
    context: &DeliveryContext,
) -> anyhow::Result<bool> {
    let existing = sqlx::query!(
        "SELECT state FROM outbox_receipts WHERE event_id = $1 AND consumer = $2",
        context.event_id,
        context.consumer,
    )
    .fetch_optional(&mut **tx)
    .await?;
    if existing.is_some_and(|receipt| receipt.state == "delivered") {
        // Redelivery of an already committed receipt is a no-op. Keep that
        // behavior when this same generation completed the aggregate, but
        // reject an older generation if the event was reclaimed and completed
        // by a different worker. For a still-pending row, require the active
        // lease even on this no-write path.
        let row = sqlx::query!(
            "SELECT delivery_state, claim_generation FROM outbox_events WHERE event_id = $1 FOR UPDATE",
            context.event_id,
        )
        .fetch_optional(&mut **tx)
        .await?;
        if row.is_some_and(|row| {
            row.delivery_state == "delivered"
                && row.claim_generation == context.claim_fence.generation()
        }) {
            return Ok(false);
        }
        fence_outbox_claim(tx, context.event_id, &context.claim_fence).await?;
        return Ok(false);
    }

    // The row lock prevents a reclaimer from taking over while the receipt
    // and the consumer's effect commit in this transaction.
    fence_outbox_claim(tx, context.event_id, &context.claim_fence).await?;
    let recorded = sqlx::query!(
        r#"INSERT INTO outbox_receipts (event_id, consumer, state, delivered_at)
           VALUES ($1, $2, 'delivered', now())
           ON CONFLICT (event_id, consumer) DO UPDATE
           SET state = 'delivered', delivered_at = now(), last_error = NULL, next_attempt_at = now(),
               settled_generation=outbox_receipts.attempt_generation
           WHERE outbox_receipts.state <> 'delivered'
           RETURNING event_id"#,
        context.event_id,
        context.consumer,
    )
    .fetch_optional(&mut **tx)
    .await?
    .is_some();
    mark_outbox_delivered_if_complete(tx, context.event_id, &context.claim_fence).await?;
    Ok(recorded)
}

/// Lock and validate an active claim inside the consumer's transaction. The
/// row lock prevents a concurrent reclaimer from taking over before the
/// effect and receipt commit; generation plus expiry reject an old claimer.
async fn fence_outbox_claim(
    tx: &mut Transaction<'_, Postgres>,
    event_id: Uuid,
    fence: &OutboxClaimFence,
) -> anyhow::Result<()> {
    let row = sqlx::query!(
        r#"SELECT event_id FROM outbox_events
           WHERE event_id = $1 AND delivery_state = 'pending'
             AND lease_owner = $2 AND claim_generation = $3
             AND lease_expires_at > clock_timestamp()
           FOR UPDATE"#,
        event_id,
        fence.owner(),
        fence.generation(),
    )
    .fetch_optional(&mut **tx)
    .await?;
    anyhow::ensure!(row.is_some(), "outbox claim fence is stale or expired");
    Ok(())
}

/// Mark the row `delivered` exactly when every expected recipient has a
/// committed receipt. Safe to call at any time; claiming a row or a consumer
/// receipt outside the expected set never satisfies it.
async fn mark_outbox_delivered_if_complete(
    tx: &mut Transaction<'_, Postgres>,
    event_id: Uuid,
    fence: &OutboxClaimFence,
) -> anyhow::Result<bool> {
    // Keep the aggregate completion write under the same generation check and
    // row lock as the consumer's receipt and effect.
    fence_outbox_claim(tx, event_id, fence).await?;
    let updated = sqlx::query!(
        r#"UPDATE outbox_events
           SET delivery_state = 'delivered',
               delivered_at = now(),
               lease_owner = NULL,
               lease_expires_at = NULL,
               next_attempt_at = now()
           WHERE event_id = $1
             AND delivery_state = 'pending'
             AND lease_owner = $2 AND claim_generation = $3
             AND lease_expires_at > clock_timestamp()
             AND NOT EXISTS (
               SELECT 1 FROM unnest(outbox_events.expected_consumers) AS expected(consumer)
               WHERE NOT EXISTS (
                 SELECT 1 FROM outbox_receipts r
                 WHERE r.event_id = outbox_events.event_id
                   AND r.consumer = expected.consumer
                   AND r.state = 'delivered'))"#,
        event_id,
        fence.owner(),
        fence.generation(),
    )
    .execute(&mut **tx)
    .await?;
    Ok(updated.rows_affected() == 1)
}

/// Record a failed delivery attempt for one consumer: retries with backoff,
/// quarantining the consumer (and with it the row) once its attempt budget is
/// spent or the failure is terminal. A consumer that already holds a delivered
/// receipt is never re-run and never regresses.
pub async fn record_outbox_consumer_failure(
    pool: &PgPool,
    event_id: Uuid,
    consumer: &str,
    fence: &OutboxClaimFence,
    error: &str,
    retry: OutboxRetry,
    max_attempts: i32,
) -> anyhow::Result<ReceiptState> {
    let (terminal, delay) = match retry {
        OutboxRetry::After(delay) => (false, delay),
        OutboxRetry::Terminal => (true, Duration::ZERO),
    };
    let delay_ms = delay.as_millis() as f64;
    let mut tx = pool.begin().await?;
    fence_outbox_claim(&mut tx, event_id, fence).await?;
    let row = sqlx::query!(
        r#"UPDATE outbox_receipts
           SET state=CASE WHEN $3::boolean OR attempts >= $4 THEN 'quarantined' ELSE 'retrying' END,
             settled_generation=$7,
             next_attempt_at=clock_timestamp()+($5::double precision*interval '1 millisecond'),
             last_error=$6
           WHERE event_id=$1 AND consumer=$2 AND state='retrying'
             AND attempt_generation=$7 AND settled_generation<>$7
           RETURNING state"#,
        event_id,
        consumer,
        terminal,
        max_attempts,
        delay_ms,
        error,
        fence.generation(),
    )
    .fetch_optional(&mut *tx)
    .await?;
    let row = match row {
        Some(row) => row.state,
        None => {
            // Repeated failure bookkeeping for this generation is a no-op.
            // A consumer skipped in backoff has no reservation and may not be charged.
            let existing = sqlx::query!(
                r#"SELECT state FROM outbox_receipts WHERE event_id=$1 AND consumer=$2
                   AND (state='delivered' OR (attempt_generation=$3 AND settled_generation=$3))"#,
                event_id,
                consumer,
                fence.generation(),
            )
            .fetch_optional(&mut *tx)
            .await?;
            let existing =
                existing.context("no unsettled outbox consumer attempt in this generation")?;
            tx.commit().await?;
            return ReceiptState::parse(&existing.state);
        }
    };
    let state = ReceiptState::parse(&row)?;
    if state == ReceiptState::Quarantined {
        sqlx::query!(
            r#"UPDATE outbox_events
               SET delivery_state = 'quarantined', last_error = $2,
                   lease_owner = NULL, lease_expires_at = NULL
               WHERE event_id = $1 AND delivery_state = 'pending'"#,
            event_id,
            error,
        )
        .execute(&mut *tx)
        .await?;
    } else {
        sqlx::query!(
            r#"UPDATE outbox_events e
               SET last_error = $2,
                   next_attempt_at = CASE
                     WHEN EXISTS (
                       SELECT 1 FROM unnest(e.expected_consumers) AS expected(consumer)
                       WHERE NOT EXISTS (
                         SELECT 1 FROM outbox_receipts r
                         WHERE r.event_id = e.event_id AND r.consumer = expected.consumer))
                     THEN now()
                     ELSE COALESCE((
                       SELECT min(r.next_attempt_at) FROM outbox_receipts r
                       WHERE r.event_id = e.event_id AND r.state = 'retrying'), now())
                   END
               WHERE e.event_id = $1"#,
            event_id,
            error,
        )
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;
    Ok(state)
}

/// Quarantine a row terminally (an unknown message contract, or operational
/// intervention). The row stops being claimed and waits for review and replay;
/// it is never silently discarded.
pub async fn quarantine_outbox_event(
    pool: &PgPool,
    event_id: Uuid,
    fence: &OutboxClaimFence,
    reason: &str,
) -> anyhow::Result<bool> {
    Ok(sqlx::query!(
        r#"UPDATE outbox_events
           SET delivery_state = 'quarantined', last_error = $2,
               lease_owner = NULL, lease_expires_at = NULL
           WHERE event_id = $1 AND delivery_state = 'pending'
             AND lease_owner = $3 AND claim_generation = $4
             AND lease_expires_at > clock_timestamp()"#,
        event_id,
        reason,
        fence.owner(),
        fence.generation(),
    )
    .execute(pool)
    .await?
    .rows_affected()
        == 1)
}

/// Settle one dispatch round: release the claim lease (only while it is still
/// owned), mark the row `delivered` if every expected receipt has committed,
/// and recompute when the row is next claimable from the missing recipients'
/// per-consumer backoff. Returns `None` when the lease was lost first — the
/// reclaiming dispatcher owns the settlement then.
pub async fn settle_outbox_event(
    pool: &PgPool,
    event_id: Uuid,
    fence: &OutboxClaimFence,
) -> anyhow::Result<Option<DeliveryState>> {
    let row = sqlx::query!(
        r#"UPDATE outbox_events e
           SET delivery_state = CASE
                 WHEN e.delivery_state = 'quarantined' THEN 'quarantined'
                 WHEN NOT EXISTS (
                   SELECT 1 FROM unnest(e.expected_consumers) AS expected(consumer)
                   WHERE NOT EXISTS (
                     SELECT 1 FROM outbox_receipts r
                     WHERE r.event_id = e.event_id
                       AND r.consumer = expected.consumer
                       AND r.state = 'delivered'))
                 THEN 'delivered'
                 WHEN EXISTS (
                   SELECT 1 FROM outbox_receipts r
                   WHERE r.event_id = e.event_id AND r.state = 'quarantined')
                 THEN 'quarantined'
                 ELSE 'pending' END,
               delivered_at = CASE
                 WHEN e.delivered_at IS NOT NULL THEN e.delivered_at
                 WHEN NOT EXISTS (
                   SELECT 1 FROM unnest(e.expected_consumers) AS expected(consumer)
                   WHERE NOT EXISTS (
                     SELECT 1 FROM outbox_receipts r
                     WHERE r.event_id = e.event_id
                       AND r.consumer = expected.consumer
                       AND r.state = 'delivered'))
                 THEN now()
                 ELSE NULL END,
               next_attempt_at = CASE
                 WHEN EXISTS (
                   SELECT 1 FROM unnest(e.expected_consumers) AS expected(consumer)
                   WHERE NOT EXISTS (
                     SELECT 1 FROM outbox_receipts r
                     WHERE r.event_id = e.event_id AND r.consumer = expected.consumer))
                 THEN now()
                 ELSE COALESCE((
                   SELECT min(r.next_attempt_at) FROM outbox_receipts r
                   WHERE r.event_id = e.event_id AND r.state = 'retrying'), now())
               END,
               lease_owner = NULL,
               lease_expires_at = NULL
           WHERE e.event_id = $1 AND e.lease_owner = $2
             AND e.claim_generation = $3 AND e.lease_expires_at > clock_timestamp()
           RETURNING e.delivery_state"#,
        event_id,
        fence.owner(),
        fence.generation(),
    )
    .fetch_optional(pool)
    .await?;
    match row {
        Some(row) => Ok(Some(DeliveryState::parse(&row.delivery_state)?)),
        None => Ok(None),
    }
}

/// Re-open a quarantined row for dispatch after operational review (explicit
/// replay with the same stable event identity). Resets the claim attempt
/// budget and the quarantined recipients' retry state; committed receipts are
/// kept and are never re-run.
pub async fn replay_outbox_event(pool: &PgPool, event_id: Uuid) -> anyhow::Result<bool> {
    let mut tx = pool.begin().await?;
    let reopened = sqlx::query!(
        r#"UPDATE outbox_events
           SET delivery_state = 'pending', attempts = 0, next_attempt_at = now(),
               lease_owner = NULL, lease_expires_at = NULL
           WHERE event_id = $1 AND delivery_state = 'quarantined'"#,
        event_id,
    )
    .execute(&mut *tx)
    .await?
    .rows_affected()
        == 1;
    if reopened {
        sqlx::query!(
            r#"UPDATE outbox_receipts
               SET state = 'retrying', attempts = 0, next_attempt_at = now(), last_error = NULL,
                   attempt_generation=0,settled_generation=0
               WHERE event_id = $1 AND state = 'quarantined'"#,
            event_id,
        )
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;
    Ok(reopened)
}

/// Every receipt row recorded for one event (delivered, retrying or
/// quarantined).
pub async fn list_outbox_receipts(
    pool: &PgPool,
    event_id: Uuid,
) -> anyhow::Result<Vec<OutboxReceiptRecord>> {
    let rows = sqlx::query!(
        r#"SELECT event_id, consumer, state, attempts, attempt_generation, settled_generation, next_attempt_at, last_error, delivered_at
           FROM outbox_receipts WHERE event_id = $1 ORDER BY consumer"#,
        event_id,
    )
    .fetch_all(pool)
    .await?;
    rows.into_iter()
        .map(|row| {
            Ok(OutboxReceiptRecord {
                event_id: row.event_id,
                consumer: row.consumer,
                state: ReceiptState::parse(&row.state)?,
                attempts: row.attempts,
                attempt_generation: row.attempt_generation,
                settled_generation: row.settled_generation,
                next_attempt_at: row.next_attempt_at,
                last_error: row.last_error,
                delivered_at: row.delivered_at,
            })
        })
        .collect()
}

/// Claim one bounded batch and dispatch every claimed row to the registry.
/// Returns the number of claimed rows (some may end up retrying).
pub async fn dispatch_outbox_batch(
    pool: &PgPool,
    registry: &OutboxRegistry,
    owner: &str,
    lease: Duration,
    limit: i64,
) -> anyhow::Result<usize> {
    let claimed = claim_outbox_batch(pool, registry, owner, lease, limit).await?;
    let count = claimed.len();
    for event in &claimed {
        dispatch_outbox_event(pool, registry, lease, event).await?;
    }
    Ok(count)
}

/// Dispatch one claimed row: invoke the registered handler for each expected
/// consumer that has no committed receipt and is not still backing off, then
/// settle the round. The row is never marked delivered here — only committed
/// receipts complete it. Unsupported contracts stay pending for a compatible
/// deployment; the claim query normally excludes them before this point.
pub async fn dispatch_outbox_event(
    pool: &PgPool,
    registry: &OutboxRegistry,
    lease: Duration,
    event: &ClaimedOutboxEvent,
) -> anyhow::Result<()> {
    if !registry.knows(&event.message_type, event.message_version) {
        park_outbox_claim(pool, event, "unsupported outbox contract").await?;
        return Ok(());
    }
    if let Some(consumer) = event.expected_consumers.iter().find(|consumer| {
        registry
            .handler(&event.message_type, event.message_version, consumer)
            .is_none()
    }) {
        park_outbox_claim(
            pool,
            event,
            &format!("unsupported frozen outbox consumer: {consumer}"),
        )
        .await?;
        return Ok(());
    }
    let receipts: BTreeMap<String, OutboxReceiptRecord> =
        list_outbox_receipts(pool, event.event_id)
            .await?
            .into_iter()
            .map(|receipt| (receipt.consumer.clone(), receipt))
            .collect();
    let now = Utc::now();
    for consumer in &event.expected_consumers {
        let attempts = match receipts.get(consumer) {
            // A consumer that already holds a receipt (delivered or
            // quarantined) is never re-run.
            Some(receipt) if receipt.state != ReceiptState::Retrying => continue,
            // A retrying consumer waits for its own per-recipient backoff.
            Some(receipt) if receipt.next_attempt_at > now => continue,
            Some(receipt)
                if receipt.attempt_generation != event.claim_fence.generation()
                    || receipt.settled_generation == receipt.attempt_generation =>
            {
                continue;
            }
            Some(receipt) => receipt.attempts,
            None => continue,
        };
        let handler = registry
            .handler(&event.message_type, event.message_version, consumer)
            .context("validated outbox consumer has a handler")?;
        let context = DeliveryContext {
            event_id: event.event_id,
            consumer: consumer.clone(),
            message_type: event.message_type.clone(),
            message_version: event.message_version,
            claim_fence: event.claim_fence.clone(),
        };
        match crate::leased_handler::run(
            lease,
            || renew_outbox_lease(pool, event.event_id, &event.claim_fence, lease),
            invoke_handler(handler, context, event.payload.clone()),
        )
        .await?
        {
            Ok(()) => {
                // The handler contract: success means the effect and the
                // receipt committed together. Verify; a handler that skipped
                // its receipt would otherwise redeliver forever.
                if !consumer_has_receipt(pool, event.event_id, consumer).await? {
                    record_outbox_consumer_failure(
                        pool,
                        event.event_id,
                        consumer,
                        &event.claim_fence,
                        "handler reported success without committing its receipt",
                        OutboxRetry::Terminal,
                        event.max_attempts,
                    )
                    .await?;
                }
            }
            Err(OutboxHandlerError::Transient(message)) => {
                tracing::warn!(
                    event_id = %event.event_id,
                    consumer,
                    message_type = %event.message_type,
                    attempts,
                    error = %message,
                    "outbox consumer failed; will retry"
                );
                record_outbox_consumer_failure(
                    pool,
                    event.event_id,
                    consumer,
                    &event.claim_fence,
                    &message,
                    OutboxRetry::After(retry_backoff(attempts)),
                    event.max_attempts,
                )
                .await?;
            }
            Err(OutboxHandlerError::Terminal(message)) => {
                tracing::error!(
                    event_id = %event.event_id,
                    consumer,
                    message_type = %event.message_type,
                    attempts,
                    error = %message,
                    "outbox consumer failed terminally"
                );
                record_outbox_consumer_failure(
                    pool,
                    event.event_id,
                    consumer,
                    &event.claim_fence,
                    &message,
                    OutboxRetry::Terminal,
                    event.max_attempts,
                )
                .await?;
            }
        }
    }
    settle_outbox_event(pool, event.event_id, &event.claim_fence).await?;
    Ok(())
}

/// Also handle a directly supplied claim against a changed registry safely.
async fn park_outbox_claim(
    pool: &PgPool,
    event: &ClaimedOutboxEvent,
    reason: &str,
) -> anyhow::Result<()> {
    sqlx::query!(
        r#"UPDATE outbox_events SET lease_owner=NULL,lease_expires_at=NULL,last_error=$4,
             next_attempt_at=clock_timestamp()+interval '5 minutes'
           WHERE event_id=$1 AND lease_owner=$2 AND claim_generation=$3
             AND delivery_state='pending' AND lease_expires_at>clock_timestamp()"#,
        event.event_id,
        event.claim_fence.owner(),
        event.claim_fence.generation(),
        reason,
    )
    .execute(pool)
    .await?;
    Ok(())
}

/// The processor owns this future directly. Catch panics without spawning a
/// detachable child task, so processor cancellation drops the handler and any
/// uncommitted transaction before the processor task can finish.
async fn invoke_handler(
    handler: &ErasedOutboxHandler,
    context: DeliveryContext,
    payload: Value,
) -> Result<(), OutboxHandlerError> {
    let consumer = context.consumer.clone();
    match AssertUnwindSafe(async move { handler(context, payload).await })
        .catch_unwind()
        .await
    {
        Ok(result) => result,
        Err(panic) => Err(OutboxHandlerError::Transient(format!(
            "handler for consumer {consumer} panicked: {}",
            crate::leased_handler::panic_message(panic)
        ))),
    }
}

async fn consumer_has_receipt(
    pool: &PgPool,
    event_id: Uuid,
    consumer: &str,
) -> anyhow::Result<bool> {
    let exists = sqlx::query!(
        "SELECT event_id FROM outbox_receipts WHERE event_id = $1 AND consumer = $2 AND state = 'delivered'",
        event_id,
        consumer,
    )
    .fetch_optional(pool)
    .await?;
    Ok(exists.is_some())
}
