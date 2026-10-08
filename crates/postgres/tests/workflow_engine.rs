#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]
//! Workflow engine persistence: a repeated notification about the same records
//! is added once, Check reads the stored Slack address, and an undone AI action
//! is no longer reported as undoable.

use deepref_application::workflows::{Endpoint, GraphEdge, GraphNode, Position, WorkflowGraph};
use deepref_domain::{Actor, ActorKind, ProjectId};
use deepref_postgres::{
    ActivityFilters, NewActivity, activity_overview, create_workflow, list_activity, migrate,
    notify_from_workflow, record_activity, validate_workflow_draft,
};
use serde_json::{Value, json};
use sqlx::{PgPool, postgres::PgPoolOptions};
use uuid::Uuid;

async fn database() -> Option<PgPool> {
    let url = std::env::var("DATABASE_URL").ok()?;
    let pool = PgPoolOptions::new()
        .max_connections(5)
        .connect(&url)
        .await
        .expect("DATABASE_URL is set but PostgreSQL is unavailable");
    migrate(&pool).await.expect("migrations should apply");
    Some(pool)
}

async fn insert_project(pool: &PgPool) -> ProjectId {
    let id = Uuid::new_v4();
    sqlx::query("INSERT INTO projects (id,name) VALUES ($1,$2)")
        .bind(id)
        .bind(format!("engine-test-{id}"))
        .execute(pool)
        .await
        .expect("project fixture");
    ProjectId::new(id)
}

async fn remove_project(pool: &PgPool, project: ProjectId) {
    sqlx::query("DELETE FROM projects WHERE id=$1")
        .bind(project.as_uuid())
        .execute(pool)
        .await
        .expect("cleanup");
}

fn node(id: &str, node_type: &str, config: Value) -> GraphNode {
    GraphNode {
        id: id.to_owned(),
        node_type: node_type.to_owned(),
        position: Position::default(),
        label: None,
        config,
    }
}

fn edge(from: &str, from_port: &str, to: &str, to_port: &str) -> GraphEdge {
    GraphEdge {
        id: None,
        from: Endpoint {
            node: from.to_owned(),
            port: from_port.to_owned(),
        },
        to: Endpoint {
            node: to.to_owned(),
            port: to_port.to_owned(),
        },
    }
}

#[tokio::test]
async fn a_repeated_notification_about_the_same_records_is_added_once() {
    let Some(pool) = database().await else {
        return;
    };
    let project = insert_project(&pool).await;
    let payload = json!({ "workflow_id": Uuid::new_v4() });
    let fingerprint = format!("fp-{}", Uuid::new_v4());

    let first = notify_from_workflow(
        &pool,
        project.as_uuid(),
        "info",
        "Found 2 new",
        Some("Two records"),
        payload.clone(),
        Some(&fingerprint),
    )
    .await
    .expect("first notification");
    let repeat = notify_from_workflow(
        &pool,
        project.as_uuid(),
        "info",
        "Found 2 new",
        Some("Two records"),
        payload.clone(),
        Some(&fingerprint),
    )
    .await
    .expect("repeat notification");
    assert!(first, "the first notification is written");
    assert!(
        !repeat,
        "the same notification about the same records is not repeated"
    );

    // A different message about the same records is new information.
    let reworded = notify_from_workflow(
        &pool,
        project.as_uuid(),
        "info",
        "Found 3 new",
        Some("Two records"),
        payload.clone(),
        Some(&format!("fp-{}", Uuid::new_v4())),
    )
    .await
    .expect("reworded notification");
    assert!(reworded);

    // Without any record involved, every run is its own event.
    let plain_a = notify_from_workflow(
        &pool,
        project.as_uuid(),
        "info",
        "Webhook received",
        None,
        payload.clone(),
        None,
    )
    .await
    .expect("plain notification");
    let plain_b = notify_from_workflow(
        &pool,
        project.as_uuid(),
        "info",
        "Webhook received",
        None,
        payload,
        None,
    )
    .await
    .expect("second plain notification");
    assert!(plain_a && plain_b);

    let rows: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM notifications WHERE project_id=$1 AND kind='workflow'",
    )
    .bind(project.as_uuid())
    .fetch_one(&pool)
    .await
    .expect("count");
    assert_eq!(rows, 4, "first, reworded and the two plain ones");
    remove_project(&pool, project).await;
}

