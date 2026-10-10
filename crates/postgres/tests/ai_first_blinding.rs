#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
//! The read boundaries that a blind audit must close. A fresh auditor who can still see a
//! cohort member's earlier judgment, or a workflow snapshot carrying one, stays eligible to
//! label the cohort while knowing what the reference says.

use deepref_application::workflows::{GraphNode, Position, WorkflowGraph};
use deepref_domain::{Actor, ActorKind, ProjectId};
use deepref_postgres::*;
use serde_json::json;
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

struct Fixture {
    pool: PgPool,
    project: Uuid,
    cohort: Uuid,
    /// Cohort member a person already included and left waiting for a full-text decision.
    control: Uuid,
    /// Cohort member a person already excluded at full text, with a reason.
    sample: Uuid,
    /// A record outside the cohort, which keeps its ordinary read state.
    outside: Uuid,
}

impl Fixture {
    /// Three reports with real prior judgments, of which two sit in an active blind audit.
    async fn new(pool: PgPool) -> Self {
        let project = Uuid::new_v4();
        let control = Uuid::new_v4();
        let sample = Uuid::new_v4();
        let outside = Uuid::new_v4();
        let protocol = Uuid::new_v4();
        let cohort = Uuid::new_v4();
        let reason = Uuid::new_v4();
        sqlx::query("INSERT INTO projects(id,name) VALUES($1,'AI first blinding test')")
            .bind(project)
            .execute(&pool)
            .await
            .unwrap();
        for (id, title) in [
            (control, "Control with an earlier include"),
            (sample, "Sample with an earlier exclude"),
            (outside, "Record outside the blind cohort"),
        ] {
            sqlx::query("INSERT INTO reports(id,title,abstract_text) VALUES($1,$2,'Abstract.')")
                .bind(id)
                .bind(title)
                .execute(&pool)
                .await
                .unwrap();
            sqlx::query("INSERT INTO project_reports(project_id,report_id) VALUES($1,$2)")
                .bind(project)
                .bind(id)
                .execute(&pool)
                .await
                .unwrap();
        }
        sqlx::query(
            "INSERT INTO exclusion_reasons (id,project_id,code,label,stage)
             VALUES ($1,$2,'wrong-population','Wrong population, adults only','full_text')",
        )
        .bind(reason)
        .bind(project)
        .execute(&pool)
        .await
        .unwrap();
        for (report, title_abstract, full_text, reason, final_status) in [
            // The control waits for full text, so it would show progress if it leaked.
            (
                control,
                "include",
                "not_required",
                None,
                "pending_full_text",
            ),
            (sample, "include", "exclude", Some(reason), "exclude"),
            (outside, "maybe", "not_required", None, "unscreened"),
        ] {
            sqlx::query(
                "INSERT INTO screening_state
                   (project_id,report_id,title_abstract_status,full_text_status,full_text_exclusion_reason_id,final_status,revision)
                 VALUES ($1,$2,$3,$4,$5,$6,1)",
            )
            .bind(project)
            .bind(report)
            .bind(title_abstract)
            .bind(full_text)
            .bind(reason)
            .bind(final_status)
            .execute(&pool)
            .await
            .unwrap();
        }
        sqlx::query(
            "INSERT INTO protocol_versions (id,project_id,version,name,status)
             VALUES ($1,$2,1,'Blinding protocol','published')",
        )
        .bind(protocol)
        .bind(project)
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO ai_screening_cohorts
               (id,project_id,protocol_version_id,semantic_bundle_hash,semantic_identity,policy_version,status,target_percent,approved_by)
             VALUES ($1,$2,$3,$4,'{}',1,'auditing',95,'owner')",
        )
        .bind(cohort)
        .bind(project)
        .bind(protocol)
        .bind("e".repeat(64))
        .execute(&pool)
        .await
        .unwrap();
        for report in [control, sample] {
            sqlx::query(
                "INSERT INTO ai_screening_cohort_members (project_id,cohort_id,report_id,evaluated_revision)
                 VALUES ($1,$2,$3,0)",
            )
            .bind(project)
            .bind(cohort)
            .bind(report)
            .execute(&pool)
            .await
            .unwrap();
        }
        Self {
            pool,
            project,
            cohort,
            control,
            sample,
            outside,
        }
    }

    /// Ends the blind audit so a reader can tell masking apart from a broken query.
    async fn unblind(&self) {
        sqlx::query("UPDATE ai_screening_cohorts SET status='finalized' WHERE id=$1")
            .bind(self.cohort)
            .execute(&self.pool)
            .await
            .unwrap();
    }
}

