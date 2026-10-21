#![allow(
    clippy::expect_used,
    clippy::panic,
    clippy::unwrap_used,
    clippy::panic_in_result_fn
)]

//! Handoff 09 slice B: the provider-neutral AI usage fact export against a
//! real PostgreSQL. Covers the spec's export tests (duplicate outbox
//! delivery, ambiguous provider timeout, provider unavailability), replay
//! idempotency, correction facts, payload minimization and the disabled mode.

use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use chrono::Datelike;
use deepref_ai::{
    AiError, AiUsageContext, UsageLedger, UsageObservation, UsagePriceSnapshot, UsagePrincipal,
    UsageReservationRequest,
};
use deepref_app::outbox::{
    claim_outbox_batch, dispatch_outbox_batch, dispatch_outbox_event, list_outbox_receipts,
};
use deepref_app::{
    PostgresUsageLedger, UsageExportReplayParams, billing_usage_export_handler, replay_usage_facts,
    usage_export_backlog, usage_export_registry,
};
use deepref_application::outbox::{DeliveryContext, OutboxMessage, OutboxRegistry};
use deepref_application::usage_export::{
    AiUsageFact, DisabledUsageExportSink, ScriptedUsageExportSink, USAGE_EXPORT_CONSUMER,
    UsageExportCounters, UsageExportOutcome, UsageExportSink,
};
use deepref_domain::{ProjectId, WorkspaceId};
use serde_json::Value;
use sqlx::PgPool;
use uuid::Uuid;

async fn database() -> Option<PgPool> {
    deepref_app::test_isolation::isolated_pool_from_env("DATABASE_URL", 24).await
}

async fn fixture(pool: &PgPool) -> (ProjectId, WorkspaceId) {
    let project_id = ProjectId::new(Uuid::new_v4());
    sqlx::query("INSERT INTO projects (id,name) VALUES ($1,'usage export fixture')")
        .bind(project_id.as_uuid())
        .execute(pool)
        .await
        .expect("project fixture inserts");
    let workspace_id: Uuid = sqlx::query_scalar("SELECT workspace_id FROM projects WHERE id = $1")
        .bind(project_id.as_uuid())
        .fetch_one(pool)
        .await
        .expect("project has a workspace");
    sqlx::query("UPDATE ai_usage_rollout_state SET ai_hard_caps_enforced = true WHERE singleton")
        .execute(pool)
        .await
        .expect("test enables the hard-cap admission path");
    sqlx::query("UPDATE ai_usage_backfill_state SET completed_at = now() WHERE singleton")
        .execute(pool)
        .await
        .expect("test marks empty legacy history backfilled");
    set_limits(pool, project_id, WorkspaceId::new(workspace_id), 10_000_000).await;
    (project_id, WorkspaceId::new(workspace_id))
}

async fn set_limits(pool: &PgPool, project_id: ProjectId, workspace_id: WorkspaceId, limit: i64) {
    sqlx::query(
        "UPDATE ai_usage_limits SET limit_micros = $3
         WHERE (scope_kind = 'platform' AND scope_id = '00000000-0000-0000-0000-000000000001')
            OR (scope_kind = 'workspace' AND scope_id = $2)
            OR (scope_kind = 'project' AND scope_id = $1)",
    )
    .bind(project_id.as_uuid())
    .bind(workspace_id.as_uuid())
    .bind(limit)
    .execute(pool)
    .await
    .expect("quota caps update");
    sqlx::query(
        "UPDATE ai_usage_period_buckets SET limit_micros = $3
         WHERE period_start = date_trunc('month', now() AT TIME ZONE 'UTC')::date
           AND ((scope_kind = 'platform' AND scope_id = '00000000-0000-0000-0000-000000000001')
             OR (scope_kind = 'workspace' AND scope_id = $2)
             OR (scope_kind = 'project' AND scope_id = $1))",
    )
    .bind(project_id.as_uuid())
    .bind(workspace_id.as_uuid())
    .bind(limit)
    .execute(pool)
    .await
    .expect("monthly quota buckets follow the fixture caps");
}

