#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
//! Runs small workflow graphs end to end against a database.
//! Skipped unless `DATABASE_URL` points at a reachable PostgreSQL.

use std::time::Duration;

use deepref_application::workflows::{Endpoint, GraphEdge, GraphNode, Position, WorkflowGraph};
use deepref_domain::{Actor, ActorKind, ProjectId};
use deepref_postgres::{
    NodeOutcome, StartRun, StepClaim, UpdateWorkflow, begin_node_execution, claim_job,
    complete_job, create_workflow, fail_abandoned_steps, finish_node_execution,
    get_published_graph, get_workflow_run, migrate, publish_workflow, start_run, update_workflow,
};
use deepref_worker::{delivery::DeliveryAction, workflows};
use serde_json::{Value, json};
use sqlx::{PgPool, postgres::PgPoolOptions};
use uuid::Uuid;

static GUARD: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

async fn database() -> Option<PgPool> {
    let url = std::env::var("DATABASE_URL").ok()?;
    let pool = PgPoolOptions::new()
        .max_connections(8)
        .connect(&url)
        .await
        .ok()?;
    migrate(&pool).await.expect("migrations apply");
    // `claim_job` takes the next job of any project, so jobs left queued by
    // other test binaries sharing this database would be claimed here. Park
    // only jobs older than this process; jobs of tests in this binary stay.
    static STARTED: std::sync::OnceLock<chrono::DateTime<chrono::Utc>> = std::sync::OnceLock::new();
    let started = *STARTED.get_or_init(chrono::Utc::now);
    sqlx::query(
        "UPDATE jobs SET state='dead', last_error='parked by worker workflow tests', completed_at=now()
         WHERE state='queued' AND created_at < $1",
    )
    .bind(started)
    .execute(&pool)
    .await
    .expect("park stale jobs");
    Some(pool)
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

/// Execute queued workflow steps until the queue of this project is empty.
async fn drain(pool: &PgPool, project: Uuid) {
    for _ in 0..50 {
        let Some(job) = claim_job(pool, "workflow-test", Duration::from_secs(60))
            .await
            .expect("claim")
        else {
            return;
        };
        assert_eq!(
            job.project_id.as_uuid(),
            project,
            "unexpected job from another project"
        );
        assert_eq!(job.kind, "workflow_step");
        match workflows::handle_step(pool, &job, None)
            .await
            .expect("step")
        {
            DeliveryAction::Ack => {
                complete_job(pool, "workflow-test", job.id)
                    .await
                    .expect("complete");
            }
            other => panic!("unexpected delivery action {other:?}"),
        }
    }
}

#[tokio::test]
async fn trigger_filter_notification_runs_and_dry_runs() {
    let _guard = GUARD.lock().await;
    let Some(pool) = database().await else { return };
    let project = ProjectId::new(Uuid::new_v4());
    sqlx::query("INSERT INTO projects (id,name) VALUES ($1,$2)")
        .bind(project.as_uuid())
        .bind(format!("wf-test-{}", project.as_uuid()))
        .execute(&pool)
        .await
        .expect("project");
    let actor = Actor::new(ActorKind::User, "wf-test").expect("actor");
    let graph = WorkflowGraph {
        nodes: vec![
            node(
                "alert",
                "trigger.publication_alert",
                json!({"query": {"terms": "statins"}, "sources": "both", "schedule": {"every": "day", "at": "08:00", "timezone": "UTC"}}),
            ),
            node(
                "recent",
                "logic.filter",
                json!({"condition": {"match": "all", "rules": [{"field": "year", "operator": "greater_than", "value": 2020}]}, "stop_if_empty": true}),
            ),
            node(
                "tell",
                "integration.notify",
                json!({"title": "Found {{count}} new", "message": "x", "severity": "success"}),
            ),
        ],
        edges: vec![
            edge("alert", "records", "recent", "records"),
            edge("recent", "records", "tell", "data"),
        ],
    };
    let workflow = create_workflow(&pool, project, "Recent papers", "", graph, &actor)
        .await
        .expect("create");
    let version = publish_workflow(&pool, project, workflow.id, None, &actor)
        .await
        .expect("publish");
    assert_eq!(version.version, 1);
    // Published versions are immutable.
    let update = sqlx::query("UPDATE workflow_versions SET note='x' WHERE id=$1")
        .bind(version.id)
        .execute(&pool)
        .await;
    assert!(update.is_err());

    let (version_id, graph) = get_published_graph(&pool, workflow.id)
        .await
        .expect("graph");
    let trigger_data = json!({"records": [
        {"title": "Old", "year": 2018}, {"title": "New", "year": 2023}, {"title": "Newer", "year": 2024}
    ]});
    let request = StartRun {
        project_id: project.as_uuid(),
        workflow_id: workflow.id,
        version_id: Some(version_id),
        graph,
        trigger_kind: "publication_alert".to_owned(),
        trigger_data,
        idempotency_key: "alert:test-1".to_owned(),
        actor_kind: "system".to_owned(),
        actor_id: "test".to_owned(),
        test_mode: false,
    };
    let started = start_run(&pool, &request).await.expect("start");
    assert!(started.created);
    // The same trigger event never starts a second run.
    let again = start_run(&pool, &request).await.expect("restart");
    assert!(!again.created);
    assert_eq!(again.run_id, started.run_id);

    drain(&pool, project.as_uuid()).await;
    let run = get_workflow_run(&pool, project, started.run_id)
        .await
        .expect("run");
    assert_eq!(run.status, "completed", "{:?}", run.error);
    let statuses: Vec<(String, String)> = run
        .nodes
        .iter()
        .map(|n| (n.node_id.clone(), n.status.clone()))
        .collect();
    assert!(
        statuses.contains(&("recent".to_owned(), "completed".to_owned())),
        "{statuses:?}"
    );
    assert!(
        statuses.contains(&("tell".to_owned(), "completed".to_owned())),
        "{statuses:?}"
    );
    let kept = run
        .nodes
        .iter()
        .find(|n| n.node_id == "recent")
        .and_then(|n| n.output.clone())
        .expect("filter output");
    assert_eq!(kept["records"].as_array().map(Vec::len), Some(2));
    let notifications: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM notifications WHERE project_id=$1 AND kind='workflow'",
    )
    .bind(project.as_uuid())
    .fetch_one(&pool)
    .await
    .expect("count");
    assert_eq!(notifications, 1);

    // Test mode: the draft runs, the notification is only described.
    let draft = update_workflow(&pool, project, workflow.id, UpdateWorkflow::default())
        .await
        .expect("draft");
    let test = start_run(
        &pool,
        &StartRun {
            project_id: project.as_uuid(),
            workflow_id: workflow.id,
            version_id: None,
            graph: draft.draft_graph,
            trigger_kind: "test".to_owned(),
            trigger_data: json!({"records": [{"title": "Sample", "year": 2022}]}),
            idempotency_key: format!("test:{}", Uuid::new_v4()),
            actor_kind: "user".to_owned(),
            actor_id: "wf-test".to_owned(),
            test_mode: true,
        },
    )
    .await
    .expect("test run");
    drain(&pool, project.as_uuid()).await;
    let run = get_workflow_run(&pool, project, test.run_id)
        .await
        .expect("test run detail");
    assert_eq!(run.status, "completed");
    let note = run
        .nodes
        .iter()
        .find(|n| n.node_id == "tell")
        .and_then(|n| n.note.clone())
        .unwrap_or_default();
    assert!(note.starts_with("Test run: nothing was changed"), "{note}");
    let notifications: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM notifications WHERE project_id=$1 AND kind='workflow'",
    )
    .bind(project.as_uuid())
    .fetch_one(&pool)
    .await
    .expect("count");
    assert_eq!(notifications, 1, "a test run must not notify");

    sqlx::query("DELETE FROM projects WHERE id=$1")
        .bind(project.as_uuid())
        .execute(&pool)
        .await
        .expect("cleanup");
}

