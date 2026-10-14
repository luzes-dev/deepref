use chrono::{DateTime, Utc};
use deepref_ai::ProposalStatus;
use deepref_application::jobs::JobLease;
use deepref_domain::ProjectId;
use deepref_review::{
    ReviewBlockCode, ReviewDefinitionKey, ReviewError, ReviewOrigin, ReviewRunId,
    ReviewRunSnapshot, ReviewRunState, ReviewRunStatus, ReviewSemanticContract, ReviewSubject, ScheduleReviewRun,
    worker::{
        AcceptedArtifactInput, CompiledReview, PreparedReviewTask, ReviewHash,
        ReviewManifestInput, ReviewNode, ReviewRunManifest,
    },
};
use serde_json::Value;
use sqlx::{PgConnection, PgPool, Postgres, Row, Transaction, postgres::PgRow};
use thiserror::Error;
use uuid::Uuid;

use crate::{
    notifications::{NotificationDraft, record_notification},
    review_calibration::{CalibrationRefusal, admit_calibration},
    review_run_setup::{
        ensure_review_automation_definition, model_identity, protocol_version_id, recipe_for,
        runtime_identity,
    },
};

#[derive(Debug, Error)]
pub enum PostgresReviewError {
    #[error("review database operation failed")]
    Database(#[from] sqlx::Error),
    #[error("review value serialization failed")]
    Serialization(#[from] serde_json::Error),
    #[error("review definition or input is invalid: {0}")]
    Review(#[from] ReviewError),
    #[error("review model route could not be resolved: {0}")]
    Ai(#[from] deepref_ai::AiError),
    #[error("review run was not found")]
    RunNotFound,
    #[error("review run state is invalid: {0}")]
    InvalidState(String),
    #[error("stored review value is invalid: {0}")]
    InvalidStoredValue(String),
    #[error("review worker does not own the automation lease")]
    WorkerOwnership,
    #[error("the next review node has been scheduled")]
    StepScheduled,
    #[error("review proposal finalization conflicts with persisted state")]
    FinalizationConflict,
    #[error(transparent)]
    CalibrationRefused(#[from] CalibrationRefusal),
}

#[derive(Debug, Clone)]
pub struct PreparedReviewRun {
    pub command: ScheduleReviewRun,
    pub task: PreparedReviewTask,
}

#[derive(Debug, Clone)]
pub struct LeasedReviewRun {
    pub snapshot: ReviewRunSnapshot,
    pub manifest: ReviewRunManifest,
    pub task: PreparedReviewTask,
    /// The reuse epoch of the run's generation; see [`ReviewScheduleIntent`].
    pub reuse_epoch: i32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReviewAttemptStart {
    Reused {
        attempt_id: Uuid,
        artifact_id: Uuid,
        artifact_hash: ReviewHash,
        payload: Value,
    },
    Started {
        attempt_id: Uuid,
        attempt_number: i32,
        input_fingerprint: ReviewHash,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AcceptedReviewAttempt {
    pub attempt_id: Uuid,
    pub artifact_id: Uuid,
    pub artifact_hash: ReviewHash,
    pub payload: Value,
    pub reused: bool,
}

pub struct ReviewAttemptCompletion<'a> {
    pub attempt_id: Uuid,
    pub payload: Value,
    pub media_type: &'a str,
    pub predecessors: &'a [AcceptedArtifactInput],
    pub model_run_id: Option<Uuid>,
    pub lease: &'a JobLease,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReviewFinalization {
    Completed { proposal_id: Uuid },
    Blocked,
}

/// How a schedule request treats an earlier run of the same manifest.
///
/// The manifest hash is the identity of the work; the generation separates
/// attempts at that work. A run that ended `failed` or `blocked` never reserves
/// its subject: the next request starts generation n+1 (review ai F1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ReviewScheduleIntent {
    /// A plain request. An in-flight or completed run is returned (double-submit
    /// dedupe); a failed or blocked run is retried as a new generation that may
    /// reuse the steps its predecessors already accepted.
    #[default]
    Request,
    /// A fresh opinion after the completed run's proposal was rejected. The new
    /// generation reuses no step or model output from earlier generations.
    Regenerate,
}

enum GenerationDecision {
    Existing(Uuid),
    New { generation: i32, reuse_epoch: i32 },
}

/// Decides which generation a schedule request maps to. Serialized per manifest
/// so that concurrent double submits observe one generation.
async fn decide_generation(
    transaction: &mut Transaction<'_, Postgres>,
    project_id: ProjectId,
    manifest_hash: &ReviewHash,
    intent: ReviewScheduleIntent,
) -> Result<GenerationDecision, PostgresReviewError> {
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1,0))")
        .bind(format!(
            "review-generation:{}:{}",
            project_id.as_uuid(),
            manifest_hash.as_str()
        ))
        .execute(&mut **transaction)
        .await?;
    let Some(latest) = sqlx::query(
        "SELECT m.automation_run_id,m.generation,m.reuse_epoch,r.status AS state,p.status AS proposal_status
         FROM review_run_manifests m
         JOIN durable_runs r ON r.project_id=m.project_id AND r.id=m.automation_run_id
         LEFT JOIN ai_proposals p ON p.project_id=m.project_id AND p.id=m.proposal_id
         WHERE m.project_id=$1 AND m.manifest_hash=$2
         ORDER BY m.generation DESC LIMIT 1",
    )
    .bind(project_id.as_uuid())
    .bind(manifest_hash.as_str())
    .fetch_optional(&mut **transaction)
    .await?
    else {
        return Ok(GenerationDecision::New {
            generation: 0,
            reuse_epoch: 0,
        });
    };
    let run_id: Uuid = latest.get("automation_run_id");
    let generation: i32 = latest.get("generation");
    let reuse_epoch: i32 = latest.get("reuse_epoch");
    let state = stored_review_status(latest.get::<String, _>("state").as_str())?;
    let proposal_status = latest.get::<Option<String>, _>("proposal_status")
        .map(|value| ProposalStatus::parse(&value).ok_or_else(|| {
            PostgresReviewError::InvalidStoredValue(format!("unknown proposal status {value}"))
        }))
        .transpose()?;
    let next = generation
        .checked_add(1)
        .ok_or_else(|| PostgresReviewError::InvalidStoredValue("generation overflow".into()))?;
    match (state, intent) {
        (ReviewRunStatus::Queued | ReviewRunStatus::Running, _)
        | (ReviewRunStatus::Completed, ReviewScheduleIntent::Request) => {
            Ok(GenerationDecision::Existing(run_id))
        }
        (ReviewRunStatus::Failed | ReviewRunStatus::Blocked, ReviewScheduleIntent::Request) => Ok(GenerationDecision::New {
            generation: next,
            reuse_epoch,
        }),
        (ReviewRunStatus::Failed | ReviewRunStatus::Blocked, ReviewScheduleIntent::Regenerate) => Ok(GenerationDecision::New {
            generation: next,
            reuse_epoch: next,
        }),
        (ReviewRunStatus::Completed, ReviewScheduleIntent::Regenerate)
            if proposal_status == Some(ProposalStatus::Rejected) =>
        {
            Ok(GenerationDecision::New {
                generation: next,
                reuse_epoch: next,
            })
        }
        (ReviewRunStatus::Completed, ReviewScheduleIntent::Regenerate) => Err(PostgresReviewError::InvalidState(
            "only a rejected suggestion can be regenerated; reject the current suggestion first"
                .to_owned(),
        )),
    }
}

/// The idempotency key of one generation. Generation 0 keeps the historical key,
/// so runs scheduled before generations existed are found again.
fn generation_key(manifest_hash: &ReviewHash, generation: i32) -> String {
    if generation == 0 {
        format!("review:{}", manifest_hash.as_str())
    } else {
        format!("review:{}:{generation}", manifest_hash.as_str())
    }
}

/// The attempt fingerprint for a run. Generations in reuse epoch 0 keep the
/// manifest fingerprint, so a retry reuses steps already accepted. A
/// regeneration opens a new epoch whose fingerprints match nothing earlier, so
/// neither step attempts nor model runs (keyed by this fingerprint) are reused.
fn epoch_fingerprint(fingerprint: ReviewHash, reuse_epoch: i32) -> ReviewHash {
    if reuse_epoch == 0 {
        fingerprint
    } else {
        ReviewHash::digest_bytes(format!(
            "deepref-review-reuse-epoch-v1:{}:{reuse_epoch}",
            fingerprint.as_str()
        ))
    }
}

pub async fn schedule_prepared_review_run(
    pool: &PgPool,
    request: PreparedReviewRun,
) -> Result<ReviewRunSnapshot, PostgresReviewError> {
    schedule_prepared_review_run_as(pool, request, ReviewScheduleIntent::Request).await
}

pub async fn schedule_prepared_review_run_as(
    pool: &PgPool,
    request: PreparedReviewRun,
    intent: ReviewScheduleIntent,
) -> Result<ReviewRunSnapshot, PostgresReviewError> {
    let manifest = {
        let mut connection = pool.acquire().await?;
        build_prepared_manifest(&mut connection, &request).await?
    };
    // Runs, cohorts and calibration bundles all key semantic identity by the
    // structured contract id; the legacy aggregate is no longer admitted.
    let semantic_contract_id = manifest.semantic_contract_id()?;

    let recipe = recipe_for(request.command.definition);
    let mut transaction = pool.begin().await?;
    if matches!(
        request.command.origin,
        ReviewOrigin::AdvisoryTriggered | ReviewOrigin::AiFirstTriggered { .. }
    ) {
        sqlx::query("SELECT id FROM projects WHERE id=$1 FOR UPDATE")
            .bind(request.command.project_id.as_uuid())
            .fetch_one(&mut *transaction)
            .await?;
        if request.command.actor.kind() != deepref_domain::ActorKind::Automation {
            return Err(PostgresReviewError::InvalidState(
                "automatic advisory/routing origin requires an automation actor".into(),
            ));
        }
        match request.command.origin {
            ReviewOrigin::AiFirstTriggered { cohort_id } => {
                crate::ai_first::admit_ai_first(
                    &mut transaction,
                    &request,
                    semantic_contract_id.as_str(),
                    cohort_id,
                )
                .await?
            }
            ReviewOrigin::AdvisoryTriggered => {
                let ReviewSubject::Screening { stage, .. } = request.command.subject else {
                    return Err(PostgresReviewError::InvalidState(
                        "advisory origin only admits screening".into(),
                    ));
                };
                let task = match stage {
                    deepref_domain::ScreeningStage::TitleAbstract => {
                        deepref_application::workflows::AutonomyTask::TitleAbstractScreening
                    }
                    deepref_domain::ScreeningStage::FullText => {
                        deepref_application::workflows::AutonomyTask::FullTextScreening
                    }
                };
                if crate::autonomy::resolve_autonomy_level_in_transaction(
                    &mut transaction,
                    request.command.project_id.as_uuid(),
                    task,
                )
                .await?
                    != deepref_application::workflows::AutonomyLevel::SecondReviewer
                {
                    return Err(PostgresReviewError::InvalidState(
                        "advisory owner ceiling is not enabled".into(),
                    ));
                }
            }
            ReviewOrigin::ReviewerRequested | ReviewOrigin::AutomationTriggered { .. } => {
                unreachable!("origin checked above")
            }
        }
    }
    if let ReviewOrigin::AutomationTriggered {
        calibration_bundle_id,
    } = request.command.origin
    {
        admit_calibration(&mut transaction, &manifest, calibration_bundle_id).await?;
    }
    let definition_id = ensure_review_automation_definition(
        &mut transaction,
        request.command.project_id,
        recipe,
        &request.command.actor,
    )
    .await?;
    let run_id = match decide_generation(
        &mut transaction,
        request.command.project_id,
        &manifest.manifest_hash,
        intent,
    )
    .await?
    {
        GenerationDecision::Existing(run_id) => run_id,
        GenerationDecision::New {
            generation,
            reuse_epoch,
        } => {
            let (run_id, _, _) = crate::automations::dispatch_automation_trigger_in_transaction(
                &mut transaction,
                request.command.project_id.as_uuid(),
                definition_id,
                "manual",
                None,
                &generation_key(&manifest.manifest_hash, generation),
                request.command.actor.kind().as_str(),
                request.command.actor.id(),
            )
            .await?;
            let manifest_json = serde_json::to_value(&manifest)?;
            let subject_json = serde_json::to_value(&request.command.subject)?;
            let origin_json = serde_json::to_value(request.command.origin)?;
            let task_json = serde_json::to_value(&request.task)?;
            sqlx::query(
                "INSERT INTO review_run_manifests
                 (project_id,automation_run_id,definition_key,definition_id,definition_version,
                  manifest_hash,semantic_bundle_hash,manifest,subject,origin,prepared_task,
                  generation,reuse_epoch)
                 VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13)
                 ON CONFLICT (project_id,automation_run_id) DO NOTHING",
            )
            .bind(request.command.project_id.as_uuid())
            .bind(run_id)
            .bind(request.command.definition.as_str())
            .bind(&manifest.definition_id)
            .bind(i32::try_from(manifest.definition_version).map_err(|_| {
                PostgresReviewError::InvalidStoredValue(
                    "definition version is too large".to_owned(),
                )
            })?)
            .bind(manifest.manifest_hash.as_str())
            .bind(semantic_contract_id.as_str())
            .bind(manifest_json)
            .bind(subject_json)
            .bind(origin_json)
            .bind(task_json)
            .bind(generation)
            .bind(reuse_epoch)
            .execute(&mut *transaction)
            .await?;
            let stored_hash = sqlx::query_scalar::<_, String>(
                "SELECT manifest_hash FROM review_run_manifests
                 WHERE project_id=$1 AND automation_run_id=$2",
            )
            .bind(request.command.project_id.as_uuid())
            .bind(run_id)
            .fetch_one(&mut *transaction)
            .await?;
            if stored_hash != manifest.manifest_hash.as_str() {
                return Err(PostgresReviewError::FinalizationConflict);
            }
            run_id
        }
    };
    if let ReviewOrigin::AiFirstTriggered { cohort_id } = request.command.origin
        && let ReviewSubject::Screening { report_id, .. } = request.command.subject
    {
        sqlx::query("UPDATE ai_screening_cohort_members SET review_run_id=$3 WHERE cohort_id=$1 AND report_id=$2 AND review_run_id IS NULL")
                .bind(cohort_id).bind(report_id.as_uuid()).bind(run_id).execute(&mut *transaction).await?;
    }
    transaction.commit().await?;
    get_review_run(pool, request.command.project_id, ReviewRunId::new(run_id)?).await
}

/// The semantic identity a run for `request` would be compiled with right now.
/// Nothing is scheduled. A calibration bundle recorded for this identity admits
/// automation for the same review.
pub async fn preview_review_identity(
    pool: &PgPool,
    request: &PreparedReviewRun,
) -> Result<ReviewSemanticContract, PostgresReviewError> {
    let mut connection = pool.acquire().await?;
    preview_review_identity_on(&mut connection, request).await
}

/// [`preview_review_identity`] on one connection. A caller that holds locks on
/// the identity inputs passes its own transaction, so the identity it compares
/// is the one those locks protect and no second pool connection is needed.
pub(crate) async fn preview_review_identity_on(
    connection: &mut PgConnection,
    request: &PreparedReviewRun,
) -> Result<ReviewSemanticContract, PostgresReviewError> {
    let manifest = build_prepared_manifest(connection, request).await?;
    manifest.semantic_contract.ok_or_else(|| {
        PostgresReviewError::InvalidStoredValue(
            "compiled review manifest has no semantic contract".to_owned(),
        )
    })
}

/// Compiles the run manifest for a prepared review: the one place that decides
/// what a scheduled run and a previewed identity are built from.
async fn build_prepared_manifest(
    connection: &mut PgConnection,
    request: &PreparedReviewRun,
) -> Result<ReviewRunManifest, PostgresReviewError> {
    request.command.validate()?;
    request.task.validate()?;
    let task_subject = request.task.subject();
    if request.command.project_id != request.task.project_id()
        || request.command.definition != request.task.definition_key()
        || request.command.subject != task_subject
    {
        return Err(ReviewError::InvalidDefinition(
            "scheduled command and prepared review task disagree".to_owned(),
        )
        .into());
    }

    let review = CompiledReview::compile(request.command.definition)?;
    let route = crate::ai::resolve_route_on(connection, request.task.model_profile()).await?;
    let source_content_hash = request.task.source_content_hash()?;
    Ok(review.build_manifest(ReviewManifestInput {
        project_id: request.command.project_id,
        subject: request.command.subject.clone(),
        origin: request.command.origin,
        protocol_version_id: protocol_version_id(&request.command.subject),
        protocol_hash: request.task.protocol_hash()?,
        source_manifest_hash: source_content_hash.clone(),
        source_content_hash,
        resolved_models: vec![model_identity(route)?],
        runtime: runtime_identity()?,
    })?)
}

pub async fn get_review_run(
    pool: &PgPool,
    project_id: ProjectId,
    run_id: ReviewRunId,
) -> Result<ReviewRunSnapshot, PostgresReviewError> {
    let row = sqlx::query(
        "SELECT m.automation_run_id,m.project_id,m.definition_key,m.subject,m.origin,r.status AS state,
                m.state_code,m.state_message,m.proposal_id,m.created_at,r.started_at,r.finished_at
         FROM review_run_manifests m
         JOIN durable_runs r ON r.project_id=m.project_id AND r.id=m.automation_run_id
         WHERE m.project_id=$1 AND m.automation_run_id=$2",
    )
    .bind(project_id.as_uuid())
    .bind(run_id.as_uuid())
    .fetch_optional(pool)
    .await?
    .ok_or(PostgresReviewError::RunNotFound)?;
    snapshot_from_row(&row)
}

/// The current state of several review runs of one project. Runs that do not
/// exist are left out of the result.
pub async fn get_review_runs(
    pool: &PgPool,
    project_id: ProjectId,
    run_ids: &[Uuid],
) -> Result<Vec<ReviewRunSnapshot>, PostgresReviewError> {
    let rows = sqlx::query(
        "SELECT m.automation_run_id,m.project_id,m.definition_key,m.subject,m.origin,r.status AS state,
                m.state_code,m.state_message,m.proposal_id,m.created_at,r.started_at,r.finished_at
         FROM review_run_manifests m
         JOIN durable_runs r ON r.project_id=m.project_id AND r.id=m.automation_run_id
         WHERE m.project_id=$1 AND m.automation_run_id = ANY($2)",
    )
    .bind(project_id.as_uuid())
    .bind(run_ids)
    .fetch_all(pool)
    .await?;
    rows.iter().map(snapshot_from_row).collect()
}

pub async fn load_leased_review_run(
    pool: &PgPool,
    project_id: ProjectId,
    run_id: ReviewRunId,
    lease: &JobLease,
) -> Result<LeasedReviewRun, PostgresReviewError> {
    let row = sqlx::query(
        "SELECT m.automation_run_id,m.project_id,m.definition_key,m.subject,m.origin,r.status AS state,
                m.state_code,m.state_message,m.proposal_id,m.created_at,r.started_at,r.finished_at,
                m.manifest,m.prepared_task,m.reuse_epoch
         FROM review_run_manifests AS m
         JOIN durable_runs AS r
           ON r.project_id=m.project_id AND r.id=m.automation_run_id
         JOIN jobs AS j ON j.project_id=r.project_id AND j.id=$5
         WHERE m.project_id=$1 AND m.automation_run_id=$2
           AND j.kind='durable_run_step' AND j.state='running'
           AND (j.payload->>'run_id')::uuid=r.id
           AND j.lease_owner=$3 AND j.attempts=$4 AND j.leased_until > now()",
    )
    .bind(project_id.as_uuid())
    .bind(run_id.as_uuid())
    .bind(&lease.owner)
    .bind(lease.attempt)
    .bind(lease.id)
    .fetch_optional(pool)
    .await?
    .ok_or(PostgresReviewError::WorkerOwnership)?;
    let snapshot = snapshot_from_row(&row)?;
    let manifest = serde_json::from_value(row.get("manifest"))?;
    let task = serde_json::from_value(row.get("prepared_task"))?;
    Ok(LeasedReviewRun {
        snapshot,
        manifest,
        task,
        reuse_epoch: row.get("reuse_epoch"),
    })
}


pub async fn block_review_run(
    pool: &PgPool,
    project_id: ProjectId,
    run_id: ReviewRunId,
    code: ReviewBlockCode,
    message: &str,
) -> Result<(), PostgresReviewError> {
    finish_review_run(pool, project_id, run_id, ReviewRunStatus::Blocked, code.as_str(), message).await
}

pub async fn fail_review_run(
    pool: &PgPool,
    project_id: ProjectId,
    run_id: ReviewRunId,
    code: &str,
    message: &str,
) -> Result<(), PostgresReviewError> {
    finish_review_run(pool, project_id, run_id, ReviewRunStatus::Failed, code, message).await
}

async fn finish_review_run(
    pool: &PgPool,
    project_id: ProjectId,
    run_id: ReviewRunId,
    state: ReviewRunStatus,
    code: &str,
    message: &str,
) -> Result<(), PostgresReviewError> {
    if code.trim().is_empty()
        || code.len() > 100
        || message.trim().is_empty()
        || message.len() > 4096
    {
        return Err(PostgresReviewError::InvalidState(
            "terminal review error metadata is invalid".to_owned(),
        ));
    }
    let mut tx=pool.begin().await?;
    let definition_key = sqlx::query_scalar::<_,String>(
        "SELECT definition_key FROM review_run_manifests WHERE project_id=$1 AND automation_run_id=$2")
        .bind(project_id.as_uuid()).bind(run_id.as_uuid()).fetch_optional(&mut *tx).await?
        .ok_or(PostgresReviewError::RunNotFound)?;
    if !crate::durable_runs::finish_run(&mut tx,run_id.as_uuid(),state.as_str(),Some(message)).await? {
        tx.commit().await?;
        let existing=get_review_run(pool,project_id,run_id).await?;
        if !existing.state.terminal() {
            return Err(PostgresReviewError::InvalidState("review run could not enter a terminal state".to_owned()));
        }
        return Ok(());
    }
    sqlx::query("UPDATE review_run_manifests SET state_code=$3,state_message=$4 WHERE project_id=$1 AND automation_run_id=$2")
        .bind(project_id.as_uuid()).bind(run_id.as_uuid()).bind(code).bind(message)
        .execute(&mut *tx).await?;
    tx.commit().await?;
    record_notification(
        pool,
        &NotificationDraft::error(
            "review_run.failed",
            Some(project_id.as_uuid()),
            &format!("{} failed", review_definition_label(&definition_key)),
            Some(message.to_owned()),
            serde_json::json!({
                "run_id": run_id.as_uuid(),
                "definition_key": definition_key,
                "code": code,
            }),
        ),
    )
    .await?;
    Ok(())
}

pub(crate) fn review_definition_label(key: &str) -> &'static str {
    match key {
        "screening" => "Title-abstract screening",
        "duplicate_detection" => "Duplicate detection",
        "study_classification" => "Study classification",
        "study_grouping" => "Study grouping",
        "appraisal_prefill" => "Appraisal prefill",
        "data_extraction" => "Data extraction",
        _ => "Review run",
    }
}

async fn schedule_review_node(
    tx: &mut Transaction<'_, Postgres>, project_id: ProjectId, run_id: ReviewRunId,
    node_id: &str, fingerprint: &str,
) -> Result<(), PostgresReviewError> {
    let status = sqlx::query_scalar::<_,String>("SELECT status FROM durable_runs WHERE project_id=$1 AND id=$2 FOR UPDATE")
        .bind(project_id.as_uuid()).bind(run_id.as_uuid()).fetch_one(&mut **tx).await?;
    if !matches!(status.as_str(),"queued"|"running"|"waiting") {
        return Err(PostgresReviewError::WorkerOwnership);
    }
    let step_key = format!("{node_id}:{fingerprint}");
    let existing=sqlx::query_scalar::<_,Uuid>("SELECT a.step_id FROM durable_attempts a
        JOIN review_attempt_details d ON d.id=a.id WHERE d.project_id=$1 AND d.run_id=$2
        AND d.node_id=$3 AND d.input_fingerprint=$4 ORDER BY a.attempt_number DESC LIMIT 1")
        .bind(project_id.as_uuid()).bind(run_id.as_uuid()).bind(node_id).bind(fingerprint)
        .fetch_optional(&mut **tx).await?;
    let step_id=match existing {
        Some(step_id)=>step_id,
        None=>sqlx::query_scalar::<_,Uuid>("SELECT automation_uuid_from_text('durable-review-step',$1)")
            .bind(format!("{}:{}:{step_key}",project_id.as_uuid(),run_id.as_uuid()))
            .fetch_one(&mut **tx).await?,
    };
    sqlx::query("INSERT INTO durable_steps(id,project_id,run_id,handler_kind,step_key,status)
        VALUES($1,$2,$3,'review',$4,'queued') ON CONFLICT(id) DO NOTHING")
        .bind(step_id).bind(project_id.as_uuid()).bind(run_id.as_uuid()).bind(step_key)
        .execute(&mut **tx).await?;
    sqlx::query("INSERT INTO jobs(id,project_id,kind,payload,max_attempts,dedupe_key)
        VALUES($1,$2,'durable_run_step',$3,5,$4) ON CONFLICT(dedupe_key) DO NOTHING")
        .bind(Uuid::new_v4()).bind(project_id.as_uuid())
        .bind(serde_json::json!({"run_id":run_id.as_uuid(),"step_id":step_id,"handler_kind":"review"}))
        .bind(format!("durable_review_step:{step_id}")).execute(&mut **tx).await?;
    Ok(())
}

pub async fn begin_review_attempt(
    pool: &PgPool,
    run: &LeasedReviewRun,
    review: &CompiledReview,
    node: &ReviewNode,
    predecessors: &[AcceptedArtifactInput],
    lease: &JobLease,
) -> Result<ReviewAttemptStart, PostgresReviewError> {
    let mut identity = review.attempt_identity(&run.manifest, node, predecessors)?;
    identity.input_fingerprint = epoch_fingerprint(identity.input_fingerprint, run.reuse_epoch);
    let project_id = run.snapshot.project_id;
    let run_id = run.snapshot.id;
    let mut transaction = pool.begin().await?;
    assert_worker_lease(&mut transaction, project_id, run_id, lease).await?;
    // Replay only artifacts completed in this generation. Global accepted reuse
    // still gets its own canonical step/attempt in the new generation.
    if let Some(row) = sqlx::query(
        "SELECT COALESCE(a.accepted_attempt_id,a.id) AS attempt_id,a.artifact_id,r.content_hash,r.payload
         FROM review_attempt_details a
         JOIN durable_attempts d ON d.project_id=a.project_id AND d.id=a.id
         JOIN review_artifacts r ON r.project_id=a.project_id AND r.id=a.artifact_id
         WHERE a.project_id=$1 AND a.run_id=$2 AND a.node_id=$3
           AND a.input_fingerprint=$4 AND d.status='completed'
         ORDER BY d.finished_at,a.id LIMIT 1",
    )
    .bind(project_id.as_uuid()).bind(run_id.as_uuid())
    .bind(node.id()).bind(identity.input_fingerprint.as_str())
    .fetch_optional(&mut *transaction).await? {
        transaction.commit().await?;
        return accepted_start_from_row(&row);
    }
    let current_step = sqlx::query(
        "SELECT s.id,s.step_key FROM jobs j
         JOIN durable_steps s ON s.project_id=j.project_id
           AND s.id=(j.payload->>'step_id')::uuid
         WHERE j.id=$1 AND s.run_id=$2",
    )
    .bind(lease.id).bind(run_id.as_uuid())
    .fetch_one(&mut *transaction).await?;
    let current_key: String = current_step.get("step_key");
    let current_matches = current_key == node.id()
        || current_key == format!("{}:{}", node.id(), identity.input_fingerprint.as_str());
    if !current_matches {
        let current_id: Uuid = current_step.get("id");
        // The recipe entry step delegates to the typed review planner once.
        // Later jobs replay completed domain artifacts without claiming them.
        let current_kind: String = sqlx::query_scalar("SELECT handler_kind FROM durable_steps WHERE id=$1")
            .bind(current_id).fetch_one(&mut *transaction).await?;
        if current_kind == "automation" && !crate::durable_runs::finish_step(
            &mut transaction,current_id,lease,"completed",None,None).await? {
            return Err(PostgresReviewError::WorkerOwnership);
        }
        schedule_review_node(&mut transaction, project_id, run_id, node.id(),
            identity.input_fingerprint.as_str()).await?;
        transaction.commit().await?;
        return Err(PostgresReviewError::StepScheduled);
    }
    let step_id: Uuid = current_step.get("id");
    let attempt_number = crate::durable_runs::begin_step(&mut transaction, step_id, lease)
        .await?
        .ok_or(PostgresReviewError::WorkerOwnership)?;
    let attempt_id = sqlx::query_scalar::<_, Uuid>(
        "SELECT id FROM durable_attempts WHERE step_id=$1 AND job_id=$2
         AND job_attempt=$3 AND status='running'",
    )
    .bind(step_id)
    .bind(lease.id)
    .bind(lease.attempt)
    .fetch_one(&mut *transaction)
    .await?;
    sqlx::query(
        "INSERT INTO review_attempt_details
         (id,project_id,run_id,node_id,node_version,input_fingerprint)
         VALUES ($1,$2,$3,$4,$5,$6)",
    )
    .bind(attempt_id)
    .bind(project_id.as_uuid())
    .bind(run_id.as_uuid())
    .bind(&identity.node_id)
    .bind(i32::try_from(identity.node_version).map_err(|_| {
        PostgresReviewError::InvalidStoredValue("node version is too large".to_owned())
    })?)
    .bind(identity.input_fingerprint.as_str())
    .execute(&mut *transaction)
    .await?;
    if let Some(row) = find_accepted_attempt(
        &mut transaction, project_id, &identity.node_id, &identity.input_fingerprint,
    ).await? {
        sqlx::query(
            "UPDATE review_attempt_details SET artifact_id=$2,accepted_attempt_id=$3
             WHERE id=$1",
        )
        .bind(attempt_id)
        .bind(row.get::<Uuid, _>("artifact_id"))
        .bind(row.get::<Uuid,_>("attempt_id"))
        .execute(&mut *transaction)
        .await?;
        if !crate::durable_runs::finish_step(
            &mut transaction, step_id, lease, "completed", None, None,
        ).await? {
            return Err(PostgresReviewError::WorkerOwnership);
        }
        transaction.commit().await?;
        return accepted_start_from_row(&row);
    }
    transaction.commit().await?;
    Ok(ReviewAttemptStart::Started {
        attempt_id,
        attempt_number,
        input_fingerprint: identity.input_fingerprint,
    })
}

pub async fn fail_review_attempt(
    pool: &PgPool,
    run: &LeasedReviewRun,
    attempt_id: Uuid,
    code: &str,
    message: &str,
    lease: &JobLease,
) -> Result<(), PostgresReviewError> {
    let mut transaction = pool.begin().await?;
    assert_worker_lease(
        &mut transaction,
        run.snapshot.project_id,
        run.snapshot.id,
        lease,
    )
    .await?;
    let step_id = sqlx::query_scalar::<_, Uuid>(
        "SELECT step_id FROM durable_attempts WHERE id=$1 AND project_id=$2 AND run_id=$3
         AND job_id=$4 AND job_attempt=$5 AND status='running'",
    )
    .bind(attempt_id)
    .bind(run.snapshot.project_id.as_uuid())
    .bind(run.snapshot.id.as_uuid())
    .bind(lease.id)
    .bind(lease.attempt)
    .fetch_optional(&mut *transaction)
    .await?
    .ok_or(PostgresReviewError::WorkerOwnership)?;
    if !crate::durable_runs::retry_step(&mut transaction, step_id, lease, Some(message)).await? {
        return Err(PostgresReviewError::WorkerOwnership);
    }
    let changed = sqlx::query(
        "UPDATE review_attempt_details SET error_code=$2,error_message=$3
         WHERE id=$1",
    )
    .bind(attempt_id)
    .bind(code)
    .bind(message)
    .execute(&mut *transaction)
    .await?
    .rows_affected();
    if changed == 0 {
        return Err(PostgresReviewError::InvalidState(
            "review attempt is not running".to_owned(),
        ));
    }
    transaction.commit().await?;
    Ok(())
}


fn stored_review_status(value: &str) -> Result<ReviewRunStatus, PostgresReviewError> {
    ReviewRunStatus::parse(value).ok_or_else(|| {
        PostgresReviewError::InvalidStoredValue(format!("unknown review state {value}"))
    })
}

fn snapshot_from_row(row: &PgRow) -> Result<ReviewRunSnapshot, PostgresReviewError> {
    let definition_name: String = row.get("definition_key");
    let definition = ReviewDefinitionKey::parse(&definition_name).ok_or_else(|| {
        PostgresReviewError::InvalidStoredValue(format!(
            "unknown review definition {definition_name}"
        ))
    })?;
    let subject: ReviewSubject = serde_json::from_value(row.get("subject"))?;
    let origin = serde_json::from_value(row.get("origin"))?;
    let state_name: String = row.get("state");
    let state_code: Option<String> = row.get("state_code");
    let state_message: Option<String> = row.get("state_message");
    let state = match stored_review_status(&state_name)? {
        ReviewRunStatus::Queued => ReviewRunState::Queued,
        ReviewRunStatus::Running => ReviewRunState::Running,
        ReviewRunStatus::Blocked => ReviewRunState::Blocked {
            code: ReviewBlockCode::parse(state_code.as_deref().unwrap_or_default()).ok_or_else(
                || PostgresReviewError::InvalidStoredValue("unknown review block code".to_owned()),
            )?,
            message: state_message.ok_or_else(|| {
                PostgresReviewError::InvalidStoredValue("blocked review has no message".to_owned())
            })?,
        },
        ReviewRunStatus::Failed => ReviewRunState::Failed {
            code: state_code.ok_or_else(|| {
                PostgresReviewError::InvalidStoredValue("failed review has no code".to_owned())
            })?,
            message: state_message.ok_or_else(|| {
                PostgresReviewError::InvalidStoredValue("failed review has no message".to_owned())
            })?,
        },
        ReviewRunStatus::Completed => ReviewRunState::Completed {
            proposal_id: row.get::<Option<Uuid>, _>("proposal_id").ok_or_else(|| {
                PostgresReviewError::InvalidStoredValue(
                    "completed review has no proposal".to_owned(),
                )
            })?,
        },
    };
    Ok(ReviewRunSnapshot {
        id: ReviewRunId::new(row.get("automation_run_id"))?,
        project_id: ProjectId::new(row.get("project_id")),
        definition,
        subject,
        origin,
        state,
        created_at: row.get::<DateTime<Utc>, _>("created_at"),
        started_at: row.get("started_at"),
        finished_at: row.get("finished_at"),
    })
}

pub(crate) async fn find_accepted_attempt(
    transaction: &mut Transaction<'_, Postgres>,
    project_id: ProjectId,
    node_id: &str,
    fingerprint: &ReviewHash,
) -> Result<Option<PgRow>, sqlx::Error> {
    sqlx::query(
        "SELECT a.id AS attempt_id,a.artifact_id,r.content_hash,r.payload
         FROM review_attempt_details AS a
         JOIN durable_attempts d ON d.project_id=a.project_id AND d.id=a.id
         JOIN review_artifacts AS r
           ON r.project_id=a.project_id AND r.id=a.artifact_id
         WHERE a.project_id=$1 AND a.node_id=$2 AND a.input_fingerprint=$3
           AND d.status='completed' AND a.accepted_at IS NOT NULL
         ORDER BY a.accepted_at,a.id LIMIT 1",
    )
    .bind(project_id.as_uuid())
    .bind(node_id)
    .bind(fingerprint.as_str())
    .fetch_optional(&mut **transaction)
    .await
}

pub(crate) async fn assert_worker_lease(
    transaction: &mut Transaction<'_, Postgres>,
    project_id: ProjectId,
    run_id: ReviewRunId,
    lease: &JobLease,
) -> Result<(), PostgresReviewError> {
    let step_id=sqlx::query_scalar::<_,Uuid>("SELECT s.id FROM jobs j JOIN durable_steps s
        ON j.payload->>'step_id'=s.id::text AND j.project_id=s.project_id
        WHERE j.id=$1 AND s.project_id=$2 AND s.run_id=$3")
        .bind(lease.id).bind(project_id.as_uuid()).bind(run_id.as_uuid())
        .fetch_optional(&mut **transaction).await?;
    let Some(step_id)=step_id else{return Err(PostgresReviewError::WorkerOwnership)};
    if !crate::durable_runs::verify_active_step_lease(transaction,step_id,lease).await? {
        return Err(PostgresReviewError::WorkerOwnership);
    }
    Ok(())
}

fn accepted_start_from_row(row: &PgRow) -> Result<ReviewAttemptStart, PostgresReviewError> {
    Ok(ReviewAttemptStart::Reused {
        attempt_id: row.get("attempt_id"),
        artifact_id: row.get("artifact_id"),
        artifact_hash: ReviewHash::parse(row.get::<String, _>("content_hash"))?,
        payload: row.get("payload"),
    })
}

pub(crate) fn accepted_attempt_from_row(
    row: &PgRow,
    reused: bool,
) -> Result<AcceptedReviewAttempt, PostgresReviewError> {
    Ok(AcceptedReviewAttempt {
        attempt_id: row.get("attempt_id"),
        artifact_id: row.get("artifact_id"),
        artifact_hash: ReviewHash::parse(row.get::<String, _>("content_hash"))?,
        payload: row.get("payload"),
        reused,
    })
}


pub(crate) async fn subject_is_current_in_transaction(
    transaction: &mut Transaction<'_, Postgres>,
    project_id: ProjectId,
    subject: &ReviewSubject,
) -> Result<bool, sqlx::Error> {
    subject_is_current_on_connection(transaction, project_id, subject).await
}

async fn subject_is_current_on_connection(
    connection: &mut PgConnection,
    project_id: ProjectId,
    subject: &ReviewSubject,
) -> Result<bool, sqlx::Error> {
    match subject {
        ReviewSubject::Screening {
            report_id,
            protocol_version_id,
            expected_revision,
            ..
        } => {
            let row = sqlx::query(
                "SELECT COALESCE(s.revision,0)::bigint AS revision,
                        (SELECT id FROM protocol_versions
                         WHERE project_id=$1 AND status='published'
                         ORDER BY version DESC,id DESC LIMIT 1) AS protocol_version_id
                 FROM project_reports AS p
                 LEFT JOIN screening_state AS s
                   ON s.project_id=p.project_id AND s.report_id=p.report_id
                 WHERE p.project_id=$1 AND p.report_id=$2",
            )
            .bind(project_id.as_uuid())
            .bind(report_id.as_uuid())
            .fetch_optional(&mut *connection)
            .await?;
            Ok(row.is_some_and(|row| {
                row.get::<i64, _>("revision") == *expected_revision
                    && row.get::<Option<Uuid>, _>("protocol_version_id")
                        == Some(protocol_version_id.as_uuid())
            }))
        }
        ReviewSubject::StudyClassification {
            study_id,
            expected_revision,
        } => {
            let revision = sqlx::query_scalar::<_, i64>(
                "SELECT study_revision FROM studies WHERE project_id=$1 AND id=$2",
            )
            .bind(project_id.as_uuid())
            .bind(study_id.as_uuid())
            .fetch_optional(&mut *connection)
            .await?;
            Ok(revision == Some(*expected_revision))
        }
        ReviewSubject::StudyGrouping {
            report_id,
            expected_previous_study_id,
            expected_previous_study_revision,
        } => {
            let row = sqlx::query(
                "SELECT s.id AS study_id,s.study_revision
                 FROM project_reports AS p
                 LEFT JOIN study_reports AS sr
                   ON sr.project_id=p.project_id AND sr.report_id=p.report_id
                 LEFT JOIN studies AS s
                   ON s.project_id=sr.project_id AND s.id=sr.study_id
                 WHERE p.project_id=$1 AND p.report_id=$2",
            )
            .bind(project_id.as_uuid())
            .bind(report_id.as_uuid())
            .fetch_optional(&mut *connection)
            .await?;
            Ok(row.is_some_and(|row| {
                row.get::<Option<Uuid>, _>("study_id")
                    == expected_previous_study_id.map(|id| id.as_uuid())
                    && row.get::<Option<i64>, _>("study_revision")
                        == *expected_previous_study_revision
            }))
        }
        ReviewSubject::DuplicateDetection {
            record_id,
            candidate_report_id,
        } => Ok(sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS(
               SELECT 1 FROM records r, project_reports p
               WHERE r.project_id=$1 AND r.id=$2
                 AND p.project_id=$1 AND p.report_id=$3
             )",
        )
        .bind(project_id.as_uuid())
        .bind(record_id.as_uuid())
        .bind(candidate_report_id.as_uuid())
        .fetch_one(&mut *connection)
        .await?),
        ReviewSubject::AppraisalPrefill { report_id, .. } => Ok(sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS(SELECT 1 FROM project_reports WHERE project_id=$1 AND report_id=$2)",
        )
        .bind(project_id.as_uuid())
        .bind(report_id.as_uuid())
        .fetch_one(&mut *connection)
        .await?),
        ReviewSubject::DataExtraction { study_id, .. } => Ok(sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS(SELECT 1 FROM studies WHERE project_id=$1 AND id=$2)",
        )
        .bind(project_id.as_uuid())
        .bind(study_id.as_uuid())
        .fetch_one(&mut *connection)
        .await?),
    }
}

#[cfg(test)]
mod lifecycle_status_tests {
    use super::{PostgresReviewError, stored_review_status};

    #[test]
    fn unknown_review_run_status_is_invalid_stored_value() {
        for value in ["", "RUNNING", "complete", " running", "cancelled"] {
            assert!(matches!(
                stored_review_status(value),
                Err(PostgresReviewError::InvalidStoredValue(_))
            ));
        }
    }
}
