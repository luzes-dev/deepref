#![allow(
    clippy::expect_used,
    clippy::panic,
    clippy::unwrap_used,
    clippy::string_slice
)]
//! Durable assistant agent runs: the worker owns execution, the browser only
//! observes. Skipped unless `DATABASE_URL` points at a reachable PostgreSQL.

use std::{sync::Arc, time::Duration};

use deepref_ai::{ModelParameters, ModelProfile, ResolvedModel, runtime::StaticModelFactory};
use deepref_application::jobs::ClaimedJob;
use deepref_postgres::{claim_job, migrate, submit_assistant_agent_run};
use deepref_worker::{
    assistant::{AssistantWorkerServices, handle_assistant_agent_run_with},
    delivery::DeliveryAction,
};
use rig_core::test_utils::{MockCompletionModel, MockStreamEvent};
use serde_json::{Value, json};
use sqlx::{PgPool, postgres::PgPoolOptions};
use uuid::Uuid;

static DATABASE_TEST_MUTEX: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

async fn database() -> Option<PgPool> {
    let url = std::env::var("DATABASE_URL").ok()?;
    let pool = PgPoolOptions::new()
        .max_connections(8)
        .connect(&url)
        .await
        .expect("DATABASE_URL must be reachable");
    migrate(&pool).await.expect("migrations apply");
    let started = chrono::Utc::now();
    sqlx::query(
        "UPDATE jobs SET state='dead', last_error='parked by worker assistant tests', completed_at=now()
         WHERE state='queued' AND created_at < $1",
    )
    .bind(started)
    .execute(&pool)
    .await
    .expect("park stale jobs");
    Some(pool)
}

async fn project(pool: &PgPool) -> (Uuid, Uuid) {
    let project_id = Uuid::new_v4();
    sqlx::query("INSERT INTO projects (id,name) VALUES ($1,'assistant worker')")
        .bind(project_id)
        .execute(pool)
        .await
        .expect("project inserts");
    let conversation_id: Uuid = sqlx::query_scalar(
        "INSERT INTO assistant_conversations (id,project_id,title) VALUES ($1,$2,'t') RETURNING id",
    )
    .bind(Uuid::new_v4())
    .bind(project_id)
    .fetch_one(pool)
    .await
    .expect("conversation inserts");
    (project_id, conversation_id)
}

fn route() -> ResolvedModel {
    ResolvedModel {
        profile: ModelProfile::Reasoning,
        provider: "test-provider".to_owned(),
        model: "test-model".to_owned(),
        model_version: "v1".to_owned(),
        parameters: ModelParameters::default(),
        route_id: None,
    }
}

fn services(model: MockCompletionModel) -> AssistantWorkerServices {
    AssistantWorkerServices {
        model_factory: Arc::new(StaticModelFactory::new(model.erase())),
        prices: deepref_ai::PriceBook::default(),
    }
}

fn stream_text(text: &str) -> Vec<MockStreamEvent> {
    vec![
        MockStreamEvent::text(text.to_owned()),
        MockStreamEvent::final_response_with_total_tokens(4),
    ]
}

fn stream_call(id: &str, name: &str, args: Value) -> Vec<MockStreamEvent> {
    vec![
        MockStreamEvent::tool_call(id.to_owned(), name.to_owned(), args)
            .with_call_id(id.to_owned()),
        MockStreamEvent::final_response_with_total_tokens(4),
    ]
}

async fn submit(pool: &PgPool, project_id: Uuid, conversation_id: Uuid) -> Uuid {
    submit_assistant_agent_run(
        pool,
        project_id,
        conversation_id,
        "exclude the off-topic one",
        "user",
        "tester",
        &serde_json::to_value(route()).expect("route serializes"),
        "assistant.agent.v2",
    )
    .await
    .expect("submit succeeds")
    .id
}

async fn claim(pool: &PgPool) -> ClaimedJob {
    claim_job(pool, "assistant-test-worker", Duration::from_secs(30))
        .await
        .expect("claim works")
        .expect("a job is queued")
}

async fn run_record(pool: &PgPool, run_id: Uuid) -> deepref_postgres::AssistantAgentRunRecord {
    deepref_postgres::get_assistant_agent_run(pool, run_id)
        .await
        .expect("run loads")
        .expect("run exists")
}

