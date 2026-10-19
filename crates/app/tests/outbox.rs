#![allow(
    clippy::expect_used,
    clippy::panic,
    clippy::panic_in_result_fn,
    clippy::unwrap_used,
    clippy::string_slice
)]
//! Handoff 04a outbox semantics against a real PostgreSQL: transactional
//! insert, exclusive claims, receipt-gated delivery, fan-out, per-consumer
//! retry/quarantine and the pilot message path. Skipped unless `DATABASE_URL`
//! points at a reachable PostgreSQL.

use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use std::time::Duration;

use anyhow::Result;
use chrono::{DateTime, Utc};
use deepref_app::outbox::{
    dispatch_outbox_batch, insert_outbox_event, list_outbox_receipts, pilot_outbox_registry,
    record_outbox_consumer_failure, record_outbox_receipt, replay_outbox_event,
    settle_outbox_event,
};
use deepref_application::outbox::{
    DeliveryContext, NewOutboxEvent, OutboxClaimFence, OutboxHandlerError, OutboxMessage,
    OutboxRegistry, OutboxRetry, TypedOutboxHandler,
};
use deepref_domain::events::{
    EntityType, EventEnvelope, MetricsRecomputeRequested, SUBJECT_METRICS_RECOMPUTE_REQUESTED,
};
use deepref_domain::{JobKind, ProjectId};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sqlx::{PgPool, Row};
use uuid::Uuid;

async fn database() -> Option<PgPool> {
    deepref_app::test_isolation::isolated_pool_from_env("DATABASE_URL", 8).await
}

async fn project(pool: &PgPool) -> Uuid {
    let project_id = Uuid::new_v4();
    sqlx::query("INSERT INTO projects (id,name) VALUES ($1,'outbox fixture')")
        .bind(project_id)
        .execute(pool)
        .await
        .expect("project fixture inserts");
    project_id
}

/// Single-consumer contract.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct Ping {
    note: String,
}

impl OutboxMessage for Ping {
    const TYPE: &'static str = "test.ping";
    const VERSION: u32 = 1;
    const CONSUMERS: &'static [&'static str] = &["alpha"];
}

/// Two-consumer contract: the fan-out case.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct Fanout {
    note: String,
}

impl OutboxMessage for Fanout {
    const TYPE: &'static str = "test.fanout";
    const VERSION: u32 = 1;
    const CONSUMERS: &'static [&'static str] = &["alpha", "beta"];
}

/// What the test handler does per invocation. The effect is a durable job
/// enqueue keyed by (event, consumer), so a redelivery is a committed no-op.
#[derive(Default)]
struct Behavior {
    calls: AtomicUsize,
    transient_failures: AtomicUsize,
    terminal_failures: AtomicUsize,
    crash_after_effect: AtomicUsize,
    success_without_receipt: AtomicUsize,
}

impl Behavior {
    fn with_transient_failures(count: usize) -> Self {
        Self {
            transient_failures: AtomicUsize::new(count),
            ..Self::default()
        }
    }
}

