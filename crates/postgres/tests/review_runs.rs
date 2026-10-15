#![allow(
    clippy::expect_used,
    clippy::panic,
    clippy::unwrap_used,
    clippy::string_slice
)]
use deepref_review::ReviewCalibrationStatus;
use std::collections::BTreeSet;

use chrono::Utc;
use deepref_ai::{
    AiProposal, AiRunRecord, AiRunStatus, AiRunStore, AiTaskKind, AuthorityTier, DedupeInput,
    DuplicateSignal, IdentityProvenance, ModelParameters, ModelProfile, ModelRouter, ProposalDraft,
    ProposalStatus, ProposalStore, ProviderEndpoint, ResolvedModel,
    ScreeningStage as AiScreeningStage, TokenUsage, register_provider_endpoint,
};
use deepref_application::jobs::JobLease;
use deepref_application::{
    AutomationRunId, ProtocolCriterionCommand, PublishProtocolCommand, SaveProtocolDraftCommand,
};
use deepref_domain::{
    Actor, ActorKind, CriterionDimension, CriterionKind, CriterionStage, FrameworkKind, ProjectId,
    ProtocolVersionId, ReportId, ScreeningStage,
};
use deepref_postgres::{
    AiProposalDecision, AiProposalDecisionRequest, CalibrationRefusal, LeasedReviewRun,
    PostgresAiStore, PostgresReviewError, PostgresReviewScheduler, PreparedReviewRun,
    ReviewAttemptCompletion, ReviewAttemptStart, ReviewCalibrationBundleInput, ReviewFinalization,
    ReviewPreparationError, ReviewScheduleIntent, begin_review_attempt, complete_review_attempt,
    decide_ai_proposal, fail_review_attempt, finalize_review_proposal, get_review_run,
    insert_model_route, insert_review_calibration_bundle, load_leased_review_run,
    mark_review_run_running, preview_review_identity, preview_screening_identity,
    schedule_prepared_review_run, schedule_prepared_review_run_as,
};
use deepref_postgres::{
    AutomationFinalization, ProtocolActor, begin_next_automation_step, block_review_run,
    fail_automation_step, fail_review_run, finalize_automation_run, get_ai_screening_target,
    get_published_protocol, publish_protocol, save_protocol_draft,
};
use deepref_review::{
    CalibrationBundleId, ReviewBlockCode, ReviewDefinitionKey, ReviewOrigin, ReviewRunSnapshot,
    ReviewRunState, ReviewScheduler, ReviewSemanticContract, ReviewSubject, ScheduleReviewRun,
    SemanticChange,
    worker::{
        AcceptedArtifactInput, CompiledReview, ExecutedReviewTask, PreparedReviewTask,
        ReviewExecutionPlan, ReviewRunManifest,
    },
};
use serde_json::json;
use sqlx::PgPool;
use uuid::Uuid;

#[test]
fn postgres_adapter_implements_the_public_review_scheduler_port() {
    fn assert_scheduler<T: ReviewScheduler>() {}
    assert_scheduler::<PostgresReviewScheduler>();
}

async fn database() -> Option<PgPool> {
    deepref_postgres::test_isolation::isolated_pool_from_env("DATABASE_URL", 8).await
}

async fn schedule_with_origin(
    pool: &PgPool,
    project_id: ProjectId,
    origin: ReviewOrigin,
) -> Result<deepref_review::ReviewRunSnapshot, PostgresReviewError> {
    let task = prepared_task(project_id);
    let subject = task.subject();
    schedule_prepared_review_run(
        pool,
        PreparedReviewRun {
            command: ScheduleReviewRun {
                project_id,
                definition: ReviewDefinitionKey::DuplicateDetection,
                subject,
                origin,
                actor: actor(),
            },
            task,
        },
    )
    .await
}

/// The stage as the AI crate names it, for previewing a screening identity.
fn ai_stage(stage: ScreeningStage) -> AiScreeningStage {
    match stage {
        ScreeningStage::TitleAbstract => AiScreeningStage::TitleAbstract,
        ScreeningStage::FullText => AiScreeningStage::FullText,
    }
}

async fn insert_route(pool: &PgPool, profile: ModelProfile, model_version: &str) {
    insert_model_route(
        pool,
        &ResolvedModel {
            profile,
            provider: format!("calibration-test-{}", Uuid::new_v4()),
            model: "reasoner".to_owned(),
            model_version: model_version.to_owned(),
            parameters: ModelParameters::default(),
            route_id: None,
        },
        Utc::now(),
    )
    .await
    .expect("route inserts");
}

/// Publishes the screening protocol. Publishing again creates a new version, which
/// changes the protocol component of every screening identity.
async fn publish_screening_protocol(pool: &PgPool, project_id: ProjectId, population: &str) {
    let actor = ProtocolActor {
        kind: "user".to_owned(),
        id: "review-run-test-user".to_owned(),
    };
    let expected_revision = get_published_protocol(pool, project_id.as_uuid())
        .await
        .map_or(0, |protocol| protocol.revision);
    let draft = save_protocol_draft(
        pool,
        &SaveProtocolDraftCommand {
            project_id,
            protocol_version_id: None,
            name: "Calibrated screening".to_owned(),
            objective: "Stage-scoped calibration".to_owned(),
            question: "Does each stage admit only its own calibration?".to_owned(),
            framework_kind: FrameworkKind::Pico,
            framework_fields: std::collections::BTreeMap::from([
                ("population".to_owned(), population.to_owned()),
                ("intervention".to_owned(), "Intervention Y".to_owned()),
                ("outcome".to_owned(), "Outcome Z".to_owned()),
            ]),
            criteria: vec![ProtocolCriterionCommand {
                id: None,
                kind: CriterionKind::Inclusion,
                stage: CriterionStage::Both,
                dimension: CriterionDimension::Population,
                label: "Population".to_owned(),
                description: population.to_owned(),
            }],
            expected_revision,
        },
        &actor,
    )
    .await
    .expect("protocol draft saves");
    publish_protocol(
        pool,
        &PublishProtocolCommand {
            project_id,
            protocol_version_id: draft.id,
            expected_revision: draft.revision,
        },
        &actor,
    )
    .await
    .expect("protocol publishes");
}

