#![allow(
    clippy::expect_used,
    clippy::panic,
    clippy::panic_in_result_fn,
    clippy::unwrap_used,
    clippy::string_slice
)]
//! Handoff 04a integration inbox semantics against a real PostgreSQL:
//! signature-before-insert, provider-scoped deduplication, commit-before-ack,
//! the processing state machine, retry/backoff and quarantine/replay. Skipped
//! unless `DATABASE_URL` points at a reachable PostgreSQL.

use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use std::time::Duration;

use anyhow::Result;
use deepref_app::inbox::{
    InboxReceiveError, ReceiveOutcome, ReceiveWebhookEvent, claim_inbox_batch,
    complete_inbox_event, complete_inbox_event_in_transaction, get_inbox_event, inbox_state_counts,
    process_inbox_batch, receive_webhook_event, requeue_inbox_event,
};
use deepref_application::inbox::{
    HmacSha256Verifier, InboxClaimFence, InboxContext, InboxHandlerError, InboxProcessingState,
    InboxRegistry, TypedInboxHandler,
};
use deepref_application::workflows::security::sign_payload;
use serde::Deserialize;
use serde_json::json;
use sqlx::PgPool;

const SECRET: &str = "inbox-test-secret";

async fn database() -> Option<PgPool> {
    deepref_app::test_isolation::isolated_pool_from_env("DATABASE_URL", 8).await
}

#[derive(Debug, Deserialize)]
struct ProviderEvent {
    #[allow(dead_code)]
    id: String,
}

#[derive(Default)]
struct Behavior {
    calls: AtomicUsize,
    transient_failures: AtomicUsize,
}

fn handler(behavior: Arc<Behavior>) -> TypedInboxHandler<ProviderEvent> {
    Arc::new(move |_message: ProviderEvent, _context: InboxContext| {
        let behavior = Arc::clone(&behavior);
        Box::pin(async move {
            behavior.calls.fetch_add(1, Ordering::SeqCst);
            if behavior.transient_failures.load(Ordering::SeqCst) > 0 {
                behavior.transient_failures.fetch_sub(1, Ordering::SeqCst);
                return Err(InboxHandlerError::Transient(
                    "provider effect not committed".to_owned(),
                ));
            }
            Ok(())
        })
    })
}

fn registry_with(provider: &str, event_type: &str, behavior: Arc<Behavior>) -> InboxRegistry {
    let mut registry = InboxRegistry::new();
    registry
        .register(provider, event_type, handler(behavior))
        .expect("registers");
    registry
}

fn claim_registry() -> InboxRegistry {
    registry_with("workos", "user.created", Arc::new(Behavior::default()))
}

/// One signed provider delivery, owning its body and signature.
struct SignedDelivery {
    event_id: String,
    event_type: String,
    body: Vec<u8>,
    signature: String,
}

impl SignedDelivery {
    fn new(event_id: &str, event_type: &str, body: &[u8]) -> Self {
        Self {
            event_id: event_id.to_owned(),
            event_type: event_type.to_owned(),
            body: body.to_vec(),
            signature: sign_payload(SECRET, body),
        }
    }

    fn event(&self) -> ReceiveWebhookEvent<'_> {
        ReceiveWebhookEvent {
            provider: "workos",
            provider_event_id: &self.event_id,
            event_type: Some(&self.event_type),
            body: &self.body,
            signature: Some(&self.signature),
        }
    }
}

async fn rows_with_provider(pool: &PgPool) -> i64 {
    sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM integration_inbox_events WHERE provider='workos'",
    )
    .fetch_one(pool)
    .await
    .expect("inbox count")
}

async fn accepted_id(
    pool: &PgPool,
    verifier: &HmacSha256Verifier,
    delivery: &SignedDelivery,
) -> i64 {
    match receive_webhook_event(pool, verifier, &delivery.event())
        .await
        .expect("receive")
    {
        ReceiveOutcome::Accepted(id) => id,
        ReceiveOutcome::Duplicate => panic!("first delivery is new"),
    }
}

/// Simulate elapsed retry backoff.
async fn force_due(pool: &PgPool, id: i64) {
    sqlx::query("UPDATE integration_inbox_events SET next_attempt_at=now() WHERE id=$1")
        .bind(id)
        .execute(pool)
        .await
        .expect("event due");
}

