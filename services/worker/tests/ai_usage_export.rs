#![allow(
    clippy::expect_used,
    clippy::panic,
    clippy::unwrap_used,
    clippy::panic_in_result_fn
)]

//! The durable usage export replay job (handoff 09 slice B) on the 05a
//! execution store: scheduled only behind its rollout gate, and emitting
//! retained usage facts through the 04a outbox contract. An N-1 binary
//! dead-letters a job kind it cannot decode, so nothing of this kind may be
//! claimable while such a binary can claim jobs.

use deepref_ai::{
    AiUsageContext, UsageLedger, UsageObservation, UsagePriceSnapshot, UsagePrincipal,
    UsageReservationRequest,
};
use std::time::Duration;

use deepref_app::test_isolation::isolated_pool_from_env;
use deepref_app::{
    PostgresDurableExecutionStore, disable_ai_usage_export, enable_ai_usage_export, scoped_job,
};
use deepref_application::jobs::{DurableExecutionStore, JobScope};
use deepref_application::{outbox::OutboxMessage, usage_export::AiUsageFact};
use deepref_config::UsageExportConfig;
use deepref_domain::{JobKind, UserId, WorkloadClass};
use deepref_worker::delivery::DeliveryAction;
use deepref_worker::processor::handle_job_with_documents_owned;
use deepref_worker::reconciler::{
    enqueue_ai_usage_export_replay_job, enqueue_ai_usage_reconciliation_jobs,
};
use serde_json::json;
use sqlx::{PgPool, Row};
use uuid::Uuid;

/// The claim statement an N-1 binary runs: no capability filter, and any kind
/// it cannot decode is dead-lettered through its claim-decode path.
const N1_CLAIM: &str = r#"
WITH candidate AS (
  SELECT id FROM jobs
  WHERE state = 'queued' AND available_at <= now() AND attempts < max_attempts
    AND (cardinality($1::text[]) = 0 OR workload_class = ANY($1))
  ORDER BY priority DESC, available_at ASC, id ASC
  FOR UPDATE SKIP LOCKED
  LIMIT $2
)
UPDATE jobs j SET state = 'running', lease_owner = 'n1-worker',
    leased_until = now() + interval '30 seconds',
    lease_renewed_at = now(), attempts = j.attempts + 1
FROM candidate WHERE j.id = candidate.id
RETURNING j.id, j.kind"#;

async fn database() -> Option<PgPool> {
    isolated_pool_from_env("DATABASE_URL", 8).await
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

/// One retained usage fact settled before the export ran.
async fn settled_fact(pool: &PgPool) -> Uuid {
    let fact_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO ai_usage_ledger
           (id, workspace_id, reservation_id, profile, provider, model, purpose,
            input_tokens, output_tokens, cost_micros, price_version)
         VALUES ($1, $2, $3, 'reasoning', 'test-provider', 'test-model', 'structured',
            10, 5, 15, 'test-price-v1')",
    )
    .bind(fact_id)
    .bind(Uuid::new_v4())
    .bind(Uuid::new_v4())
    .execute(pool)
    .await
    .expect("usage fact fixture inserts");
    fact_id
}

fn worker_config(export_enabled: bool) -> deepref_worker::config::WorkerConfig {
    deepref_worker::config::WorkerConfig::from_source(&|name| match name {
        "APP_ENV" => Some("development".into()),
        "DEEPREF_USAGE_EXPORT_ENABLED" if export_enabled => Some("1".into()),
        _ => None,
    })
    .expect("worker configuration builds")
}

fn enabled_settings() -> UsageExportConfig {
    UsageExportConfig {
        enabled: true,
        replay_batch_size: 500,
    }
}

async fn claimed_export_job(pool: &PgPool) -> deepref_application::jobs::ClaimedJob {
    let store = PostgresDurableExecutionStore { pool: pool.clone() };
    let dedupe_key = format!("test:ai_usage_export_replay:{}", Uuid::new_v4());
    store
        .enqueue(scoped_job(
            Uuid::new_v4(),
            JobScope::Platform,
            JobKind::AiUsageExportReplay,
            json!({}),
            dedupe_key,
        ))
        .await
        .expect("export replay job enqueues");
    let mut claimed = store
        .claim(
            "worker-export-test",
            Duration::from_secs(30),
            &[WorkloadClass::Ai],
            1,
        )
        .await
        .expect("export replay job is claimable from the AI pool");
    assert_eq!(claimed.len(), 1);
    claimed.remove(0)
}

async fn export_jobs_in_state(pool: &PgPool, state: &str) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM jobs WHERE kind='ai_usage_export_replay' AND state=$1")
        .bind(state)
        .fetch_one(pool)
        .await
        .expect("export job count")
}

/// One N-1 claim round over every workload pool.
async fn n1_claim(pool: &PgPool) -> Vec<sqlx::postgres::PgRow> {
    sqlx::query(N1_CLAIM)
        .bind(Vec::<String>::new())
        .bind(10_i64)
        .fetch_all(pool)
        .await
        .expect("n1 claim")
}

async fn workspace_fixture(pool: &PgPool) {
    sqlx::query("INSERT INTO projects (id,name) VALUES ($1,'usage export scheduling fixture')")
        .bind(Uuid::new_v4())
        .execute(pool)
        .await
        .expect("project fixture creates its workspace");
}