/// A project with a published screening protocol and one report to screen.
async fn screening_project(pool: &PgPool, name: &str) -> (ProjectId, Uuid) {
    let project_id = ProjectId::new(Uuid::new_v4());
    sqlx::query("INSERT INTO projects (id,name) VALUES ($1,$2)")
        .bind(project_id.as_uuid())
        .bind(name)
        .execute(pool)
        .await
        .expect("project inserts");
    publish_screening_protocol(pool, project_id, "Adults with condition X").await;
    let report_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO reports (id,title,abstract_text) VALUES ($1,$2,'Adults with condition X.')",
    )
    .bind(report_id)
    .bind(name)
    .execute(pool)
    .await
    .expect("report inserts");
    sqlx::query("INSERT INTO project_reports (project_id,report_id) VALUES ($1,$2)")
        .bind(project_id.as_uuid())
        .bind(report_id)
        .execute(pool)
        .await
        .expect("project membership inserts");
    (project_id, report_id)
}

/// The identity a screening review of this report at this stage compiles to now.
async fn screening_identity(
    pool: &PgPool,
    project_id: ProjectId,
    report_id: Uuid,
    stage: ScreeningStage,
) -> ReviewSemanticContract {
    preview_screening_identity(pool, project_id.as_uuid(), report_id, ai_stage(stage))
        .await
        .expect("screening identity previews")
}

/// The identity a duplicate-detection review compiles to now.
async fn duplicate_identity(pool: &PgPool, project_id: ProjectId) -> ReviewSemanticContract {
    let task = prepared_task(project_id);
    let command = ScheduleReviewRun {
        project_id,
        definition: ReviewDefinitionKey::DuplicateDetection,
        subject: task.subject(),
        origin: ReviewOrigin::ReviewerRequested,
        actor: actor(),
    };
    preview_review_identity(pool, &PreparedReviewRun { command, task })
        .await
        .expect("duplicate detection identity previews")
}

async fn calibrate(
    pool: &PgPool,
    project_id: ProjectId,
    identity: ReviewSemanticContract,
    status: ReviewCalibrationStatus,
) -> CalibrationBundleId {
    let id = CalibrationBundleId::new(Uuid::new_v4()).expect("bundle id");
    insert_review_calibration_bundle(
        pool,
        ReviewCalibrationBundleInput {
            id,
            project_id: project_id.as_uuid(),
            definition: identity.definition,
            identity,
            evaluation_set_id: "expert-adjudicated-v1".to_owned(),
            thresholds: json!({"precision": 0.99}),
            metrics: json!({"precision": 1.0}),
            reviewer_metadata: json!({"reviewer_ids": ["expert-1"]}),
            status,
            evaluated_at: Utc::now(),
        },
    )
    .await
    .expect("calibration bundle persists");
    id
}

/// Schedules an automation-triggered screening review of this report at this stage,
/// admitted (or refused) by `bundle`.
async fn screening_run(
    pool: &PgPool,
    project_id: ProjectId,
    report_id: Uuid,
    stage: ScreeningStage,
    bundle: CalibrationBundleId,
) -> Result<ReviewRunSnapshot, ReviewPreparationError> {
    let protocol = get_published_protocol(pool, project_id.as_uuid())
        .await
        .expect("published protocol");
    let target = get_ai_screening_target(pool, project_id.as_uuid(), report_id)
        .await
        .expect("screening target");
    PostgresReviewScheduler::new(pool)
        .schedule(ScheduleReviewRun {
            project_id,
            definition: ReviewDefinitionKey::Screening,
            subject: ReviewSubject::Screening {
                report_id: ReportId::new(report_id),
                stage,
                protocol_version_id: ProtocolVersionId::new(protocol.id),
                expected_revision: target.expected_revision,
            },
            origin: ReviewOrigin::AutomationTriggered {
                calibration_bundle_id: bundle,
            },
            actor: actor(),
        })
        .await
}

/// The refusal an admission returned. A run that was scheduled instead fails the test.
fn refusal_of(result: Result<ReviewRunSnapshot, ReviewPreparationError>) -> CalibrationRefusal {
    match result {
        Err(ReviewPreparationError::Review(PostgresReviewError::CalibrationRefused(refusal))) => {
            refusal
        }
        Err(other) => panic!("expected a calibration refusal, got: {other}"),
        Ok(_) => panic!("expected a calibration refusal, but a run was scheduled"),
    }
}

async fn automation_run_count(pool: &PgPool, project_id: ProjectId) -> i64 {
    sqlx::query_scalar(
        "SELECT count(*) FROM review_run_manifests
         WHERE project_id=$1 AND origin->>'kind'='automation_triggered'",
    )
    .bind(project_id.as_uuid())
    .fetch_one(pool)
    .await
    .expect("automation run count")
}