fn inbox_context(event: &deepref_app::inbox::ClaimedInboxEvent) -> InboxContext {
    InboxContext {
        inbox_id: event.id,
        provider: event.provider.clone(),
        provider_event_id: event.provider_event_id.clone(),
        event_type: event.event_type.clone(),
        claim_fence: event.claim_fence.clone(),
    }
}

#[tokio::test]
async fn a_bad_signature_is_rejected_before_any_insert() -> Result<()> {
    let Some(pool) = database().await else {
        return Ok(());
    };
    let verifier = HmacSha256Verifier::new(SECRET);
    let body = br#"{"id":"evt_bad"}"#;

    let tampered_secret = ReceiveWebhookEvent {
        provider: "workos",
        provider_event_id: "evt_bad",
        event_type: Some("user.created"),
        body,
        signature: Some(&sign_payload("wrong-secret", body)),
    };
    assert!(matches!(
        receive_webhook_event(&pool, &verifier, &tampered_secret).await,
        Err(InboxReceiveError::Signature(_))
    ));

    let unsigned = ReceiveWebhookEvent {
        provider: "workos",
        provider_event_id: "evt_bad",
        event_type: Some("user.created"),
        body,
        signature: None,
    };
    assert!(matches!(
        receive_webhook_event(&pool, &verifier, &unsigned).await,
        Err(InboxReceiveError::Signature(_))
    ));

    // A valid signature over a different body is still rejected.
    let tampered_body = ReceiveWebhookEvent {
        provider: "workos",
        provider_event_id: "evt_bad",
        event_type: Some("user.created"),
        body: br#"{"id":"evt_other"}"#,
        signature: Some(&sign_payload(SECRET, body)),
    };
    assert!(matches!(
        receive_webhook_event(&pool, &verifier, &tampered_body).await,
        Err(InboxReceiveError::Signature(_))
    ));
    assert_eq!(rows_with_provider(&pool).await, 0, "no row for a rejection");
    Ok(())
}

