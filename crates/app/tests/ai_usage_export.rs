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
    claim_outbox_batch, dispatch_outbox_batch, dispatch_outbox_event, insert_outbox_event,
    list_outbox_receipts,
};
use deepref_app::{
    AiUsageError, PostgresUsageLedger, UsageExportError, UsageExportReplayParams,
    billing_usage_export_handler, disable_ai_hard_caps, disable_ai_usage_export,
    enable_ai_hard_caps, enable_ai_usage_export, enqueue_job, enqueue_job_pool, job,
    replay_usage_facts, scoped_job, usage_export_backlog, usage_export_registry,
};
use deepref_application::jobs::JobScope;
use deepref_application::outbox::{DeliveryContext, NewOutboxEvent, OutboxMessage, OutboxRegistry};
use deepref_application::usage_export::{
    AiUsageFact, DisabledUsageExportSink, ScriptedUsageExportSink, USAGE_EXPORT_CONSUMER,
    UsageExportCounters, UsageExportOutcome, UsageExportSink,
};
use deepref_domain::{ProjectId, UserId, WorkspaceId};
use serde_json::{Value, json};
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

    // Fresh, still unemitted facts: two replay tasks start together on the
    // same unemitted page and race to insert the same events.
    let mut fresh: Vec<Uuid> = Vec::new();
    for _ in 0..8 {
        fresh.push(settled_fact(&pool, project, 100).await);
    }
    // Stop both transactions inside their first INSERT, after their scans.
    // A start barrier alone allows the second scan to see zero facts.
    sqlx::raw_sql(
        "CREATE FUNCTION test_pause_export_insert() RETURNS trigger LANGUAGE plpgsql AS $$
         BEGIN PERFORM pg_advisory_xact_lock_shared(914, 9); RETURN NEW; END $$;
         CREATE TRIGGER test_pause_export_insert BEFORE INSERT ON outbox_events
         FOR EACH ROW EXECUTE FUNCTION test_pause_export_insert();",
    )
    .execute(&pool)
    .await?;
    let mut blocker = pool.acquire().await?;
    sqlx::query("SELECT pg_advisory_lock(914, 9)")
        .execute(&mut *blocker)
        .await?;
    let mut tasks = tokio::task::JoinSet::new();
    for _ in 0..2 {
        let pool = pool.clone();
        tasks.spawn(async move {
            let counters = UsageExportCounters::default();
            replay_usage_facts(&pool, &counters, UsageExportReplayParams::enabled(500))
                .await
                .expect("concurrent replay completes")
        });
    }
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let waiting: i64 = sqlx::query_scalar(
                "SELECT count(*) FROM pg_locks WHERE locktype='advisory'
                 AND database=(SELECT oid FROM pg_database WHERE datname=current_database())
                 AND classid=914 AND objid=9 AND NOT granted",
            )
            .fetch_one(&pool)
            .await
            .expect("insert waiters are observable");
            if waiting == 2 {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("both replays scanned before either inserts");
    sqlx::query("SELECT pg_advisory_unlock(914, 9)")
        .execute(&mut *blocker)
        .await?;
    let mut summaries = Vec::new();
    while let Some(result) = tasks.join_next().await {
        let summary = result.expect("replay task joins");
        assert_eq!(
            summary.facts_scanned,
            summary.facts_emitted + summary.facts_already_emitted,
            "every scanned fact is either emitted or already emitted"
        );
        summaries.push(summary);
    }
    let emitted: u64 = summaries.iter().map(|summary| summary.facts_emitted).sum();
    assert_eq!(
        emitted, 8,
        "concurrent replay emits each fresh fact exactly once"
    );
    assert!(summaries.iter().all(|summary| summary.facts_scanned == 8));
    assert_eq!(
        summaries
            .iter()
            .map(|summary| summary.facts_already_emitted)
            .sum::<u64>(),
        8,
        "every competing insert reports the existing fact as a duplicate"
    );
    assert_eq!(fact_event_count(&pool).await, 13);

    let event_ids: Vec<Uuid> = sqlx::query_scalar(
        "SELECT event_id FROM outbox_events WHERE message_type='ai_usage.fact' ORDER BY event_id",
    )
    .fetch_all(&pool)
    .await?;
    let mut expected = facts.clone();
    expected.extend(fresh);
    expected.sort();
    assert_eq!(event_ids, expected, "exactly one outbox row per fact");
    let distinct: i64 = sqlx::query_scalar(
        "SELECT count(DISTINCT event_id) FROM outbox_events WHERE message_type='ai_usage.fact'",
    )
    .fetch_one(&pool)
    .await?;
    assert_eq!(distinct, 13, "no fact id was emitted twice");

    // A later replay finds nothing left to emit.
    let summary =
        replay_usage_facts(&pool, &counters, UsageExportReplayParams::enabled(500)).await?;
    assert!(summary.complete);
    assert_eq!(summary.facts_emitted, 0);
    assert_eq!(fact_event_count(&pool).await, 13);
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

async fn operator(pool: &PgPool) -> UserId {
    let operator = UserId::new(Uuid::new_v4());
    sqlx::query(
        "INSERT INTO users (id, workos_user_id, primary_email, status)
         VALUES ($1, $2, $3, 'active')",
    )
    .bind(operator.as_uuid())
    .bind(format!("usage-export-operator-{}", operator.as_uuid()))
    .bind(format!("usage-export-{}@example.test", operator.as_uuid()))
    .execute(pool)
    .await
    .expect("operator user exists for audit provenance");
    operator
}

async fn insert_correction(
    pool: &PgPool,
    correction_id: Uuid,
    ledger_id: Uuid,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO ai_usage_corrections
           (id, ledger_id, original_cost_micros, corrected_cost_micros,
            original_price_version, corrected_price_version, reason)
         VALUES ($1, $2, 100, 150, 'test-price-v1', 'test-price-v2', 'price table corrected')",
    )
    .bind(correction_id)
    .bind(ledger_id)
    .execute(pool)
    .await?;
    Ok(())
}