#[tokio::test]
async fn filter_with_no_match_skips_the_rest_and_for_each_fans_out() {
    let _guard = GUARD.lock().await;
    let Some(pool) = database().await else { return };
    let project = ProjectId::new(Uuid::new_v4());
    sqlx::query("INSERT INTO projects (id,name) VALUES ($1,$2)")
        .bind(project.as_uuid())
        .bind(format!("wf-test-{}", project.as_uuid()))
        .execute(&pool)
        .await
        .expect("project");
    let actor = Actor::new(ActorKind::User, "wf-test").expect("actor");
    let graph = WorkflowGraph {
        nodes: vec![
            node(
                "alert",
                "trigger.publication_alert",
                json!({"query": {"terms": "x"}, "sources": "both", "schedule": {"every": "day", "at": "08:00", "timezone": "UTC"}}),
            ),
            node("each", "logic.for_each", json!({})),
            node("tell", "integration.notify", json!({"title": "{{title}}"})),
            node("all", "logic.merge", json!({})),
            node("done", "integration.notify", json!({"title": "Finished"})),
        ],
        edges: vec![
            edge("alert", "records", "each", "items"),
            edge("each", "item", "tell", "data"),
            edge("tell", "data", "all", "inputs"),
            edge("all", "merged", "done", "data"),
        ],
    };
    let workflow = create_workflow(&pool, project, "Loop", "", graph, &actor)
        .await
        .expect("create");
    publish_workflow(&pool, project, workflow.id, None, &actor)
        .await
        .expect("publish");
    let (version_id, graph) = get_published_graph(&pool, workflow.id)
        .await
        .expect("graph");
    let started = start_run(
        &pool,
        &StartRun {
            project_id: project.as_uuid(),
            workflow_id: workflow.id,
            version_id: Some(version_id),
            graph,
            trigger_kind: "publication_alert".to_owned(),
            trigger_data: json!({"records": [{"title": "A"}, {"title": "B"}, {"title": "C"}]}),
            idempotency_key: "alert:loop".to_owned(),
            actor_kind: "system".to_owned(),
            actor_id: "test".to_owned(),
            test_mode: false,
        },
    )
    .await
    .expect("start");
    drain(&pool, project.as_uuid()).await;
    let run = get_workflow_run(&pool, project, started.run_id)
        .await
        .expect("run");
    assert_eq!(run.status, "completed", "{:?}", run.error);
    let tells = run
        .nodes
        .iter()
        .filter(|n| n.node_id == "tell" && n.status == "completed")
        .count();
    assert_eq!(tells, 3);
    assert!(
        run.nodes
            .iter()
            .any(|n| n.node_id == "done" && n.status == "completed")
    );
    let titles: Vec<String> =
        sqlx::query_scalar("SELECT title FROM notifications WHERE project_id=$1 ORDER BY title")
            .bind(project.as_uuid())
            .fetch_all(&pool)
            .await
            .expect("titles");
    assert_eq!(titles, vec!["A", "B", "C", "Finished"]);
    sqlx::query("DELETE FROM projects WHERE id=$1")
        .bind(project.as_uuid())
        .execute(&pool)
        .await
        .expect("cleanup");
}