fn request(
    project_id: ProjectId,
    run_id: Uuid,
    input_bound: u64,
    run_limit_micros: i64,
) -> UsageReservationRequest {
    UsageReservationRequest {
        reservation_id: Uuid::new_v4(),
        request_fingerprint: "a".repeat(64),
        attribution: AiUsageContext {
            workspace_id: None,
            principal: Some(UsagePrincipal::Service("usage-export-test".to_owned())),
            run_id,
            run_budget_id: run_id,
            step_id: Some("step:export".to_owned()),
            job_id: None,
            attempt_fence: None,
            run_limit_micros: Some(run_limit_micros),
        },
        project_id: Some(project_id),
        profile: "reasoning".to_owned(),
        provider: "test-provider".to_owned(),
        model: "test-model".to_owned(),
        purpose: "structured".to_owned(),
        input_token_bound: Some(input_bound),
        max_output_tokens: Some(1),
        max_reasoning_tokens: None,
        other_cost_micros: 0,
        rounding_margin_micros: 0,
        price: UsagePriceSnapshot {
            version: "test-price-v1".to_owned(),
            input_micros_per_million_tokens: 1_000_000,
            output_micros_per_million_tokens: 1_000_000,
            reasoning_micros_per_million_tokens: None,
        },
    }
}

/// One immutable usage fact, produced by the real reservation and settlement
/// path (the export replays settled facts; it never runs at settlement).
async fn settled_fact(pool: &PgPool, project_id: ProjectId, input_tokens: u64) -> Uuid {
    let ledger = PostgresUsageLedger::new(pool);
    let run_id = Uuid::new_v4();
    let reservation = ledger
        .reserve(request(project_id, run_id, input_tokens, 10_000_000))
        .await
        .expect("reservation accepted");
    let observation = UsageObservation {
        input_tokens: Some(input_tokens),
        output_tokens: Some(0),
        reasoning_tokens: Some(0),
        commercial_units: Some(2),
        provider_request_id: Some("export-request".to_owned()),
        provider_response_id: Some("export-response".to_owned()),
    };
    ledger
        .settle(&reservation, observation, "test-price-v1")
        .await
        .expect("settlement commits its immutable fact");
    let fact_id: Uuid =
        sqlx::query_scalar("SELECT id FROM ai_usage_ledger WHERE reservation_id = $1")
            .bind(reservation.id)
            .fetch_one(pool)
            .await
            .expect("settlement wrote exactly one fact");
    fact_id
}

fn fact_event_registry(
    pool: &PgPool,
    sink: Arc<dyn UsageExportSink>,
) -> (OutboxRegistry, Arc<UsageExportCounters>) {
    let counters = Arc::new(UsageExportCounters::default());
    let registry = usage_export_registry(pool, sink, Arc::clone(&counters))
        .expect("usage export registry builds");
    (registry, counters)
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

async fn delivery_state(pool: &PgPool, event_id: Uuid) -> String {
    sqlx::query_scalar("SELECT delivery_state FROM outbox_events WHERE event_id=$1")
        .bind(event_id)
        .fetch_one(pool)
        .await
        .expect("event row")
}

async fn fact_event_count(pool: &PgPool) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM outbox_events WHERE message_type='ai_usage.fact'")
        .fetch_one(pool)
        .await
        .expect("event count")
}