/// The semantic bundle hash a scheduled run was compiled with, as stored.
async fn stored_identity_hash(
    pool: &PgPool,
    project_id: ProjectId,
    run: &ReviewRunSnapshot,
) -> String {
    sqlx::query_scalar(
        "SELECT semantic_bundle_hash FROM review_run_manifests
         WHERE project_id=$1 AND automation_run_id=$2",
    )
    .bind(project_id.as_uuid())
    .bind(run.id.as_uuid())
    .fetch_one(pool)
    .await
    .expect("stored semantic hash loads")
}

async fn delete_project(pool: &PgPool, project_id: ProjectId) {
    sqlx::query("DELETE FROM projects WHERE id=$1")
        .bind(project_id.as_uuid())
        .execute(pool)
        .await
        .expect("fixtures clean up");
}

#[tokio::test]
async fn automation_reviews_require_an_exact_passing_immutable_calibration_bundle() {
    let Some(pool) = database().await else { return };
    let project_id = ProjectId::new(Uuid::new_v4());
    sqlx::query("INSERT INTO projects (id,name) VALUES ($1,'calibration admission')")
        .bind(project_id.as_uuid())
        .execute(&pool)
        .await
        .expect("project inserts");
    insert_route(&pool, ModelProfile::FastClassifier, "2026-08").await;

    let result = async {
        let identity = duplicate_identity(&pool, project_id).await;
        let reviewer_run = schedule_with_origin(&pool, project_id, ReviewOrigin::ReviewerRequested)
            .await
            .expect("reviewer-requested runs do not require calibration");
        assert_eq!(
            stored_identity_hash(&pool, project_id, &reviewer_run).await,
            identity.id().as_str(),
            "the previewed identity is the one a run is compiled with"
        );
        // The persisted scheme 3 contract carries the explicit semantic
        // version the calibration gate compares; source-tree and dependency
        // hashes live in provenance, never in this identity.
        assert_eq!(identity.scheme, deepref_review::SEMANTIC_CONTRACT_SCHEME);
        assert_eq!(
            identity.semantic_version,
            deepref_review::DUPLICATE_DETECTION_SEMANTIC_VERSION
        );
        assert_eq!(identity.models.len(), 1);

        let missing_id = CalibrationBundleId::new(Uuid::new_v4()).expect("bundle id");
        assert_eq!(
            refusal_of(
                schedule_with_origin(
                    &pool,
                    project_id,
                    ReviewOrigin::AutomationTriggered {
                        calibration_bundle_id: missing_id,
                    },
                )
                .await
                .map_err(ReviewPreparationError::from)
            ),
            CalibrationRefusal::Missing
        );

        let failed_id = calibrate(
            &pool,
            project_id,
            identity.clone(),
            ReviewCalibrationStatus::Failed,
        )
        .await;
        assert_eq!(
            refusal_of(
                schedule_with_origin(
                    &pool,
                    project_id,
                    ReviewOrigin::AutomationTriggered {
                        calibration_bundle_id: failed_id,
                    },
                )
                .await
                .map_err(ReviewPreparationError::from)
            ),
            CalibrationRefusal::Failed
        );

        let mut earlier = identity.clone();
        earlier.models[0].model_version = "earlier model route".to_owned();
        let stale_id =
            calibrate(&pool, project_id, earlier, ReviewCalibrationStatus::Passing).await;
        assert_eq!(
            refusal_of(
                schedule_with_origin(
                    &pool,
                    project_id,
                    ReviewOrigin::AutomationTriggered {
                        calibration_bundle_id: stale_id,
                    },
                )
                .await
                .map_err(ReviewPreparationError::from)
            ),
            CalibrationRefusal::Stale {
                changes: BTreeSet::from([SemanticChange::Model]),
            }
        );

        let passing_id = calibrate(
            &pool,
            project_id,
            identity,
            ReviewCalibrationStatus::Passing,
        )
        .await;
        let automated = schedule_with_origin(
            &pool,
            project_id,
            ReviewOrigin::AutomationTriggered {
                calibration_bundle_id: passing_id,
            },
        )
        .await
        .expect("exact passing calibration admits automation");
        assert!(matches!(automated.state, ReviewRunState::Queued));

        let immutable = sqlx::query(
            "UPDATE review_calibration_bundles SET status='failed'
             WHERE project_id=$1 AND id=$2",
        )
        .bind(project_id.as_uuid())
        .bind(passing_id.as_uuid())
        .execute(&pool)
        .await;
        assert!(immutable.is_err());
    }
    .await;

    delete_project(&pool, project_id).await;
    result
}