/// The overview is a set of disjoint buckets that add up to the report total. Counting a
/// hidden cohort member as `unscreened` while still counting its earlier decision would put
/// it in two buckets and inflate every figure the assistant quotes.
#[tokio::test]
async fn overview_counts_each_hidden_cohort_member_once() {
    let Some(pool) = database().await else {
        return;
    };
    let f = Fixture::new(pool).await;

    let overview = get_agent_project_overview(&f.pool, f.project)
        .await
        .unwrap();
    let title_abstract = &overview["title_abstract"];
    let bucketed: i64 = ["unscreened", "include", "exclude", "maybe"]
        .iter()
        .map(|key| title_abstract[*key].as_i64().expect("bucket count"))
        .sum();
    assert_eq!(
        bucketed,
        overview["reports"].as_i64().expect("report count"),
        "the title/abstract buckets must add up to reports: {overview}"
    );
    // Both hidden members read as unscreened; only the record outside the cohort keeps a
    // decision, and its `maybe` is the only non-unscreened bucket left.
    assert_eq!(title_abstract["unscreened"], 2, "{overview}");
    assert_eq!(title_abstract["include"], 0, "{overview}");
    assert_eq!(title_abstract["exclude"], 0, "{overview}");
    assert_eq!(title_abstract["maybe"], 1, "{overview}");
    // The hidden control is waiting for full text, which a blinded read must not report.
    assert_eq!(overview["full_text"]["awaiting_decision"], 0, "{overview}");
    assert_eq!(
        overview["full_text"]["awaiting_decision_with_full_text"], 0,
        "{overview}"
    );

    // The same mask has to hold for a single record, or the assistant can read one
    // control's judgment at a time and still infer the reference.
    for report in [f.control, f.sample] {
        let state = get_agent_screening_state(&f.pool, f.project, report)
            .await
            .expect("masked state");
        assert_eq!(state.title_abstract_status, "unscreened", "{report}");
        assert_eq!(state.full_text_status, "not_required", "{report}");
        assert_eq!(state.final_status, "unscreened", "{report}");
        assert_eq!(state.full_text_exclusion_reason_id, None, "{report}");
        assert_eq!(state.revision, 0, "{report}");
    }
    let state = get_agent_screening_state(&f.pool, f.project, f.outside)
        .await
        .expect("state outside the cohort");
    assert_eq!(state.title_abstract_status, "maybe");

    f.unblind().await;
    let visible = get_agent_project_overview(&f.pool, f.project)
        .await
        .unwrap();
    let title_abstract = &visible["title_abstract"];
    // Both cohort members were included at title/abstract; the sample's exclusion was at
    // full text, so it returns to `include` and reports its full-text progress again.
    assert_eq!(title_abstract["include"], 2, "{visible}");
    assert_eq!(title_abstract["exclude"], 0, "{visible}");
    assert_eq!(title_abstract["maybe"], 1, "{visible}");
    assert_eq!(title_abstract["unscreened"], 0, "{visible}");
    assert_eq!(visible["full_text"]["awaiting_decision"], 1, "{visible}");
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

/// A workflow run's node log repeats raw node input and output, including the screening
/// judgments a `data.find_records` step wrote. Withholding the run is the only way to keep
/// those out of an auditor's reach, because masking a snapshot still publishes the rest.
#[tokio::test]
async fn blind_audit_withholds_workflow_run_inspection() {
    let Some(pool) = database().await else {
        return;
    };
    let f = Fixture::new(pool).await;
    let project = ProjectId::new(f.project);
    let actor = Actor::new(ActorKind::User, "blinding-test").expect("actor");
    let graph = WorkflowGraph {
        nodes: vec![
            node("start", "trigger.manual"),
            node("find", "data.find_records"),
        ],
        edges: vec![],
    };
    let workflow = create_workflow(
        &f.pool,
        project,
        "Find records",
        "Blinding",
        graph.clone(),
        &actor,
    )
    .await
    .expect("workflow")
    .id;
    let started = start_run(
        &f.pool,
        &StartRun {
            project_id: f.project,
            workflow_id: workflow,
            version_id: None,
            graph,
            trigger_kind: "test".to_owned(),
            trigger_data: json!({}),
            idempotency_key: format!("test:{}", Uuid::new_v4()),
            actor_kind: "user".to_owned(),
            actor_id: "blinding-test".to_owned(),
            test_mode: true,
        },
    )
    .await
    .expect("run starts");
    // The step output carries a raw human judgment, which is what the auditor must not read.
    sqlx::query(
        "INSERT INTO workflow_node_runs
           (project_id, run_id, node_id, iteration, node_type, status, input, output)
         VALUES ($1,$2,'find','' ,'data.find_records','completed',$3,$4)
         ON CONFLICT (run_id, node_id, iteration)
         DO UPDATE SET input = EXCLUDED.input, output = EXCLUDED.output",
    )
    .bind(f.project)
    .bind(started.run_id)
    .bind(json!({"records": [{"id": f.control}]}))
    .bind(json!({"records": [{"id": f.control, "screening": {"title_abstract": "include"}}]}))
    .execute(&f.pool)
    .await
    .expect("node run fixture");

    assert!(
        workflow_run_inspection_withheld(&f.pool, project)
            .await
            .expect("withheld flag"),
        "an active blind audit must withhold run inspection"
    );
    assert!(
        matches!(
            get_workflow_run(&f.pool, project, started.run_id).await,
            Err(WorkflowError::RunNotFound)
        ),
        "the run detail must not be readable during a blind audit"
    );
    assert!(
        list_workflow_runs(&f.pool, project, None, 25)
            .await
            .expect("list")
            .is_empty(),
        "the run list must stay empty during a blind audit"
    );

    f.unblind().await;
    assert!(
        !workflow_run_inspection_withheld(&f.pool, project)
            .await
            .expect("withheld flag"),
        "a finished cohort releases run inspection"
    );
    let run = get_workflow_run(&f.pool, project, started.run_id)
        .await
        .expect("run is readable after the audit");
    assert_eq!(run.id, started.run_id);
    assert_eq!(run.nodes.len(), 2);
    let find = run
        .nodes
        .iter()
        .find(|node| node.node_type == "data.find_records")
        .expect("the find step is in the log");
    assert_eq!(
        find.output
            .as_ref()
            .expect("the find step wrote its output"),
        &json!({"records": [{"id": f.control, "screening": {"title_abstract": "include"}}]})
    );
    assert_eq!(
        list_workflow_runs(&f.pool, project, None, 25)
            .await
            .expect("list")
            .len(),
        1
    );
}