#[tokio::test]
async fn the_replay_emits_exactly_one_event_per_settled_fact() -> Result<()> {
    let Some(pool) = database().await else {
        return Ok(());
    };
    let (project, _workspace) = fixture(&pool).await;
    let fact = settled_fact(&pool, project, 100).await;
    let summary = replay_usage_facts(
        &pool,
        &UsageExportCounters::default(),
        UsageExportReplayParams::enabled(500),
    )
    .await?;
    assert!(summary.enabled);
    assert_eq!(summary.facts_emitted, 1);
    assert!(summary.complete);
    assert_eq!(fact_event_count(&pool).await, 1);
    let payload: Value =
        sqlx::query_scalar("SELECT payload FROM outbox_events WHERE message_type='ai_usage.fact'")
            .fetch_one(&pool)
            .await?;
    // The event identity is the immutable fact id, and the payload carries the
    // fact's attribution and counters.
    assert_eq!(payload["fact_id"], Value::String(fact.to_string()));
    assert_eq!(payload["provider"], "test-provider");
    assert_eq!(payload["model"], "test-model");
    assert_eq!(payload["input_tokens"], 100);
    assert_eq!(payload["price_version"], "test-price-v1");
    assert_eq!(payload["correction_of"], Value::Null);
    assert_eq!(payload["principal_kind"], "service");
    Ok(())
}

#[tokio::test]
async fn replay_is_idempotent_across_runs_and_concurrent_tasks() -> Result<()> {
    let Some(pool) = database().await else {
        return Ok(());
    };
    let (project, _workspace) = fixture(&pool).await;
    let facts: Vec<Uuid> = {
        let mut facts = Vec::new();
        for _ in 0..5 {
            facts.push(settled_fact(&pool, project, 100).await);
        }
        facts
    };
    // Small limits exercise the keyset paging across batches.
    let counters = UsageExportCounters::default();
    for _ in 0..10 {
        let summary =
            replay_usage_facts(&pool, &counters, UsageExportReplayParams::enabled(2)).await?;
        if summary.complete {
            break;
        }
    }
    assert_eq!(fact_event_count(&pool).await, 5);
    // Running again emits nothing new.
    replay_usage_facts(&pool, &counters, UsageExportReplayParams::enabled(500)).await?;
    assert_eq!(fact_event_count(&pool).await, 5);

    // Two concurrent replay tasks converge on the same single row per fact.
    let left_counters = UsageExportCounters::default();
    let right_counters = UsageExportCounters::default();
    let (left, right) = tokio::join!(
        replay_usage_facts(&pool, &left_counters, UsageExportReplayParams::enabled(500)),
        replay_usage_facts(
            &pool,
            &right_counters,
            UsageExportReplayParams::enabled(500)
        ),
    );
    assert!(left?.complete);
    assert!(right?.complete);
    assert_eq!(fact_event_count(&pool).await, 5);
    let event_ids: Vec<Uuid> = sqlx::query_scalar(
        "SELECT event_id FROM outbox_events WHERE message_type='ai_usage.fact' ORDER BY event_id",
    )
    .fetch_all(&pool)
    .await?;
    let mut expected = facts.clone();
    expected.sort();
    assert_eq!(event_ids, expected);
    Ok(())
}

/// Delivers like its inner fake, but lets the outbox claim lease expire before
/// it answers: the provider has the event while the worker's receipt cannot
/// commit. That crash window is where at-least-once delivery double-delivers.
struct LeaseExpiringSink {
    pool: PgPool,
    first_delivery: std::sync::atomic::AtomicBool,
    inner: ScriptedUsageExportSink,
}

impl UsageExportSink for LeaseExpiringSink {
    fn deliver<'a>(
        &'a self,
        event: &'a AiUsageFact,
    ) -> futures::future::BoxFuture<'a, UsageExportOutcome> {
        Box::pin(async move {
            let outcome = self.inner.deliver(event).await;
            if self
                .first_delivery
                .swap(false, std::sync::atomic::Ordering::SeqCst)
            {
                sqlx::query(
                    "UPDATE outbox_events SET lease_expires_at = clock_timestamp() - interval '1 millisecond'
                     WHERE event_id = $1",
                )
                .bind(event.fact_id)
                .execute(&self.pool)
                .await
                .expect("claim lease expires mid-delivery");
            }
            outcome
        })
    }
}

