#![allow(
    clippy::expect_used,
    clippy::panic,
    clippy::unwrap_used,
    clippy::string_slice
)]
//! Durable assistant agent runs: lifecycle, idempotency and event ordering.

use deepref_postgres::{
    AppendAssistantMessage, AssistantAgentRunStatus, CompletedAssistantAgentRun,
    NewAssistantAgentRun, NewAssistantPlan, append_assistant_run_event, begin_assistant_agent_run,
    complete_assistant_agent_run, create_assistant_agent_run, get_assistant_agent_run,
    list_assistant_run_events, migrate, submit_assistant_agent_run,
};
use serde_json::json;
use sqlx::{PgPool, postgres::PgPoolOptions};
use uuid::Uuid;

async fn database() -> Option<PgPool> {
    let url = std::env::var("DATABASE_URL").ok()?;
    let pool = PgPoolOptions::new()
        .max_connections(4)
        .connect(&url)
        .await
        .expect("DATABASE_URL database must be reachable");
    migrate(&pool)
        .await
        .expect("DATABASE_URL migrations must apply");
    Some(pool)
}

async fn project(pool: &PgPool) -> (Uuid, Uuid) {
    let project_id = Uuid::new_v4();
    sqlx::query("INSERT INTO projects (id,name) VALUES ($1,'assistant runs')")
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

fn new_run(project_id: Uuid, conversation_id: Uuid, trigger: Uuid) -> NewAssistantAgentRun {
    NewAssistantAgentRun {
        id: Uuid::new_v4(),
        conversation_id,
        project_id,
        trigger_message_id: trigger,
        actor_kind: "user".to_owned(),
        actor_id: "tester".to_owned(),
        model_route: json!({"profile": "reasoning"}),
        semantic_contract_id: None,
        runtime_version: "assistant.agent.v2".to_owned(),
        build_provenance: json!({}),
        answer_message_id: Uuid::new_v4(),
        plan_id: Uuid::new_v4(),
    }
}

async fn user_message(pool: &PgPool, conversation_id: Uuid) -> Uuid {
    deepref_postgres::append_assistant_message(
        pool,
        &AppendAssistantMessage {
            id: None,
            conversation_id,
            role: "user".to_owned(),
            content: "hello".to_owned(),
            tool_calls: None,
            tool_results: None,
            metadata: None,
        },
    )
    .await
    .expect("user message appends")
    .id
}

#[tokio::test]
async fn submit_creates_message_run_and_job_atomically() {
    let Some(pool) = database().await else { return };
    let (project_id, conversation_id) = project(&pool).await;
    let record = submit_assistant_agent_run(
        &pool,
        project_id,
        conversation_id,
        "hello",
        "user",
        "tester",
        &json!({"profile": "reasoning"}),
        "assistant.agent.v2",
    )
    .await
    .expect("submit succeeds");
    assert_eq!(record.status, AssistantAgentRunStatus::Queued);
    let job: String =
        sqlx::query_scalar("SELECT kind FROM jobs WHERE payload->>'assistant_run_id' = $1")
            .bind(record.id.to_string())
            .fetch_one(&pool)
            .await
            .expect("job enqueued");
    assert_eq!(job, "assistant_agent_run");
    sqlx::query("DELETE FROM projects WHERE id=$1")
        .bind(project_id)
        .execute(&pool)
        .await
        .expect("cleanup");
}

#[tokio::test]
async fn run_lifecycle_is_idempotent_under_retry() {
    let Some(pool) = database().await else { return };
    let (project_id, conversation_id) = project(&pool).await;
    let trigger = user_message(&pool, conversation_id).await;
    let input = new_run(project_id, conversation_id, trigger);
    let created = create_assistant_agent_run(&pool, &input)
        .await
        .expect("run creates");
    assert_eq!(created.status, AssistantAgentRunStatus::Queued);

    // Begin takes queued -> running and clears the event log.
    append_assistant_run_event(&pool, created.id, "text", &json!({"delta": "stale"}))
        .await
        .expect("event appends");
    let claimed = begin_assistant_agent_run(&pool, created.id)
        .await
        .expect("begin works")
        .expect("run claimed");
    assert_eq!(claimed.record.status, AssistantAgentRunStatus::Queued);
    assert_eq!(
        list_assistant_run_events(&pool, created.id, -1, 10)
            .await
            .expect("events list")
            .len(),
        0,
        "takeover starts from a clean event log"
    );

    // Complete writes the answer and plan idempotently.
    let answer = AppendAssistantMessage {
        id: claimed.record.answer_message_id,
        conversation_id,
        role: "assistant".to_owned(),
        content: "done".to_owned(),
        tool_calls: None,
        tool_results: None,
        metadata: None,
    };
    let plan = NewAssistantPlan {
        id: claimed.record.plan_id.expect("plan id"),
        project_id,
        conversation_id,
        summary: "plan".to_owned(),
        actions: json!([]),
        created_by_kind: "user".to_owned(),
        created_by_id: "tester".to_owned(),
        model: "m".to_owned(),
        prompt_version: "v".to_owned(),
        evidence: json!([]),
    };
    let completed = CompletedAssistantAgentRun {
        answer,
        plan: Some(plan),
        effect_log: Some(json!({"records": []})),
        run_spec_hash: Some("abc".to_owned()),
    };
    assert!(
        complete_assistant_agent_run(&pool, created.id, &completed)
            .await
            .expect("complete works")
    );
    // A duplicate completion is a no-op, not a second answer or plan.
    assert!(
        !complete_assistant_agent_run(&pool, created.id, &completed)
            .await
            .expect("re-complete works")
    );
    let stored = get_assistant_agent_run(&pool, created.id)
        .await
        .expect("run loads")
        .expect("run exists");
    assert_eq!(stored.status, AssistantAgentRunStatus::Completed);
    assert!(stored.completed_at.is_some());
    let messages: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM assistant_messages WHERE conversation_id=$1 AND role='assistant'",
    )
    .bind(conversation_id)
    .fetch_one(&pool)
    .await
    .expect("message count");
    assert_eq!(messages, 1, "exactly one final answer");
    let plans: i64 =
        sqlx::query_scalar("SELECT count(*) FROM assistant_plans WHERE conversation_id=$1")
            .bind(conversation_id)
            .fetch_one(&pool)
            .await
            .expect("plan count");
    assert_eq!(plans, 1, "exactly one plan");

    // Terminal runs are never re-executed.
    assert!(
        begin_assistant_agent_run(&pool, created.id)
            .await
            .expect("begin works")
            .is_none()
    );

    sqlx::query("DELETE FROM projects WHERE id=$1")
        .bind(project_id)
        .execute(&pool)
        .await
        .expect("cleanup");
}