async fn jobs_of_kind_in_state(pool: &PgPool, kind: &str, state: &str) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM jobs WHERE kind=$1 AND state=$2")
        .bind(kind)
        .bind(state)
        .fetch_one(pool)
        .await
        .expect("job count")
}

/// Rollout gate (N-1 compatibility): `enable-export` refuses while N-1 AI
/// dispatchers are still writing legacy ledger rows.
#[tokio::test]
async fn enable_export_refuses_during_the_n1_quiet_window() -> Result<()> {
    let Some(pool) = database().await else {
        return Ok(());
    };
    let operator = operator(&pool).await;
    sqlx::query(
        "UPDATE ai_usage_rollout_state SET last_legacy_ledger_insert_at = now() WHERE singleton",
    )
    .execute(&pool)
    .await
    .expect("an N-1 dispatcher just wrote a legacy ledger row");
    let error = enable_ai_usage_export(&pool, operator, 300)
        .await
        .expect_err("N-1 activity inside the quiet window keeps the gate closed");
    assert!(matches!(error, AiUsageError::UsageExportGate(_)));

    sqlx::query(
        "UPDATE ai_usage_rollout_state
         SET last_legacy_ledger_insert_at = now() - interval '1 hour' WHERE singleton",
    )
    .execute(&pool)
    .await
    .expect("the quiet window passes");
    enable_ai_usage_export(&pool, operator, 300)
        .await
        .expect("the quiet window is quiet");
    Ok(())
}

/// Suspending admissions retains queued liability recovery work.
#[tokio::test]
async fn disable_hard_caps_preserves_queued_reconcile_jobs() -> Result<()> {
    let Some(pool) = database().await else {
        return Ok(());
    };
    let (_project, workspace) = fixture(&pool).await;
    let operator = operator(&pool).await;
    for _ in 0..2 {
        enqueue_job_pool(
            &pool,
            &scoped_job(
                Uuid::new_v4(),
                JobScope::Workspace(workspace),
                deepref_domain::JobKind::AiUsageReconcile,
                json!({}),
                format!("n1-compat:reconcile:{}", Uuid::new_v4()),
            ),
        )
        .await?;
    }
    assert_eq!(
        jobs_of_kind_in_state(&pool, "ai_usage_reconcile", "capability_wait").await,
        2
    );
    disable_ai_hard_caps(&pool, operator).await?;
    assert_eq!(
        jobs_of_kind_in_state(&pool, "ai_usage_reconcile", "capability_wait").await,
        2
    );
    assert_eq!(
        jobs_of_kind_in_state(&pool, "ai_usage_reconcile", "failed").await,
        0
    );
    Ok(())
}