/// Spec test 8: the same event is delivered twice after a lease expiry and a
/// reclaim. The sink sees the same event id both times and counts one fact.
#[tokio::test]
async fn duplicate_delivery_counts_the_fact_once() -> Result<()> {
    let Some(pool) = database().await else {
        return Ok(());
    };
    let (project, _workspace) = fixture(&pool).await;
    let fact = settled_fact(&pool, project, 100).await;
    replay_usage_facts(
        &pool,
        &UsageExportCounters::default(),
        UsageExportReplayParams::enabled(500),
    )
    .await?;

    let sink = Arc::new(LeaseExpiringSink {
        pool: pool.clone(),
        first_delivery: std::sync::atomic::AtomicBool::new(true),
        inner: ScriptedUsageExportSink::new(vec![
            UsageExportOutcome::Accepted,
            UsageExportOutcome::Duplicate,
        ]),
    });
    let (registry, _counters) = fact_event_registry(&pool, sink.clone());

    // The first worker's claim expires while it delivers: the provider has the
    // event, but the receipt cannot commit against the lost claim.
    let claimed =
        claim_outbox_batch(&pool, &registry, "worker-a", Duration::from_secs(30), 8).await?;
    assert_eq!(claimed.len(), 1);
    let stale = dispatch_outbox_event(&pool, &registry, Duration::from_secs(30), &claimed[0]).await;
    assert!(stale.is_err(), "a lost claim cannot commit its receipt");
    assert_ne!(delivery_state(&pool, fact).await, "delivered");

    // A reclaiming worker first recovers the abandoned claim, which settles the
    // lost attempt and backs the consumer off, and then delivers the same
    // event id again.
    let recovered =
        claim_outbox_batch(&pool, &registry, "worker-b", Duration::from_secs(30), 8).await?;
    assert!(
        recovered.is_empty(),
        "the reclaimed event waits out its retry backoff first"
    );
    force_due(&pool, fact).await;
    let reclaimed =
        claim_outbox_batch(&pool, &registry, "worker-b", Duration::from_secs(30), 8).await?;
    assert_eq!(reclaimed.len(), 1);
    dispatch_outbox_event(&pool, &registry, Duration::from_secs(30), &reclaimed[0]).await?;

    assert_eq!(sink.inner.delivery_count(), 2);
    assert_eq!(
        sink.inner.recorded_fact_ids(),
        vec![fact],
        "the fake counts the fact once no matter how often it is delivered"
    );
    assert_eq!(delivery_state(&pool, fact).await, "delivered");
    let receipts = list_outbox_receipts(&pool, fact).await?;
    assert_eq!(receipts.len(), 1);
    assert_eq!(receipts[0].consumer, USAGE_EXPORT_CONSUMER);
    Ok(())
}

/// Spec test 9: an ambiguous timeout is not success. The event is retried with
/// the same id and marked delivered only after Accepted.
#[tokio::test]
async fn ambiguous_timeout_retries_the_same_event_id_until_accepted() -> Result<()> {
    let Some(pool) = database().await else {
        return Ok(());
    };
    let (project, _workspace) = fixture(&pool).await;
    let fact = settled_fact(&pool, project, 100).await;
    replay_usage_facts(
        &pool,
        &UsageExportCounters::default(),
        UsageExportReplayParams::enabled(500),
    )
    .await?;

    let sink = Arc::new(ScriptedUsageExportSink::new(vec![
        UsageExportOutcome::Ambiguous("client timed out".to_owned()),
        UsageExportOutcome::Accepted,
    ]));
    let (registry, counters) = fact_event_registry(&pool, sink.clone());

    dispatch_outbox_batch(&pool, &registry, "worker-a", Duration::from_secs(30), 8).await?;
    assert_eq!(sink.delivery_count(), 1);
    assert_ne!(
        delivery_state(&pool, fact).await,
        "delivered",
        "Ambiguous is not success"
    );

    force_due(&pool, fact).await;
    dispatch_outbox_batch(&pool, &registry, "worker-a", Duration::from_secs(30), 8).await?;
    assert_eq!(sink.delivery_count(), 2);
    let delivered: Vec<Uuid> = sink.deliveries().iter().map(|fact| fact.fact_id).collect();
    assert_eq!(
        delivered,
        vec![fact, fact],
        "the same event id on both attempts"
    );
    assert_eq!(delivery_state(&pool, fact).await, "delivered");
    let snapshot = counters.snapshot();
    assert_eq!(snapshot.facts_ambiguous, 1);
    assert_eq!(snapshot.facts_delivered, 1);
    Ok(())
}