#[tokio::test]
async fn the_export_replay_job_emits_retained_usage_facts() {
    let Some(pool) = database().await else {
        return;
    };
    sqlx::query(
        "UPDATE ai_usage_rollout_state SET ai_usage_export_activated = true WHERE singleton",
    )
    .execute(&pool)
    .await
    .expect("export gate is open");
    let fact = settled_fact(&pool).await;
    let job = claimed_export_job(&pool).await;
    assert_eq!(job.kind, JobKind::AiUsageExportReplay);
    assert_eq!(job.scope, JobScope::Platform);

    let action = handle_job_with_documents_owned(
        pool.clone(),
        &job,
        &job.lease("worker-export-test"),
        Duration::from_secs(30),
        None,
        None,
        &worker_config(true),
    )
    .await
    .expect("the replay job runs");
    assert_eq!(action, DeliveryAction::Ack);

    let events: i64 =
        sqlx::query_scalar("SELECT count(*) FROM outbox_events WHERE message_type=$1")
            .bind(AiUsageFact::TYPE)
            .fetch_one(&pool)
            .await
            .expect("outbox rows counted");
    assert_eq!(events, 1, "one outbox event per retained fact");
    let event_id: Uuid =
        sqlx::query_scalar("SELECT event_id FROM outbox_events WHERE message_type=$1")
            .bind(AiUsageFact::TYPE)
            .fetch_one(&pool)
            .await
            .expect("outbox row");
    assert_eq!(event_id, fact, "the event id is the immutable fact id");
}

#[tokio::test]
async fn the_export_replay_job_emits_nothing_while_export_is_disabled() {
    let Some(pool) = database().await else {
        return;
    };
    settled_fact(&pool).await;
    sqlx::query("UPDATE ai_usage_rollout_state SET ai_usage_export_activated=true WHERE singleton")
        .execute(&pool)
        .await
        .expect("gate opens before claim");
    let job = claimed_export_job(&pool).await;
    disable_ai_usage_export(&pool, operator(&pool).await)
        .await
        .expect("disable after claim");
    let action = handle_job_with_documents_owned(
        pool.clone(),
        &job,
        &job.lease("worker-export-test"),
        Duration::from_secs(30),
        None,
        None,
        &worker_config(false),
    )
    .await
    .expect("the replay job runs");
    assert_eq!(action, DeliveryAction::Ack);
    let events: i64 =
        sqlx::query_scalar("SELECT count(*) FROM outbox_events WHERE message_type=$1")
            .bind(AiUsageFact::TYPE)
            .fetch_one(&pool)
            .await
            .expect("outbox rows counted");
    assert_eq!(events, 0, "disabled export emits nothing and loses nothing");
}

#[tokio::test]
async fn scheduling_needs_both_the_env_switch_and_the_durable_gate() {
    let Some(pool) = database().await else {
        return;
    };
    let scheduled = enqueue_ai_usage_export_replay_job(&pool, &UsageExportConfig::default())
        .await
        .expect("schedule attempt");
    assert_eq!(scheduled, 0, "no replay job without the env switch");

    let scheduled = enqueue_ai_usage_export_replay_job(&pool, &enabled_settings())
        .await
        .expect("schedule attempt");
    assert_eq!(
        scheduled, 0,
        "no replay job before ai-usage enable-export opens the durable gate"
    );

    enable_ai_usage_export(&pool, operator(&pool).await, 60)
        .await
        .expect("enable-export accepts a fleet that decodes the kind");
    let scheduled = enqueue_ai_usage_export_replay_job(&pool, &enabled_settings())
        .await
        .expect("schedule attempt");
    assert_eq!(scheduled, 1);
    let row = sqlx::query(
        "SELECT kind, scope, workload_class FROM jobs WHERE kind='ai_usage_export_replay'",
    )
    .fetch_one(&pool)
    .await
    .expect("scheduled replay job exists");
    assert_eq!(row.get::<String, _>("kind"), "ai_usage_export_replay");
    assert_eq!(row.get::<String, _>("scope"), "platform");
    assert_eq!(
        row.get::<String, _>("workload_class"),
        WorkloadClass::Ai.as_str(),
        "the replay runs in the AI pool beside ai_usage_reconcile"
    );
}

#[tokio::test]
async fn the_reconciler_schedules_recovery_independently_of_admission() {
    let Some(pool) = database().await else {
        return;
    };
    workspace_fixture(&pool).await;
    let scheduled = enqueue_ai_usage_reconciliation_jobs(&pool)
        .await
        .expect("schedule attempt");
    assert!(
        scheduled >= 1,
        "recovery remains available before admission activation"
    );
    assert!(
        n1_claim(&pool).await.is_empty(),
        "the database fences N-1 recovery claims"
    );

    sqlx::query("UPDATE ai_usage_rollout_state SET ai_hard_caps_enforced = true WHERE singleton")
        .execute(&pool)
        .await
        .expect("hard-cap gate is open");
    let scheduled = enqueue_ai_usage_reconciliation_jobs(&pool)
        .await
        .expect("schedule attempt");
    assert!(
        scheduled >= 1,
        "the recovery kind runs behind its rollout gate"
    );
}

