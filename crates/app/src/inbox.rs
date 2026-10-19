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
use serde_json::Value;
use sqlx::{PgPool, Postgres, Transaction};

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

/// Atomically claim a bounded batch of due inbox rows: `received` and `failed`
/// rows that are due, plus `processing` rows whose lease expired (the worker
/// crashed mid-processing). The claim spends one processing attempt.
pub async fn claim_inbox_batch(
    pool: &PgPool,
    owner: &str,
    lease: std::time::Duration,
    limit: i64,
) -> anyhow::Result<Vec<ClaimedInboxEvent>> {
    let lease_ms = lease.as_millis() as f64;
    let rows = sqlx::query!(
        r#"WITH candidate AS (
             SELECT id
             FROM integration_inbox_events
             WHERE next_attempt_at <= now()
               AND (processing_state IN ('received', 'failed')
                 OR (processing_state = 'processing' AND lease_expires_at < clock_timestamp()))
             ORDER BY received_at
             FOR UPDATE SKIP LOCKED
             LIMIT $3
           )
           UPDATE integration_inbox_events i
           SET processing_state = 'processing',
               lease_owner = $1,
               lease_expires_at = clock_timestamp() + ($2::double precision * interval '1 millisecond'),
               attempts = i.attempts + 1,
               claim_generation = i.claim_generation + 1
           FROM candidate
           WHERE i.id = candidate.id
           RETURNING i.id, i.provider, i.provider_event_id, i.event_type, i.payload,
                     i.attempts, i.max_attempts, i.claim_generation"#,
        owner,
        lease_ms,
        limit,
    )
    .fetch_all(pool)
    .await?;
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
               lease_owner = NULL, lease_expires_at = NULL, next_attempt_at = now()
           WHERE id = $1 AND lease_owner = $2 AND claim_generation = $3
             AND processing_state = 'processing' AND lease_expires_at > clock_timestamp()
           RETURNING id"#,
        context.inbox_id,
        context.claim_fence.owner(),
        context.claim_fence.generation(),
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
    let claimed = claim_inbox_batch(pool, owner, lease, limit).await?;
    let count = claimed.len();
    for event in &claimed {
        process_inbox_event(pool, registry, lease, event).await?;
    }
    Ok(count)
}

/// Process one claimed inbox row. An event type no handler is registered for
/// is quarantined terminally — visible for review and replay, never silently
/// discarded. Handlers must be idempotent and order-safe: duplicate and
/// out-of-order provider delivery is normal.
pub async fn process_inbox_event(
    pool: &PgPool,
    registry: &InboxRegistry,
    lease: std::time::Duration,
    event: &ClaimedInboxEvent,
) -> anyhow::Result<()> {
    let event_type = event.event_type.clone().unwrap_or_default();
    let Some(handler) = registry.handler(&event.provider, &event_type) else {
        fail_inbox_event(
            pool,
            event.id,
            &event.claim_fence,
            &format!(
                "no handler registered for {}/{event_type}; quarantined terminally",
                event.provider
            ),
            InboxRetry::Terminal,
        )
        .await?;
        return Ok(());
    };
    let context = InboxContext {
        inbox_id: event.id,
        provider: event.provider.clone(),
        provider_event_id: event.provider_event_id.clone(),
        event_type,
        claim_fence: event.claim_fence.clone(),
    };
    if !renew_inbox_lease(pool, event.id, &event.claim_fence, lease).await? {
        return Ok(());
    }
    match invoke_handler(handler, context.clone(), event.payload.clone()).await {
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

/// Run a handler on its own task so a panic becomes this event's failure
/// instead of tearing down the processor.
async fn invoke_handler(
    handler: &ErasedInboxHandler,
    context: InboxContext,
    payload: Value,
) -> Result<(), InboxHandlerError> {
    let work = handler(context.clone(), payload);
    match tokio::spawn(work).await {
        Ok(result) => result,
        Err(error) if error.is_panic() => {
            let panic = error.into_panic();
            let message = panic
                .downcast_ref::<&str>()
                .map(|message| (*message).to_owned())
                .or_else(|| panic.downcast_ref::<String>().cloned())
                .unwrap_or_else(|| "non-string panic payload".to_owned());
            Err(InboxHandlerError::Transient(format!(
                "handler for {} panicked: {message}",
                context.provider_event_id
            )))
        }
        Err(error) => Err(InboxHandlerError::Transient(format!(
            "handler for {} was cancelled: {error}",
            context.provider_event_id
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