/// A title/abstract bundle and a full-text bundle coexist for one project. Each
/// admits only its own stage, and a refused run is not scheduled.
#[tokio::test]
async fn calibration_admission_is_stage_scoped_and_fails_closed() {
    let Some(pool) = database().await else { return };
    insert_route(&pool, ModelProfile::Reasoning, "v1").await;
    insert_route(&pool, ModelProfile::LongContextReasoning, "v1").await;
    let (project_id, report_id) = screening_project(&pool, "stage scoped calibration").await;
    let (other_project_id, other_report_id) =
        screening_project(&pool, "full text calibration only").await;

    let title =
        screening_identity(&pool, project_id, report_id, ScreeningStage::TitleAbstract).await;
    let full = screening_identity(&pool, project_id, report_id, ScreeningStage::FullText).await;
    assert_ne!(title.id(), full.id(), "the stage is part of the identity");

    let title_bundle = calibrate(
        &pool,
        project_id,
        title.clone(),
        ReviewCalibrationStatus::Passing,
    )
    .await;
    let full_bundle = calibrate(&pool, project_id, full, ReviewCalibrationStatus::Passing).await;

    let title_run = screening_run(
        &pool,
        project_id,
        report_id,
        ScreeningStage::TitleAbstract,
        title_bundle,
    )
    .await
    .expect("title/abstract admits its own bundle");
    assert_eq!(
        stored_identity_hash(&pool, project_id, &title_run).await,
        title.id().as_str(),
        "the admitted run carries the identity that was previewed"
    );
    screening_run(
        &pool,
        project_id,
        report_id,
        ScreeningStage::FullText,
        full_bundle,
    )
    .await
    .expect("full text admits its own bundle");

    let runs_before = automation_run_count(&pool, project_id).await;
    assert_eq!(
        refusal_of(
            screening_run(
                &pool,
                project_id,
                report_id,
                ScreeningStage::TitleAbstract,
                full_bundle,
            )
            .await
        ),
        CalibrationRefusal::StageMismatch {
            bundle: Some(ScreeningStage::FullText),
            requested: Some(ScreeningStage::TitleAbstract),
        }
    );
    assert_eq!(
        refusal_of(
            screening_run(
                &pool,
                project_id,
                report_id,
                ScreeningStage::FullText,
                title_bundle,
            )
            .await
        ),
        CalibrationRefusal::StageMismatch {
            bundle: Some(ScreeningStage::TitleAbstract),
            requested: Some(ScreeningStage::FullText),
        }
    );
    assert_eq!(
        automation_run_count(&pool, project_id).await,
        runs_before,
        "a refused stage schedules nothing"
    );

    // A project whose only bundle is for full text refuses title/abstract the same way.
    let full_only = calibrate(
        &pool,
        other_project_id,
        screening_identity(
            &pool,
            other_project_id,
            other_report_id,
            ScreeningStage::FullText,
        )
        .await,
        ReviewCalibrationStatus::Passing,
    )
    .await;
    assert_eq!(
        refusal_of(
            screening_run(
                &pool,
                other_project_id,
                other_report_id,
                ScreeningStage::TitleAbstract,
                full_only,
            )
            .await
        ),
        CalibrationRefusal::StageMismatch {
            bundle: Some(ScreeningStage::FullText),
            requested: Some(ScreeningStage::TitleAbstract),
        }
    );
    assert_eq!(automation_run_count(&pool, other_project_id).await, 0);

    delete_project(&pool, project_id).await;
    delete_project(&pool, other_project_id).await;
}

/// A changed route stales only the stage that resolves that route.
#[tokio::test]
async fn a_route_change_stales_only_the_stage_that_uses_it() {
    let Some(pool) = database().await else { return };
    insert_route(&pool, ModelProfile::Reasoning, "v1").await;
    insert_route(&pool, ModelProfile::LongContextReasoning, "v1").await;
    let (project_id, report_id) = screening_project(&pool, "route change calibration").await;
    let title_bundle = calibrate(
        &pool,
        project_id,
        screening_identity(&pool, project_id, report_id, ScreeningStage::TitleAbstract).await,
        ReviewCalibrationStatus::Passing,
    )
    .await;
    let full_bundle = calibrate(
        &pool,
        project_id,
        screening_identity(&pool, project_id, report_id, ScreeningStage::FullText).await,
        ReviewCalibrationStatus::Passing,
    )
    .await;

    // Replacing the full-text route changes the full-text identity only.
    insert_route(&pool, ModelProfile::LongContextReasoning, "v2").await;
    assert_eq!(
        refusal_of(
            screening_run(
                &pool,
                project_id,
                report_id,
                ScreeningStage::FullText,
                full_bundle,
            )
            .await
        ),
        CalibrationRefusal::Stale {
            changes: BTreeSet::from([SemanticChange::Model]),
        }
    );
    screening_run(
        &pool,
        project_id,
        report_id,
        ScreeningStage::TitleAbstract,
        title_bundle,
    )
    .await
    .expect("title/abstract still admits under its unchanged route");

    // Calibrating full text against the new route admits it again.
    let full_bundle_v2 = calibrate(
        &pool,
        project_id,
        screening_identity(&pool, project_id, report_id, ScreeningStage::FullText).await,
        ReviewCalibrationStatus::Passing,
    )
    .await;
    screening_run(
        &pool,
        project_id,
        report_id,
        ScreeningStage::FullText,
        full_bundle_v2,
    )
    .await
    .expect("recalibrated full text admits");

    // Replacing the title/abstract route changes that identity only.
    insert_route(&pool, ModelProfile::Reasoning, "v2").await;
    assert_eq!(
        refusal_of(
            screening_run(
                &pool,
                project_id,
                report_id,
                ScreeningStage::TitleAbstract,
                title_bundle,
            )
            .await
        ),
        CalibrationRefusal::Stale {
            changes: BTreeSet::from([SemanticChange::Model]),
        }
    );
    screening_run(
        &pool,
        project_id,
        report_id,
        ScreeningStage::FullText,
        full_bundle_v2,
    )
    .await
    .expect("full text is unaffected by the title/abstract route");

    delete_project(&pool, project_id).await;
}

