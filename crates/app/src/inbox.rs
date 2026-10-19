//! Generic integration inbox persistence and processor shell (handoff 04a).
//!
//! Provider webhooks are received through PostgreSQL: the signature is
//! verified before any insert, the `(provider, provider_event_id)` unique key
//! makes a duplicate delivery a committed no-op, and the caller answers `2xx`
//! only after the insert or duplicate check commits. Processing follows
//! `received -> processing -> processed`, with `failed` retrying on backoff
//! until bounded attempts quarantine the event for operational review and
//! replay (`requeue_inbox_event`). Nothing is silently discarded.
//!
//! No provider routes exist yet: WorkOS and Polar adapters (handoffs 02 and
//! 08) implement `WebhookVerifier` and register typed handlers. Until then
//! [`HmacSha256Verifier`] covers tests and development.

use deepref_application::inbox::{
    ErasedInboxHandler, InboxClaimFence, InboxContext, InboxHandlerError, InboxProcessingState,
    InboxRegistry, InboxRetry, WebhookVerifier,
};
use deepref_application::outbox::retry_backoff;
use futures::FutureExt;
use serde_json::Value;
use sqlx::{PgPool, Postgres, Transaction};
use std::panic::AssertUnwindSafe;

/// Maximum authenticated request body retained by the inbox: 256 KiB.
pub const MAX_INBOX_PAYLOAD_BYTES: usize = 256 * 1024;
/// Processed payloads are eligible for minimization 30 days after completion.
pub const INBOX_PAYLOAD_RETENTION_DAYS: i32 = 30;

/// One provider webhook delivery to admit into the inbox.
#[derive(Debug, Clone)]
pub struct ReceiveWebhookEvent<'a> {
    pub provider: &'a str,
    pub provider_event_id: &'a str,
    pub event_type: Option<&'a str>,
    /// Raw request body; verified and stored as JSON.
    pub body: &'a [u8],
    /// Provider signature header, when the provider signs its webhooks.
    pub signature: Option<&'a str>,
}

/// The outcome a caller may map to `2xx`: either this call committed a new
/// inbox row, or the event was already committed and is acknowledged without
/// reprocessing. An error means no acknowledgement.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReceiveOutcome {
    Accepted(i64),
    Duplicate,
}

