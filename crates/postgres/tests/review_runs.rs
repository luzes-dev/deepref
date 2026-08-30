use chrono::Utc;
use deepref_ai::{
    AiRunRecord, AiRunStatus, AiRunStore, AiTaskKind, AuthorityTier, DedupeInput, DuplicateSignal,
    IdentityProvenance, ModelParameters, ModelProfile, ProposalDraft, ResolvedModel, TokenUsage,
};
use deepref_domain::{Actor, ActorKind, ProjectId};
use deepref_postgres::{
    PostgresAiStore, PostgresReviewError, PreparedReviewRun, ReviewAttemptCompletion,
    ReviewAttemptStart, ReviewFinalization, begin_review_attempt, complete_review_attempt,
    fail_review_attempt, finalize_review_proposal, get_review_run, insert_model_route,
    load_leased_review_run, mark_review_run_running, migrate, schedule_prepared_review_run,
};
use deepref_review::{
    AcceptedArtifactInput, ReviewCatalog, ReviewDefinitionKey, ReviewOrigin, ReviewRunState,
    ScheduleReviewRun,
    execution::{ExecutedReviewTask, PreparedReviewTask},
};
use sqlx::{PgPool, postgres::PgPoolOptions};
use uuid::Uuid;

static DATABASE_TEST_MUTEX: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

async fn database() -> Option<PgPool> {
    let url = std::env::var("DATABASE_URL").ok()?;
    let pool = PgPoolOptions::new()
        .max_connections(8)
        .connect(&url)
        .await
        .expect("DATABASE_URL database must be reachable");
    migrate(&pool)
        .await
        .expect("DATABASE_URL migrations must apply");
    Some(pool)
}

fn actor() -> Actor {
    Actor::new(ActorKind::User, "review-run-test-user").expect("valid actor")
}

fn prepared_task(project_id: ProjectId) -> PreparedReviewTask {
    let source_record_id = Uuid::new_v4();
    let candidate_report_id = Uuid::new_v4();
    let grounded_provenance = vec![
        IdentityProvenance {
            entity_type: "record".to_owned(),
            entity_id: source_record_id.to_string(),
            field: "title".to_owned(),
            content_hash: "a".repeat(64),
        },
        IdentityProvenance {
            entity_type: "report".to_owned(),
            entity_id: candidate_report_id.to_string(),
            field: "title".to_owned(),
            content_hash: "b".repeat(64),
        },
    ];
    PreparedReviewTask::DuplicateDetection {
        input: DedupeInput {
            project_id,
            source_record_id: source_record_id.into(),
            candidate_report_id: candidate_report_id.into(),
            source_title: Some("Source title".to_owned()),
            candidate_title: Some("Candidate title".to_owned()),
            source_year: Some(2025),
            candidate_year: Some(2025),
            source_author: Some("Luzes".to_owned()),
            candidate_author: Some("Luzes".to_owned()),
            source_title_hash: "a".repeat(64),
            candidate_title_hash: "b".repeat(64),
            grounded_signals: vec![DuplicateSignal::TitleSimilarity {
                similarity: 0.95,
                supports_match: true,
            }],
            grounded_provenance,
        },
    }
}

async fn schedule(pool: &PgPool, project_id: ProjectId) -> deepref_review::ReviewRunSnapshot {
    let task = prepared_task(project_id);
    let subject = task.subject();
    schedule_prepared_review_run(
        pool,
        PreparedReviewRun {
            command: ScheduleReviewRun {
                project_id,
                definition: ReviewDefinitionKey::DuplicateDetection,
                subject,
                origin: ReviewOrigin::ReviewerRequested,
                actor: actor(),
            },
            task,
        },
    )
    .await
    .expect("review schedules")
}