/// Republishing the protocol is a shared semantic change: both stages go stale, and
/// the protocol is the component that changed.
#[tokio::test]
async fn republishing_the_protocol_stales_both_stages_on_protocol() {
    let Some(pool) = database().await else { return };
    insert_route(&pool, ModelProfile::Reasoning, "v1").await;
    insert_route(&pool, ModelProfile::LongContextReasoning, "v1").await;
    let (project_id, report_id) = screening_project(&pool, "protocol republish calibration").await;
    let title_bundle = calibrate(
        &pool,
        project_id,
        screening_identity(&pool, project_id, report_id, ScreeningStage::TitleAbstract).await,
        ReviewCalibrationStatus::Passing,
    )
    .await;
    let full_bundle = calibrate(
        &pool,
        project_id,
        screening_identity(&pool, project_id, report_id, ScreeningStage::FullText).await,
        ReviewCalibrationStatus::Passing,
    )
    .await;

    publish_screening_protocol(&pool, project_id, "Adolescents with condition X").await;

    let protocol_only = CalibrationRefusal::Stale {
        changes: BTreeSet::from([SemanticChange::Protocol]),
    };
    assert_eq!(
        refusal_of(
            screening_run(
                &pool,
                project_id,
                report_id,
                ScreeningStage::TitleAbstract,
                title_bundle,
            )
            .await
        ),
        protocol_only
    );
    assert_eq!(
        refusal_of(
            screening_run(
                &pool,
                project_id,
                report_id,
                ScreeningStage::FullText,
                full_bundle,
            )
            .await
        ),
        protocol_only
    );

    let title_v2 = calibrate(
        &pool,
        project_id,
        screening_identity(&pool, project_id, report_id, ScreeningStage::TitleAbstract).await,
        ReviewCalibrationStatus::Passing,
    )
    .await;
    screening_run(
        &pool,
        project_id,
        report_id,
        ScreeningStage::TitleAbstract,
        title_v2,
    )
    .await
    .expect("a calibration for the republished protocol admits");

    delete_project(&pool, project_id).await;
}

fn actor() -> Actor {
    Actor::new(ActorKind::User, "review-run-test-user").expect("valid actor")
}

fn prepared_task(project_id: ProjectId) -> PreparedReviewTask {
    prepared_task_for(project_id, Uuid::new_v4(), Uuid::new_v4())
}

/// The same duplicate-detection request every time for one record and candidate,
/// so repeated schedules compile to the same manifest hash.
fn prepared_task_for(
    project_id: ProjectId,
    source_record_id: Uuid,
    candidate_report_id: Uuid,
) -> PreparedReviewTask {
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

/// A review that fails is announced once. The automation run that carries it
/// must not add a second "Automation failed" entry for the same failure.
#[tokio::test]
async fn a_failed_review_run_is_announced_once() {
    let Some(pool) = database().await else { return };
    let project_id = ProjectId::new(Uuid::new_v4());
    sqlx::query("INSERT INTO projects (id,name) VALUES ($1,'review failure notice')")
        .bind(project_id.as_uuid())
        .execute(&pool)
        .await
        .expect("projects insert");
    let route = ResolvedModel {
        profile: ModelProfile::FastClassifier,
        provider: format!("review-notice-{}", Uuid::new_v4()),
        model: "classifier".to_owned(),
        model_version: "2026-08".to_owned(),
        parameters: ModelParameters::default(),
        route_id: None,
    };
    insert_model_route(&pool, &route, Utc::now())
        .await
        .expect("route inserts");

    let snapshot = schedule(&pool, project_id).await;
    let run_id = AutomationRunId::new(snapshot.id.as_uuid()).expect("automation run id");
    let owner = &claim_run(&pool, snapshot.id.as_uuid(), "review-notice-worker").await;
    let message = "review execution failed: AI output failed semantic validation";

    // The worker's order for a failed review: the review fails, its step
    // fails, then the automation run is finalised.
    fail_review_run(
        &pool,
        project_id,
        snapshot.id,
        "review_execution_failed",
        message,
    )
    .await
    .expect("review fails");
    let step = begin_next_automation_step(&pool, project_id, run_id, owner)
        .await
        .expect("step starts")
        .expect("the review step is queued");
    fail_automation_step(&pool, project_id, step.id, owner, message)
        .await
        .expect("step fails");
    let finalization = finalize_automation_run(&pool, project_id, run_id)
        .await
        .expect("run finalises");
    assert_eq!(finalization, AutomationFinalization::Failed);

    let kinds: Vec<String> = sqlx::query_scalar(
        "SELECT kind FROM notifications WHERE payload->>'run_id'=$1 ORDER BY revision",
    )
    .bind(snapshot.id.as_uuid().to_string())
    .fetch_all(&pool)
    .await
    .expect("notification kinds");
    assert_eq!(kinds, vec!["review_run.failed".to_owned()]);
}

async fn claim_run(pool: &PgPool, run_id: Uuid, owner: &str) -> JobLease {
    let job_id: Uuid = sqlx::query_scalar("SELECT job_id FROM automation_runs WHERE id=$1")
        .bind(run_id)
        .fetch_one(pool)
        .await
        .expect("run job exists");
    let attempt: i32 = sqlx::query_scalar(
        "UPDATE jobs
         SET state='running',lease_owner=$2,leased_until=now()+interval '5 minutes',
             lease_renewed_at=now(),attempts=attempts+1
         WHERE id=$1 AND state='queued'
         RETURNING attempts",
    )
    .bind(job_id)
    .bind(owner)
    .fetch_one(pool)
    .await
    .expect("job claim updates");
    JobLease {
        id: job_id,
        owner: owner.to_owned(),
        attempt,
    }
}

#[tokio::test]
async fn review_attempts_enforce_scope_lease_exact_reuse_lineage_and_immutability() {
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

        let owner = &claim_run(&pool, snapshot.id.as_uuid(), "review-worker").await;
        let run = load_leased_review_run(&pool, project_id, snapshot.id, owner)
            .await
            .expect("owner loads leased run");
        assert!(matches!(
            load_leased_review_run(&pool, project_id, snapshot.id, &JobLease {
                    owner: "wrong-worker".to_owned(),
                    ..owner.clone()
                }).await,
            Err(PostgresReviewError::WorkerOwnership)
        ));
        mark_review_run_running(&pool, project_id, snapshot.id, owner)
            .await
            .expect("leased run starts");
        let review = CompiledReview::compile(ReviewDefinitionKey::DuplicateDetection)
            .expect("definition compiles");
        let ReviewExecutionPlan::Standard(plan) = review.plan() else {
            panic!("duplicate detection uses the standard execution plan")
        };

        assert!(matches!(
            begin_review_attempt(&pool, &run, &review, &plan.prepare, &[], &JobLease {
                    owner: "wrong-worker".to_owned(),
                    ..owner.clone()
                }).await,
            Err(PostgresReviewError::WorkerOwnership)
        ));
        let first = begin_review_attempt(&pool, &run, &review, &plan.prepare, &[], owner)
            .await
            .expect("first attempt begins");
        let first_id = match first {
            ReviewAttemptStart::Started { attempt_id, .. } => attempt_id,
            ReviewAttemptStart::Reused { .. } => panic!("running attempts cannot reserve reuse"),
        };
        let second = begin_review_attempt(&pool, &run, &review, &plan.prepare, &[], owner)
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
                lease: owner,
            },
        )
        .await
        .expect("second attempt is accepted");
        let reused = begin_review_attempt(&pool, &run, &review, &plan.prepare, &[], owner)
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
            &review,
            &plan.generate,
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
                lease: owner,
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
            &review,
            &plan.validate,
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
                lease: owner,
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
                &review,
                &plan.assemble,
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
        .bind(&owner.owner)
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
                provider_served_model: None,
                provider_system_fingerprint: None,
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
        let proposal_created_before_finalization = PostgresAiStore::new(&pool)
            .create(AiProposal {
                id: Uuid::new_v4(),
                draft: executed.proposal.clone(),
                model_run_id,
                status: ProposalStatus::Pending,
                resolved_at: None,
                resolved_by_actor_id: None,
            })
            .await
            .expect("proposal creation survives a crash before finalization linkage");
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
        assert_eq!(proposal_id, proposal_created_before_finalization.id);
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