/// Spec test 9, second half: Ambiguous followed by Duplicate also completes.
#[tokio::test]
async fn ambiguous_timeout_followed_by_duplicate_completes() -> Result<()> {
    let Some(pool) = database().await else {
        return Ok(());
    };
    let (project, _workspace) = fixture(&pool).await;
    let fact = settled_fact(&pool, project, 100).await;
    replay_usage_facts(
        &pool,
        &UsageExportCounters::default(),
        UsageExportReplayParams::enabled(500),
    )
    .await?;

    let sink = Arc::new(ScriptedUsageExportSink::new(vec![
        UsageExportOutcome::Ambiguous("client timed out".to_owned()),
        UsageExportOutcome::Duplicate,
    ]));
    let (registry, counters) = fact_event_registry(&pool, sink.clone());

    dispatch_outbox_batch(&pool, &registry, "worker-a", Duration::from_secs(30), 8).await?;
    assert_ne!(delivery_state(&pool, fact).await, "delivered");
    force_due(&pool, fact).await;
    dispatch_outbox_batch(&pool, &registry, "worker-a", Duration::from_secs(30), 8).await?;
    assert_eq!(delivery_state(&pool, fact).await, "delivered");
    assert_eq!(sink.recorded_fact_ids(), vec![fact]);
    assert_eq!(counters.snapshot().facts_duplicate, 1);
    Ok(())
}

/// Spec test 10: while the provider is unavailable the local ledger and its
/// enforcement are untouched, and the backlog drains once the sink recovers.
#[tokio::test]
async fn provider_unavailability_never_touches_local_accounting() -> Result<()> {
    let Some(pool) = database().await else {
        return Ok(());
    };
    let (project, workspace) = fixture(&pool).await;
    set_limits(&pool, project, workspace, 1_000).await;
    let first_fact = settled_fact(&pool, project, 100).await;
    replay_usage_facts(
        &pool,
        &UsageExportCounters::default(),
        UsageExportReplayParams::enabled(500),
    )
    .await?;

    let sink = Arc::new(ScriptedUsageExportSink::new(vec![
        UsageExportOutcome::RetryableFailure("provider down".to_owned()),
        UsageExportOutcome::RetryableFailure("provider down".to_owned()),
        UsageExportOutcome::RetryableFailure("provider down".to_owned()),
        UsageExportOutcome::Accepted,
    ]));
    let (registry, counters) = fact_event_registry(&pool, sink.clone());

    for _ in 0..3 {
        force_due(&pool, first_fact).await;
        dispatch_outbox_batch(&pool, &registry, "worker-a", Duration::from_secs(30), 8).await?;
    }
    assert_eq!(delivery_state(&pool, first_fact).await, "pending");
    assert_eq!(counters.snapshot().facts_retryable_failures, 3);

    // Local accounting is unchanged by the export backlog.
    let period = chrono::Utc::now()
        .date_naive()
        .with_day(1)
        .expect("month start");
    for (kind, id) in [
        ("platform", Uuid::from_u128(1)),
        ("workspace", workspace.as_uuid()),
        ("project", project.as_uuid()),
    ] {
        let spent: i64 = sqlx::query_scalar(
            "SELECT spent_micros FROM ai_usage_period_buckets
             WHERE scope_kind=$1 AND scope_id=$2 AND period_start=$3",
        )
        .bind(kind)
        .bind(id)
        .bind(period)
        .fetch_one(&pool)
        .await?;
        assert_eq!(spent, 100, "{kind} spend survived the export outage");
    }
    let blocks: i64 = sqlx::query_scalar("SELECT count(*) FROM ai_usage_workspace_blocks")
        .fetch_one(&pool)
        .await?;
    assert_eq!(blocks, 0, "an export outage never blocks a workspace");

    // Enforcement behaves identically during the outage.
    let second_fact = settled_fact(&pool, project, 100).await;
    let ledger = PostgresUsageLedger::new(&pool);
    let denied = ledger
        .reserve(request(project, Uuid::new_v4(), 5_000, 10_000_000))
        .await;
    assert!(
        matches!(denied, Err(AiError::UsageLimitExceeded)),
        "hard caps still deny work beyond the configured allowance"
    );

    // The backlog drains once the sink recovers.
    replay_usage_facts(
        &pool,
        &UsageExportCounters::default(),
        UsageExportReplayParams::enabled(500),
    )
    .await?;
    for _ in 0..3 {
        force_due(&pool, first_fact).await;
        force_due(&pool, second_fact).await;
        dispatch_outbox_batch(&pool, &registry, "worker-a", Duration::from_secs(30), 8).await?;
        if delivery_state(&pool, first_fact).await == "delivered"
            && delivery_state(&pool, second_fact).await == "delivered"
        {
            break;
        }
    }
    assert_eq!(delivery_state(&pool, first_fact).await, "delivered");
    assert_eq!(delivery_state(&pool, second_fact).await, "delivered");
    let backlog = usage_export_backlog(&pool).await?;
    assert_eq!(backlog.unemitted_facts, 0);
    assert_eq!(backlog.pending_events, 0, "the backlog drains");
    assert_eq!(fact_event_count(&pool).await, 2, "one event per fact");
    assert_eq!(
        sink.recorded_fact_ids().len(),
        2,
        "no fact is delivered twice and none is lost"
    );
    Ok(())
}