#[tokio::test]
async fn a_duplicate_event_is_a_committed_no_op() -> Result<()> {
    let Some(pool) = database().await else {
        return Ok(());
    };
    let verifier = HmacSha256Verifier::new(SECRET);
    let delivery = SignedDelivery::new("evt_dup", "user.created", br#"{"id":"evt_dup"}"#);

    let id = accepted_id(&pool, &verifier, &delivery).await;
    assert_eq!(rows_with_provider(&pool).await, 1);

    let registry = registry_with("workos", "user.created", Arc::new(Behavior::default()));
    process_inbox_batch(&pool, &registry, "inbox-owner", Duration::from_secs(30), 8).await?;
    let record = get_inbox_event(&pool, id).await?.expect("inbox row");
    assert_eq!(record.processing_state, InboxProcessingState::Processed);

    // A duplicate after processing acknowledges without reprocessing.
    let duplicate = receive_webhook_event(&pool, &verifier, &delivery.event()).await?;
    assert_eq!(duplicate, ReceiveOutcome::Duplicate);
    assert_eq!(rows_with_provider(&pool).await, 1);
    let record = get_inbox_event(&pool, id).await?.expect("inbox row");
    assert_eq!(record.processing_state, InboxProcessingState::Processed);
    assert_eq!(record.attempts, 1, "the duplicate spent no attempt");
    Ok(())
}

#[tokio::test]
async fn concurrent_duplicate_webhooks_commit_one_inbox_row() -> Result<()> {
    let Some(pool) = database().await else {
        return Ok(());
    };
    let verifier = HmacSha256Verifier::new(SECRET);
    let delivery = SignedDelivery::new(
        "evt_concurrent_duplicate",
        "user.created",
        br#"{"id":"evt_concurrent_duplicate"}"#,
    );
    let first_event = delivery.event();
    let second_event = delivery.event();

    let (first, second) = tokio::join!(
        receive_webhook_event(&pool, &verifier, &first_event),
        receive_webhook_event(&pool, &verifier, &second_event),
    );
    let outcomes = [first?, second?];
    assert_eq!(
        outcomes
            .iter()
            .filter(|outcome| matches!(outcome, ReceiveOutcome::Accepted(_)))
            .count(),
        1
    );
    assert_eq!(
        outcomes
            .iter()
            .filter(|outcome| matches!(outcome, ReceiveOutcome::Duplicate))
            .count(),
        1
    );
    assert_eq!(rows_with_provider(&pool).await, 1);
    Ok(())
}

#[tokio::test]
async fn the_ack_follows_the_commit_and_a_failed_insert_is_not_acknowledged() -> Result<()> {
    let Some(pool) = database().await else {
        return Ok(());
    };
    let verifier = HmacSha256Verifier::new(SECRET);
    let delivery = SignedDelivery::new("evt_commit", "user.created", br#"{"id":"evt_commit"}"#);
    let id = accepted_id(&pool, &verifier, &delivery).await;
    // The row is durable when the call returned: only now may the caller 2xx.
    assert!(get_inbox_event(&pool, id).await?.is_some());

    // A rejected insert (the dedup key's length check) is not acknowledged.
    let long_id = "x".repeat(256);
    let long_body = json!({"id": long_id}).to_string();
    let long_delivery = SignedDelivery::new(&long_id, "user.created", long_body.as_bytes());
    assert!(matches!(
        receive_webhook_event(&pool, &verifier, &long_delivery.event()).await,
        Err(InboxReceiveError::Database(_))
    ));
    assert_eq!(
        rows_with_provider(&pool).await,
        1,
        "the failed commit inserted nothing"
    );
    Ok(())
}

#[tokio::test]
async fn processing_retries_with_backoff_then_quarantines_and_can_be_replayed() -> Result<()> {
    let Some(pool) = database().await else {
        return Ok(());
    };
    let verifier = HmacSha256Verifier::new(SECRET);
    let delivery = SignedDelivery::new("evt_retry", "user.created", br#"{"id":"evt_retry"}"#);
    let id = accepted_id(&pool, &verifier, &delivery).await;
    sqlx::query("UPDATE integration_inbox_events SET max_attempts=2 WHERE id=$1")
        .bind(id)
        .execute(&pool)
        .await?;

    let behavior = Arc::new(Behavior {
        transient_failures: AtomicUsize::new(2),
        ..Behavior::default()
    });
    let registry = registry_with("workos", "user.created", Arc::clone(&behavior));

    process_inbox_batch(&pool, &registry, "inbox-owner", Duration::from_secs(30), 8).await?;
    let record = get_inbox_event(&pool, id).await?.expect("inbox row");
    assert_eq!(record.processing_state, InboxProcessingState::Failed);
    assert_eq!(record.attempts, 1);
    assert!(record.last_error.is_some(), "the failure is diagnosable");

    force_due(&pool, id).await;
    process_inbox_batch(&pool, &registry, "inbox-owner", Duration::from_secs(30), 8).await?;
    let record = get_inbox_event(&pool, id).await?.expect("inbox row");
    assert_eq!(record.processing_state, InboxProcessingState::Quarantined);
    assert_eq!(record.attempts, 2);
    let reason = record
        .quarantine_reason
        .expect("quarantine reason preserved");
    assert!(
        reason.contains("provider effect not committed"),
        "got: {reason}"
    );

    let counts = inbox_state_counts(&pool).await?;
    assert_eq!(counts.quarantined, 1);

    // Operational replay: fresh attempt budget, same provider event identity.
    behavior.transient_failures.store(0, Ordering::SeqCst);
    assert!(requeue_inbox_event(&pool, id).await?);
    let record = get_inbox_event(&pool, id).await?.expect("inbox row");
    assert_eq!(record.processing_state, InboxProcessingState::Received);
    assert_eq!(record.attempts, 0);
    process_inbox_batch(&pool, &registry, "inbox-owner", Duration::from_secs(30), 8).await?;
    let record = get_inbox_event(&pool, id).await?.expect("inbox row");
    assert_eq!(record.processing_state, InboxProcessingState::Processed);
    assert_eq!(behavior.calls.load(Ordering::SeqCst), 3);
    Ok(())
}

#[tokio::test]
async fn an_unhandled_event_type_stays_received() -> Result<()> {
    let Some(pool) = database().await else {
        return Ok(());
    };
    let verifier = HmacSha256Verifier::new(SECRET);
    let delivery = SignedDelivery::new("evt_mystery", "mystery.event", br#"{"id":"evt_mystery"}"#);
    let id = accepted_id(&pool, &verifier, &delivery).await;
    let registry = registry_with("workos", "user.created", Arc::new(Behavior::default()));

    process_inbox_batch(&pool, &registry, "inbox-owner", Duration::from_secs(30), 8).await?;
    let record = get_inbox_event(&pool, id).await?.expect("inbox row");
    assert_eq!(record.processing_state, InboxProcessingState::Received);
    assert_eq!(record.attempts, 0);
    assert!(record.quarantine_reason.is_none());
    Ok(())
}

#[tokio::test]
async fn an_expired_processing_lease_is_reclaimed() -> Result<()> {
    let Some(pool) = database().await else {
        return Ok(());
    };
    let verifier = HmacSha256Verifier::new(SECRET);
    let delivery = SignedDelivery::new("evt_lease", "user.created", br#"{"id":"evt_lease"}"#);
    let id = accepted_id(&pool, &verifier, &delivery).await;

    let claimed = claim_inbox_batch(
        &pool,
        &claim_registry(),
        "owner-a",
        Duration::from_secs(2),
        8,
    )
    .await?;
    assert_eq!(claimed.len(), 1);
    assert_eq!(claimed[0].attempts, 1);
    let record = get_inbox_event(&pool, id).await?.expect("inbox row");
    assert_eq!(record.processing_state, InboxProcessingState::Processing);

    let second = claim_inbox_batch(
        &pool,
        &claim_registry(),
        "owner-b",
        Duration::from_secs(30),
        8,
    )
    .await?;
    assert!(second.is_empty(), "an active lease is exclusive");

    tokio::time::sleep(Duration::from_millis(2300)).await;
    assert!(
        claim_inbox_batch(
            &pool,
            &claim_registry(),
            "recovery",
            Duration::from_secs(30),
            8
        )
        .await?
        .is_empty()
    );
    force_due(&pool, id).await;
    let reclaimed = claim_inbox_batch(
        &pool,
        &claim_registry(),
        "owner-b",
        Duration::from_secs(30),
        8,
    )
    .await?;
    assert_eq!(reclaimed.len(), 1, "the expired lease is reclaimed");
    assert_eq!(reclaimed[0].attempts, 2, "reclaim spends an attempt");

    // Completion takes effect only for the current claim generation.
    let stranger = InboxClaimFence::new("someone-else", reclaimed[0].claim_fence.generation());
    let mut stranger_context = inbox_context(&reclaimed[0]);
    stranger_context.claim_fence = stranger;
    assert!(
        complete_inbox_event(&pool, &stranger_context)
            .await
            .is_err()
    );
    assert!(complete_inbox_event(&pool, &inbox_context(&reclaimed[0])).await?);
    let record = get_inbox_event(&pool, id).await?.expect("inbox row");
    assert_eq!(record.processing_state, InboxProcessingState::Processed);
    Ok(())
}

#[tokio::test]
async fn stale_inbox_generation_cannot_commit_effect_or_completion() -> Result<()> {
    let Some(pool) = database().await else {
        return Ok(());
    };
    let verifier = HmacSha256Verifier::new(SECRET);
    let delivery = SignedDelivery::new("evt_stale", "user.created", br#"{"id":"evt_stale"}"#);
    let id = accepted_id(&pool, &verifier, &delivery).await;
    let old_claim = claim_inbox_batch(
        &pool,
        &claim_registry(),
        "same-owner",
        Duration::from_secs(30),
        8,
    )
    .await?;
    assert_eq!(old_claim.len(), 1);
    sqlx::query(
        "UPDATE integration_inbox_events SET lease_expires_at = clock_timestamp() - interval '1 second' WHERE id=$1",
    )
    .bind(id)
    .execute(&pool)
    .await?;
    assert!(
        claim_inbox_batch(
            &pool,
            &claim_registry(),
            "recovery",
            Duration::from_secs(30),
            8
        )
        .await?
        .is_empty()
    );
    force_due(&pool, id).await;
    let new_claim = claim_inbox_batch(
        &pool,
        &claim_registry(),
        "same-owner",
        Duration::from_secs(30),
        8,
    )
    .await?;
    assert_eq!(new_claim.len(), 1);
    assert!(new_claim[0].claim_fence.generation() > old_claim[0].claim_fence.generation());

    let project_id = uuid::Uuid::new_v4();
    let mut stale_tx = pool.begin().await?;
    sqlx::query("INSERT INTO projects (id,name) VALUES ($1,'stale inbox effect')")
        .bind(project_id)
        .execute(&mut *stale_tx)
        .await?;
    assert!(
        complete_inbox_event_in_transaction(&mut stale_tx, &inbox_context(&old_claim[0]),)
            .await
            .is_err()
    );
    stale_tx.rollback().await?;
    assert!(
        sqlx::query("SELECT id FROM projects WHERE id=$1")
            .bind(project_id)
            .fetch_optional(&pool)
            .await?
            .is_none()
    );
    let record = get_inbox_event(&pool, id).await?.expect("inbox row");
    assert_eq!(record.processing_state, InboxProcessingState::Processing);

    assert!(complete_inbox_event(&pool, &inbox_context(&new_claim[0])).await?);
    let record = get_inbox_event(&pool, id).await?.expect("inbox row");
    assert_eq!(record.processing_state, InboxProcessingState::Processed);
    Ok(())
}

#[tokio::test]
async fn abandoned_inbox_attempts_back_off_then_quarantine() -> Result<()> {
    let Some(pool) = database().await else {
        return Ok(());
    };
    let verifier = HmacSha256Verifier::new(SECRET);
    let delivery = SignedDelivery::new("evt_crashes", "user.created", br#"{"id":"evt_crashes"}"#);
    let id = accepted_id(&pool, &verifier, &delivery).await;
    sqlx::query("UPDATE integration_inbox_events SET max_attempts=2 WHERE id=$1")
        .bind(id)
        .execute(&pool)
        .await?;
    let registry = claim_registry();
    for attempt in 1..=2 {
        let claimed = claim_inbox_batch(
            &pool,
            &registry,
            "crashed-worker",
            Duration::from_secs(60),
            1,
        )
        .await?;
        assert_eq!(claimed.len(), 1);
        assert_eq!(claimed[0].attempts, attempt);
        sqlx::query("UPDATE integration_inbox_events SET lease_expires_at=clock_timestamp()-interval '1 second' WHERE id=$1").bind(id).execute(&pool).await?;
        assert!(
            claim_inbox_batch(&pool, &registry, "recovery", Duration::from_secs(60), 1)
                .await?
                .is_empty()
        );
        let row = get_inbox_event(&pool, id).await?.expect("row");
        assert_eq!(row.attempts, attempt);
        assert!(
            row.last_error
                .as_deref()
                .is_some_and(|e| e.contains("abandoned inbox claim"))
        );
        if attempt == 1 {
            assert_eq!(row.processing_state, InboxProcessingState::Failed);
            let delayed: bool = sqlx::query_scalar(
                "SELECT next_attempt_at>clock_timestamp() FROM integration_inbox_events WHERE id=$1",
            )
            .bind(id)
            .fetch_one(&pool)
            .await?;
            assert!(delayed);
            assert!(
                claim_inbox_batch(&pool, &registry, "too-early", Duration::from_secs(60), 1)
                    .await?
                    .is_empty()
            );
            force_due(&pool, id).await;
        }
    }
    let row = get_inbox_event(&pool, id).await?.expect("row");
    assert_eq!(row.processing_state, InboxProcessingState::Quarantined);
    assert!(
        row.quarantine_reason
            .is_some_and(|e| e.contains("attempt budget exhausted"))
    );
    Ok(())
}

#[tokio::test]
async fn an_empty_registry_shares_the_database_with_a_provider_registry() -> Result<()> {
    let Some(pool) = database().await else {
        return Ok(());
    };
    let verifier = HmacSha256Verifier::new(SECRET);
    let delivery = SignedDelivery::new(
        "evt_new_provider",
        "user.created",
        br#"{"id":"evt_new_provider"}"#,
    );
    let id = accepted_id(&pool, &verifier, &delivery).await;
    assert_eq!(
        process_inbox_batch(
            &pool,
            &InboxRegistry::new(),
            "old-worker",
            Duration::from_secs(60),
            8
        )
        .await?,
        0
    );
    let row = get_inbox_event(&pool, id).await?.expect("row");
    assert_eq!(row.processing_state, InboxProcessingState::Received);
    assert_eq!(row.attempts, 0);
    assert_eq!(
        process_inbox_batch(
            &pool,
            &claim_registry(),
            "provider-worker",
            Duration::from_secs(60),
            8
        )
        .await?,
        1
    );
    assert_eq!(
        get_inbox_event(&pool, id)
            .await?
            .expect("row")
            .processing_state,
        InboxProcessingState::Processed
    );
    Ok(())
}

#[tokio::test]
async fn receipt_rejects_payloads_above_256_kib_with_a_typed_error() -> Result<()> {
    let Some(pool) = database().await else {
        return Ok(());
    };
    let verifier = HmacSha256Verifier::new(SECRET);
    let body = vec![b' '; deepref_app::inbox::MAX_INBOX_PAYLOAD_BYTES + 1];
    let delivery = SignedDelivery::new("evt_large", "user.created", &body);
    assert!(matches!(
        receive_webhook_event(&pool, &verifier, &delivery.event()).await,
        Err(InboxReceiveError::PayloadTooLarge { size, limit })
            if size == limit + 1 && limit == 256 * 1024
    ));
    assert_eq!(rows_with_provider(&pool).await, 0);
    Ok(())
}

#[tokio::test]
async fn payload_purge_keeps_dedup_identity_and_unprocessed_payloads() -> Result<()> {
    let Some(pool) = database().await else {
        return Ok(());
    };
    let verifier = HmacSha256Verifier::new(SECRET);
    let processed =
        SignedDelivery::new("evt_retained", "user.created", br#"{"id":"evt_retained"}"#);
    let processed_id = accepted_id(&pool, &verifier, &processed).await;
    process_inbox_batch(
        &pool,
        &claim_registry(),
        "worker",
        Duration::from_secs(60),
        8,
    )
    .await?;
    let pending = SignedDelivery::new("evt_pending", "user.created", br#"{"id":"evt_pending"}"#);
    let pending_id = accepted_id(&pool, &verifier, &pending).await;
    let retention: bool = sqlx::query_scalar(
        "SELECT payload_expires_at >= processed_at+interval '29 days' FROM integration_inbox_events WHERE id=$1",
    )
    .bind(processed_id)
    .fetch_one(&pool)
    .await?;
    assert!(retention);
    assert_eq!(
        deepref_app::inbox::purge_processed_inbox_payloads(&pool, 8).await?,
        0
    );
    sqlx::query("UPDATE integration_inbox_events SET payload_expires_at=clock_timestamp()-interval '1 day' WHERE id=$1").bind(processed_id).execute(&pool).await?;
    assert_eq!(
        deepref_app::inbox::purge_processed_inbox_payloads(&pool, 8).await?,
        1
    );
    let row: (Option<serde_json::Value>, bool) = sqlx::query_as(
        "SELECT payload,payload_purged_at IS NOT NULL FROM integration_inbox_events WHERE id=$1",
    )
    .bind(processed_id)
    .fetch_one(&pool)
    .await?;
    assert_eq!(row, (None, true));
    let retained: bool =
        sqlx::query_scalar("SELECT payload IS NOT NULL FROM integration_inbox_events WHERE id=$1")
            .bind(pending_id)
            .fetch_one(&pool)
            .await?;
    assert!(retained);
    assert_eq!(
        receive_webhook_event(&pool, &verifier, &processed.event()).await?,
        ReceiveOutcome::Duplicate
    );
    assert_eq!(rows_with_provider(&pool).await, 2);
    Ok(())
}

#[tokio::test]
async fn receipt_rejects_missing_or_empty_event_types() -> Result<()> {
    let Some(pool) = database().await else {
        return Ok(());
    };
    let verifier = HmacSha256Verifier::new(SECRET);
    let delivery = SignedDelivery::new(
        "evt_without_type",
        "user.created",
        br#"{"id":"evt_without_type"}"#,
    );
    for kind in [None, Some(""), Some(" "), Some(" user.created")] {
        let mut event = delivery.event();
        event.event_type = kind;
        assert!(matches!(
            receive_webhook_event(&pool, &verifier, &event).await,
            Err(InboxReceiveError::MissingEventType)
        ));
    }
    assert_eq!(rows_with_provider(&pool).await, 0);
    Ok(())
}