#[tokio::test]
async fn scheduled_reviews_store_the_normalized_endpoint_of_their_route() {
    let Some(pool) = database().await else { return };
    let project_id = ProjectId::new(Uuid::new_v4());
    sqlx::query("INSERT INTO projects (id,name) VALUES ($1,'endpoint identity')")
        .bind(project_id.as_uuid())
        .execute(&pool)
        .await
        .expect("project inserts");
    // A provider id used by this test only, so the process-wide endpoint registry is not shared.
    let provider = format!("endpoint-identity-{}", Uuid::new_v4());
    let endpoint = ProviderEndpoint::from_configured_url(
        "https://proxy.example/zen/go/v1/?api_key=query-token-must-not-persist",
    )
    .expect("endpoint normalizes");
    register_provider_endpoint(&provider, endpoint.clone()).expect("endpoint registers");
    insert_model_route(
        &pool,
        &ResolvedModel {
            profile: ModelProfile::FastClassifier,
            provider,
            model: "classifier".to_owned(),
            model_version: "2026-08".to_owned(),
            parameters: ModelParameters::default(),
            route_id: None,
        },
        Utc::now(),
    )
    .await
    .expect("route inserts");

    let snapshot = schedule_with_origin(&pool, project_id, ReviewOrigin::ReviewerRequested)
        .await
        .expect("review schedules");
    let stored: serde_json::Value = sqlx::query_scalar(
        "SELECT manifest FROM review_run_manifests
         WHERE project_id=$1 AND automation_run_id=$2",
    )
    .bind(project_id.as_uuid())
    .bind(snapshot.id.as_uuid())
    .fetch_one(&pool)
    .await
    .expect("manifest loads");
    let rendered = stored.to_string();
    assert!(
        rendered.contains("\"endpoint\":\"https://proxy.example/zen/go/v1\""),
        "{rendered}"
    );
    assert!(
        !rendered.contains("query-token-must-not-persist"),
        "{rendered}"
    );
    let manifest: ReviewRunManifest =
        serde_json::from_value(stored).expect("stored manifest deserializes");
    assert_eq!(manifest.resolved_models.len(), 1);
    assert_eq!(manifest.resolved_models[0].endpoint, Some(endpoint));
}