/// Usage facts and correction records share one id namespace, enforced for
/// every writer (including N-1 ledger inserts) by the database.
#[tokio::test]
async fn fact_ids_form_one_shared_namespace() -> Result<()> {
    let Some(pool) = database().await else {
        return Ok(());
    };
    let (project, _workspace) = fixture(&pool).await;
    let fact = settled_fact(&pool, project, 100).await;

    let error = insert_correction(&pool, fact, fact)
        .await
        .expect_err("a correction may not reuse a usage fact id");
    assert!(
        error
            .to_string()
            .contains("already used by another fact record"),
        "{error}"
    );

    let correction = Uuid::new_v4();
    insert_correction(&pool, correction, fact).await?;
    let error = sqlx::query(
        "INSERT INTO ai_usage_ledger
           (id, workspace_id, reservation_id, profile, provider, model, purpose,
            input_tokens, output_tokens, cost_micros, price_version)
         VALUES ($1, $2, $3, 'reasoning', 'test-provider', 'test-model', 'structured',
            10, 5, 15, 'test-price-v1')",
    )
    .bind(correction)
    .bind(Uuid::new_v4())
    .bind(Uuid::new_v4())
    .execute(&pool)
    .await
    .expect_err("a usage fact may not reuse a correction id");
    assert!(
        error
            .to_string()
            .contains("already used by another fact record"),
        "{error}"
    );

    // Re-registering the same fact row id is that row's own primary key to
    // decide, and fact ids are never rewritten.
    sqlx::query("UPDATE ai_usage_ledger SET profile = 'renamed' WHERE id = $1")
        .bind(fact)
        .execute(&pool)
        .await
        .expect("facts may be updated");
    let error = sqlx::query("UPDATE ai_usage_ledger SET id = $2 WHERE id = $1")
        .bind(fact)
        .bind(Uuid::new_v4())
        .execute(&pool)
        .await
        .expect_err("fact ids are immutable");
    assert!(error.to_string().contains("immutable"), "{error}");
    Ok(())
}

/// An outbox event that is not this fact's export event is an identity
/// collision: the replay refuses it loudly and the backlog never counts the
/// fact as emitted.
#[tokio::test]
async fn an_unrelated_outbox_event_cannot_shadow_a_fact() -> Result<()> {
    let Some(pool) = database().await else {
        return Ok(());
    };
    let (project, _workspace) = fixture(&pool).await;
    let fact = settled_fact(&pool, project, 100).await;

    let mut tx = pool.begin().await?;
    insert_outbox_event(
        &mut tx,
        &NewOutboxEvent::untyped(
            fact,
            "test.unrelated",
            1,
            json!({"note": "squatting"}),
            &["someone-else"],
        ),
    )
    .await?;
    tx.commit().await?;

    let error = replay_usage_facts(
        &pool,
        &UsageExportCounters::default(),
        UsageExportReplayParams::enabled(500),
    )
    .await
    .expect_err("the collision is not a successful earlier emission");
    assert!(
        matches!(
            &error,
            UsageExportError::EventIdentityConflict { fact_id, message_type }
                if *fact_id == fact && message_type == "test.unrelated"
        ),
        "{error:?}"
    );
    let backlog = usage_export_backlog(&pool).await?;
    assert_eq!(backlog.conflicting_facts, 1);
    assert_eq!(
        backlog.unemitted_facts, 1,
        "the fact is never counted as emitted"
    );
    assert_eq!(backlog.emitted_events, 0);

    // The fact is still not lost: later replays keep refusing loudly instead
    // of skipping it forever.
    let error = replay_usage_facts(
        &pool,
        &UsageExportCounters::default(),
        UsageExportReplayParams::enabled(500),
    )
    .await
    .expect_err("the collision persists until an operator resolves it");
    assert!(matches!(
        error,
        UsageExportError::EventIdentityConflict { .. }
    ));
    Ok(())
}

#[tokio::test]
async fn export_audit_checks_finish_validated() -> Result<()> {
    let Some(pool) = database().await else {
        return Ok(());
    };
    let checks: Vec<bool> = sqlx::query_scalar(
        "SELECT convalidated FROM pg_constraint WHERE conrelid='admin_audit_events'::regclass
         AND conname IN ('admin_audit_events_event_type_check','admin_audit_events_check')",
    )
    .fetch_all(&pool)
    .await?;
    assert_eq!(checks, vec![true, true]);
    let job_state_validated: bool = sqlx::query_scalar("SELECT convalidated FROM pg_constraint WHERE conrelid='jobs'::regclass AND conname='jobs_state_check'")
        .fetch_one(&pool).await?;
    assert!(job_state_validated);

    Ok(())
}