fn enqueue_handler<T: OutboxMessage>(
    pool: PgPool,
    project_id: Uuid,
    behavior: Arc<Behavior>,
) -> TypedOutboxHandler<T> {
    Arc::new(move |_message: T, context: DeliveryContext| {
        let pool = pool.clone();
        let behavior = Arc::clone(&behavior);
        Box::pin(async move {
            let call = behavior.calls.fetch_add(1, Ordering::SeqCst) + 1;
            if behavior.transient_failures.load(Ordering::SeqCst) > 0 {
                behavior.transient_failures.fetch_sub(1, Ordering::SeqCst);
                return Err(OutboxHandlerError::Transient(format!(
                    "transient failure on call {call}"
                )));
            }
            if behavior.terminal_failures.load(Ordering::SeqCst) > 0 {
                behavior.terminal_failures.fetch_sub(1, Ordering::SeqCst);
                return Err(OutboxHandlerError::Terminal(format!(
                    "terminal failure on call {call}"
                )));
            }
            let mut tx = pool
                .begin()
                .await
                .map_err(|error| OutboxHandlerError::Transient(error.to_string()))?;
            let dedupe_key = format!("outbox-test:{}:{}", context.event_id, context.consumer);
            deepref_app::enqueue_job(
                &mut tx,
                &deepref_app::job(
                    Uuid::new_v4(),
                    ProjectId::new(project_id),
                    JobKind::RecomputeMetrics,
                    json!({"project_id": project_id, "revision": 0}),
                    dedupe_key,
                ),
            )
            .await
            .map_err(|error| OutboxHandlerError::Transient(error.to_string()))?;
            if behavior.crash_after_effect.load(Ordering::SeqCst) > 0 {
                behavior.crash_after_effect.fetch_sub(1, Ordering::SeqCst);
                // The effect commits, the receipt does not: a crash between
                // the two commits redelivers the event.
                tx.commit()
                    .await
                    .map_err(|error| OutboxHandlerError::Transient(error.to_string()))?;
                return Err(OutboxHandlerError::Transient(
                    "crashed after the effect committed".to_owned(),
                ));
            }
            if behavior.success_without_receipt.load(Ordering::SeqCst) == 0 {
                record_outbox_receipt(&mut tx, &context)
                    .await
                    .map_err(|error| OutboxHandlerError::Transient(error.to_string()))?;
            } else {
                behavior
                    .success_without_receipt
                    .fetch_sub(1, Ordering::SeqCst);
            }
            tx.commit()
                .await
                .map_err(|error| OutboxHandlerError::Transient(error.to_string()))?;
            Ok(())
        })
    })
}

fn ping_registry(pool: &PgPool, project_id: Uuid, behavior: Arc<Behavior>) -> OutboxRegistry {
    let mut registry = OutboxRegistry::new();
    registry
        .register::<Ping>(&[("alpha", enqueue_handler(pool.clone(), project_id, behavior))])
        .expect("registers");
    registry
}

fn fanout_registry(
    pool: &PgPool,
    project_id: Uuid,
    alpha: Arc<Behavior>,
    beta: Arc<Behavior>,
) -> OutboxRegistry {
    let mut registry = OutboxRegistry::new();
    registry
        .register::<Fanout>(&[
            ("alpha", enqueue_handler(pool.clone(), project_id, alpha)),
            ("beta", enqueue_handler(pool.clone(), project_id, beta)),
        ])
        .expect("registers");
    registry
}

async fn insert(pool: &PgPool, event: &NewOutboxEvent) {
    let mut tx = pool.begin().await.expect("begin");
    insert_outbox_event(&mut tx, event).await.expect("insert");
    tx.commit().await.expect("commit");
}

async fn event_row(
    pool: &PgPool,
    event_id: Uuid,
) -> (
    String,
    Option<DateTime<Utc>>,
    Option<String>,
    Option<DateTime<Utc>>,
) {
    let row = sqlx::query(
        "SELECT delivery_state, delivered_at, last_error, next_attempt_at
         FROM outbox_events WHERE event_id=$1",
    )
    .bind(event_id)
    .fetch_one(pool)
    .await
    .expect("event row");
    (
        row.get("delivery_state"),
        row.get("delivered_at"),
        row.get("last_error"),
        row.get("next_attempt_at"),
    )
}

fn delivery_context(
    event: &deepref_app::outbox::ClaimedOutboxEvent,
    consumer: &str,
) -> DeliveryContext {
    DeliveryContext {
        event_id: event.event_id,
        consumer: consumer.to_owned(),
        message_type: event.message_type.clone(),
        message_version: event.message_version,
        claim_fence: event.claim_fence.clone(),
    }
}

/// Simulate elapsed retry backoff so the next dispatch may claim and retry.
async fn force_due(pool: &PgPool, event_id: Uuid) {
    sqlx::query("UPDATE outbox_events SET next_attempt_at=now() WHERE event_id=$1")
        .bind(event_id)
        .execute(pool)
        .await
        .expect("event due");
    sqlx::query("UPDATE outbox_receipts SET next_attempt_at=now() WHERE event_id=$1")
        .bind(event_id)
        .execute(pool)
        .await
        .expect("receipts due");
}