/// The notification step of a run, as the run shows it.
/// Failure notifications written for one run: (title, body, project, payload).
async fn run_failure_rows(
    pool: &PgPool,
    run_id: Uuid,
) -> Vec<(String, Option<String>, Option<Uuid>, Value)> {
    sqlx::query_as(
        "SELECT title, body, project_id, payload FROM notifications
         WHERE kind='workflow_run.failed' AND payload->>'run_id'=$1",
    )
    .bind(run_id.to_string())
    .fetch_all(pool)
    .await
    .expect("failure rows")
}

/// A run whose step fails is recorded once, named after the workflow, with the
/// reason and the run it belongs to. A test run that fails stays silent.
#[tokio::test]
async fn a_failed_run_is_recorded_once_and_a_test_run_stays_silent() {
    let _guard = GUARD.lock().await;
    let Some(pool) = database().await else { return };
    let project = ProjectId::new(Uuid::new_v4());
    sqlx::query("INSERT INTO projects (id,name) VALUES ($1,$2)")
        .bind(project.as_uuid())
        .bind(format!("wf-fail-{}", project.as_uuid()))
        .execute(&pool)
        .await
        .expect("project");
    let actor = Actor::new(ActorKind::User, "wf-test").expect("actor");
    // A loopback address is refused by the HTTP block, so the step fails at once.
    let graph = WorkflowGraph {
        nodes: vec![
            node("start", "trigger.manual", json!({})),
            node(
                "fetch",
                "integration.http_request",
                json!({"method": "GET", "url": "http://127.0.0.1:8099/"}),
            ),
        ],
        edges: vec![edge("start", "data", "fetch", "data")],
    };
    // Publishing refuses a loopback address (Check reports it), so the version is
    // published from a harmless draft and the runs are started from the bad graph.
    // The step must still fail at run time, as it would for a name that turns out
    // to be private.
    let harmless = WorkflowGraph {
        nodes: vec![
            node("start", "trigger.manual", json!({})),
            node("tell", "integration.notify", json!({"title": "Checked"})),
        ],
        edges: vec![edge("start", "data", "tell", "data")],
    };
    let workflow = create_workflow(
        &pool,
        project,
        "Fetch the sample page",
        "",
        harmless,
        &actor,
    )
    .await
    .expect("create");
    publish_workflow(&pool, project, workflow.id, None, &actor)
        .await
        .expect("publish");
    let (version_id, _published) = get_published_graph(&pool, workflow.id)
        .await
        .expect("graph");

    let started = start_run(
        &pool,
        &StartRun {
            project_id: project.as_uuid(),
            workflow_id: workflow.id,
            version_id: Some(version_id),
            graph: graph.clone(),
            trigger_kind: "manual".to_owned(),
            trigger_data: json!({}),
            idempotency_key: format!("manual:{}", Uuid::new_v4()),
            actor_kind: "user".to_owned(),
            actor_id: "wf-test".to_owned(),
            test_mode: false,
        },
    )
    .await
    .expect("start");
    drain(&pool, project.as_uuid()).await;
    let run = get_workflow_run(&pool, project, started.run_id)
        .await
        .expect("run");
    assert_eq!(run.status, "failed", "{:?}", run.error);

    let rows = run_failure_rows(&pool, started.run_id).await;
    assert_eq!(rows.len(), 1, "exactly one notification for the failed run");
    let (title, body, row_project, payload) = &rows[0];
    assert_eq!(title, "Fetch the sample page failed");
    assert!(
        body.as_deref()
            .unwrap_or_default()
            .contains("private or local network"),
        "{body:?}"
    );
    assert_eq!(*row_project, Some(project.as_uuid()));
    assert_eq!(payload["run_id"], json!(started.run_id));
    assert_eq!(payload["workflow_id"], json!(workflow.id));

    // The same graph as a test run: the step fails the same way, and nothing
    // is announced for it.
    let test = start_run(
        &pool,
        &StartRun {
            project_id: project.as_uuid(),
            workflow_id: workflow.id,
            version_id: None,
            graph,
            trigger_kind: "test".to_owned(),
            trigger_data: json!({}),
            idempotency_key: format!("test:{}", Uuid::new_v4()),
            actor_kind: "user".to_owned(),
            actor_id: "wf-test".to_owned(),
            test_mode: true,
        },
    )
    .await
    .expect("test start");
    let claim = claim_job(&pool, "wf-test-fail", Duration::from_secs(60))
        .await
        .expect("claim")
        .expect("the test step is queued");
    assert_eq!(claim.project_id.as_uuid(), project.as_uuid());
    let node_run_id =
        Uuid::parse_str(claim.payload["node_run_id"].as_str().expect("payload")).expect("uuid");
    assert!(matches!(
        begin_node_execution(&pool, node_run_id)
            .await
            .expect("begin"),
        StepClaim::Execute(_)
    ));
    finish_node_execution(
        &pool,
        node_run_id,
        NodeOutcome::Failed {
            message: "The other service did not answer in time.".to_owned(),
        },
    )
    .await
    .expect("fail the test step");
    complete_job(&pool, "wf-test-fail", claim.id)
        .await
        .expect("complete");
    let test_run = get_workflow_run(&pool, project, test.run_id)
        .await
        .expect("test run");
    assert_eq!(test_run.status, "failed");
    assert!(
        run_failure_rows(&pool, test.run_id).await.is_empty(),
        "a test run must not notify"
    );

    sqlx::query("DELETE FROM projects WHERE id=$1")
        .bind(project.as_uuid())
        .execute(&pool)
        .await
        .expect("cleanup");
}