async fn claim_run(pool: &PgPool, run_id: Uuid, owner: &str) {
    let job_id: Uuid = sqlx::query_scalar("SELECT job_id FROM automation_runs WHERE id=$1")
        .bind(run_id)
        .fetch_one(pool)
        .await
        .expect("run job exists");
    let changed = sqlx::query(
        "UPDATE jobs
         SET state='running',lease_owner=$2,leased_until=now()+interval '5 minutes',
             lease_renewed_at=now(),attempts=attempts+1
         WHERE id=$1 AND state='queued'",
    )
    .bind(job_id)
    .bind(owner)
    .execute(pool)
    .await
    .expect("job claim updates")
    .rows_affected();
    assert_eq!(changed, 1);
}

#[tokio::test]
async fn review_attempts_enforce_scope_lease_exact_reuse_lineage_and_immutability() {
    let _guard = DATABASE_TEST_MUTEX.lock().await;
    let Some(pool) = database().await else { return };
    let project_id = ProjectId::new(Uuid::new_v4());
    let other_project_id = ProjectId::new(Uuid::new_v4());
    sqlx::query("INSERT INTO projects (id,name) VALUES ($1,'review run'),($2,'other project')")
        .bind(project_id.as_uuid())
        .bind(other_project_id.as_uuid())
        .execute(&pool)
        .await
        .expect("projects insert");
    let route = ResolvedModel {
        profile: ModelProfile::FastClassifier,
        provider: format!("review-test-{}", Uuid::new_v4()),
        model: "classifier".to_owned(),
        model_version: "2026-08".to_owned(),
        parameters: ModelParameters::default(),
        route_id: None,
    };
    insert_model_route(&pool, &route, Utc::now())
        .await
        .expect("route inserts");

    let result = async {
        let snapshot = schedule(&pool, project_id).await;
        assert!(matches!(snapshot.state, ReviewRunState::Queued));
        assert!(matches!(
            get_review_run(&pool, other_project_id, snapshot.id).await,
            Err(PostgresReviewError::RunNotFound)
        ));

        let owner = "review-worker";
        claim_run(&pool, snapshot.id.as_uuid(), owner).await;
        let run = load_leased_review_run(&pool, project_id, snapshot.id, owner)
            .await
            .expect("owner loads leased run");
        assert!(matches!(
            load_leased_review_run(&pool, project_id, snapshot.id, "wrong-worker").await,
            Err(PostgresReviewError::WorkerOwnership)
        ));
        mark_review_run_running(&pool, project_id, snapshot.id, owner)
            .await
            .expect("leased run starts");
        let definition = ReviewCatalog
            .compile(ReviewDefinitionKey::DuplicateDetection)
            .expect("definition compiles");

        assert!(matches!(
            begin_review_attempt(&pool, &run, &definition, "prepare", &[], "wrong-worker").await,
            Err(PostgresReviewError::WorkerOwnership)
        ));
        let first = begin_review_attempt(&pool, &run, &definition, "prepare", &[], owner)
            .await
            .expect("first attempt begins");
        let first_id = match first {
            ReviewAttemptStart::Started { attempt_id, .. } => attempt_id,
            ReviewAttemptStart::Reused { .. } => panic!("running attempts cannot reserve reuse"),
        };
        let second = begin_review_attempt(&pool, &run, &definition, "prepare", &[], owner)
            .await
            .expect("concurrent attempt begins");
        let second_id = match second {
            ReviewAttemptStart::Started {
                attempt_id,
                attempt_number,
                ..
            } => {
                assert_eq!(attempt_number, 2);
                attempt_id
            }
            ReviewAttemptStart::Reused { .. } => panic!("running attempts cannot reserve reuse"),
        };
        fail_review_attempt(
            &pool,
            &run,
            first_id,
            "test_failure",
            "fixture failure",
            owner,
        )
        .await
        .expect("running attempt fails");
        let prepare = complete_review_attempt(
            &pool,
            &run,
            ReviewAttemptCompletion {
                attempt_id: second_id,
                payload: serde_json::json!({"artifact":"prepare"}),
                media_type: "application/json",
                predecessors: &[],
                model_run_id: None,
                worker_id: owner,
            },
        )
        .await
        .expect("second attempt is accepted");
        let reused = begin_review_attempt(&pool, &run, &definition, "prepare", &[], owner)
            .await
            .expect("accepted attempt is reusable");
        assert!(matches!(
            reused,
            ReviewAttemptStart::Reused { attempt_id, artifact_id, .. }
                if attempt_id == second_id && artifact_id == prepare.artifact_id
        ));

        let prepare_input = AcceptedArtifactInput {
            artifact_id: prepare.artifact_id,
            content_hash: prepare.artifact_hash.clone(),
        };
        let generated = begin_review_attempt(
            &pool,
            &run,
            &definition,
            "generate",
            std::slice::from_ref(&prepare_input),
            owner,
        )
        .await
        .expect("generate begins");
        let generated_id = match generated {
            ReviewAttemptStart::Started { attempt_id, .. } => attempt_id,
            ReviewAttemptStart::Reused { .. } => panic!("new fingerprint cannot reuse"),
        };
        let generated = complete_review_attempt(
            &pool,
            &run,
            ReviewAttemptCompletion {
                attempt_id: generated_id,
                payload: serde_json::json!({"artifact":"generated"}),
                media_type: "application/json",
                predecessors: std::slice::from_ref(&prepare_input),
                model_run_id: None,
                worker_id: owner,
            },
        )
        .await
        .expect("generated artifact persists");
        let generated_input = AcceptedArtifactInput {
            artifact_id: generated.artifact_id,
            content_hash: generated.artifact_hash,
        };
        let validated = begin_review_attempt(
            &pool,
            &run,
            &definition,
            "validate",
            std::slice::from_ref(&generated_input),
            owner,
        )
        .await
        .expect("validation begins");
        let validated_id = match validated {
            ReviewAttemptStart::Started { attempt_id, .. } => attempt_id,
            ReviewAttemptStart::Reused { .. } => panic!("new node cannot reuse"),
        };
        let validated = complete_review_attempt(
            &pool,
            &run,
            ReviewAttemptCompletion {
                attempt_id: validated_id,
                payload: serde_json::json!({"artifact":"prepare"}),
                media_type: "application/json",
                predecessors: std::slice::from_ref(&generated_input),
                model_run_id: None,
                worker_id: owner,
            },
        )
        .await
        .expect("existing content-addressed artifact is reused without mutation");
        assert_eq!(validated.artifact_id, prepare.artifact_id);
        let lineage: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM review_artifact_lineage
             WHERE project_id=$1 AND artifact_id=$2 AND predecessor_artifact_id=$3",
        )
        .bind(project_id.as_uuid())
        .bind(validated.artifact_id)
        .bind(generated_input.artifact_id)
        .fetch_one(&pool)
        .await
        .expect("lineage query");
        assert_eq!(lineage, 1);

        let immutable = sqlx::query(
            "UPDATE review_run_manifests SET definition_version=definition_version+1
             WHERE project_id=$1 AND automation_run_id=$2",
        )
        .bind(project_id.as_uuid())
        .bind(snapshot.id.as_uuid())
        .execute(&pool)
        .await;
        assert!(immutable.is_err());

        let job_id: Uuid = sqlx::query_scalar("SELECT job_id FROM automation_runs WHERE id=$1")
            .bind(snapshot.id.as_uuid())
            .fetch_one(&pool)
            .await
            .expect("job id");
        sqlx::query("UPDATE jobs SET leased_until=now()-interval '1 second' WHERE id=$1")
            .bind(job_id)
            .execute(&pool)
            .await
            .expect("lease expires");
        assert!(matches!(
            begin_review_attempt(
                &pool,
                &run,
                &definition,
                "assemble",
                std::slice::from_ref(&generated_input),
                owner,
            )
            .await,
            Err(PostgresReviewError::WorkerOwnership)
        ));
        sqlx::query(
            "UPDATE jobs SET leased_until=now()+interval '5 minutes' WHERE id=$1 AND lease_owner=$2",
        )
        .bind(job_id)
        .bind(owner)
        .execute(&pool)
        .await
        .expect("lease restores");

        let (record_id, report_id) = match &snapshot.subject {
            deepref_review::ReviewSubject::DuplicateDetection {
                record_id,
                candidate_report_id,
            } => (record_id.as_uuid(), candidate_report_id.as_uuid()),
            _ => panic!("fixture schedules duplicate detection"),
        };
        sqlx::query("INSERT INTO reports (id,title) VALUES ($1,'candidate report')")
            .bind(report_id)
            .execute(&pool)
            .await
            .expect("candidate report inserts");
        sqlx::query("INSERT INTO project_reports (project_id,report_id) VALUES ($1,$2)")
            .bind(project_id.as_uuid())
            .bind(report_id)
            .execute(&pool)
            .await
            .expect("candidate membership inserts");
        sqlx::query(
            "INSERT INTO records (id,project_id,source,source_key,title,raw)
             VALUES ($1,$2,'test',$3,'source record','{}'::jsonb)",
        )
        .bind(record_id)
        .bind(project_id.as_uuid())
        .bind(record_id.to_string())
        .execute(&pool)
        .await
        .expect("source record inserts");
        let model_run_id = Uuid::new_v4();
        let now = Utc::now();
        PostgresAiStore::new(&pool)
            .save_run(AiRunRecord {
                id: model_run_id,
                project_id: Some(project_id),
                task_kind: AiTaskKind::DuplicateCandidateDetection,
                route: route.clone(),
                prompt_version: "fixture.v1".to_owned(),
                prompt_hash: "1".repeat(64),
                schema_version: "fixture.v1".to_owned(),
                schema_hash: "2".repeat(64),
                input_hash: "3".repeat(64),
                reuse_hash: "4".repeat(64),
                protocol_hash: None,
                document_hash: None,
                evidence_hash: None,
                evidence_refs: Vec::new(),
                usage: TokenUsage {
                    input_tokens: 1,
                    output_tokens: 1,
                },
                cost_micros: None,
                output: Some(serde_json::json!({"decision":"match"})),
                status: AiRunStatus::Completed,
                error: None,
                parent_automation_run_id: Some(snapshot.id.as_uuid()),
                created_at: now,
                completed_at: Some(now),
            })
            .await
            .expect("completed model run persists");
        let executed = ExecutedReviewTask {
            output: serde_json::json!({"decision":"match"}),
            model_run_id,
            proposal: ProposalDraft {
                project_id,
                entity_type: "record".to_owned(),
                entity_id: Some(record_id),
                operation: "duplicate_assistance".to_owned(),
                payload: serde_json::json!({
                    "kind":"duplicate_detection",
                    "task_kind":"duplicate_candidate_detection",
                    "record_id":record_id,
                    "candidate_report_id":report_id,
                    "decision":"match"
                }),
                authority: AuthorityTier::WorkflowSuggestion,
            },
        };
        let first_finalization =
            finalize_review_proposal(&pool, &run, executed.clone(), owner)
                .await
                .expect("proposal finalizes");
        let second_finalization = finalize_review_proposal(&pool, &run, executed, owner)
            .await
            .expect("proposal finalization replays");
        let proposal_id = match first_finalization {
            ReviewFinalization::Completed { proposal_id } => proposal_id,
            ReviewFinalization::Blocked => panic!("current subject should complete"),
        };
        assert_eq!(
            second_finalization,
            ReviewFinalization::Completed { proposal_id }
        );
        let proposal_count: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM ai_proposals WHERE project_id=$1 AND model_run_id=$2",
        )
        .bind(project_id.as_uuid())
        .bind(model_run_id)
        .fetch_one(&pool)
        .await
        .expect("proposal count");
        assert_eq!(proposal_count, 1);
    }
    .await;

    sqlx::query("DELETE FROM projects WHERE id = ANY($1)")
        .bind(vec![project_id.as_uuid(), other_project_id.as_uuid()])
        .execute(&pool)
        .await
        .expect("fixtures clean up");
    result
}
