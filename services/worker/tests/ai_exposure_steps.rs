#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
//! Workflow steps that show an AI verdict record it as an exposure, in the run
//! output, before the output can be read. Runs against a database and is
//! skipped unless `DATABASE_URL` points at a reachable PostgreSQL.
//!
//! One test function, on purpose: the worker installs a process-wide autonomy
//! gate that holds a database pool, and that pool dies with the test's runtime.
//! Two test functions in one process would leave the second with a dead gate.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use deepref_application::workflows::{Endpoint, GraphEdge, GraphNode, Position, WorkflowGraph};
use deepref_application::{
    ProtocolCriterionCommand, PublishProtocolCommand, SaveProtocolDraftCommand, ScreenReportCommand,
};
use deepref_domain::{
    Actor, ActorKind, CriterionDimension, CriterionKind, CriterionStage, FrameworkKind, ProjectId,
    ProtocolVersionId, ReportId, ScreeningDecision, ScreeningStage,
};
use deepref_postgres::{
    ProjectAutonomyGate, ProtocolActor, StartRun, claim_job, complete_job, create_workflow,
    get_published_graph, get_published_protocol, independent_reviewer_pairs, migrate,
    publish_protocol, publish_workflow, save_protocol_draft, screen_report, set_ai_budget,
    start_run,
};
use deepref_worker::{delivery::DeliveryAction, workflows};
use serde_json::{Value, json};
use sqlx::{PgPool, postgres::PgPoolOptions};
use uuid::Uuid;

