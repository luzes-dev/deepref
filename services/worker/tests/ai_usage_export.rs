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

use std::time::Duration;

use deepref_app::test_isolation::isolated_pool_from_env;
use deepref_app::{
    PostgresDurableExecutionStore, disable_ai_usage_export, enable_ai_usage_export, scoped_job,
};
use deepref_application::jobs::{DurableExecutionStore, JobScope};
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

    let events: i64 = sqlx::query_scalar("SELECT count(*) FROM outbox_events")
        .fetch_one(&pool)
        .await
        .expect("outbox rows counted");
    assert_eq!(events, 1, "one outbox event per retained fact");
    let event_id: Uuid = sqlx::query_scalar("SELECT event_id FROM outbox_events")
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
    let job = claimed_export_job(&pool).await;
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
    let events: i64 = sqlx::query_scalar("SELECT count(*) FROM outbox_events")
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
async fn the_reconciler_schedules_usage_reconcile_only_after_enable_hard_caps() {
    let Some(pool) = database().await else {
        return;
    };
    workspace_fixture(&pool).await;
    let scheduled = enqueue_ai_usage_reconciliation_jobs(&pool)
        .await
        .expect("schedule attempt");
    assert_eq!(
        scheduled, 0,
        "the 09a recovery kind is unknown to N-1 binaries until enable-hard-caps"
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
    assert_eq!(export_jobs_in_state(&pool, "queued").await, 3);
    let parked = disable_ai_usage_export(&pool, operator)
        .await
        .expect("disable-export parks queued work");
    assert_eq!(parked, 3);
    assert_eq!(export_jobs_in_state(&pool, "queued").await, 0);
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