/// Export disabled: nothing is emitted and nothing is lost; enabling the
/// replay emits every fact that settled while it was off.
#[tokio::test]
async fn facts_settled_while_export_is_disabled_are_emitted_after_enabling() -> Result<()> {
    let Some(pool) = database().await else {
        return Ok(());
    };
    let (project, _workspace) = fixture(&pool).await;
    let facts = vec![
        settled_fact(&pool, project, 100).await,
        settled_fact(&pool, project, 100).await,
    ];
    let counters = UsageExportCounters::default();
    let summary = replay_usage_facts(&pool, &counters, UsageExportReplayParams::disabled()).await?;
    assert!(!summary.enabled);
    assert_eq!(fact_event_count(&pool).await, 0);
    assert_eq!(summary.backlog.unemitted_facts, 2);
    assert!(
        summary.backlog.oldest_unemitted_age_seconds.is_some(),
        "the backlog reports the waiting facts"
    );

    let summary =
        replay_usage_facts(&pool, &counters, UsageExportReplayParams::enabled(500)).await?;
    assert!(summary.enabled);
    assert_eq!(summary.facts_emitted, 2);
    assert_eq!(fact_event_count(&pool).await, 2);
    let event_ids: Vec<Uuid> = sqlx::query_scalar(
        "SELECT event_id FROM outbox_events WHERE message_type='ai_usage.fact' ORDER BY event_id",
    )
    .fetch_all(&pool)
    .await?;
    let mut expected = facts;
    expected.sort();
    assert_eq!(event_ids, expected);
    Ok(())
}

