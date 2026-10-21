#![allow(
    clippy::expect_used,
    clippy::panic,
    clippy::unwrap_used,
    clippy::panic_in_result_fn
)]

//! The durable usage export replay job (handoff 09 slice B) on the 05a
//! execution store: scheduled while export is enabled, and emitting retained
//! usage facts through the 04a outbox contract.

use std::time::Duration;

use deepref_app::test_isolation::isolated_pool_from_env;
use deepref_app::{PostgresDurableExecutionStore, scoped_job};
use deepref_application::jobs::{DurableExecutionStore, JobScope};
use deepref_config::UsageExportConfig;
use deepref_domain::{JobKind, WorkloadClass};
use deepref_worker::delivery::DeliveryAction;
use deepref_worker::processor::handle_job_with_documents_owned;
use sqlx::{PgPool, Row};
use uuid::Uuid;

async fn database() -> Option<PgPool> {
    isolated_pool_from_env("DATABASE_URL", 8).await
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

async fn claimed_export_job(pool: &PgPool) -> deepref_application::jobs::ClaimedJob {
    let store = PostgresDurableExecutionStore { pool: pool.clone() };
    let dedupe_key = format!("test:ai_usage_export_replay:{}", Uuid::new_v4());
    store
        .enqueue(scoped_job(
            Uuid::new_v4(),
            JobScope::Platform,
            JobKind::AiUsageExportReplay,
            serde_json::json!({}),
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

#[tokio::test]
async fn the_export_replay_job_emits_retained_usage_facts() {
    let Some(pool) = database().await else {
        return;
    };
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
async fn the_reconciler_schedules_the_replay_only_while_enabled() {
    let Some(pool) = database().await else {
        return;
    };
    let scheduled = deepref_worker::reconciler::enqueue_ai_usage_export_replay_job(
        &pool,
        &UsageExportConfig::default(),
    )
    .await
    .expect("schedule attempt");
    assert_eq!(scheduled, 0, "no replay job while export is disabled");

    let scheduled = deepref_worker::reconciler::enqueue_ai_usage_export_replay_job(
        &pool,
        &UsageExportConfig {
            enabled: true,
            replay_batch_size: 500,
        },
    )
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