/// A rollout command must answer; a hang is a failure, not a slow pass.
async fn bounded<T: std::fmt::Debug>(command: impl std::future::Future<Output = T>) -> T {
    tokio::time::timeout(Duration::from_secs(10), command)
        .await
        .expect("a rollout transition answers instead of waiting")
}

/// Every rollout transition shares one lock pattern: job-write advisory lock
/// exclusively, then the rollout singleton row. It must never wait for either
/// while holding the other — a waiting exclusive holder closes the cycle with
/// ordinary enqueues (project row, then job INSERT) and N-1 ledger writers
/// (rollout row, then project row). Contention is a typed busy error returned
/// after every lock is released; ordinary job writes stay concurrent; all four
/// commands succeed again as soon as the row is free.
#[tokio::test]
async fn every_rollout_transition_refuses_instead_of_waiting_for_the_rollout_row() -> Result<()> {
    let Some(pool) = database().await else {
        return Ok(());
    };
    let (project, _workspace) = fixture(&pool).await;
    let operator = operator(&pool).await;
    // Another writer owns the rollout singleton row, exactly as an N-1 ledger
    // insert does before it waits for a project row.
    let mut owner = pool.begin().await?;
    sqlx::query("SELECT singleton FROM ai_usage_rollout_state WHERE singleton FOR UPDATE")
        .execute(&mut *owner)
        .await?;
    // Ordinary job writes share the guard and stay concurrent with the held
    // rollout row.
    tokio::time::timeout(
        Duration::from_secs(5),
        enqueue_job_pool(
            &pool,
            &job(
                Uuid::new_v4(),
                project,
                deepref_domain::JobKind::RecomputeMetrics,
                json!({}),
                format!("rollout-contention:{}", Uuid::new_v4()),
            ),
        ),
    )
    .await
    .expect("a held rollout row never blocks ordinary job writes")?;
    // Two rounds prove the refusals release their locks instead of leaking one.
    for round in 0..2 {
        let busy = bounded(enable_ai_hard_caps(&pool, operator, 60)).await;
        assert!(
            matches!(busy, Err(AiUsageError::RolloutBusy)),
            "round {round}: enable-hard-caps refuses instead of waiting: {busy:?}"
        );
        let busy = bounded(disable_ai_hard_caps(&pool, operator)).await;
        assert!(
            matches!(busy, Err(AiUsageError::RolloutBusy)),
            "round {round}: disable-hard-caps refuses instead of waiting: {busy:?}"
        );
        let busy = bounded(enable_ai_usage_export(&pool, operator, 60)).await;
        assert!(
            matches!(busy, Err(AiUsageError::RolloutBusy)),
            "round {round}: enable-export refuses instead of waiting: {busy:?}"
        );
        let busy = bounded(disable_ai_usage_export(&pool, operator)).await;
        assert!(
            matches!(busy, Err(AiUsageError::RolloutBusy)),
            "round {round}: disable-export refuses instead of waiting: {busy:?}"
        );
    }
    owner.commit().await?;
    // The refusals released every lock: all four commands work again.
    bounded(enable_ai_usage_export(&pool, operator, 60)).await?;
    bounded(disable_ai_usage_export(&pool, operator)).await?;
    bounded(enable_ai_hard_caps(&pool, operator, 60)).await?;
    bounded(disable_ai_hard_caps(&pool, operator)).await?;
    Ok(())
}