async fn notify_step(
    pool: &PgPool,
    project: ProjectId,
    run_id: Uuid,
) -> deepref_postgres::NodeRunRecord {
    let run = get_workflow_run(pool, project, run_id).await.expect("run");
    run.nodes
        .into_iter()
        .find(|n| n.node_id == "tell")
        .expect("tell node")
}

async fn workflow_notifications(pool: &PgPool, project: Uuid) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM notifications WHERE project_id=$1 AND kind='workflow'")
        .bind(project)
        .fetch_one(pool)
        .await
        .expect("count")
}

/// Take the one queued step of the project and make it wait for `poll` 1 hour.
async fn defer_queued_step(pool: &PgPool, project: ProjectId) -> Uuid {
    let claim = claim_job(pool, "wait-test", Duration::from_secs(60))
        .await
        .expect("claim")
        .expect("the notification step is queued");
    assert_eq!(claim.project_id.as_uuid(), project.as_uuid());
    let node_run_id =
        Uuid::parse_str(claim.payload["node_run_id"].as_str().expect("payload")).expect("uuid");
    match begin_node_execution(pool, node_run_id)
        .await
        .expect("begin")
    {
        StepClaim::Execute(execution) => assert_eq!(execution.node_id, "tell"),
        StepClaim::Stale => panic!("the step should start"),
    }
    let waiting = |wait_state: Value| NodeOutcome::Waiting {
        delay_secs: 3600,
        poll: 1,
        note: "Waiting for the AI.".to_owned(),
        wait_state,
    };
    finish_node_execution(
        pool,
        node_run_id,
        waiting(json!({"polls": 1, "reviews": []})),
    )
    .await
    .expect("defer");
    // Recording the same look twice queues it once.
    finish_node_execution(
        pool,
        node_run_id,
        waiting(json!({"polls": 1, "reviews": []})),
    )
    .await
    .expect("defer again");
    complete_job(pool, "wait-test", claim.id)
        .await
        .expect("complete");
    node_run_id
}