async fn database() -> Option<PgPool> {
    let url = std::env::var("DATABASE_URL").ok()?;
    let pool = PgPoolOptions::new()
        .max_connections(8)
        .connect(&url)
        .await
        .expect("DATABASE_URL must be reachable");
    migrate(&pool).await.expect("migrations apply");
    // `claim_job` takes the next job of any project, so jobs left queued by other
    // test binaries sharing this database would be claimed here. Park only the jobs
    // older than this process.
    let started = chrono::Utc::now();
    sqlx::query(
        "UPDATE jobs SET state='dead', last_error='parked by worker AI exposure tests', completed_at=now()
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

/// Executes queued steps until the project's queue is empty.
async fn drain(pool: &PgPool, project: Uuid) {
    for _ in 0..50 {
        let Some(job) = claim_job(pool, "ai-exposure-test", Duration::from_secs(60))
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
        match workflows::handle_step(pool, &job, None)
            .await
            .expect("step")
        {
            DeliveryAction::Ack => {
                complete_job(pool, "ai-exposure-test", job.id)
                    .await
                    .expect("complete");
            }
            other => panic!("unexpected delivery action {other:?}"),
        }
    }
}

async fn new_project(pool: &PgPool, label: &str) -> ProjectId {
    let project = ProjectId::new(Uuid::new_v4());
    sqlx::query("INSERT INTO projects (id,name) VALUES ($1,$2)")
        .bind(project.as_uuid())
        .bind(format!("{label}-{}", project.as_uuid()))
        .execute(pool)
        .await
        .expect("project");
    project
}

/// A record of the project, referred to by the workflow's records.
async fn project_report(pool: &PgPool, project: Uuid, title: &str) -> Uuid {
    let report_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO reports (id,title,abstract_text) VALUES ($1,$2,'Adults with condition X.')",
    )
    .bind(report_id)
    .bind(title)
    .execute(pool)
    .await
    .expect("report");
    sqlx::query("INSERT INTO project_reports (project_id,report_id) VALUES ($1,$2)")
        .bind(project)
        .bind(report_id)
        .execute(pool)
        .await
        .expect("membership");
    report_id
}

/// Publishes a protocol so that a person's screening decision can be recorded.
async fn publish_protocol_for(pool: &PgPool, project: Uuid) -> Uuid {
    let actor = ProtocolActor {
        kind: "user".to_owned(),
        id: "ai-exposure-test".to_owned(),
    };
    let draft = save_protocol_draft(
        pool,
        &SaveProtocolDraftCommand {
            project_id: ProjectId::new(project),
            protocol_version_id: None,
            name: "Workflow exposure protocol".to_owned(),
            objective: "Blinding".to_owned(),
            question: "Is an AI verdict visible in the run output?".to_owned(),
            framework_kind: FrameworkKind::Pico,
            framework_fields: BTreeMap::from([
                (
                    "population".to_owned(),
                    "Adults with condition X".to_owned(),
                ),
                ("intervention".to_owned(), "Intervention Y".to_owned()),
                ("outcome".to_owned(), "Outcome Z".to_owned()),
            ]),
            criteria: vec![ProtocolCriterionCommand {
                id: None,
                kind: CriterionKind::Inclusion,
                stage: CriterionStage::Both,
                dimension: CriterionDimension::Population,
                label: "Population".to_owned(),
                description: "Adults with condition X".to_owned(),
            }],
            expected_revision: 0,
        },
        &actor,
    )
    .await
    .expect("draft saves");
    publish_protocol(
        pool,
        &PublishProtocolCommand {
            project_id: ProjectId::new(project),
            protocol_version_id: draft.id,
            expected_revision: draft.revision,
        },
        &actor,
    )
    .await
    .expect("protocol publishes");
    get_published_protocol(pool, project)
        .await
        .expect("published protocol")
        .id
}

/// Runs a graph once, to the end, on records given by a trigger. Returns the run id.
async fn run_on_records(
    pool: &PgPool,
    project: ProjectId,
    graph: WorkflowGraph,
    key: &str,
    records: Value,
) -> Uuid {
    let actor = Actor::new(ActorKind::User, "ai-exposure-test").expect("actor");
    let workflow = create_workflow(pool, project, "AI exposure", "", graph, &actor)
        .await
        .expect("create");
    publish_workflow(pool, project, workflow.id, None, &actor)
        .await
        .expect("publish");
    let (version_id, graph) = get_published_graph(pool, workflow.id).await.expect("graph");
    let request = StartRun {
        project_id: project.as_uuid(),
        workflow_id: workflow.id,
        version_id: Some(version_id),
        graph,
        trigger_kind: "publication_alert".to_owned(),
        trigger_data: json!({ "records": records }),
        idempotency_key: key.to_owned(),
        actor_kind: "system".to_owned(),
        actor_id: "test".to_owned(),
        test_mode: false,
    };
    let started = start_run(pool, &request).await.expect("start");
    drain(pool, project.as_uuid()).await;
    started.run_id
}

type ExposureRow = (
    Uuid,
    String,
    String,
    Option<Uuid>,
    Option<Uuid>,
    Option<Uuid>,
);

/// Each exposure of the project: record, stage, source, workflow run, decision and proposal.
async fn exposure_rows(pool: &PgPool, project: Uuid) -> Vec<ExposureRow> {
    sqlx::query_as(
        "SELECT report_id, stage, exposure_source, workflow_run_id, ai_reviewer_decision_id,
                proposal_id
         FROM ai_opinion_exposures WHERE project_id=$1 ORDER BY report_id, exposure_source",
    )
    .bind(project)
    .fetch_all(pool)
    .await
    .expect("exposure rows")
}

/// The alert trigger that feeds records into a step.
fn trigger() -> GraphNode {
    node(
        "alert",
        "trigger.publication_alert",
        json!({"query": {"terms": "statins"}, "sources": "both", "schedule": {"every": "day", "at": "08:00", "timezone": "UTC"}}),
    )
}

/// A second-opinion step records its opinion for each record, and the run output names
/// it, so the opinion is exposed. A person who decides afterwards is excluded.
async fn second_opinion_step_exposes_its_opinions(pool: &PgPool) {
    let project = new_project(pool, "wf-second-opinion").await;
    let protocol = publish_protocol_for(pool, project.as_uuid()).await;
    let first = project_report(pool, project.as_uuid(), "Second opinion one").await;
    let second = project_report(pool, project.as_uuid(), "Second opinion two").await;
    let graph = WorkflowGraph {
        nodes: vec![
            trigger(),
            node(
                "opine",
                "action.record_decision",
                json!({"stage": "title_abstract", "decision": "exclude", "note": "Children only."}),
            ),
        ],
        edges: vec![edge("alert", "records", "opine", "records")],
    };
    let run_id = run_on_records(
        pool,
        project,
        graph,
        "exposure:second-opinion",
        json!([{"report_id": first, "title": "One"}, {"report_id": second, "title": "Two"}]),
    )
    .await;

    let rows = exposure_rows(pool, project.as_uuid()).await;
    assert_eq!(rows.len(), 2, "one exposure per recorded opinion");
    for (report, stage, source, run, decision, proposal) in &rows {
        assert!(*report == first || *report == second);
        assert_eq!(stage, "title_abstract");
        assert_eq!(source, "workflow_run_output");
        assert_eq!(*run, Some(run_id));
        assert!(
            decision.is_some(),
            "the exposure names the opinion it shows"
        );
        assert_eq!(*proposal, None);
    }

    // The person decides the first record after the opinion was in the run output.
    screen_report(
        pool,
        ScreenReportCommand {
            project_id: project,
            report_id: ReportId::new(first),
            stage: ScreeningStage::TitleAbstract,
            decision: ScreeningDecision::Include,
            exclusion_reason_id: None,
            protocol_version_id: ProtocolVersionId::new(protocol),
            expected_revision: 0,
            notes: None,
            actor: Actor::new(ActorKind::User, "ai-exposure-test").expect("actor"),
        },
    )
    .await
    .expect("decision");
    let pairs = independent_reviewer_pairs(pool, project.as_uuid(), Some("title_abstract"))
        .await
        .expect("pairs");
    assert!(pairs.independent.is_empty());
    assert_eq!(pairs.exposed_excluded, 1);
}

/// A screening step with no AI budget left does not review. Each record is still routed
/// on the "not sure" port, and that verdict is recorded as exposed.
async fn screening_step_exposes_the_verdicts_it_routes(pool: &PgPool) {
    let project = new_project(pool, "wf-screening").await;
    let first = project_report(pool, project.as_uuid(), "Screened one").await;
    let second = project_report(pool, project.as_uuid(), "Screened two").await;
    set_ai_budget(pool, project.as_uuid(), 0)
        .await
        .expect("budget");
    let graph = WorkflowGraph {
        nodes: vec![
            trigger(),
            node(
                "screen",
                "ai.run_review",
                json!({"task": "screening", "stage": "title_abstract"}),
            ),
        ],
        edges: vec![edge("alert", "records", "screen", "records")],
    };
    let run_id = run_on_records(
        pool,
        project,
        graph,
        "exposure:screening",
        json!([{"report_id": first, "title": "One"}, {"report_id": second, "title": "Two"}]),
    )
    .await;

    let rows = exposure_rows(pool, project.as_uuid()).await;
    assert_eq!(rows.len(), 2, "one exposure per routed record");
    for (report, stage, source, run, decision, proposal) in &rows {
        assert!(*report == first || *report == second);
        assert_eq!(stage, "title_abstract");
        assert_eq!(source, "workflow_run_output");
        assert_eq!(*run, Some(run_id));
        assert_eq!(*decision, None);
        assert_eq!(*proposal, None, "no AI review ran, so there is no proposal");
    }
}

#[tokio::test]
async fn workflow_steps_record_the_ai_opinions_they_expose() {
    let Some(pool) = database().await else { return };
    // The worker process installs the project's autonomy gate at start-up. Without it
    // every block is limited to suggestions, and these steps would not record opinions.
    workflows::set_autonomy_gate(Arc::new(ProjectAutonomyGate::new(&pool)));
    second_opinion_step_exposes_its_opinions(&pool).await;
    screening_step_exposes_the_verdicts_it_routes(&pool).await;
}