#[tokio::test]
async fn worker_completes_a_run_with_one_answer_one_plan_and_a_cassette() {
    let _guard = DATABASE_TEST_MUTEX.lock().await;
    let Some(pool) = database().await else { return };
    let (project_id, conversation_id) = project(&pool).await;
    let report_id = Uuid::new_v4();
    let model = MockCompletionModel::from_stream_turns(vec![
        stream_call(
            "c1",
            "screen_reports",
            json!({
                "report_ids": [report_id],
                "decision": "exclude",
                "summary": "Exclude 1 off-topic record",
                "rationale": "Not about the review question",
            }),
        ),
        stream_call(
            "c2",
            "request_protocol_publish",
            json!({"summary": "Publish protocol", "rationale": "asked"}),
        ),
        stream_text("I prepared a plan; nothing has changed yet."),
    ]);
    let run_id = submit(&pool, project_id, conversation_id).await;
    let job = claim(&pool).await;
    assert_eq!(job.kind, "assistant_agent_run");

    let action = handle_assistant_agent_run_with(&pool, &job, &services(model)).await;
    assert!(
        matches!(action, Ok(DeliveryAction::Ack)),
        "completed runs ack, got {action:?}"
    );

    let run = run_record(&pool, run_id).await;
    assert_eq!(
        run.status,
        deepref_postgres::AssistantAgentRunStatus::Completed
    );
    assert!(run.completed_at.is_some());
    assert!(run.effect_log.is_some(), "cassette effect log persists");
    assert!(run.run_spec_hash.is_some());

    let message: Value =
        sqlx::query_scalar("SELECT to_jsonb(m) FROM assistant_messages m WHERE id=$1")
            .bind(run.answer_message_id.expect("answer id"))
            .fetch_one(&pool)
            .await
            .expect("answer loads");
    assert_eq!(
        message["content"],
        "I prepared a plan; nothing has changed yet."
    );
    assert_eq!(message["role"], "assistant");

    let plan: Value = sqlx::query_scalar("SELECT to_jsonb(p) FROM assistant_plans p WHERE id=$1")
        .bind(run.plan_id.expect("plan id"))
        .fetch_one(&pool)
        .await
        .expect("plan loads");
    assert_eq!(plan["status"], "pending");
    assert_eq!(plan["actions"].as_array().map(Vec::len), Some(2));
    assert_eq!(plan["actions"][0]["tool"], "screen_reports");
    assert_eq!(plan["actions"][1]["tool"], "request_protocol_publish");

    // Nothing was applied: proposals stay plans until a human confirms.
    let events: Vec<String> =
        sqlx::query_scalar("SELECT kind FROM assistant_run_events WHERE run_id=$1 ORDER BY seq")
            .bind(run_id)
            .fetch_all(&pool)
            .await
            .expect("events load");
    assert!(events.contains(&"tool_start".to_owned()), "{events:?}");
    assert!(events.contains(&"tool_complete".to_owned()), "{events:?}");
    assert!(events.contains(&"plan".to_owned()), "{events:?}");
    assert!(events.contains(&"done".to_owned()), "{events:?}");

    sqlx::query("DELETE FROM projects WHERE id=$1")
        .bind(project_id)
        .execute(&pool)
        .await
        .expect("cleanup");
}

#[tokio::test]
async fn duplicate_delivery_keeps_one_answer_and_one_plan() {
    let _guard = DATABASE_TEST_MUTEX.lock().await;
    let Some(pool) = database().await else { return };
    let (project_id, conversation_id) = project(&pool).await;
    let model = MockCompletionModel::from_stream_turns(vec![stream_text("Done.")]);
    let run_id = submit(&pool, project_id, conversation_id).await;
    let job = claim(&pool).await;

    for _ in 0..2 {
        let action = handle_assistant_agent_run_with(&pool, &job, &services(model.clone())).await;
        assert!(matches!(action, Ok(DeliveryAction::Ack)), "{action:?}");
    }

    let messages: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM assistant_messages WHERE conversation_id=$1 AND role='assistant'",
    )
    .bind(conversation_id)
    .fetch_one(&pool)
    .await
    .expect("message count");
    assert_eq!(messages, 1, "duplicate delivery keeps one answer");
    let run = run_record(&pool, run_id).await;
    assert_eq!(
        run.status,
        deepref_postgres::AssistantAgentRunStatus::Completed
    );

    sqlx::query("DELETE FROM projects WHERE id=$1")
        .bind(project_id)
        .execute(&pool)
        .await
        .expect("cleanup");
}