/// A project with a duplicate-detection route. Returns the project and the fixed
/// record and candidate, so every schedule compiles to the same manifest hash.
async fn generation_fixture(pool: &PgPool, name: &str) -> (ProjectId, Uuid, Uuid) {
    let project_id = ProjectId::new(Uuid::new_v4());
    sqlx::query("INSERT INTO projects (id,name) VALUES ($1,$2)")
        .bind(project_id.as_uuid())
        .bind(name)
        .execute(pool)
        .await
        .expect("project inserts");
    insert_route(pool, ModelProfile::FastClassifier, "2026-08").await;
    (project_id, Uuid::new_v4(), Uuid::new_v4())
}

/// Claims the run's job and loads it as the worker would.
async fn lease(
    pool: &PgPool,
    project_id: ProjectId,
    run: &ReviewRunSnapshot,
    owner: &str,
) -> (LeasedReviewRun, JobLease) {
    let lease = claim_run(pool, run.id.as_uuid(), owner).await;
    let leased = load_leased_review_run(pool, project_id, run.id, &lease)
        .await
        .expect("leased run loads");
    mark_review_run_running(pool, project_id, run.id, &lease)
        .await
        .expect("leased run starts");
    (leased, lease)
}

/// Begins the first node of a duplicate review.
async fn begin_prepare(
    pool: &PgPool,
    run: &LeasedReviewRun,
    lease: &JobLease,
) -> ReviewAttemptStart {
    let review = CompiledReview::compile(ReviewDefinitionKey::DuplicateDetection)
        .expect("definition compiles");
    let ReviewExecutionPlan::Standard(plan) = review.plan() else {
        panic!("duplicate detection uses the standard execution plan")
    };
    begin_review_attempt(pool, run, &review, &plan.prepare, &[], lease)
        .await
        .expect("prepare begins")
}

/// Begins and accepts the first node, so later generations could reuse it.
async fn accept_prepare(pool: &PgPool, run: &LeasedReviewRun, lease: &JobLease) {
    let ReviewAttemptStart::Started { attempt_id, .. } = begin_prepare(pool, run, lease).await
    else {
        panic!("a first attempt starts fresh")
    };
    complete_review_attempt(
        pool,
        run,
        ReviewAttemptCompletion {
            attempt_id,
            payload: json!({"artifact":"prepare"}),
            media_type: "application/json",
            predecessors: &[],
            model_run_id: None,
            lease,
        },
    )
    .await
    .expect("prepare is accepted");
}

/// Completes the leased run with a pending duplicate proposal and returns it.
async fn complete_with_proposal(
    pool: &PgPool,
    project_id: ProjectId,
    run: &LeasedReviewRun,
    record: Uuid,
    candidate: Uuid,
    lease: &JobLease,
) -> Uuid {
    sqlx::query("INSERT INTO reports (id,title) VALUES ($1,'candidate report')")
        .bind(candidate)
        .execute(pool)
        .await
        .expect("candidate report inserts");
    sqlx::query("INSERT INTO project_reports (project_id,report_id) VALUES ($1,$2)")
        .bind(project_id.as_uuid())
        .bind(candidate)
        .execute(pool)
        .await
        .expect("candidate membership inserts");
    sqlx::query(
        "INSERT INTO records (id,project_id,source,source_key,title,raw)
         VALUES ($1,$2,'test',$3,'source record','{}'::jsonb)",
    )
    .bind(record)
    .bind(project_id.as_uuid())
    .bind(record.to_string())
    .execute(pool)
    .await
    .expect("source record inserts");
    let route = PostgresAiStore::new(pool)
        .resolve(ModelProfile::FastClassifier)
        .await
        .expect("route resolves");
    let model_run_id = Uuid::new_v4();
    let now = Utc::now();
    PostgresAiStore::new(pool)
        .save_run(AiRunRecord {
            id: model_run_id,
            project_id: Some(project_id),
            task_kind: AiTaskKind::DuplicateCandidateDetection,
            route,
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
            provider_served_model: None,
            provider_system_fingerprint: None,
            output: Some(json!({"decision":"match"})),
            status: AiRunStatus::Completed,
            error: None,
            parent_automation_run_id: Some(run.snapshot.id.as_uuid()),
            created_at: now,
            completed_at: Some(now),
        })
        .await
        .expect("completed model run persists");
    let executed = ExecutedReviewTask {
        output: json!({"decision":"match"}),
        model_run_id,
        proposal: ProposalDraft {
            project_id,
            entity_type: "record".to_owned(),
            entity_id: Some(record),
            operation: "duplicate_assistance".to_owned(),
            payload: json!({
                "kind":"duplicate_detection",
                "task_kind":"duplicate_candidate_detection",
                "record_id":record,
                "candidate_report_id":candidate,
                "decision":"match"
            }),
            authority: AuthorityTier::WorkflowSuggestion,
        },
    };
    match finalize_review_proposal(pool, run, executed, lease)
        .await
        .expect("proposal finalizes")
    {
        ReviewFinalization::Completed { proposal_id } => proposal_id,
        ReviewFinalization::Blocked => panic!("a current subject completes"),
    }
}

#[tokio::test]
async fn a_retry_reuses_the_steps_its_failed_predecessor_accepted() {
    let Some(pool) = database().await else { return };
    let (project_id, record, candidate) = generation_fixture(&pool, "retry reuse").await;
    let request = || duplicate_request(project_id, record, candidate);
    let owner = "retry-reuse-worker";
    let first = schedule_prepared_review_run(&pool, request())
        .await
        .expect("first run schedules");
    let (leased, job_lease) = lease(&pool, project_id, &first, owner).await;
    accept_prepare(&pool, &leased, &job_lease).await;
    fail_review_run(
        &pool,
        project_id,
        first.id,
        "provider_unavailable",
        "outage",
    )
    .await
    .expect("run fails");

    let retried = schedule_prepared_review_run(&pool, request())
        .await
        .expect("retry schedules");
    assert_ne!(retried.id, first.id);
    let (leased, job_lease) = lease(&pool, project_id, &retried, owner).await;
    assert!(matches!(
        begin_prepare(&pool, &leased, &job_lease).await,
        ReviewAttemptStart::Reused { .. }
    ));
}