/// A step that waits for something outside the flow is queued again as its own
/// look, runs only when that look is due, and a look that dies fails the step
/// instead of leaving the run waiting forever.
#[tokio::test]
async fn a_waiting_step_is_looked_at_later_and_a_lost_look_fails_it() {
    let _guard = GUARD.lock().await;
    let Some(pool) = database().await else { return };
    let project = ProjectId::new(Uuid::new_v4());
    sqlx::query("INSERT INTO projects (id,name) VALUES ($1,$2)")
        .bind(project.as_uuid())
        .bind(format!("wf-wait-{}", project.as_uuid()))
        .execute(&pool)
        .await
        .expect("project");
    let actor = Actor::new(ActorKind::User, "wf-test").expect("actor");
    let graph = WorkflowGraph {
        nodes: vec![
            node("start", "trigger.manual", json!({})),
            node("tell", "integration.notify", json!({"title": "Waited"})),
        ],
        edges: vec![edge("start", "data", "tell", "data")],
    };
    let workflow = create_workflow(&pool, project, "Wait", "", graph.clone(), &actor)
        .await
        .expect("create");
    let start = |key: &str| StartRun {
        project_id: project.as_uuid(),
        workflow_id: workflow.id,
        version_id: None,
        graph: graph.clone(),
        trigger_kind: "manual".to_owned(),
        trigger_data: json!({}),
        idempotency_key: key.to_owned(),
        actor_kind: "user".to_owned(),
        actor_id: "wf-test".to_owned(),
        test_mode: false,
    };

    // Run one waits for an hour, then finishes once its look is due.
    let first = start_run(&pool, &start("wait:one"))
        .await
        .expect("start")
        .run_id;
    let node_run_id = defer_queued_step(&pool, project).await;
    let step = notify_step(&pool, project, first).await;
    assert_eq!(step.status, "queued");
    assert_eq!(step.note.as_deref(), Some("Waiting for the AI."));
    let saved: Option<Value> =
        sqlx::query_scalar("SELECT wait_state FROM workflow_node_runs WHERE id=$1")
            .bind(node_run_id)
            .fetch_one(&pool)
            .await
            .expect("wait state");
    assert_eq!(saved, Some(json!({"polls": 1, "reviews": []})));
    let looks: i64 = sqlx::query_scalar("SELECT count(*) FROM jobs WHERE dedupe_key=$1")
        .bind(format!("workflow_step:{node_run_id}:wait:1"))
        .fetch_one(&pool)
        .await
        .expect("looks");
    assert_eq!(looks, 1);
    // Nothing runs while the look is not due, and nothing has been notified.
    assert!(
        claim_job(&pool, "wait-test", Duration::from_secs(60))
            .await
            .expect("claim")
            .is_none()
    );
    assert_eq!(workflow_notifications(&pool, project.as_uuid()).await, 0);

    sqlx::query("UPDATE jobs SET available_at=now() WHERE dedupe_key=$1")
        .bind(format!("workflow_step:{node_run_id}:wait:1"))
        .execute(&pool)
        .await
        .expect("make due");
    drain(&pool, project.as_uuid()).await;
    let run = get_workflow_run(&pool, project, first).await.expect("run");
    assert_eq!(run.status, "completed", "{:?}", run.error);
    assert_eq!(notify_step(&pool, project, first).await.status, "completed");
    assert_eq!(workflow_notifications(&pool, project.as_uuid()).await, 1);

    // Run two: its look dies. The reaper fails the step, so the run ends.
    let second = start_run(&pool, &start("wait:two"))
        .await
        .expect("start")
        .run_id;
    let node_run_id = defer_queued_step(&pool, project).await;
    sqlx::query("UPDATE jobs SET state='dead' WHERE dedupe_key=$1")
        .bind(format!("workflow_step:{node_run_id}:wait:1"))
        .execute(&pool)
        .await
        .expect("kill the look");
    assert!(fail_abandoned_steps(&pool).await.expect("reap") >= 1);
    let run = get_workflow_run(&pool, project, second).await.expect("run");
    assert_eq!(run.status, "failed");
    assert_eq!(notify_step(&pool, project, second).await.status, "failed");
    assert_eq!(workflow_notifications(&pool, project.as_uuid()).await, 1);

    sqlx::query("DELETE FROM projects WHERE id=$1")
        .bind(project.as_uuid())
        .execute(&pool)
        .await
        .expect("cleanup");
}