#[tokio::test]
async fn provider_failure_requeues_then_terminal_exhaustion_fails_the_run() {
    let _guard = DATABASE_TEST_MUTEX.lock().await;
    let Some(pool) = database().await else { return };
    let (project_id, conversation_id) = project(&pool).await;
    let failing = MockCompletionModel::from_stream_turns(vec![vec![MockStreamEvent::error(
        "boom".to_owned(),
    )]]);
    let run_id = submit(&pool, project_id, conversation_id).await;
    let job = claim(&pool).await;

    let action = handle_assistant_agent_run_with(&pool, &job, &services(failing)).await;
    assert!(
        matches!(action, Ok(DeliveryAction::Nak(_))),
        "transient provider failures requeue, got {action:?}"
    );
    let run = run_record(&pool, run_id).await;
    assert_eq!(
        run.status,
        deepref_postgres::AssistantAgentRunStatus::Running
    );
    let messages: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM assistant_messages WHERE conversation_id=$1 AND role='assistant'",
    )
    .bind(conversation_id)
    .fetch_one(&pool)
    .await
    .expect("message count");
    assert_eq!(messages, 0, "failed runs append no answer");

    // Last attempt: the run fails closed instead of retrying forever.
    let exhausted = ClaimedJob {
        attempts: job.max_attempts,
        ..job
    };
    let action =
        handle_assistant_agent_run_with(&pool, &exhausted, &services(failing_model())).await;
    assert!(
        matches!(action, Ok(DeliveryAction::Terminate)),
        "exhausted runs terminate, got {action:?}"
    );
    let run = run_record(&pool, run_id).await;
    assert_eq!(
        run.status,
        deepref_postgres::AssistantAgentRunStatus::Failed
    );
    assert!(run.error.is_some());

    sqlx::query("DELETE FROM projects WHERE id=$1")
        .bind(project_id)
        .execute(&pool)
        .await
        .expect("cleanup");
}

fn failing_model() -> MockCompletionModel {
    MockCompletionModel::from_stream_turns(vec![vec![MockStreamEvent::error("boom".to_owned())]])
}

#[tokio::test]
async fn exhausted_budget_fails_the_run_without_an_answer() {
    let _guard = DATABASE_TEST_MUTEX.lock().await;
    let Some(pool) = database().await else { return };
    let (project_id, conversation_id) = project(&pool).await;
    deepref_postgres::set_ai_budget(&pool, project_id, 1)
        .await
        .expect("budget");
    sqlx::query(
        "INSERT INTO ai_usage_ledger (project_id,profile,provider,model,purpose,input_tokens,output_tokens,cost_micros)
         VALUES ($1,'reasoning','test-provider','test-model','chat',1,1,10)",
    )
    .bind(project_id)
    .execute(&pool)
    .await
    .expect("ledger insert");
    let model = MockCompletionModel::from_stream_turns(vec![stream_text("unused")]);
    let run_id = submit(&pool, project_id, conversation_id).await;
    let job = claim(&pool).await;

    let action = handle_assistant_agent_run_with(&pool, &job, &services(model)).await;
    assert!(matches!(action, Ok(DeliveryAction::Ack)), "{action:?}");
    let run = run_record(&pool, run_id).await;
    assert_eq!(
        run.status,
        deepref_postgres::AssistantAgentRunStatus::Failed
    );
    assert!(
        run.error
            .as_ref()
            .and_then(|error| error.get("code"))
            .and_then(Value::as_str)
            == Some("ai_budget_exceeded")
    );
    let messages: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM assistant_messages WHERE conversation_id=$1 AND role='assistant'",
    )
    .bind(conversation_id)
    .fetch_one(&pool)
    .await
    .expect("message count");
    assert_eq!(messages, 0);

    sqlx::query("DELETE FROM projects WHERE id=$1")
        .bind(project_id)
        .execute(&pool)
        .await
        .expect("cleanup");
}

#[tokio::test]
async fn recovered_lease_redrives_to_a_single_completion() {
    let _guard = DATABASE_TEST_MUTEX.lock().await;
    let Some(pool) = database().await else { return };
    let (project_id, conversation_id) = project(&pool).await;
    let model = MockCompletionModel::from_stream_turns(vec![stream_text("Done.")]);
    let run_id = submit(&pool, project_id, conversation_id).await;
    let job = claim(&pool).await;

    // First attempt fails transiently after beginning the run.
    let failing = MockCompletionModel::from_stream_turns(vec![vec![MockStreamEvent::error(
        "blip".to_owned(),
    )]]);
    let action = handle_assistant_agent_run_with(&pool, &job, &services(failing)).await;
    assert!(matches!(action, Ok(DeliveryAction::Nak(_))), "{action:?}");

    // Recovery re-drives the still-running run from a clean event log.
    let action = handle_assistant_agent_run_with(&pool, &job, &services(model)).await;
    assert!(matches!(action, Ok(DeliveryAction::Ack)), "{action:?}");

    let run = run_record(&pool, run_id).await;
    assert_eq!(
        run.status,
        deepref_postgres::AssistantAgentRunStatus::Completed
    );
    let messages: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM assistant_messages WHERE conversation_id=$1 AND role='assistant'",
    )
    .bind(conversation_id)
    .fetch_one(&pool)
    .await
    .expect("message count");
    assert_eq!(messages, 1);

    sqlx::query("DELETE FROM projects WHERE id=$1")
        .bind(project_id)
        .execute(&pool)
        .await
        .expect("cleanup");
}