#[derive(Debug, thiserror::Error)]
pub enum InboxReceiveError {
    #[error("webhook body is {size} bytes; inbox limit is {limit} bytes")]
    PayloadTooLarge { size: usize, limit: usize },
    #[error("webhook rejected before insert: {0}")]
    Signature(#[from] deepref_application::inbox::WebhookVerificationError),
    #[error("webhook body is not valid JSON: {0}")]
    InvalidPayload(#[from] serde_json::Error),
    #[error("inbox insert failed: {0}")]
    Database(#[from] sqlx::Error),
}

/// Verify, then admit one provider event. The signature is checked before any
/// database write; a rejected request creates no row. The duplicate check is
/// the unique `(provider, provider_event_id)` key. Both the insert and the
/// duplicate check commit before the caller may acknowledge receipt.
pub async fn receive_webhook_event(
    pool: &PgPool,
    verifier: &dyn WebhookVerifier,
    event: &ReceiveWebhookEvent<'_>,
) -> Result<ReceiveOutcome, InboxReceiveError> {
    if event.body.len() > MAX_INBOX_PAYLOAD_BYTES {
        return Err(InboxReceiveError::PayloadTooLarge {
            size: event.body.len(),
            limit: MAX_INBOX_PAYLOAD_BYTES,
        });
    }
    verifier.verify(event.body, event.signature)?;
    let payload: Value = serde_json::from_slice(event.body)?;
    let mut tx: Transaction<'_, Postgres> = pool.begin().await?;
    let inserted = sqlx::query!(
        r#"INSERT INTO integration_inbox_events
             (provider, provider_event_id, event_type, payload)
           VALUES ($1, $2, $3, $4)
           ON CONFLICT (provider, provider_event_id) DO NOTHING
           RETURNING id"#,
        event.provider,
        event.provider_event_id,
        event.event_type,
        payload,
    )
    .fetch_optional(&mut *tx)
    .await?;
    let Some(row) = inserted else {
        tx.commit().await?;
        return Ok(ReceiveOutcome::Duplicate);
    };
    tx.commit().await?;
    Ok(ReceiveOutcome::Accepted(row.id))
}

/// One claimed inbox row awaiting processing. Processing bookkeeping: handlers
/// see [`deepref_application::inbox::InboxContext`] and their typed payload,
/// never claim rows.
#[derive(Debug, Clone, PartialEq)]
pub struct ClaimedInboxEvent {
    pub id: i64,
    pub provider: String,
    pub provider_event_id: String,
    pub event_type: Option<String>,
    pub payload: Value,
    pub attempts: i32,
    pub max_attempts: i32,
    pub claim_fence: InboxClaimFence,
}

/// Recover supported abandoned claims, then claim due work. Attempts are
/// charged when a claim starts, so an expired attempt already spent its budget.
/// Recovery fences that generation and persists backoff/quarantine before a
/// subsequent claim can increment the attempt count again.
pub async fn claim_inbox_batch(
    pool: &PgPool,
    registry: &InboxRegistry,
    owner: &str,
    lease: std::time::Duration,
    limit: i64,
) -> anyhow::Result<Vec<ClaimedInboxEvent>> {
    anyhow::ensure!(
        !lease.is_zero() && limit > 0,
        "claim lease and limit must be positive"
    );
    let (providers, types): (Vec<String>, Vec<String>) = registry
        .contracts()
        .map(|(provider, kind)| (provider.to_owned(), kind.to_owned()))
        .unzip();
    if providers.is_empty() {
        return Ok(Vec::new());
    }
    let mut tx = pool.begin().await?;
    let expired = sqlx::query!(
        r#"SELECT id,attempts,max_attempts,claim_generation,lease_owner AS "lease_owner!"
           FROM integration_inbox_events
           WHERE processing_state='processing' AND lease_expires_at <= clock_timestamp()
             AND (provider,event_type) IN (SELECT * FROM unnest($1::text[],$2::text[]))
           ORDER BY lease_expires_at,id FOR UPDATE SKIP LOCKED LIMIT $3"#,
        &providers,
        &types,
        limit,
    )
    .fetch_all(&mut *tx)
    .await?;
    for event in expired {
        let exhausted = event.attempts >= event.max_attempts;
        let state = if exhausted { "quarantined" } else { "failed" };
        let reason = format!(
            "abandoned inbox claim after lease expiry (generation {}){}",
            event.claim_generation,
            if exhausted {
                "; attempt budget exhausted"
            } else {
                ""
            }
        );
        let quarantine_reason = exhausted.then_some(reason.as_str());
        let delay_ms = retry_backoff(event.attempts).as_millis() as f64;
        sqlx::query!(
            r#"UPDATE integration_inbox_events
               SET processing_state=$4,last_error=$5,quarantine_reason=$6,
                 next_attempt_at=clock_timestamp()+($7::double precision*interval '1 millisecond'),
                 lease_owner=NULL,lease_expires_at=NULL,claim_generation=claim_generation+1
               WHERE id=$1 AND lease_owner=$2 AND claim_generation=$3
                 AND processing_state='processing' AND lease_expires_at <= clock_timestamp()"#,
            event.id,
            event.lease_owner,
            event.claim_generation,
            state,
            reason,
            quarantine_reason,
            delay_ms,
        )
        .execute(&mut *tx)
        .await?;
    }
    let lease_ms = lease.as_millis() as f64;
    let rows = sqlx::query!(
        r#"WITH candidate AS (
             SELECT id FROM integration_inbox_events
             WHERE next_attempt_at <= clock_timestamp()
               AND processing_state IN ('received','failed') AND lease_owner IS NULL
               AND (provider,event_type) IN (SELECT * FROM unnest($4::text[],$5::text[]))
             ORDER BY received_at,id FOR UPDATE SKIP LOCKED LIMIT $3
           )
           UPDATE integration_inbox_events i
           SET processing_state='processing',lease_owner=$1,
               lease_expires_at=clock_timestamp()+($2::double precision*interval '1 millisecond'),
               attempts=i.attempts+1,claim_generation=i.claim_generation+1
           FROM candidate WHERE i.id=candidate.id
           RETURNING i.id,i.provider,i.provider_event_id,i.event_type,i.payload AS "payload!",
                     i.attempts,i.max_attempts,i.claim_generation"#,
        owner,
        lease_ms,
        limit,
        &providers,
        &types,
    )
    .fetch_all(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(rows
        .into_iter()
        .map(|row| ClaimedInboxEvent {
            id: row.id,
            provider: row.provider,
            provider_event_id: row.provider_event_id,
            event_type: row.event_type,
            payload: row.payload,
            attempts: row.attempts,
            max_attempts: row.max_attempts,
            claim_fence: InboxClaimFence::new(owner, row.claim_generation),
        })
        .collect())
}

/// Park a directly supplied unsupported claim. Normal claims filter it out.
pub async fn release_inbox_lease(
    pool: &PgPool,
    id: i64,
    fence: &InboxClaimFence,
) -> anyhow::Result<bool> {
    Ok(sqlx::query!(
        r#"UPDATE integration_inbox_events SET processing_state='received',lease_owner=NULL,lease_expires_at=NULL
           WHERE id=$1 AND lease_owner=$2 AND claim_generation=$3
             AND processing_state='processing' AND lease_expires_at > clock_timestamp()"#,
        id, fence.owner(), fence.generation(),
    ).execute(pool).await?.rows_affected() == 1)
}

/// Extend an inbox claim while its generation is still current and unexpired.
pub async fn renew_inbox_lease(
    pool: &PgPool,
    id: i64,
    fence: &InboxClaimFence,
    lease: std::time::Duration,
) -> anyhow::Result<bool> {
    let lease_ms = lease.as_millis() as f64;
    Ok(sqlx::query!(
        r#"UPDATE integration_inbox_events
           SET lease_expires_at = clock_timestamp() + ($3::double precision * interval '1 millisecond')
           WHERE id = $1 AND lease_owner = $2 AND claim_generation = $4
             AND processing_state = 'processing' AND lease_expires_at > clock_timestamp()"#,
        id,
        fence.owner(),
        lease_ms,
        fence.generation(),
    )
    .execute(pool)
    .await?
    .rows_affected()
        == 1)
}

/// Lock and validate an active inbox claim inside the provider effect's
/// transaction. Call this before applying authoritative state so a stale
/// handler rolls back instead of applying an effect after its lease expired.
pub async fn fence_inbox_event_effect(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    context: &InboxContext,
) -> anyhow::Result<()> {
    let row = sqlx::query!(
        r#"SELECT id FROM integration_inbox_events
           WHERE id = $1 AND processing_state = 'processing'
             AND lease_owner = $2 AND claim_generation = $3
             AND lease_expires_at > clock_timestamp()
           FOR UPDATE"#,
        context.inbox_id,
        context.claim_fence.owner(),
        context.claim_fence.generation(),
    )
    .fetch_optional(&mut **tx)
    .await?;
    anyhow::ensure!(row.is_some(), "inbox claim fence is stale or expired");
    Ok(())
}

/// Mark the event processed in the same transaction as a provider's effect.
/// The row lock remains held until the caller commits, fencing a concurrent
/// reclaimer from taking over between the effect and this transition.
pub async fn complete_inbox_event_in_transaction(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    context: &InboxContext,
) -> anyhow::Result<bool> {
    // The handler may have committed the effect and completion together. The
    // processor's post-handler completion then becomes a no-op for that same
    // claim generation, while older generations still fail below.
    let already_processed = sqlx::query!(
        r#"SELECT id FROM integration_inbox_events
           WHERE id = $1 AND processing_state = 'processed' AND claim_generation = $2
           FOR UPDATE"#,
        context.inbox_id,
        context.claim_fence.generation(),
    )
    .fetch_optional(&mut **tx)
    .await?;
    if already_processed.is_some() {
        return Ok(true);
    }

    fence_inbox_event_effect(tx, context).await?;
    let updated = sqlx::query!(
        r#"UPDATE integration_inbox_events
           SET processing_state = 'processed', processed_at = now(),
               payload_expires_at = clock_timestamp() + make_interval(days => $4),
               lease_owner = NULL, lease_expires_at = NULL, next_attempt_at = now()
           WHERE id = $1 AND lease_owner = $2 AND claim_generation = $3
             AND processing_state = 'processing' AND lease_expires_at > clock_timestamp()
           RETURNING id"#,
        context.inbox_id,
        context.claim_fence.owner(),
        context.claim_fence.generation(),
        INBOX_PAYLOAD_RETENTION_DAYS,
    )
    .fetch_optional(&mut **tx)
    .await?;
    anyhow::ensure!(
        updated.is_some(),
        "inbox claim fence expired before completion"
    );
    Ok(true)
}

/// Mark one claimed row processed. Provider adapters with database effects
/// should call [`complete_inbox_event_in_transaction`] in the same transaction
/// as those effects; this helper is for handlers whose effect is external or a
/// no-op and is fenced by the current lease generation.
pub async fn complete_inbox_event(pool: &PgPool, context: &InboxContext) -> anyhow::Result<bool> {
    let mut tx = pool.begin().await?;
    let completed = complete_inbox_event_in_transaction(&mut tx, context).await?;
    tx.commit().await?;
    Ok(completed)
}

/// Record a failed processing attempt: `failed` with backoff, or `quarantined`
/// when the failure is terminal or the attempt budget is spent. Quarantined
/// events wait for operational review and `requeue_inbox_event`.
pub async fn fail_inbox_event(
    pool: &PgPool,
    id: i64,
    fence: &InboxClaimFence,
    error: &str,
    retry: InboxRetry,
) -> anyhow::Result<Option<InboxProcessingState>> {
    let (terminal, delay) = match retry {
        InboxRetry::After(delay) => (false, delay),
        InboxRetry::Terminal => (true, std::time::Duration::ZERO),
    };
    let delay_ms = delay.as_millis() as f64;
    let row = sqlx::query!(
        r#"UPDATE integration_inbox_events
           SET processing_state = CASE
                 WHEN $3::boolean OR attempts >= max_attempts THEN 'quarantined'
                 ELSE 'failed' END,
             quarantine_reason = CASE
                 WHEN $3::boolean OR attempts >= max_attempts THEN $4
                 ELSE NULL END,
             last_error = $4,
             next_attempt_at = now() + ($5::double precision * interval '1 millisecond'),
             lease_owner = NULL, lease_expires_at = NULL
           WHERE id = $1 AND lease_owner = $2 AND processing_state = 'processing'
             AND lease_expires_at > clock_timestamp() AND claim_generation = $6
           RETURNING processing_state"#,
        id,
        fence.owner(),
        terminal,
        error,
        delay_ms,
        fence.generation(),
    )
    .fetch_optional(pool)
    .await?;
    Ok(row.map(|row| parse_state(&row.processing_state)))
}

/// Replay a quarantined event after operational review: the state machine
/// restarts with a fresh attempt budget and the same provider event identity.
pub async fn requeue_inbox_event(pool: &PgPool, id: i64) -> anyhow::Result<bool> {
    Ok(sqlx::query!(
        r#"UPDATE integration_inbox_events
           SET processing_state = 'received', attempts = 0, next_attempt_at = now(),
               quarantine_reason = NULL, last_error = NULL,
               lease_owner = NULL, lease_expires_at = NULL
           WHERE id = $1 AND processing_state = 'quarantined'"#,
        id,
    )
    .execute(pool)
    .await?
    .rows_affected()
        == 1)
}

/// Minimize a bounded batch of expired processed payloads. This is an explicit
/// maintenance operation, not a scheduler. Provider/event identity and processing
/// evidence remain, so a late duplicate is still acknowledged without replay.
pub async fn purge_processed_inbox_payloads(pool: &PgPool, limit: i64) -> anyhow::Result<u64> {
    anyhow::ensure!(limit > 0, "payload purge limit must be positive");
    Ok(sqlx::query!(
        r#"WITH expired AS (
             SELECT id FROM integration_inbox_events
             WHERE processing_state='processed' AND payload IS NOT NULL
               AND payload_expires_at <= clock_timestamp()
             ORDER BY payload_expires_at,id FOR UPDATE SKIP LOCKED LIMIT $1
           )
           UPDATE integration_inbox_events i
           SET payload=NULL,payload_purged_at=clock_timestamp()
           FROM expired WHERE i.id=expired.id"#,
        limit,
    )
    .execute(pool)
    .await?
    .rows_affected())
}

/// Current processing state and failure bookkeeping of one inbox row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InboxEventRecord {
    pub id: i64,
    pub provider: String,
    pub provider_event_id: String,
    pub processing_state: InboxProcessingState,
    pub attempts: i32,
    pub last_error: Option<String>,
    pub quarantine_reason: Option<String>,
}

/// Load one inbox row for inspection (operational review, tests).
pub async fn get_inbox_event(pool: &PgPool, id: i64) -> anyhow::Result<Option<InboxEventRecord>> {
    let row = sqlx::query!(
        r#"SELECT id, provider, provider_event_id, processing_state, attempts,
                  last_error, quarantine_reason
           FROM integration_inbox_events WHERE id = $1"#,
        id,
    )
    .fetch_optional(pool)
    .await?;
    Ok(row.map(|row| InboxEventRecord {
        id: row.id,
        provider: row.provider,
        provider_event_id: row.provider_event_id,
        processing_state: parse_state(&row.processing_state),
        attempts: row.attempts,
        last_error: row.last_error,
        quarantine_reason: row.quarantine_reason,
    }))
}

fn parse_state(text: &str) -> InboxProcessingState {
    match text {
        "processing" => InboxProcessingState::Processing,
        "processed" => InboxProcessingState::Processed,
        "failed" => InboxProcessingState::Failed,
        "quarantined" => InboxProcessingState::Quarantined,
        _ => InboxProcessingState::Received,
    }
}

/// Claim one bounded batch and process every claimed row through the typed
/// handler registry. Returns the number of claimed rows.
pub async fn process_inbox_batch(
    pool: &PgPool,
    registry: &InboxRegistry,
    owner: &str,
    lease: std::time::Duration,
    limit: i64,
) -> anyhow::Result<usize> {
    let claimed = claim_inbox_batch(pool, registry, owner, lease, limit).await?;
    let count = claimed.len();
    for event in &claimed {
        process_inbox_event(pool, registry, lease, event).await?;
    }
    Ok(count)
}

/// Process one claimed inbox row. An event type no handler is registered for
/// stays pending for a compatible deployment. Handlers must be idempotent and
/// order-safe: duplicate and out-of-order provider delivery is normal.
pub async fn process_inbox_event(
    pool: &PgPool,
    registry: &InboxRegistry,
    lease: std::time::Duration,
    event: &ClaimedInboxEvent,
) -> anyhow::Result<()> {
    let event_type = event.event_type.clone().unwrap_or_default();
    let Some(handler) = registry.handler(&event.provider, &event_type) else {
        release_inbox_lease(pool, event.id, &event.claim_fence).await?;
        return Ok(());
    };
    let context = InboxContext {
        inbox_id: event.id,
        provider: event.provider.clone(),
        provider_event_id: event.provider_event_id.clone(),
        event_type,
        claim_fence: event.claim_fence.clone(),
    };
    match crate::leased_handler::run(
        lease,
        || renew_inbox_lease(pool, event.id, &event.claim_fence, lease),
        invoke_handler(handler, context.clone(), event.payload.clone()),
    )
    .await?
    {
        Ok(()) => {
            if !complete_inbox_event(pool, &context).await? {
                tracing::warn!(
                    inbox_id = event.id,
                    provider = %event.provider,
                    "inbox completion rejected because the lease is no longer owned"
                );
            }
        }
        Err(InboxHandlerError::Transient(message)) => {
            let attempts = event.attempts;
            tracing::warn!(
                inbox_id = event.id,
                provider = %event.provider,
                attempts,
                error = %message,
                "inbox processing failed; will retry"
            );
            fail_inbox_event(
                pool,
                event.id,
                &event.claim_fence,
                &message,
                InboxRetry::After(retry_backoff(attempts)),
            )
            .await?;
        }
        Err(InboxHandlerError::Terminal(message)) => {
            tracing::error!(
                inbox_id = event.id,
                provider = %event.provider,
                attempts = event.attempts,
                error = %message,
                "inbox processing failed terminally"
            );
            fail_inbox_event(
                pool,
                event.id,
                &event.claim_fence,
                &message,
                InboxRetry::Terminal,
            )
            .await?;
        }
    }
    Ok(())
}

/// Inline ownership lets processor cancellation drop a handler's transaction;
/// panic isolation does not create a detached child task.
async fn invoke_handler(
    handler: &ErasedInboxHandler,
    context: InboxContext,
    payload: Value,
) -> Result<(), InboxHandlerError> {
    let event_id = context.provider_event_id.clone();
    match AssertUnwindSafe(async move { handler(context, payload).await })
        .catch_unwind()
        .await
    {
        Ok(result) => result,
        Err(panic) => Err(InboxHandlerError::Transient(format!(
            "handler for {event_id} panicked: {}",
            crate::leased_handler::panic_message(panic)
        ))),
    }
}

/// Inbox events grouped by state, for operational review and the quarantine
/// backlog.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct InboxStateCounts {
    pub received: i64,
    pub processing: i64,
    pub processed: i64,
    pub failed: i64,
    pub quarantined: i64,
}

pub async fn inbox_state_counts(pool: &PgPool) -> anyhow::Result<InboxStateCounts> {
    let rows = sqlx::query!(
        r#"SELECT processing_state, count(*) AS "count!"
           FROM integration_inbox_events GROUP BY processing_state"#,
    )
    .fetch_all(pool)
    .await?;
    let mut counts = InboxStateCounts::default();
    for row in rows {
        match parse_state(&row.processing_state) {
            InboxProcessingState::Received => counts.received = row.count,
            InboxProcessingState::Processing => counts.processing = row.count,
            InboxProcessingState::Processed => counts.processed = row.count,
            InboxProcessingState::Failed => counts.failed = row.count,
            InboxProcessingState::Quarantined => counts.quarantined = row.count,
        }
    }
    Ok(counts)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_state_defaults_unrecognized_to_received() {
        assert_eq!(parse_state("processed"), InboxProcessingState::Processed);
        assert_eq!(parse_state("nonsense"), InboxProcessingState::Received);
    }
}