/// The exact three-transaction interleaving the blocking rollout lock created
/// (A -> C -> B -> A): an ordinary enqueue transaction owns the project row
/// before its job INSERT (graph `enqueue_metrics_recompute`), an N-1 ledger
/// writer owns the rollout singleton row before it waits for that project row
/// (0120 `ai_usage_ledger_count_legacy_spend`), and `disable-export` runs
/// concurrently in between. Ordinary work and the legacy writer must both
/// commit — no 40P01 — while disable reports the contention and succeeds on a
/// clean retry.
#[tokio::test]
async fn rollout_transitions_never_deadlock_an_enqueue_and_a_legacy_ledger_writer() -> Result<()> {
    let Some(pool) = database().await else {
        return Ok(());
    };
    let (project, _workspace) = fixture(&pool).await;
    let operator = operator(&pool).await;

    // A: `enqueue_metrics_recompute`'s own shape, paused between the project
    // row it owns and the job INSERT that shares the job-write guard.
    let mut enqueueing = pool.begin().await?;
    let enqueue_pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *enqueueing)
        .await?;
    sqlx::query(
        "UPDATE projects SET metric_requested_revision = metric_requested_revision + 1
         WHERE id = $1",
    )
    .bind(project.as_uuid())
    .execute(&mut *enqueueing)
    .await?;

    // B: an N-1 legacy ledger write. Its trigger owns the rollout singleton
    // row, then waits for A's project row.
    let mut legacy = pool.acquire().await?;
    let legacy_pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *legacy)
        .await?;
    let fact = Uuid::new_v4();
    let legacy_write = tokio::spawn(async move {
        sqlx::query(
            "INSERT INTO ai_usage_ledger
               (id, project_id, profile, provider, model, purpose,
                input_tokens, output_tokens, cost_micros)
             VALUES ($1, $2, 'reasoning', 'test-provider', 'test-model', 'structured', 10, 5, 15)",
        )
        .bind(fact)
        .bind(project.as_uuid())
        .execute(&mut *legacy)
        .await
    });
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let blocked_by_enqueue: bool = sqlx::query_scalar(
                "SELECT cardinality(pg_blocking_pids($1)) > 0 AND $2 = ANY(pg_blocking_pids($1))",
            )
            .bind(legacy_pid)
            .bind(enqueue_pid)
            .fetch_one(&pool)
            .await
            .expect("legacy writer lock state");
            if blocked_by_enqueue {
                break;
            }
            assert!(
                !legacy_write.is_finished(),
                "the legacy writer waits on the project row instead of failing"
            );
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("the N-1 ledger writer owns the rollout row and waits for the project row");

    // C: disable-export runs concurrently, the way an operator command does.
    // It must refuse cleanly instead of waiting for B's rollout row while
    // holding the job-write lock.
    let disabling = tokio::spawn({
        let pool = pool.clone();
        async move { disable_ai_usage_export(&pool, operator).await }
    });
    // Give the command its transaction window. A blocking acquisition parks
    // here on B's rollout row while owning the job-write lock — the state that
    // turned the next INSERT into a three-transaction cycle.
    tokio::time::sleep(Duration::from_millis(250)).await;

    // A reaches its job INSERT while C is active and commits: ordinary work
    // survives. With the blocking lock this closes A -> C -> B -> A and
    // PostgreSQL must abort one of the three with 40P01.
    tokio::time::timeout(
        Duration::from_secs(10),
        enqueue_job(
            &mut enqueueing,
            &job(
                Uuid::new_v4(),
                project,
                deepref_domain::JobKind::RecomputeMetrics,
                json!({}),
                format!("deadlock-regression:{fact}"),
            ),
        ),
    )
    .await
    .expect("the ordinary job INSERT is never trapped behind a rollout transition")?;

    // B is still blocked on A's project row, so the command's outcome is
    // decided contention, not a race with B's release.
    let outcome = tokio::time::timeout(Duration::from_secs(10), disabling)
        .await
        .expect("disable-export answers instead of queueing behind the rollout row")
        .expect("disable-export task joins");
    enqueueing.commit().await?;

    // B commits too: the legacy spend is counted and no transaction was
    // aborted with 40P01. With the blocking lock the database must abort one
    // of the three transactions, and this is where that abort surfaces.
    tokio::time::timeout(Duration::from_secs(10), legacy_write)
        .await
        .expect("the legacy ledger write completes once the project row is free")??;
    assert!(
        matches!(outcome, Err(AiUsageError::RolloutBusy)),
        "disable-export refuses instead of closing the A -> C -> B -> A cycle: {outcome:?}"
    );
    let last_legacy: Option<chrono::DateTime<chrono::Utc>> = sqlx::query_scalar(
        "SELECT last_legacy_ledger_insert_at FROM ai_usage_rollout_state WHERE singleton",
    )
    .fetch_one(&pool)
    .await?;
    assert!(
        last_legacy.is_some(),
        "the legacy accounting write landed and is counted"
    );
    let enqueued: i64 =
        sqlx::query_scalar("SELECT count(*) FROM jobs WHERE kind='recompute_metrics'")
            .fetch_one(&pool)
            .await?;
    assert_eq!(enqueued, 1, "the ordinary enqueue committed");

    // The operator retries after the writers are done and succeeds.
    assert_eq!(disable_ai_usage_export(&pool, operator).await?, 0);
    Ok(())
}