#[tokio::test]
async fn check_reads_the_stored_slack_address_and_flags_a_wrong_one() {
    let Some(pool) = database().await else {
        return;
    };
    let project = insert_project(&pool).await;
    let actor = Actor::new(ActorKind::User, "engine-test").expect("actor");
    let slack = |webhook: &str| WorkflowGraph {
        nodes: vec![
            node("t", "trigger.manual", json!({})),
            node(
                "s",
                "integration.slack",
                json!({"webhook": webhook, "message": "hi"}),
            ),
        ],
        edges: vec![edge("t", "data", "s", "data")],
    };

    // The address is stored as a secret; the draft keeps only a placeholder.
    let wrong = create_workflow(
        &pool,
        project,
        "Not Slack",
        "",
        slack("https://example.com/not-slack"),
        &actor,
    )
    .await
    .expect("create");
    let report = validate_workflow_draft(&pool, project, wrong.id)
        .await
        .expect("check");
    assert!(
        report.issues.iter().any(|issue| issue
            .message
            .contains("does not look like a Slack webhook address")),
        "{:?}",
        report.issues
    );

    let good = create_workflow(
        &pool,
        project,
        "Slack",
        "",
        slack("https://hooks.slack.com/services/T000/B000/XXXX"),
        &actor,
    )
    .await
    .expect("create");
    let report = validate_workflow_draft(&pool, project, good.id)
        .await
        .expect("check");
    assert!(report.is_ok(), "{:?}", report.issues);
    remove_project(&pool, project).await;
}

#[tokio::test]
async fn an_undone_ai_action_is_no_longer_undoable() {
    let Some(pool) = database().await else {
        return;
    };
    let project = insert_project(&pool).await;
    let actor = Actor::new(ActorKind::Automation, "workflow:engine-test").expect("actor");
    let mut entry = NewActivity::new(
        project.as_uuid(),
        "automation",
        "Automation",
        actor,
        "screening",
        "screening_decision",
        "An automation marked a record as \"include\".",
    );
    entry.undo_kind = Some("screening_event");
    entry.batch_id = Some(Uuid::new_v4());
    let id = record_activity(&pool, &entry).await.expect("record");

    let before = list_activity(
        &pool,
        project.as_uuid(),
        ActivityFilters::default(),
        None,
        10,
    )
    .await
    .expect("list");
    let row = before.iter().find(|row| row.id == id).expect("entry");
    assert!(row.undoable && row.undone_at.is_none());
    assert_eq!(
        activity_overview(&pool, project.as_uuid())
            .await
            .expect("overview")
            .undoable,
        1
    );

    // The undo service claims the entry by stamping it before it reverses it.
    sqlx::query(
        "UPDATE ai_activity SET undone_at=now(), undone_by_kind='user', undone_by_id='engine-test'
         WHERE id=$1",
    )
    .bind(id)
    .execute(&pool)
    .await
    .expect("undo");

    let after = list_activity(
        &pool,
        project.as_uuid(),
        ActivityFilters::default(),
        None,
        10,
    )
    .await
    .expect("list");
    let row = after.iter().find(|row| row.id == id).expect("entry");
    assert!(row.undone_at.is_some());
    assert!(!row.undoable, "an undone entry cannot be undone again");
    assert_eq!(
        activity_overview(&pool, project.as_uuid())
            .await
            .expect("overview")
            .undoable,
        0
    );
    remove_project(&pool, project).await;
}