/// The disabled sink is the default while no billing provider is configured:
/// events stay pending on the outbox's retry schedule and are never dropped as
/// delivered.
#[tokio::test]
async fn the_disabled_sink_leaves_events_pending_never_delivered() -> Result<()> {
    let Some(pool) = database().await else {
        return Ok(());
    };
    let (project, _workspace) = fixture(&pool).await;
    let fact = settled_fact(&pool, project, 100).await;
    replay_usage_facts(
        &pool,
        &UsageExportCounters::default(),
        UsageExportReplayParams::enabled(500),
    )
    .await?;
    let (registry, counters) = fact_event_registry(&pool, Arc::new(DisabledUsageExportSink));

    dispatch_outbox_batch(&pool, &registry, "worker-a", Duration::from_secs(30), 8).await?;
    assert_eq!(delivery_state(&pool, fact).await, "pending");
    let receipts = list_outbox_receipts(&pool, fact).await?;
    assert_eq!(receipts.len(), 1);
    assert_ne!(
        receipts[0].state,
        deepref_app::outbox::ReceiptState::Delivered
    );
    assert!(
        receipts[0]
            .last_error
            .as_deref()
            .unwrap_or_default()
            .contains("no billing provider adapter"),
        "the retry reason names the missing adapter"
    );
    assert_eq!(counters.snapshot().facts_retryable_failures, 1);
    assert_eq!(counters.snapshot().facts_delivered, 0);
    Ok(())
}

/// A correction record exports as its own fact referencing the original. The
/// original event is never edited.
#[tokio::test]
async fn a_correction_exports_as_its_own_fact_and_never_edits_the_original() -> Result<()> {
    let Some(pool) = database().await else {
        return Ok(());
    };
    let (project, _workspace) = fixture(&pool).await;
    let original = settled_fact(&pool, project, 100).await;
    replay_usage_facts(
        &pool,
        &UsageExportCounters::default(),
        UsageExportReplayParams::enabled(500),
    )
    .await?;
    let original_payload: Value =
        sqlx::query_scalar("SELECT payload FROM outbox_events WHERE event_id=$1")
            .bind(original)
            .fetch_one(&pool)
            .await?;

    let correction_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO ai_usage_corrections
           (id, ledger_id, original_cost_micros, corrected_cost_micros,
            original_price_version, corrected_price_version, reason)
         VALUES ($1, $2, 100, 150, 'test-price-v1', 'test-price-v2', 'price table corrected')",
    )
    .bind(correction_id)
    .bind(original)
    .execute(&pool)
    .await
    .expect("correction record inserts");

    let summary = replay_usage_facts(
        &pool,
        &UsageExportCounters::default(),
        UsageExportReplayParams::enabled(500),
    )
    .await?;
    assert_eq!(summary.corrections_emitted, 1);
    assert_eq!(fact_event_count(&pool).await, 2);

    let correction_payload: Value =
        sqlx::query_scalar("SELECT payload FROM outbox_events WHERE event_id=$1")
            .bind(correction_id)
            .fetch_one(&pool)
            .await?;
    assert_eq!(
        correction_payload["fact_id"],
        Value::String(correction_id.to_string())
    );
    assert_eq!(
        correction_payload["correction_of"],
        Value::String(original.to_string()),
        "the correction references its original fact"
    );
    assert_eq!(correction_payload["provider_cost_micros"], 150);
    assert_eq!(correction_payload["price_version"], "test-price-v2");

    let original_payload_after: Value =
        sqlx::query_scalar("SELECT payload FROM outbox_events WHERE event_id=$1")
            .bind(original)
            .fetch_one(&pool)
            .await?;
    assert_eq!(
        original_payload, original_payload_after,
        "the original event is never edited"
    );
    assert_eq!(original_payload_after["provider_cost_micros"], 100);
    Ok(())
}