async fn jobs_with_dedupe(pool: &PgPool, dedupe_key: &str) -> i64 {
    sqlx::query_scalar::<_, i64>("SELECT count(*) FROM jobs WHERE dedupe_key=$1")
        .bind(dedupe_key)
        .fetch_one(pool)
        .await
        .expect("job count")
}

fn ping_event(event_id: Uuid, note: &str) -> NewOutboxEvent {
    NewOutboxEvent::for_message(
        event_id,
        &Ping {
            note: note.to_owned(),
        },
    )
    .expect("serializes")
}

fn fanout_event(event_id: Uuid, note: &str) -> NewOutboxEvent {
    NewOutboxEvent::for_message(
        event_id,
        &Fanout {
            note: note.to_owned(),
        },
    )
    .expect("serializes")
}

#[tokio::test]
async fn outbox_row_commits_and_rolls_back_with_the_business_transaction() -> Result<()> {
    let Some(pool) = database().await else {
        return Ok(());
    };
    let committed = ping_event(Uuid::new_v4(), "commit");
    let mut tx = pool.begin().await?;
    insert_outbox_event(&mut tx, &committed).await?;
    tx.commit().await?;
    let (state, delivered_at, ..) = event_row(&pool, committed.event_id).await;
    assert_eq!(state, "pending");
    assert!(delivered_at.is_none(), "claim/insert never delivers");

    let rolled_back = ping_event(Uuid::new_v4(), "rollback");
    let mut tx = pool.begin().await?;
    insert_outbox_event(&mut tx, &rolled_back).await?;
    tx.rollback().await?;
    let gone = sqlx::query("SELECT event_id FROM outbox_events WHERE event_id=$1")
        .bind(rolled_back.event_id)
        .fetch_optional(&pool)
        .await?;
    assert!(gone.is_none(), "rollback removes the outbox row");
    Ok(())
}

#[tokio::test]
async fn claims_are_exclusive_and_alone_never_mark_delivery() -> Result<()> {
    let Some(pool) = database().await else {
        return Ok(());
    };
    let event = ping_event(Uuid::new_v4(), "claim");
    insert(&pool, &event).await;

    let first =
        deepref_app::outbox::claim_outbox_batch(&pool, "owner-a", Duration::from_secs(60), 8)
            .await?;
    assert_eq!(first.len(), 1);
    let second =
        deepref_app::outbox::claim_outbox_batch(&pool, "owner-b", Duration::from_secs(60), 8)
            .await?;
    assert!(second.is_empty(), "an active lease is exclusive");
    let (state, delivered_at, ..) = event_row(&pool, event.event_id).await;
    assert_eq!(state, "pending", "claim alone never marks delivered");
    assert!(delivered_at.is_none());

    // An expired lease is reclaimable: the dispatcher crashed case.
    let expiring = ping_event(Uuid::new_v4(), "expiry");
    insert(&pool, &expiring).await;
    let short =
        deepref_app::outbox::claim_outbox_batch(&pool, "owner-c", Duration::from_millis(100), 8)
            .await?;
    assert_eq!(short.len(), 1);
    tokio::time::sleep(Duration::from_millis(300)).await;
    let reclaimed =
        deepref_app::outbox::claim_outbox_batch(&pool, "owner-b", Duration::from_secs(60), 8)
            .await?;
    assert_eq!(reclaimed.len(), 1, "the expired lease is reclaimed");
    assert_eq!(reclaimed[0].attempts, 2, "reclaim spends a claim attempt");
    Ok(())
}

#[tokio::test]
async fn concurrent_claimers_cannot_claim_the_same_event() -> Result<()> {
    let Some(pool) = database().await else {
        return Ok(());
    };
    let event = ping_event(Uuid::new_v4(), "concurrent claim");
    insert(&pool, &event).await;

    let (first, second) = tokio::join!(
        deepref_app::outbox::claim_outbox_batch(&pool, "concurrent-a", Duration::from_secs(30), 1,),
        deepref_app::outbox::claim_outbox_batch(&pool, "concurrent-b", Duration::from_secs(30), 1,),
    );
    let first = first?;
    let second = second?;
    assert_eq!(first.len() + second.len(), 1, "one claim wins the row lock");
    Ok(())
}