/// N-1 compatibility: while the gate is closed nothing of the export kind is
/// ever claimable, so the N-1 claim (which dead-letters kinds it cannot
/// decode) has nothing to destroy. Disabling parks queued jobs in a state no
/// claim accepts.
#[tokio::test]
async fn an_n1_claim_never_sees_the_export_kind() {
    let Some(pool) = database().await else {
        return;
    };
    let operator = operator(&pool).await;
    workspace_fixture(&pool).await;

    // The gate is closed: the scheduler creates no job of this kind at all.
    enqueue_ai_usage_export_replay_job(&pool, &enabled_settings())
        .await
        .expect("schedule attempt");
    let claimed = n1_claim(&pool).await;
    assert!(
        claimed.is_empty(),
        "no export job exists for an N-1 claim to dead-letter"
    );
    assert_eq!(export_jobs_in_state(&pool, "dead").await, 0);

    // The gate opens and the kind is scheduled; rolling back parks every
    // queued job before any N-1 binary can claim one.
    enable_ai_usage_export(&pool, operator, 60)
        .await
        .expect("enable-export opens the gate");
    for _ in 0..3 {
        let dedupe_key = format!("n1-compat:{}", Uuid::new_v4());
        deepref_app::enqueue_job_pool(
            &pool,
            &scoped_job(
                Uuid::new_v4(),
                JobScope::Platform,
                JobKind::AiUsageExportReplay,
                json!({}),
                dedupe_key,
            ),
        )
        .await
        .expect("export job enqueues");
    }
    assert_eq!(export_jobs_in_state(&pool, "capability_wait").await, 3);
    let parked = disable_ai_usage_export(&pool, operator)
        .await
        .expect("disable-export parks queued work");
    assert_eq!(parked, 3);
    assert_eq!(export_jobs_in_state(&pool, "capability_wait").await, 0);
    assert_eq!(export_jobs_in_state(&pool, "failed").await, 3);

    let claimed = n1_claim(&pool).await;
    assert!(
        claimed.is_empty(),
        "parked export jobs are never claimable by an N-1 binary"
    );
    assert_eq!(
        export_jobs_in_state(&pool, "dead").await,
        0,
        "the N-1 claim dead-lettered nothing"
    );
    let errors: Vec<String> = sqlx::query_scalar(
        "SELECT last_error FROM jobs WHERE kind='ai_usage_export_replay' ORDER BY id",
    )
    .fetch_all(&pool)
    .await
    .expect("park reasons");
    assert!(
        errors.iter().all(|error| error.contains("disable-export")),
        "parked rows keep the operator action as their reason"
    );
}