/// The payload is facts only. It carries no email, name, prompt text, secret or
/// principal identity.
#[tokio::test]
async fn export_payloads_carry_no_pii_or_secrets() -> Result<()> {
    let Some(pool) = database().await else {
        return Ok(());
    };
    let (project, workspace) = fixture(&pool).await;
    let original = settled_fact(&pool, project, 100).await;
    let correction_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO ai_usage_corrections
           (id, ledger_id, original_cost_micros, corrected_cost_micros,
            original_price_version, corrected_price_version, reason)
         VALUES ($1, $2, 100, 150, 'test-price-v1', 'test-price-v2',
                 'operator note that stays local: re-priced by hand')",
    )
    .bind(correction_id)
    .bind(original)
    .execute(&pool)
    .await
    .expect("correction record inserts");
    replay_usage_facts(
        &pool,
        &UsageExportCounters::default(),
        UsageExportReplayParams::enabled(500),
    )
    .await?;

    let payloads: Vec<Value> = sqlx::query_scalar(
        "SELECT payload FROM outbox_events WHERE message_type='ai_usage.fact' ORDER BY event_id",
    )
    .fetch_all(&pool)
    .await?;
    assert_eq!(payloads.len(), 2);
    let allowed = [
        "fact_id",
        "workspace_id",
        "project_id",
        "principal_kind",
        "provider",
        "model",
        "input_tokens",
        "output_tokens",
        "reasoning_tokens",
        "provider_cost_micros",
        "commercial_units",
        "price_version",
        "period_start",
        "settled_at",
        "correction_of",
    ];
    let denied = [
        "email",
        "name",
        "user_name",
        "username",
        "full_name",
        "principal_id",
        "user_id",
        "prompt",
        "prompt_text",
        "messages",
        "content",
        "reason",
        "secret",
        "password",
        "api_key",
        "access_token",
        "authorization",
        "request_fingerprint",
        "provider_request_id",
        "provider_response_id",
        "step_id",
        "run_id",
        "job_id",
    ];
    for payload in &payloads {
        let object = payload.as_object().expect("payload is an object");
        for key in object.keys() {
            assert!(
                allowed.contains(&key.as_str()),
                "unexpected payload field {key}"
            );
            assert!(!denied.contains(&key.as_str()), "PII or secret field {key}");
        }
        assert_eq!(
            payload["workspace_id"],
            Value::String(workspace.as_uuid().to_string()),
            "workspace id is the external customer identity"
        );
    }
    Ok(())
}

/// The consumer contract: one message version, one consumer, and the handler
/// refuses a payload whose fact id does not match its event id.
#[tokio::test]
async fn the_export_contract_registers_one_consumer_and_binds_fact_id_to_the_event() -> Result<()> {
    let Some(pool) = database().await else {
        return Ok(());
    };
    let sink = Arc::new(ScriptedUsageExportSink::new(vec![
        UsageExportOutcome::Accepted,
    ]));
    let (registry, _counters) = fact_event_registry(&pool, sink.clone());
    assert!(registry.knows(AiUsageFact::TYPE, 1));
    let consumers = registry
        .expected_consumers(AiUsageFact::TYPE, 1)
        .expect("contract registered");
    assert_eq!(
        consumers.iter().cloned().collect::<Vec<_>>(),
        vec![USAGE_EXPORT_CONSUMER.to_owned()]
    );

    let (project, _workspace) = fixture(&pool).await;
    let fact = settled_fact(&pool, project, 100).await;
    replay_usage_facts(
        &pool,
        &UsageExportCounters::default(),
        UsageExportReplayParams::enabled(500),
    )
    .await?;
    let payload: Value = sqlx::query_scalar("SELECT payload FROM outbox_events WHERE event_id=$1")
        .bind(fact)
        .fetch_one(&pool)
        .await?;
    let mut message: AiUsageFact = serde_json::from_value(payload)?;
    message.fact_id = Uuid::new_v4();
    let handler = billing_usage_export_handler(
        &pool,
        Arc::new(ScriptedUsageExportSink::new(vec![
            UsageExportOutcome::Accepted,
        ])),
        Arc::new(UsageExportCounters::default()),
    );
    let context = DeliveryContext {
        event_id: fact,
        consumer: USAGE_EXPORT_CONSUMER.to_owned(),
        message_type: AiUsageFact::TYPE.to_owned(),
        message_version: 1,
        claim_fence: deepref_application::outbox::OutboxClaimFence::new("worker", 0),
    };
    let outcome = handler(message, context).await;
    assert!(
        matches!(
            outcome,
            Err(deepref_application::outbox::OutboxHandlerError::Terminal(_))
        ),
        "a mismatched fact id is terminal, not a retry"
    );
    Ok(())
}