#[tokio::test]
async fn stale_generation_cannot_commit_effect_or_receipt_after_reclaim() -> Result<()> {
    let Some(pool) = database().await else {
        return Ok(());
    };
    let project_id = Uuid::new_v4();
    let event = ping_event(Uuid::new_v4(), "stale completion");
    insert(&pool, &event).await;

    // Reuse the owner string to prove the generation, rather than owner name,
    // is what fences an expired worker.
    let old_claim =
        deepref_app::outbox::claim_outbox_batch(&pool, "same-owner", Duration::from_secs(30), 1)
            .await?;
    assert_eq!(old_claim.len(), 1);
    sqlx::query(
        "UPDATE outbox_events SET lease_expires_at = clock_timestamp() - interval '1 second' WHERE event_id=$1",
    )
    .bind(event.event_id)
    .execute(&pool)
    .await?;
    let new_claim =
        deepref_app::outbox::claim_outbox_batch(&pool, "same-owner", Duration::from_secs(30), 1)
            .await?;
    assert_eq!(new_claim.len(), 1);
    assert!(new_claim[0].claim_fence.generation() > old_claim[0].claim_fence.generation());

    let mut stale_tx = pool.begin().await?;
    sqlx::query("INSERT INTO projects (id,name) VALUES ($1,'stale effect')")
        .bind(project_id)
        .execute(&mut *stale_tx)
        .await?;
    let stale_context = delivery_context(&old_claim[0], "alpha");
    assert!(
        record_outbox_receipt(&mut stale_tx, &stale_context)
            .await
            .is_err()
    );
    stale_tx.rollback().await?;

    let effect = sqlx::query("SELECT id FROM projects WHERE id=$1")
        .bind(project_id)
        .fetch_optional(&pool)
        .await?;
    assert!(
        effect.is_none(),
        "the stale receipt aborts its effect transaction"
    );
    assert!(
        list_outbox_receipts(&pool, event.event_id)
            .await?
            .is_empty()
    );
    let (state, delivered_at, ..) = event_row(&pool, event.event_id).await;
    assert_eq!(state, "pending");
    assert!(delivered_at.is_none());

    // The current generation can commit the effect and receipt atomically.
    let mut current_tx = pool.begin().await?;
    sqlx::query("INSERT INTO projects (id,name) VALUES ($1,'current effect')")
        .bind(project_id)
        .execute(&mut *current_tx)
        .await?;
    record_outbox_receipt(&mut current_tx, &delivery_context(&new_claim[0], "alpha")).await?;
    current_tx.commit().await?;
    assert_eq!(event_row(&pool, event.event_id).await.0, "delivered");

    // A delivered receipt is not an escape hatch for a stale handler that
    // wrote its effect first. The receipt API fences before its idempotent
    // early return, so the stale effect transaction still rolls back.
    let stale_effect_id = Uuid::new_v4();
    let mut stale_duplicate_tx = pool.begin().await?;
    sqlx::query("INSERT INTO projects (id,name) VALUES ($1,'stale duplicate effect')")
        .bind(stale_effect_id)
        .execute(&mut *stale_duplicate_tx)
        .await?;
    assert!(
        record_outbox_receipt(&mut stale_duplicate_tx, &stale_context)
            .await
            .is_err()
    );
    stale_duplicate_tx.rollback().await?;
    assert!(
        sqlx::query("SELECT id FROM projects WHERE id=$1")
            .bind(stale_effect_id)
            .fetch_optional(&pool)
            .await?
            .is_none()
    );
    Ok(())
}

