#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]
//! The run history shows how many records an AI review sent out on each
//! verdict. Runs without a finished screening step have no counts.

use deepref_application::workflows::{GraphNode, Position, WorkflowGraph};
use deepref_domain::{Actor, ActorKind, ProjectId};
use deepref_postgres::{
    ReviewCounts, StartRun, create_workflow, migrate, review_counts_for_runs, start_run,
};
use serde_json::{Value, json};
use sqlx::{PgPool, postgres::PgPoolOptions};
use uuid::Uuid;

async fn database() -> Option<PgPool> {
    let url = std::env::var("DATABASE_URL").ok()?;
    let pool = PgPoolOptions::new()
        .max_connections(4)
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
        .bind(format!("review-counts-{id}"))
        .execute(pool)
        .await
        .expect("project fixture");
    ProjectId::new(id)
}

fn node(id: &str, node_type: &str) -> GraphNode {
    GraphNode {
        id: id.to_owned(),
        node_type: node_type.to_owned(),
        position: Position::default(),
        label: None,
        config: json!({}),
    }
}

/// Starts a test run of `graph` and records the given finished steps as its
/// node runs (node id, node type, output).
async fn run_with_steps(
    pool: &PgPool,
    project: Uuid,
    workflow: Uuid,
    graph: &WorkflowGraph,
    steps: &[(&str, &str, Value)],
) -> Uuid {
    let started = start_run(
        pool,
        &StartRun {
            project_id: project,
            workflow_id: workflow,
            version_id: None,
            graph: graph.clone(),
            trigger_kind: "test".to_owned(),
            trigger_data: json!({}),
            idempotency_key: format!("test:{}", Uuid::new_v4()),
            actor_kind: "user".to_owned(),
            actor_id: "review-counts".to_owned(),
            test_mode: true,
        },
    )
    .await
    .expect("run starts");
    for (node_id, node_type, output) in steps {
        // A run already has a row for every node of its graph; record the outcome on it.
        sqlx::query(
            "INSERT INTO workflow_node_runs
               (project_id, run_id, node_id, iteration, node_type, status, output)
             VALUES ($1,$2,$3,'',$4,'completed',$5)
             ON CONFLICT (run_id, node_id, iteration)
             DO UPDATE SET node_type = EXCLUDED.node_type,
                           status = 'completed',
                           output = EXCLUDED.output",
        )
        .bind(project)
        .bind(started.run_id)
        .bind(*node_id)
        .bind(*node_type)
        .bind(output)
        .execute(pool)
        .await
        .expect("node run fixture");
    }
    started.run_id
}

#[tokio::test]
async fn verdicts_of_every_screening_step_are_counted_per_run() {
    let Some(pool) = database().await else {
        return;
    };
    let project = insert_project(&pool).await;
    let actor = Actor::new(ActorKind::User, "review-counts").expect("actor");
    let graph = WorkflowGraph {
        nodes: vec![
            node("start", "trigger.manual"),
            node("screen", "ai.run_review"),
            node("screen_again", "ai.run_review"),
        ],
        edges: vec![],
    };
    let workflow = create_workflow(
        &pool,
        project,
        "Screen with AI",
        "Review counts",
        graph.clone(),
        &actor,
    )
    .await
    .expect("workflow")
    .id;

    let screened = run_with_steps(
        &pool,
        project.as_uuid(),
        workflow,
        &graph,
        &[
            ("start", "trigger.manual", json!({"data": {}})),
            (
                "screen",
                "ai.run_review",
                json!({
                    "records": [{}, {}, {}, {}, {}, {}],
                    "included": [{}, {}, {}],
                    "excluded": [{}],
                    "unsure": [{}, {}],
                }),
            ),
            (
                "screen_again",
                "ai.run_review",
                json!({"records": [{}], "included": [{}], "excluded": [], "unsure": []}),
            ),
        ],
    )
    .await;
    let plain = run_with_steps(
        &pool,
        project.as_uuid(),
        workflow,
        &graph,
        &[("start", "trigger.manual", json!({"data": {}}))],
    )
    .await;
    // A review that is not screening has no verdict outputs.
    let other_task = run_with_steps(
        &pool,
        project.as_uuid(),
        workflow,
        &graph,
        &[("screen", "ai.run_review", json!({"records": [{}, {}]}))],
    )
    .await;
    // A screening step still waiting for the AI has no output yet.
    let waiting = run_with_steps(
        &pool,
        project.as_uuid(),
        workflow,
        &graph,
        &[("screen", "ai.run_review", Value::Null)],
    )
    .await;

    let counts = review_counts_for_runs(&pool, &[screened, plain, other_task, waiting])
        .await
        .expect("counts load");
    assert_eq!(
        counts.get(&screened),
        Some(&ReviewCounts {
            included: 4,
            excluded: 1,
            unsure: 2,
        })
    );
    assert!(!counts.contains_key(&plain));
    assert!(!counts.contains_key(&other_task));
    assert!(!counts.contains_key(&waiting));
    assert!(
        review_counts_for_runs(&pool, &[])
            .await
            .expect("no runs")
            .is_empty()
    );

    sqlx::query("DELETE FROM projects WHERE id=$1")
        .bind(project.as_uuid())
        .execute(&pool)
        .await
        .expect("cleanup");
}