async fn wait_for_advisory_waiters(pool: &PgPool, count: i64) {
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let waiting: i64 = sqlx::query_scalar(
                "SELECT count(*) FROM pg_locks WHERE locktype='advisory'
                 AND database=(SELECT oid FROM pg_database WHERE datname=current_database())
                 AND classid=913 AND objid=9 AND NOT granted",
            )
            .fetch_one(pool)
            .await
            .expect("job write lock waiters");
            if waiting == count {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("job operation waits for its serialization lock");
}

#[tokio::test]
async fn stale_and_absent_heartbeats_cannot_expose_protected_kinds_to_n1() {
    let Some(pool) = database().await else {
        return;
    };
    let operator = operator(&pool).await;
    workspace_fixture(&pool).await;
    // This old worker is still claiming even though its heartbeat is stale.
    sqlx::query(
        "INSERT INTO worker_heartbeats (owner,seen_at) VALUES ('n1-worker',now()-interval '1 day')",
    )
    .execute(&pool)
    .await
    .expect("stale heartbeat");
    enable_ai_usage_export(&pool, operator, 60)
        .await
        .expect("DB fencing permits activation");
    enqueue_ai_usage_export_replay_job(&pool, &enabled_settings())
        .await
        .expect("export scheduled");
    enqueue_ai_usage_reconciliation_jobs(&pool)
        .await
        .expect("recovery scheduled");
    sqlx::query("UPDATE ai_usage_backfill_state SET completed_at=now() WHERE singleton")
        .execute(&pool)
        .await
        .unwrap();
    deepref_app::enable_ai_hard_caps(&pool, operator, 60)
        .await
        .expect("stale N-1 claimer is fenced independently of admission activation");
    let before: i64 = sqlx::query_scalar("SELECT count(*) FROM jobs WHERE state='capability_wait'")
        .fetch_one(&pool)
        .await
        .expect("queued count");
    assert!(before >= 2);
    assert!(n1_claim(&pool).await.is_empty());
    sqlx::query("DELETE FROM worker_heartbeats")
        .execute(&pool)
        .await
        .expect("heartbeat absent");
    deepref_app::enable_ai_hard_caps(&pool, operator, 60)
        .await
        .expect("absent N-1 heartbeat cannot bypass the database fence");
    assert!(n1_claim(&pool).await.is_empty());
    let attempts: i64 = sqlx::query_scalar("SELECT sum(attempts) FROM jobs")
        .fetch_one(&pool)
        .await
        .expect("attempt count");
    assert_eq!(
        attempts, 0,
        "N-1 never spends attempts or sees a RETURNING row"
    );
    let n = deepref_app::claim_jobs(&pool, "capable-n", Duration::from_secs(30), &[], 100)
        .await
        .expect("N declares supported kinds");
    assert_eq!(n.len() as i64, before);
    for job in n {
        deepref_app::release_job(&pool, &job.lease("capable-n"))
            .await
            .expect("release");
    }
    assert!(
        n1_claim(&pool).await.is_empty(),
        "SET LOCAL never leaks via pooled connections"
    );
}

/// The rollout lock is nonblocking, so disable never queues behind an in-flight
/// job write while holding the exclusive side: it reports the contention and
/// the operator retries. Once it succeeds its parking pass covers the writer
/// that was in flight, and the row guard parks later writers against the
/// closed gate, so "disable, drain, rollback" still leaves no claimable job.
#[tokio::test]
async fn disable_reports_busy_against_an_inflight_enqueue_and_still_parks_late_writers() {
    let Some(pool) = database().await else {
        return;
    };
    let operator = operator(&pool).await;
    enable_ai_usage_export(&pool, operator, 60)
        .await
        .expect("enable");
    assert!(
        deepref_app::ai_usage_rollout_gates(&pool)
            .await
            .unwrap()
            .export_activated
    );
    // The scheduler has read the gate; its enqueue is in flight and owns the
    // shared statement lock until commit. Disable must not wait for it.
    let mut tx = pool.begin().await.unwrap();
    deepref_app::enqueue_job(
        &mut tx,
        &scoped_job(
            Uuid::new_v4(),
            JobScope::Platform,
            JobKind::AiUsageExportReplay,
            json!({}),
            "inflight-export".to_owned(),
        ),
    )
    .await
    .expect("uncommitted enqueue");
    let busy = disable_ai_usage_export(&pool, operator).await;
    assert!(
        matches!(busy, Err(deepref_app::AiUsageError::RolloutBusy)),
        "disable reports contention instead of waiting: {busy:?}"
    );
    assert!(
        deepref_app::ai_usage_rollout_gates(&pool)
            .await
            .unwrap()
            .export_activated,
        "a refused disable changes nothing"
    );
    tx.commit().await.unwrap();
    // Retrying after the writer is done parks that enqueue.
    assert_eq!(
        disable_ai_usage_export(&pool, operator)
            .await
            .expect("disable-export parks queued work"),
        1
    );
    // The scheduler paused after its old gate read, before INSERT.
    deepref_app::enqueue_job_pool(
        &pool,
        &scoped_job(
            Uuid::new_v4(),
            JobScope::Platform,
            JobKind::AiUsageExportReplay,
            json!({}),
            "late-export".to_owned(),
        ),
    )
    .await
    .expect("late enqueue is parked by the DB");
    assert_eq!(export_jobs_in_state(&pool, "capability_wait").await, 0);
    assert_eq!(export_jobs_in_state(&pool, "failed").await, 2);
    assert!(n1_claim(&pool).await.is_empty());
}

#[tokio::test]
async fn enqueue_waiting_on_disable_reads_the_committed_gate() {
    let Some(pool) = database().await else {
        return;
    };
    enable_ai_usage_export(&pool, operator(&pool).await, 60)
        .await
        .unwrap();
    // Hold the same lock/order as disable, pausing before its commit.
    let mut disabling = pool.begin().await.unwrap();
    sqlx::query("SELECT pg_advisory_xact_lock(913,9)")
        .execute(&mut *disabling)
        .await
        .unwrap();
    sqlx::query(
        "UPDATE ai_usage_rollout_state SET ai_usage_export_activated=false WHERE singleton",
    )
    .execute(&mut *disabling)
    .await
    .unwrap();
    let enqueue = tokio::spawn({
        let pool = pool.clone();
        async move {
            deepref_app::enqueue_job_pool(
                &pool,
                &scoped_job(
                    Uuid::new_v4(),
                    JobScope::Platform,
                    JobKind::AiUsageExportReplay,
                    json!({}),
                    "blocked-export".to_owned(),
                ),
            )
            .await
        }
    });
    wait_for_advisory_waiters(&pool, 1).await;
    disabling.commit().await.unwrap();
    enqueue.await.unwrap().unwrap();
    assert_eq!(export_jobs_in_state(&pool, "failed").await, 1);
    assert!(n1_claim(&pool).await.is_empty());
}

#[tokio::test]
async fn release_retry_and_expiry_after_disable_never_republish_claimable_export() {
    let Some(pool) = database().await else {
        return;
    };
    let operator = operator(&pool).await;
    enable_ai_usage_export(&pool, operator, 60).await.unwrap();
    let released = claimed_export_job(&pool).await;
    let retried = claimed_export_job(&pool).await;
    let expired = claimed_export_job(&pool).await;
    disable_ai_usage_export(&pool, operator).await.unwrap();
    assert!(
        deepref_app::release_job(&pool, &released.lease("worker-export-test"))
            .await
            .unwrap()
    );
    assert!(
        deepref_app::fail_job(
            &pool,
            &retried.lease("worker-export-test"),
            "retry",
            deepref_application::jobs::JobRetry::After(Duration::from_secs(1))
        )
        .await
        .unwrap()
    );
    sqlx::query("UPDATE jobs SET leased_until=now()-interval '1 second' WHERE id=$1")
        .bind(expired.id)
        .execute(&pool)
        .await
        .unwrap();
    deepref_app::recover_expired_jobs(&pool).await.unwrap();
    assert_eq!(export_jobs_in_state(&pool, "failed").await, 3);
    assert_eq!(export_jobs_in_state(&pool, "capability_wait").await, 0);
    assert!(n1_claim(&pool).await.is_empty());
}

fn request(
    project_id: deepref_domain::ProjectId,
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

#[tokio::test]
async fn scheduler_and_processor_recover_failed_settlement_after_admission_disable() {
    let Some(pool) = database().await else {
        return;
    };
    workspace_fixture(&pool).await;
    let project: Uuid = sqlx::query_scalar("SELECT id FROM projects LIMIT 1")
        .fetch_one(&pool)
        .await
        .unwrap();
    let workspace: Uuid = sqlx::query_scalar("SELECT workspace_id FROM projects WHERE id=$1")
        .bind(project)
        .fetch_one(&pool)
        .await
        .unwrap();
    sqlx::query("UPDATE ai_usage_limits SET limit_micros=10000000")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("UPDATE ai_usage_period_buckets SET limit_micros=10000000")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("UPDATE ai_usage_rollout_state SET ai_hard_caps_enforced=true WHERE singleton")
        .execute(&pool)
        .await
        .unwrap();
    let ledger = deepref_app::PostgresUsageLedger::new(&pool);
    let project = deepref_domain::ProjectId::new(project);
    let workspace_scope = JobScope::Workspace(deepref_domain::WorkspaceId::new(workspace));
    deepref_app::enqueue_job_pool(
        &pool,
        &scoped_job(
            Uuid::new_v4(),
            workspace_scope,
            JobKind::AiUsageReconcile,
            json!({}),
            "original-provider-job".to_owned(),
        ),
    )
    .await
    .unwrap();
    let original_job =
        deepref_app::claim_jobs(&pool, "provider-n", Duration::from_secs(30), &[], 1)
            .await
            .unwrap()
            .pop()
            .unwrap();
    let mut observed_request = request(project, Uuid::new_v4(), 100, 1000000);
    observed_request.attribution.job_id = Some(original_job.id);
    observed_request.attribution.attempt_fence = Some(original_job.claim_seq);
    let mut unknown_request = request(project, Uuid::new_v4(), 100, 1000000);
    unknown_request.attribution.job_id = Some(original_job.id);
    unknown_request.attribution.attempt_fence = Some(original_job.claim_seq);
    let observed = ledger.reserve(observed_request).await.unwrap();
    let unknown = ledger.reserve(unknown_request).await.unwrap();
    ledger
        .unresolved(&unknown, None, "provider outcome unknown")
        .await
        .unwrap();
    let evidence = UsageObservation {
        input_tokens: Some(25),
        output_tokens: Some(0),
        reasoning_tokens: Some(0),
        commercial_units: None,
        provider_request_id: None,
        provider_response_id: None,
    };
    ledger.observe(&observed, evidence.clone()).await.unwrap();
    sqlx::raw_sql("CREATE FUNCTION test_fail_settlement() RETURNS trigger LANGUAGE plpgsql AS $$
        BEGIN RAISE EXCEPTION 'injected ledger outage'; END $$;
        CREATE TRIGGER test_fail_settlement BEFORE INSERT ON ai_usage_ledger FOR EACH ROW EXECUTE FUNCTION test_fail_settlement();")
        .execute(&pool).await.unwrap();
    assert!(
        ledger
            .settle(&observed, evidence, "test-price-v1")
            .await
            .is_err()
    );
    deepref_app::complete_job(&pool, &original_job.lease("provider-n"))
        .await
        .unwrap();
    let operator = operator(&pool).await;
    deepref_app::disable_ai_hard_caps(&pool, operator)
        .await
        .unwrap();
    assert!(
        ledger
            .reserve(request(project, Uuid::new_v4(), 100, 1000000))
            .await
            .is_err(),
        "admission is closed"
    );
    sqlx::query("DROP TRIGGER test_fail_settlement ON ai_usage_ledger")
        .execute(&pool)
        .await
        .unwrap();
    assert!(enqueue_ai_usage_reconciliation_jobs(&pool).await.unwrap() >= 1);
    assert!(
        n1_claim(&pool).await.is_empty(),
        "N-1 cannot destroy queued recovery"
    );
    let jobs = deepref_app::claim_jobs(
        &pool,
        "recovery-n",
        Duration::from_secs(30),
        &[WorkloadClass::Ai],
        100,
    )
    .await
    .unwrap();
    assert!(!jobs.is_empty());
    for job in jobs {
        let action = handle_job_with_documents_owned(
            pool.clone(),
            &job,
            &job.lease("recovery-n"),
            Duration::from_secs(30),
            None,
            None,
            &worker_config(false),
        )
        .await
        .expect("recovery runs through the real processor");
        assert_eq!(action, DeliveryAction::Ack);
    }
    let state: String = sqlx::query_scalar("SELECT state FROM ai_usage_reservations WHERE id=$1")
        .bind(observed.id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(state, "settled");
    let state: String = sqlx::query_scalar("SELECT state FROM ai_usage_reservations WHERE id=$1")
        .bind(unknown.id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(state, "unresolved", "unknown liability remains observable");
    let held: i64=sqlx::query_scalar("SELECT reserved_micros FROM ai_usage_period_buckets WHERE scope_kind='workspace' AND scope_id=$1 AND period_start=$2")
        .bind(workspace).bind(unknown.period_start).fetch_one(&pool).await.unwrap();
    assert!(held > 0, "unresolved liability retains its hold");
}

#[tokio::test]
async fn protected_pending_work_never_starves_an_n1_limit_one_claim() {
    let Some(pool) = database().await else {
        return;
    };
    let mut protected = scoped_job(
        Uuid::new_v4(),
        JobScope::Platform,
        JobKind::AiUsageReconcile,
        json!({}),
        "protected-head".to_owned(),
    );
    protected.priority = 100;
    deepref_app::enqueue_job_pool(&pool, &protected)
        .await
        .unwrap();
    for ordinal in 0..3 {
        let ordinary = scoped_job(
            Uuid::new_v4(),
            JobScope::Platform,
            JobKind::RecomputeMetrics,
            json!({}),
            format!("ordinary:{ordinal}"),
        );
        deepref_app::enqueue_job_pool(&pool, &ordinary)
            .await
            .unwrap();
        let rows = sqlx::query(N1_CLAIM)
            .bind(Vec::<String>::new())
            .bind(1_i64)
            .fetch_all(&pool)
            .await
            .unwrap();
        assert_eq!(rows.len(), 1, "protected head must not consume N-1's LIMIT");
        assert_eq!(rows[0].get::<Uuid, _>("id"), ordinary.id);
    }
    let row = sqlx::query("SELECT state,attempts FROM jobs WHERE id=$1")
        .bind(protected.id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(row.get::<String, _>("state"), "capability_wait");
    assert_eq!(row.get::<i32, _>("attempts"), 0);
    let claimed = deepref_app::claim_jobs(&pool, "capable-n", Duration::from_secs(30), &[], 1)
        .await
        .unwrap();
    assert_eq!(
        claimed[0].id, protected.id,
        "N can execute protected pending work"
    );
}

#[tokio::test]
async fn n1_expiry_recovery_never_returns_unknown_kinds_and_still_recovers_ordinary_work() {
    let Some(pool) = database().await else {
        return;
    };
    let protected = scoped_job(
        Uuid::new_v4(),
        JobScope::Platform,
        JobKind::AiUsageReconcile,
        json!({}),
        "protected-expiry".to_owned(),
    );
    let ordinary = scoped_job(
        Uuid::new_v4(),
        JobScope::Platform,
        JobKind::RecomputeMetrics,
        json!({}),
        "ordinary-expiry".to_owned(),
    );
    for job in [&protected, &ordinary] {
        deepref_app::enqueue_job_pool(&pool, job).await.unwrap();
    }
    assert_eq!(
        deepref_app::claim_jobs(&pool, "n", Duration::from_secs(30), &[], 10)
            .await
            .unwrap()
            .len(),
        2
    );
    sqlx::query("UPDATE jobs SET leased_until=now()-interval '1 second'")
        .execute(&pool)
        .await
        .unwrap();
    // Exact pre-fence UPDATE/RETURNING shape: no decoder filter or declaration.
    let recovered=sqlx::query("UPDATE jobs SET state=CASE WHEN attempts>=max_attempts THEN 'dead' ELSE 'queued' END,
        last_error='lease expired',available_at=now()+least(30*power(2,greatest(attempts-1,0)),900)*interval '1 second',
        lease_owner=NULL,leased_until=NULL,lease_renewed_at=NULL WHERE state='running' AND leased_until<now()
        RETURNING id,kind,attempts,max_attempts,state").fetch_all(&pool).await.unwrap();
    assert_eq!(recovered.len(), 1);
    assert_eq!(recovered[0].get::<Uuid, _>("id"), ordinary.id);
    assert_eq!(recovered[0].get::<String, _>("kind"), "recompute_metrics");
    let state: String = sqlx::query_scalar("SELECT state FROM jobs WHERE id=$1")
        .bind(protected.id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(
        state, "running",
        "expired protected lease stays available to capable recovery"
    );
    assert_eq!(deepref_app::recover_expired_jobs(&pool).await.unwrap(), 1);
    let state: String = sqlx::query_scalar("SELECT state FROM jobs WHERE id=$1")
        .bind(protected.id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(state, "capability_wait");
}

#[tokio::test]
async fn independent_job_writers_share_the_guard_without_serializing_each_other() {
    let Some(pool) = database().await else {
        return;
    };
    let mut held = pool.begin().await.unwrap();
    deepref_app::enqueue_job(
        &mut held,
        &scoped_job(
            Uuid::new_v4(),
            JobScope::Platform,
            JobKind::AiUsageReconcile,
            json!({}),
            "held-write".to_owned(),
        ),
    )
    .await
    .unwrap();
    tokio::time::timeout(
        Duration::from_secs(5),
        deepref_app::enqueue_job_pool(
            &pool,
            &scoped_job(
                Uuid::new_v4(),
                JobScope::Platform,
                JobKind::RecomputeMetrics,
                json!({}),
                "independent-write".to_owned(),
            ),
        ),
    )
    .await
    .expect("compatible shared lock must permit another writer")
    .unwrap();
    held.commit().await.unwrap();
}

#[tokio::test]
async fn account_deletion_keeps_orphan_workspace_liability_recoverable() {
    use deepref_app::account_deletion::{complete_account_deletions, request_account_deletion};
    use deepref_app::identity::AuditRequest;
    use deepref_domain::{ProjectId, WorkspaceId};

    let Some(pool) = database().await else { return };
    let owner = operator(&pool).await;
    let workspace = WorkspaceId::new(Uuid::new_v4());
    sqlx::query(
        "INSERT INTO workspaces(id,kind,name,slug) VALUES($1,'personal','Deletion recovery',$2)",
    )
    .bind(workspace.as_uuid())
    .bind(format!("deletion-{}", workspace.as_uuid()))
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query("INSERT INTO workspace_memberships(id,workspace_id,user_id,role,status,grant_source) VALUES($1,$2,$3,'owner','active','local')")
        .bind(Uuid::new_v4()).bind(workspace.as_uuid()).bind(owner.as_uuid()).execute(&pool).await.unwrap();
    let observed_project = ProjectId::new(Uuid::new_v4());
    let unknown_project = ProjectId::new(Uuid::new_v4());
    for project in [observed_project, unknown_project] {
        sqlx::query("INSERT INTO projects(id,name,workspace_id) VALUES($1,'Deletion recovery',$2)")
            .bind(project.as_uuid())
            .bind(workspace.as_uuid())
            .execute(&pool)
            .await
            .unwrap();
    }
    sqlx::query("UPDATE ai_usage_limits SET limit_micros=10000000")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("UPDATE ai_usage_period_buckets SET limit_micros=10000000")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("UPDATE ai_usage_rollout_state SET ai_hard_caps_enforced=true WHERE singleton")
        .execute(&pool)
        .await
        .unwrap();
    let ledger = deepref_app::PostgresUsageLedger::new(&pool);
    let mut reservations = Vec::new();
    let mut job_ids = Vec::new();
    for project in [observed_project, unknown_project] {
        let job_id = Uuid::new_v4();
        deepref_app::enqueue_job_pool(
            &pool,
            &scoped_job(
                job_id,
                JobScope::Project(project),
                JobKind::AssistantAgentRun,
                json!({}),
                format!("delete-recovery:{job_id}"),
            ),
        )
        .await
        .unwrap();
        let job = deepref_app::claim_jobs(
            &pool,
            "provider-n",
            Duration::from_secs(30),
            &[WorkloadClass::Ai],
            1,
        )
        .await
        .unwrap()
        .pop()
        .unwrap();
        assert_eq!(job.id, job_id);
        let mut req = request(project, Uuid::new_v4(), 100, 1000000);
        req.attribution.principal = Some(UsagePrincipal::User(owner));
        req.attribution.job_id = Some(job.id);
        req.attribution.attempt_fence = Some(job.claim_seq);
        reservations.push(ledger.reserve(req).await.unwrap());
        job_ids.push(job_id);
    }
    let observed = &reservations[0];
    let unknown = &reservations[1];
    ledger
        .observe(
            observed,
            UsageObservation {
                input_tokens: Some(25),
                output_tokens: Some(0),
                reasoning_tokens: Some(0),
                commercial_units: None,
                provider_request_id: None,
                provider_response_id: None,
            },
        )
        .await
        .unwrap();
    ledger
        .unresolved(unknown, None, "provider outcome unknown")
        .await
        .unwrap();
    request_account_deletion(&pool, owner, AuditRequest::default())
        .await
        .unwrap();
    sqlx::query("UPDATE account_deletion_requests SET requested_at=now()-interval '31 days' WHERE user_id=$1")
        .bind(owner.as_uuid()).execute(&pool).await.unwrap();
    let report =
        complete_account_deletions(&pool, "test-deletion-digest-key", AuditRequest::default())
            .await
            .unwrap();
    assert_eq!(report.logically_anonymized, 1);
    assert_eq!(report.completed, 0);
    assert_eq!(report.physically_pending, 1);
    let fulfilled: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM fulfilled_account_deletions WHERE user_id=$1)",
    )
    .bind(owner.as_uuid())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(
        !fulfilled,
        "logical deletion preserves liabilities without claiming physical erasure"
    );
    assert_eq!(report.workspaces_deleted, 1);
    let remains: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM workspaces WHERE id=$1)")
        .bind(workspace.as_uuid())
        .fetch_one(&pool)
        .await
        .unwrap();
    assert!(!remains);
    let jobs_left: i64 = sqlx::query_scalar("SELECT count(*) FROM jobs WHERE id=ANY($1)")
        .bind(&job_ids)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(
        jobs_left, 0,
        "project deletion removes original jobs, not financial liabilities"
    );
    let held_before:i64=sqlx::query_scalar("SELECT reserved_micros FROM ai_usage_period_buckets WHERE scope_kind='workspace' AND scope_id=$1 AND period_start=$2")
        .bind(workspace.as_uuid()).bind(observed.period_start).fetch_one(&pool).await.unwrap();
    assert_eq!(
        held_before,
        observed.reserved_cost_micros + unknown.reserved_cost_micros
    );
    enqueue_ai_usage_reconciliation_jobs(&pool).await.unwrap();
    assert!(
        n1_claim(&pool).await.is_empty(),
        "the capability fence still excludes N-1"
    );
    let jobs = deepref_app::claim_jobs(
        &pool,
        "deleted-tenant-recovery",
        Duration::from_secs(30),
        &[WorkloadClass::Ai],
        100,
    )
    .await
    .unwrap();
    assert!(
        jobs.iter()
            .any(|j| j.scope == JobScope::Workspace(workspace)),
        "deleted tenant with pending liability must be scheduled"
    );
    for job in jobs {
        let lease = job.lease("deleted-tenant-recovery");
        let action = handle_job_with_documents_owned(
            pool.clone(),
            &job,
            &lease,
            Duration::from_secs(30),
            None,
            None,
            &worker_config(false),
        )
        .await
        .unwrap();
        assert_eq!(action, DeliveryAction::Ack);
        deepref_app::complete_job(&pool, &lease).await.unwrap();
    }
    let state: String = sqlx::query_scalar("SELECT state FROM ai_usage_reservations WHERE id=$1")
        .bind(observed.id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(state, "settled");
    let fact: (Uuid, Option<Uuid>, i64) = sqlx::query_as(
        "SELECT workspace_id,project_id,cost_micros FROM ai_usage_ledger WHERE reservation_id=$1",
    )
    .bind(observed.id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        fact,
        (workspace.as_uuid(), Some(observed_project.as_uuid()), 25),
        "financial history retains tenant/project identity"
    );
    let state: String = sqlx::query_scalar("SELECT state FROM ai_usage_reservations WHERE id=$1")
        .bind(unknown.id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(state, "unresolved");
    let held:i64=sqlx::query_scalar("SELECT reserved_micros FROM ai_usage_period_buckets WHERE scope_kind='workspace' AND scope_id=$1 AND period_start=$2")
        .bind(workspace.as_uuid()).bind(unknown.period_start).fetch_one(&pool).await.unwrap();
    assert_eq!(
        held, unknown.reserved_cost_micros,
        "only proven usage releases its hold"
    );
    for project in [observed_project, unknown_project] {
        let detached: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM ai_usage_detached_projects WHERE project_id=$1)",
        )
        .bind(project.as_uuid())
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(detached, project == unknown_project);
        let buckets:i64=sqlx::query_scalar("SELECT count(*) FROM ai_usage_period_buckets WHERE project_id=$1 AND scope_kind IN ('project','run')")
            .bind(project.as_uuid()).fetch_one(&pool).await.unwrap();
        assert_eq!(buckets, if project == unknown_project { 2 } else { 0 });
    }
    // Once all liabilities are terminal, retained history must not keep an
    // extinct workspace in the scheduler forever.
    deepref_app::resolve_ai_usage_reservation(
        &pool,
        unknown.id,
        operator(&pool).await,
        deepref_app::AiUsageResolution::Void {
            reason: "operator verified zero liability".into(),
        },
    )
    .await
    .unwrap();
    sqlx::query("DELETE FROM jobs WHERE kind='ai_usage_reconcile' AND workspace_id=$1 AND state='completed'")
        .bind(workspace.as_uuid()).execute(&pool).await.unwrap();
    enqueue_ai_usage_reconciliation_jobs(&pool).await.unwrap();
    let scheduled: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM jobs WHERE kind='ai_usage_reconcile' AND workspace_id=$1)",
    )
    .bind(workspace.as_uuid())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(!scheduled);

    // A restore or interrupted historical cleanup can leave a detached marker
    // with terminal liabilities only. It still needs a scheduled sweep.
    sqlx::query("INSERT INTO ai_usage_detached_projects(project_id,workspace_id) VALUES($1,$2)")
        .bind(observed_project.as_uuid())
        .bind(workspace.as_uuid())
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO ai_usage_period_buckets(scope_kind,scope_id,workspace_id,project_id,period_start,limit_micros) VALUES('project',$1,$2,$1,$3,10000000)")
        .bind(observed_project.as_uuid()).bind(workspace.as_uuid()).bind(observed.period_start).execute(&pool).await.unwrap();
    enqueue_ai_usage_reconciliation_jobs(&pool).await.unwrap();
    let jobs = deepref_app::claim_jobs(
        &pool,
        "detached-cleanup",
        Duration::from_secs(30),
        &[WorkloadClass::Ai],
        100,
    )
    .await
    .unwrap();
    assert!(
        jobs.iter()
            .any(|j| j.scope == JobScope::Workspace(workspace)),
        "detached marker alone schedules cleanup"
    );
    for job in jobs {
        let lease = job.lease("detached-cleanup");
        assert_eq!(
            handle_job_with_documents_owned(
                pool.clone(),
                &job,
                &lease,
                Duration::from_secs(30),
                None,
                None,
                &worker_config(false)
            )
            .await
            .unwrap(),
            DeliveryAction::Ack
        );
        deepref_app::complete_job(&pool, &lease).await.unwrap();
    }
    let remaining: i64 =
        sqlx::query_scalar("SELECT count(*) FROM ai_usage_detached_projects WHERE workspace_id=$1")
            .bind(workspace.as_uuid())
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(remaining, 0);
    let buckets: i64 =
        sqlx::query_scalar("SELECT count(*) FROM ai_usage_period_buckets WHERE project_id=$1")
            .bind(observed_project.as_uuid())
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(buckets, 0);
}