#[tokio::test]
async fn two_dispatchers_racing_never_double_deliver() -> Result<()> {
    let Some(pool) = database().await else {
        return Ok(());
    };
    let project_id = project(&pool).await;
    let event = ping_event(Uuid::new_v4(), "race");
    insert(&pool, &event).await;
    let behavior = Arc::new(Behavior::default());
    let registry = Arc::new(ping_registry(&pool, project_id, Arc::clone(&behavior)));

    let mut racers = Vec::new();
    for owner in ["racer-a", "racer-b"] {
        let pool = pool.clone();
        let registry = Arc::clone(&registry);
        let owner = owner.to_owned();
        let event_id = event.event_id;
        racers.push(tokio::spawn(async move {
            for _ in 0..20 {
                dispatch_outbox_batch(&pool, &registry, &owner, Duration::from_secs(30), 8)
                    .await
                    .expect("dispatch batch");
                let (state, ..) = event_row(&pool, event_id).await;
                if state != "pending" {
                    return;
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        }));
    }
    for racer in racers {
        tokio::time::timeout(Duration::from_secs(30), racer).await??;
    }

    let (state, delivered_at, ..) = event_row(&pool, event.event_id).await;
    assert_eq!(state, "delivered");
    assert!(delivered_at.is_some());
    assert_eq!(
        behavior.calls.load(Ordering::SeqCst),
        1,
        "exactly one delivery"
    );
    let dedupe_key = format!("outbox-test:{}:alpha", event.event_id);
    assert_eq!(jobs_with_dedupe(&pool, &dedupe_key).await, 1, "one effect");
    let receipts = list_outbox_receipts(&pool, event.event_id).await?;
    assert_eq!(receipts.len(), 1);
    assert_eq!(
        receipts[0].state,
        deepref_app::outbox::ReceiptState::Delivered
    );
    Ok(())
}

#[tokio::test]
async fn crash_after_effect_before_receipt_replays_idempotently() -> Result<()> {
    let Some(pool) = database().await else {
        return Ok(());
    };
    let project_id = project(&pool).await;
    let event = ping_event(Uuid::new_v4(), "crash");
    insert(&pool, &event).await;
    let behavior = Arc::new(Behavior {
        crash_after_effect: AtomicUsize::new(1),
        ..Behavior::default()
    });
    let registry = ping_registry(&pool, project_id, Arc::clone(&behavior));

    dispatch_outbox_batch(&pool, &registry, "crash-owner", Duration::from_secs(30), 8).await?;
    let (state, ..) = event_row(&pool, event.event_id).await;
    assert_eq!(
        state, "pending",
        "the effect committed but the receipt did not"
    );
    let receipts = list_outbox_receipts(&pool, event.event_id).await?;
    assert_eq!(receipts.len(), 1);
    assert_eq!(
        receipts[0].state,
        deepref_app::outbox::ReceiptState::Retrying
    );

    force_due(&pool, event.event_id).await;
    dispatch_outbox_batch(&pool, &registry, "crash-owner", Duration::from_secs(30), 8).await?;
    let (state, ..) = event_row(&pool, event.event_id).await;
    assert_eq!(state, "delivered");
    assert_eq!(behavior.calls.load(Ordering::SeqCst), 2, "redelivered once");
    let dedupe_key = format!("outbox-test:{}:alpha", event.event_id);
    assert_eq!(
        jobs_with_dedupe(&pool, &dedupe_key).await,
        1,
        "the idempotent effect is not duplicated by redelivery"
    );
    Ok(())
}

#[tokio::test]
async fn fan_out_partial_failure_retries_only_the_missing_consumer() -> Result<()> {
    let Some(pool) = database().await else {
        return Ok(());
    };
    let project_id = project(&pool).await;
    let event = fanout_event(Uuid::new_v4(), "fanout");
    insert(&pool, &event).await;
    let alpha = Arc::new(Behavior::default());
    let beta = Arc::new(Behavior::with_transient_failures(1));
    let registry = fanout_registry(&pool, project_id, Arc::clone(&alpha), Arc::clone(&beta));

    dispatch_outbox_batch(&pool, &registry, "fanout-owner", Duration::from_secs(30), 8).await?;
    let (state, delivered_at, ..) = event_row(&pool, event.event_id).await;
    assert_eq!(state, "pending", "one receipt short of completion");
    assert!(delivered_at.is_none());
    assert_eq!(alpha.calls.load(Ordering::SeqCst), 1);
    assert_eq!(beta.calls.load(Ordering::SeqCst), 1);

    force_due(&pool, event.event_id).await;
    dispatch_outbox_batch(&pool, &registry, "fanout-owner", Duration::from_secs(30), 8).await?;
    let (state, delivered_at, ..) = event_row(&pool, event.event_id).await;
    assert_eq!(state, "delivered");
    assert!(delivered_at.is_some());
    assert_eq!(
        alpha.calls.load(Ordering::SeqCst),
        1,
        "a consumer with a committed receipt is never re-run"
    );
    assert_eq!(beta.calls.load(Ordering::SeqCst), 2);
    Ok(())
}

#[tokio::test]
async fn a_receipt_outside_the_expected_set_is_not_counted() -> Result<()> {
    let Some(pool) = database().await else {
        return Ok(());
    };
    let event = fanout_event(Uuid::new_v4(), "rogue");
    insert(&pool, &event).await;
    let claimed =
        deepref_app::outbox::claim_outbox_batch(&pool, "rogue-owner", Duration::from_secs(30), 1)
            .await?;
    assert_eq!(claimed.len(), 1);

    let mut tx = pool.begin().await?;
    record_outbox_receipt(&mut tx, &delivery_context(&claimed[0], "alpha")).await?;
    record_outbox_receipt(&mut tx, &delivery_context(&claimed[0], "rogue")).await?;
    tx.commit().await?;
    let (state, ..) = event_row(&pool, event.event_id).await;
    assert_eq!(state, "pending", "the frozen set still misses beta");

    let mut tx = pool.begin().await?;
    record_outbox_receipt(&mut tx, &delivery_context(&claimed[0], "beta")).await?;
    tx.commit().await?;
    let (state, ..) = event_row(&pool, event.event_id).await;
    assert_eq!(state, "delivered");
    Ok(())
}

#[tokio::test]
async fn a_consumer_that_already_holds_a_receipt_is_a_no_op() -> Result<()> {
    let Some(pool) = database().await else {
        return Ok(());
    };
    let event = ping_event(Uuid::new_v4(), "no-op");
    insert(&pool, &event).await;
    let claimed =
        deepref_app::outbox::claim_outbox_batch(&pool, "noop-owner", Duration::from_secs(30), 1)
            .await?;
    assert_eq!(claimed.len(), 1);
    let mut tx = pool.begin().await?;
    let context = delivery_context(&claimed[0], "alpha");
    let first = record_outbox_receipt(&mut tx, &context).await?;
    tx.commit().await?;
    let mut tx = pool.begin().await?;
    let second = record_outbox_receipt(&mut tx, &context).await?;
    tx.commit().await?;
    assert!(first, "the first receipt commits the delivery");
    assert!(!second, "redelivery of a delivered receipt is a no-op");
    Ok(())
}

#[tokio::test]
async fn unknown_message_type_is_handled_terminally() -> Result<()> {
    let Some(pool) = database().await else {
        return Ok(());
    };
    let project_id = project(&pool).await;
    let event = NewOutboxEvent::untyped(
        Uuid::new_v4(),
        "test.unknown",
        9,
        json!({"note": "no contract"}),
        &["alpha"],
    );
    insert(&pool, &event).await;
    let behavior = Arc::new(Behavior::default());
    let registry = ping_registry(&pool, project_id, behavior.clone());

    dispatch_outbox_batch(
        &pool,
        &registry,
        "unknown-owner",
        Duration::from_secs(30),
        8,
    )
    .await?;
    let (state, _, last_error, ..) = event_row(&pool, event.event_id).await;
    assert_eq!(state, "quarantined");
    let last_error = last_error.expect("quarantine keeps the reason");
    assert!(
        last_error.contains("unknown message type test.unknown v9"),
        "unexpected reason: {last_error}"
    );

    // Terminal means terminal: the row is never claimed again.
    let claimed =
        deepref_app::outbox::claim_outbox_batch(&pool, "unknown-owner", Duration::from_secs(30), 8)
            .await?;
    assert!(claimed.is_empty());
    Ok(())
}

#[tokio::test]
async fn failed_consumers_back_off_then_quarantine_and_can_be_replayed() -> Result<()> {
    let Some(pool) = database().await else {
        return Ok(());
    };
    let project_id = project(&pool).await;
    let mut event = ping_event(Uuid::new_v4(), "backoff");
    event.max_attempts = 2;
    insert(&pool, &event).await;
    let behavior = Arc::new(Behavior::with_transient_failures(2));
    let registry = ping_registry(&pool, project_id, Arc::clone(&behavior));

    dispatch_outbox_batch(&pool, &registry, "retry-owner", Duration::from_secs(30), 8).await?;
    let receipts = list_outbox_receipts(&pool, event.event_id).await?;
    assert_eq!(
        receipts[0].state,
        deepref_app::outbox::ReceiptState::Retrying
    );
    assert_eq!(receipts[0].attempts, 1);
    assert!(
        receipts[0].next_attempt_at > Utc::now(),
        "the retry is delayed by backoff"
    );
    let (state, _, last_error, ..) = event_row(&pool, event.event_id).await;
    assert_eq!(state, "pending");
    assert!(
        last_error.is_some(),
        "the failure is preserved for diagnosis"
    );

    force_due(&pool, event.event_id).await;
    dispatch_outbox_batch(&pool, &registry, "retry-owner", Duration::from_secs(30), 8).await?;
    let receipts = list_outbox_receipts(&pool, event.event_id).await?;
    assert_eq!(
        receipts[0].state,
        deepref_app::outbox::ReceiptState::Quarantined
    );
    assert_eq!(receipts[0].attempts, 2);
    let (state, delivered_at, ..) = event_row(&pool, event.event_id).await;
    assert_eq!(state, "quarantined", "the attempt budget is spent");
    assert!(delivered_at.is_none(), "quarantine is not delivery");

    // Explicit replay after review: the same stable event identity, a fresh
    // attempt budget; the handler now succeeds.
    assert!(replay_outbox_event(&pool, event.event_id).await?);
    let (state, ..) = event_row(&pool, event.event_id).await;
    assert_eq!(state, "pending");
    force_due(&pool, event.event_id).await;
    dispatch_outbox_batch(&pool, &registry, "retry-owner", Duration::from_secs(30), 8).await?;
    let (state, delivered_at, ..) = event_row(&pool, event.event_id).await;
    assert_eq!(state, "delivered");
    assert!(delivered_at.is_some());
    assert_eq!(behavior.calls.load(Ordering::SeqCst), 3);
    Ok(())
}

#[tokio::test]
async fn a_terminal_consumer_failure_quarantines_without_burning_the_budget() -> Result<()> {
    let Some(pool) = database().await else {
        return Ok(());
    };
    let project_id = project(&pool).await;
    let event = ping_event(Uuid::new_v4(), "terminal");
    insert(&pool, &event).await;
    let behavior = Arc::new(Behavior {
        terminal_failures: AtomicUsize::new(1),
        ..Behavior::default()
    });
    let registry = ping_registry(&pool, project_id, Arc::clone(&behavior));

    dispatch_outbox_batch(
        &pool,
        &registry,
        "terminal-owner",
        Duration::from_secs(30),
        8,
    )
    .await?;
    let receipts = list_outbox_receipts(&pool, event.event_id).await?;
    assert_eq!(
        receipts[0].state,
        deepref_app::outbox::ReceiptState::Quarantined
    );
    assert_eq!(
        receipts[0].attempts, 1,
        "terminal means no retry budget spent"
    );
    let (state, ..) = event_row(&pool, event.event_id).await;
    assert_eq!(state, "quarantined");
    Ok(())
}

#[tokio::test]
async fn a_handler_that_reports_success_without_its_receipt_is_quarantined() -> Result<()> {
    let Some(pool) = database().await else {
        return Ok(());
    };
    let project_id = project(&pool).await;
    let event = ping_event(Uuid::new_v4(), "missing receipt");
    insert(&pool, &event).await;
    let behavior = Arc::new(Behavior {
        success_without_receipt: AtomicUsize::new(1),
        ..Behavior::default()
    });
    let registry = ping_registry(&pool, project_id, Arc::clone(&behavior));

    dispatch_outbox_batch(
        &pool,
        &registry,
        "receipt-owner",
        Duration::from_secs(30),
        8,
    )
    .await?;
    let (state, _, last_error, ..) = event_row(&pool, event.event_id).await;
    assert_eq!(state, "quarantined");
    let last_error = last_error.expect("quarantine keeps the reason");
    assert!(
        last_error.contains("without committing its receipt"),
        "unexpected reason: {last_error}"
    );
    Ok(())
}

#[tokio::test]
async fn terminal_failure_records_are_leases_aware_and_settle_releases() -> Result<()> {
    let Some(pool) = database().await else {
        return Ok(());
    };
    let event = ping_event(Uuid::new_v4(), "settle");
    insert(&pool, &event).await;
    let claimed =
        deepref_app::outbox::claim_outbox_batch(&pool, "settle-owner", Duration::from_secs(60), 8)
            .await?;
    assert_eq!(claimed.len(), 1);

    let state = record_outbox_consumer_failure(
        &pool,
        event.event_id,
        "alpha",
        &claimed[0].claim_fence,
        "diagnosable failure",
        OutboxRetry::After(Duration::from_millis(1)),
        8,
    )
    .await?;
    assert_eq!(state, deepref_app::outbox::ReceiptState::Retrying);

    // Settlement only while the lease is owned: a stranger cannot release it.
    let stranger = settle_outbox_event(
        &pool,
        event.event_id,
        &OutboxClaimFence::new("someone-else", claimed[0].claim_fence.generation()),
    )
    .await?;
    assert!(stranger.is_none());
    let settled = settle_outbox_event(&pool, event.event_id, &claimed[0].claim_fence).await?;
    assert_eq!(settled, Some(deepref_app::outbox::DeliveryState::Pending));
    force_due(&pool, event.event_id).await;
    let claimed_again =
        deepref_app::outbox::claim_outbox_batch(&pool, "other-owner", Duration::from_secs(60), 8)
            .await?;
    assert_eq!(claimed_again.len(), 1, "settle released the lease");
    Ok(())
}

#[tokio::test]
async fn the_pilot_message_runs_end_to_end_through_the_dispatcher() -> Result<()> {
    let Some(pool) = database().await else {
        return Ok(());
    };
    let project_id = project(&pool).await;
    let envelope = EventEnvelope::v1(
        SUBJECT_METRICS_RECOMPUTE_REQUESTED,
        "outbox-test",
        EntityType::Project,
        format!("project:{project_id}"),
        1,
        Uuid::new_v4(),
        None,
        MetricsRecomputeRequested {
            project_id,
            ingestion_id: None,
        },
    );
    let event = NewOutboxEvent::for_message(envelope.event_id, &envelope).expect("serializes");
    let mut tx = pool.begin().await?;
    insert_outbox_event(&mut tx, &event).await?;
    tx.commit().await?;

    let registry = pilot_outbox_registry(&pool)?;
    dispatch_outbox_batch(&pool, &registry, "pilot-owner", Duration::from_secs(30), 8).await?;

    let (state, delivered_at, ..) = event_row(&pool, event.event_id).await;
    assert_eq!(state, "delivered");
    assert!(delivered_at.is_some());
    let dedupe_key = format!("outbox-metrics-recompute:{}", event.event_id);
    assert_eq!(
        jobs_with_dedupe(&pool, &dedupe_key).await,
        1,
        "the pilot consumer committed its durable job enqueue"
    );
    let receipts = list_outbox_receipts(&pool, event.event_id).await?;
    assert_eq!(receipts.len(), 1);
    assert_eq!(receipts[0].consumer, "metrics-recompute");
    Ok(())
}