#[tokio::test]
async fn failed_runs_record_errors_without_answers() {
    let Some(pool) = database().await else { return };
    let (project_id, conversation_id) = project(&pool).await;
    let trigger = user_message(&pool, conversation_id).await;
    let created = create_assistant_agent_run(&pool, &new_run(project_id, conversation_id, trigger))
        .await
        .expect("run creates");
    begin_assistant_agent_run(&pool, created.id)
        .await
        .expect("begin works")
        .expect("run claimed");
    assert!(
        deepref_postgres::fail_assistant_agent_run(&pool, created.id, &json!({"code": "x"}))
            .await
            .expect("fail works")
    );
    // Failing twice is a no-op.
    assert!(
        !deepref_postgres::fail_assistant_agent_run(&pool, created.id, &json!({"code": "x"}))
            .await
            .expect("re-fail works")
    );
    let stored = get_assistant_agent_run(&pool, created.id)
        .await
        .expect("run loads")
        .expect("run exists");
    assert_eq!(stored.status, AssistantAgentRunStatus::Failed);
    let messages: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM assistant_messages WHERE conversation_id=$1 AND role='assistant'",
    )
    .bind(conversation_id)
    .fetch_one(&pool)
    .await
    .expect("message count");
    assert_eq!(messages, 0, "failures append no answer");
    sqlx::query("DELETE FROM projects WHERE id=$1")
        .bind(project_id)
        .execute(&pool)
        .await
        .expect("cleanup");
}

#[tokio::test]
async fn run_events_keep_sequence_order() {
    let Some(pool) = database().await else { return };
    let (project_id, conversation_id) = project(&pool).await;
    let trigger = user_message(&pool, conversation_id).await;
    let created = create_assistant_agent_run(&pool, &new_run(project_id, conversation_id, trigger))
        .await
        .expect("run creates");
    for (kind, payload) in [
        ("status", json!({"message": "Thinking"})),
        ("text", json!({"delta": "hi"})),
        ("done", json!({"message_id": Uuid::new_v4()})),
    ] {
        append_assistant_run_event(&pool, created.id, kind, &payload)
            .await
            .expect("event appends");
    }
    let events = list_assistant_run_events(&pool, created.id, -1, 10)
        .await
        .expect("events list");
    assert_eq!(
        events.iter().map(|event| event.seq).collect::<Vec<_>>(),
        vec![0, 1, 2]
    );
    let tail = list_assistant_run_events(&pool, created.id, 0, 10)
        .await
        .expect("tail lists");
    assert_eq!(tail.len(), 2);
    assert_eq!(tail[0].kind, "text");
    sqlx::query("DELETE FROM projects WHERE id=$1")
        .bind(project_id)
        .execute(&pool)
        .await
        .expect("cleanup");
}