#[tokio::test]
async fn a_rejected_suggestion_regenerates_a_fresh_generation_that_reuses_nothing() {
    let Some(pool) = database().await else { return };
    let (project_id, record, candidate) = generation_fixture(&pool, "regenerate").await;
    let request = || duplicate_request(project_id, record, candidate);
    let owner = "regenerate-worker";
    let first = schedule_prepared_review_run(&pool, request())
        .await
        .expect("first run schedules");
    let (leased, job_lease) = lease(&pool, project_id, &first, owner).await;
    accept_prepare(&pool, &leased, &job_lease).await;
    let proposal_id =
        complete_with_proposal(&pool, project_id, &leased, record, candidate, &job_lease).await;

    let again = schedule_prepared_review_run(&pool, request())
        .await
        .expect("double submit");
    assert_eq!(
        again.id, first.id,
        "a completed run answers a double submit"
    );
    assert!(
        matches!(
            schedule_prepared_review_run_as(&pool, request(), ReviewScheduleIntent::Regenerate)
                .await,
            Err(PostgresReviewError::InvalidState(_))
        ),
        "a pending suggestion cannot be regenerated"
    );

    decide_ai_proposal(
        &pool,
        AiProposalDecisionRequest {
            project_id: project_id.as_uuid(),
            proposal_id,
            decision: AiProposalDecision::Reject,
            reason: "Not a duplicate.".to_owned(),
            reviewed_payload: None,
            actor: actor(),
        },
    )
    .await
    .expect("suggestion is rejected");

    let regenerated =
        schedule_prepared_review_run_as(&pool, request(), ReviewScheduleIntent::Regenerate)
            .await
            .expect("regeneration schedules");
    assert_ne!(regenerated.id, first.id);
    assert!(matches!(regenerated.state, ReviewRunState::Queued));
    let twice = schedule_prepared_review_run_as(&pool, request(), ReviewScheduleIntent::Regenerate)
        .await
        .expect("double regenerate");
    assert_eq!(
        twice.id, regenerated.id,
        "a double regenerate returns the in-flight run"
    );

    let (leased, job_lease) = lease(&pool, project_id, &regenerated, owner).await;
    assert!(
        matches!(
            begin_prepare(&pool, &leased, &job_lease).await,
            ReviewAttemptStart::Started { .. }
        ),
        "a regeneration must not reuse the rejected opinion's steps"
    );
}

fn duplicate_request(project_id: ProjectId, record: Uuid, candidate: Uuid) -> PreparedReviewRun {
    let task = prepared_task_for(project_id, record, candidate);
    PreparedReviewRun {
        command: ScheduleReviewRun {
            project_id,
            definition: ReviewDefinitionKey::DuplicateDetection,
            subject: task.subject(),
            origin: ReviewOrigin::ReviewerRequested,
            actor: actor(),
        },
        task,
    }
}

#[tokio::test]
async fn a_failed_review_run_is_retried_as_a_new_queued_generation() {
    let Some(pool) = database().await else { return };
    let (project_id, record, candidate) = generation_fixture(&pool, "retry failed").await;
    let request = || duplicate_request(project_id, record, candidate);
    let first = schedule_prepared_review_run(&pool, request())
        .await
        .expect("first run schedules");
    fail_review_run(
        &pool,
        project_id,
        first.id,
        "provider_unavailable",
        "the provider timed out",
    )
    .await
    .expect("run fails");

    let retried = schedule_prepared_review_run(&pool, request())
        .await
        .expect("retry schedules");
    assert_ne!(
        retried.id, first.id,
        "a failed run must not be returned again"
    );
    assert!(matches!(retried.state, ReviewRunState::Queued));
    let again = schedule_prepared_review_run(&pool, request())
        .await
        .expect("double submit");
    assert_eq!(
        again.id, retried.id,
        "a double submit returns the in-flight retry"
    );
    assert!(matches!(
        get_review_run(&pool, project_id, first.id)
            .await
            .expect("history is kept")
            .state,
        ReviewRunState::Failed { .. }
    ));
}

#[tokio::test]
async fn a_blocked_review_run_is_retried_as_a_new_queued_generation() {
    let Some(pool) = database().await else { return };
    let (project_id, record, candidate) = generation_fixture(&pool, "retry blocked").await;
    let request = || duplicate_request(project_id, record, candidate);
    let first = schedule_prepared_review_run(&pool, request())
        .await
        .expect("first run schedules");
    block_review_run(
        &pool,
        project_id,
        first.id,
        ReviewBlockCode::RepairBudgetExhausted,
        "repair budget exhausted",
    )
    .await
    .expect("run blocks");

    let retried = schedule_prepared_review_run(&pool, request())
        .await
        .expect("retry schedules");
    assert_ne!(
        retried.id, first.id,
        "a blocked run must not be returned again"
    );
    assert!(matches!(retried.state, ReviewRunState::Queued));
}
