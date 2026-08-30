use chrono::{DateTime, Utc};
use deepref_ai::{
    AiProposal, ModelRouter, ProposalStatus, ProposalStore, ResolvedModel, hash_json,
};
use deepref_application::BuiltInAutomationRecipe;
use deepref_domain::ProjectId;
use deepref_review::{
    AcceptedArtifactInput, ReviewBlockCode, ReviewCatalog, ReviewDefinitionKey, ReviewError,
    ReviewHash, ReviewManifestInput, ReviewModelIdentity, ReviewRunId, ReviewRunManifest,
    ReviewRunSnapshot, ReviewRunState, ReviewRuntimeIdentity, ReviewSubject, ScheduleReviewRun,
    execution::{ExecutedReviewTask, PreparedReviewTask},
    fingerprint_node,
};
use serde_json::Value;
use sqlx::{PgPool, Postgres, Row, Transaction, postgres::PgRow};
use thiserror::Error;
use uuid::Uuid;

use crate::PostgresAiStore;

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
    #[error("review proposal finalization conflicts with persisted state")]
    FinalizationConflict,
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
    pub worker_id: &'a str,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReviewFinalization {
    Completed { proposal_id: Uuid },
    Blocked,
}

pub async fn schedule_prepared_review_run(
    pool: &PgPool,
    request: PreparedReviewRun,
) -> Result<ReviewRunSnapshot, PostgresReviewError> {
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

    let definition = ReviewCatalog.compile(request.command.definition)?;
    let route = PostgresAiStore::new(pool)
        .resolve(request.task.model_profile())
        .await?;
    let source_content_hash = request.task.source_content_hash()?;
    let manifest = ReviewRunManifest::build(
        &definition,
        ReviewManifestInput {
            project_id: request.command.project_id,
            subject: request.command.subject.clone(),
            origin: request.command.origin,
            protocol_version_id: protocol_version_id(&request.command.subject),
            protocol_hash: request.task.protocol_hash()?,
            source_manifest_hash: source_content_hash.clone(),
            source_content_hash,
            resolved_models: vec![model_identity(route)?],
            runtime: runtime_identity(),
        },
    )?;

    let recipe = recipe_for(request.command.definition);
    let mut transaction = pool.begin().await?;
    let definition_id = ensure_review_automation_definition(
        &mut transaction,
        request.command.project_id,
        recipe,
        &request.command.actor,
    )
    .await?;
    let idempotency_key = format!("review:{}", manifest.manifest_hash);
    let dispatch = sqlx::query(
        "SELECT run_id, job_id, created
         FROM dispatch_automation_trigger($1,$2,'manual',NULL,$3,$4,$5)",
    )
    .bind(request.command.project_id.as_uuid())
    .bind(definition_id)
    .bind(idempotency_key)
    .bind(request.command.actor.kind().as_str())
    .bind(request.command.actor.id())
    .fetch_one(&mut *transaction)
    .await?;
    let run_id: Uuid = dispatch.get("run_id");
    let manifest_json = serde_json::to_value(&manifest)?;
    let subject_json = serde_json::to_value(&request.command.subject)?;
    let origin_json = serde_json::to_value(request.command.origin)?;
    let task_json = serde_json::to_value(&request.task)?;
    sqlx::query(
        "INSERT INTO review_run_manifests
         (project_id,automation_run_id,definition_key,definition_id,definition_version,
          manifest_hash,semantic_bundle_hash,manifest,subject,origin,prepared_task)
         VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11)
         ON CONFLICT (project_id,automation_run_id) DO NOTHING",
    )
    .bind(request.command.project_id.as_uuid())
    .bind(run_id)
    .bind(request.command.definition.as_str())
    .bind(&manifest.definition_id)
    .bind(i32::try_from(manifest.definition_version).map_err(|_| {
        PostgresReviewError::InvalidStoredValue("definition version is too large".to_owned())
    })?)
    .bind(manifest.manifest_hash.as_str())
    .bind(manifest.semantic_bundle_hash.as_str())
    .bind(manifest_json)
    .bind(subject_json)
    .bind(origin_json)
    .bind(task_json)
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
    transaction.commit().await?;
    get_review_run(pool, request.command.project_id, ReviewRunId::new(run_id)?).await
}

pub async fn get_review_run(
    pool: &PgPool,
    project_id: ProjectId,
    run_id: ReviewRunId,
) -> Result<ReviewRunSnapshot, PostgresReviewError> {
    let row = sqlx::query(
        "SELECT automation_run_id,project_id,definition_key,subject,origin,state,
                state_code,state_message,proposal_id,created_at,started_at,finished_at
         FROM review_run_manifests
         WHERE project_id=$1 AND automation_run_id=$2",
    )
    .bind(project_id.as_uuid())
    .bind(run_id.as_uuid())
    .fetch_optional(pool)
    .await?
    .ok_or(PostgresReviewError::RunNotFound)?;
    snapshot_from_row(&row)
}

pub async fn load_leased_review_run(
    pool: &PgPool,
    project_id: ProjectId,
    run_id: ReviewRunId,
    worker_id: &str,
) -> Result<LeasedReviewRun, PostgresReviewError> {
    let row = sqlx::query(
        "SELECT m.automation_run_id,m.project_id,m.definition_key,m.subject,m.origin,m.state,
                m.state_code,m.state_message,m.proposal_id,m.created_at,m.started_at,m.finished_at,
                m.manifest,m.prepared_task
         FROM review_run_manifests AS m
         JOIN automation_runs AS r
           ON r.project_id=m.project_id AND r.id=m.automation_run_id
         JOIN jobs AS j ON j.project_id=r.project_id AND j.id=r.job_id
         WHERE m.project_id=$1 AND m.automation_run_id=$2
           AND j.kind='automation_run' AND j.state='running'
           AND j.lease_owner=$3 AND j.leased_until > now()",
    )
    .bind(project_id.as_uuid())
    .bind(run_id.as_uuid())
    .bind(worker_id)
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
    })
}

pub async fn mark_review_run_running(
    pool: &PgPool,
    project_id: ProjectId,
    run_id: ReviewRunId,
    worker_id: &str,
) -> Result<(), PostgresReviewError> {
    let changed = sqlx::query(
        "UPDATE review_run_manifests AS m
         SET state='running',started_at=COALESCE(m.started_at,now()),finished_at=NULL,
             state_code=NULL,state_message=NULL
         FROM automation_runs AS r, jobs AS j
         WHERE m.project_id=$1 AND m.automation_run_id=$2
           AND m.state IN ('queued','running')
           AND r.project_id=m.project_id AND r.id=m.automation_run_id
           AND j.project_id=r.project_id AND j.id=r.job_id
           AND j.state='running' AND j.lease_owner=$3 AND j.leased_until > now()",
    )
    .bind(project_id.as_uuid())
    .bind(run_id.as_uuid())
    .bind(worker_id)
    .execute(pool)
    .await?
    .rows_affected();
    if changed == 0 {
        return Err(PostgresReviewError::InvalidState(
            "review run is already terminal".to_owned(),
        ));
    }
    Ok(())
}

pub async fn block_review_run(
    pool: &PgPool,
    project_id: ProjectId,
    run_id: ReviewRunId,
    code: ReviewBlockCode,
    message: &str,
) -> Result<(), PostgresReviewError> {
    finish_review_run(pool, project_id, run_id, "blocked", code.as_str(), message).await
}

pub async fn fail_review_run(
    pool: &PgPool,
    project_id: ProjectId,
    run_id: ReviewRunId,
    code: &str,
    message: &str,
) -> Result<(), PostgresReviewError> {
    finish_review_run(pool, project_id, run_id, "failed", code, message).await
}

async fn finish_review_run(
    pool: &PgPool,
    project_id: ProjectId,
    run_id: ReviewRunId,
    state: &str,
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
    let changed = sqlx::query(
        "UPDATE review_run_manifests
         SET state=$3,started_at=COALESCE(started_at,now()),finished_at=now(),
             state_code=$4,state_message=$5
         WHERE project_id=$1 AND automation_run_id=$2 AND state IN ('queued','running')",
    )
    .bind(project_id.as_uuid())
    .bind(run_id.as_uuid())
    .bind(state)
    .bind(code)
    .bind(message)
    .execute(pool)
    .await?
    .rows_affected();
    if changed == 0 {
        let existing = get_review_run(pool, project_id, run_id).await?;
        if !existing.state.terminal() {
            return Err(PostgresReviewError::InvalidState(
                "review run could not enter a terminal state".to_owned(),
            ));
        }
    }
    Ok(())
}

pub async fn begin_review_attempt(
    pool: &PgPool,
    run: &LeasedReviewRun,
    definition: &deepref_review::CompiledReviewDefinition,
    node_id: &str,
    predecessors: &[AcceptedArtifactInput],
    worker_id: &str,
) -> Result<ReviewAttemptStart, PostgresReviewError> {
    let fingerprint = fingerprint_node(definition, &run.manifest, node_id, predecessors)?;
    let node_version = definition
        .node_version(node_id)
        .ok_or_else(|| ReviewError::InvalidWorkflow(format!("unknown node {node_id}")))?;
    let project_id = run.snapshot.project_id;
    let run_id = run.snapshot.id;
    let mut transaction = pool.begin().await?;
    assert_worker_lease(&mut transaction, project_id, run_id, worker_id).await?;
    if let Some(row) =
        find_accepted_attempt(&mut transaction, project_id, node_id, &fingerprint).await?
    {
        transaction.commit().await?;
        return accepted_start_from_row(&row);
    }
    let attempt_number = sqlx::query_scalar::<_, i32>(
        "SELECT COALESCE(max(attempt_number),0)+1
         FROM review_step_attempts
         WHERE project_id=$1 AND automation_run_id=$2 AND node_id=$3",
    )
    .bind(project_id.as_uuid())
    .bind(run_id.as_uuid())
    .bind(node_id)
    .fetch_one(&mut *transaction)
    .await?;
    let attempt_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO review_step_attempts
         (id,project_id,automation_run_id,node_id,node_version,attempt_number,
          input_fingerprint,status,worker_id)
         VALUES ($1,$2,$3,$4,$5,$6,$7,'running',$8)",
    )
    .bind(attempt_id)
    .bind(project_id.as_uuid())
    .bind(run_id.as_uuid())
    .bind(node_id)
    .bind(i32::try_from(node_version).map_err(|_| {
        PostgresReviewError::InvalidStoredValue("node version is too large".to_owned())
    })?)
    .bind(attempt_number)
    .bind(fingerprint.as_str())
    .bind(worker_id)
    .execute(&mut *transaction)
    .await?;
    transaction.commit().await?;
    Ok(ReviewAttemptStart::Started {
        attempt_id,
        attempt_number,
        input_fingerprint: fingerprint,
    })
}

pub async fn complete_review_attempt(
    pool: &PgPool,
    run: &LeasedReviewRun,
    completion: ReviewAttemptCompletion<'_>,
) -> Result<AcceptedReviewAttempt, PostgresReviewError> {
    let ReviewAttemptCompletion {
        attempt_id,
        payload,
        media_type,
        predecessors,
        model_run_id,
        worker_id,
    } = completion;
    if media_type.trim().is_empty() || media_type.len() > 200 {
        return Err(PostgresReviewError::InvalidStoredValue(
            "artifact media type is invalid".to_owned(),
        ));
    }
    let content_hash = ReviewHash::parse(hash_json(&payload)?)?;
    let project_id = run.snapshot.project_id;
    let mut transaction = pool.begin().await?;
    assert_worker_lease(&mut transaction, project_id, run.snapshot.id, worker_id).await?;
    let attempt = sqlx::query(
        "SELECT node_id,input_fingerprint,status
         FROM review_step_attempts
         WHERE id=$1 AND project_id=$2 AND automation_run_id=$3
         FOR UPDATE",
    )
    .bind(attempt_id)
    .bind(project_id.as_uuid())
    .bind(run.snapshot.id.as_uuid())
    .fetch_optional(&mut *transaction)
    .await?
    .ok_or(PostgresReviewError::RunNotFound)?;
    if attempt.get::<String, _>("status") != "running" {
        return Err(PostgresReviewError::InvalidState(
            "review attempt is not running".to_owned(),
        ));
    }
    let node_id: String = attempt.get("node_id");
    let input_fingerprint = ReviewHash::parse(attempt.get::<String, _>("input_fingerprint"))?;
    sqlx::query("SELECT pg_advisory_xact_lock(hashtext($1))")
        .bind(format!(
            "{}:{node_id}:{input_fingerprint}",
            project_id.as_uuid()
        ))
        .execute(&mut *transaction)
        .await?;
    if let Some(row) =
        find_accepted_attempt(&mut transaction, project_id, &node_id, &input_fingerprint).await?
    {
        sqlx::query(
            "UPDATE review_step_attempts
             SET status='failed',finished_at=now(),error_code='superseded_attempt',
                 error_message='an exact accepted attempt completed first'
             WHERE id=$1 AND status='running'",
        )
        .bind(attempt_id)
        .execute(&mut *transaction)
        .await?;
        let accepted = accepted_attempt_from_row(&row, true)?;
        transaction.commit().await?;
        return Ok(accepted);
    }
    let artifact_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO review_artifacts (id,project_id,content_hash,media_type,payload)
         VALUES ($1,$2,$3,$4,$5)
         ON CONFLICT (project_id,content_hash) DO NOTHING",
    )
    .bind(artifact_id)
    .bind(project_id.as_uuid())
    .bind(content_hash.as_str())
    .bind(media_type)
    .bind(&payload)
    .execute(&mut *transaction)
    .await?;
    let row = sqlx::query(
        "SELECT id,payload FROM review_artifacts
         WHERE project_id=$1 AND content_hash=$2",
    )
    .bind(project_id.as_uuid())
    .bind(content_hash.as_str())
    .fetch_one(&mut *transaction)
    .await?;
    let persisted_artifact_id: Uuid = row.get("id");
    if row.get::<Value, _>("payload") != payload {
        return Err(PostgresReviewError::FinalizationConflict);
    }
    for predecessor in predecessors {
        sqlx::query(
            "INSERT INTO review_artifact_lineage
             (project_id,artifact_id,predecessor_artifact_id)
             VALUES ($1,$2,$3) ON CONFLICT DO NOTHING",
        )
        .bind(project_id.as_uuid())
        .bind(persisted_artifact_id)
        .bind(predecessor.artifact_id)
        .execute(&mut *transaction)
        .await?;
    }
    sqlx::query(
        "UPDATE review_step_attempts
         SET status='completed',artifact_id=$2,model_run_id=$3,
             finished_at=now(),accepted_at=now()
         WHERE id=$1 AND status='running'",
    )
    .bind(attempt_id)
    .bind(persisted_artifact_id)
    .bind(model_run_id)
    .execute(&mut *transaction)
    .await?;
    transaction.commit().await?;
    Ok(AcceptedReviewAttempt {
        attempt_id,
        artifact_id: persisted_artifact_id,
        artifact_hash: content_hash,
        payload,
        reused: false,
    })
}

pub async fn fail_review_attempt(
    pool: &PgPool,
    run: &LeasedReviewRun,
    attempt_id: Uuid,
    code: &str,
    message: &str,
    worker_id: &str,
) -> Result<(), PostgresReviewError> {
    let mut transaction = pool.begin().await?;
    assert_worker_lease(
        &mut transaction,
        run.snapshot.project_id,
        run.snapshot.id,
        worker_id,
    )
    .await?;
    let changed = sqlx::query(
        "UPDATE review_step_attempts
         SET status='failed',finished_at=now(),error_code=$2,error_message=$3
         WHERE id=$1 AND project_id=$4 AND automation_run_id=$5
           AND worker_id=$6 AND status='running'",
    )
    .bind(attempt_id)
    .bind(code)
    .bind(message)
    .bind(run.snapshot.project_id.as_uuid())
    .bind(run.snapshot.id.as_uuid())
    .bind(worker_id)
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

pub async fn bind_review_step_acceptance(
    pool: &PgPool,
    project_id: ProjectId,
    automation_step_id: deepref_application::AutomationStepRunId,
    accepted_attempt_id: Uuid,
    worker_id: &str,
) -> Result<(), PostgresReviewError> {
    let changed = sqlx::query(
        "UPDATE automation_step_runs AS s
         SET accepted_attempt_id=a.id,input_fingerprint=a.input_fingerprint
         FROM review_step_attempts AS a, automation_runs AS r, jobs AS j
         WHERE s.project_id=$1 AND s.id=$2 AND s.status='running'
           AND s.claimed_by=$3
           AND a.id=$4 AND a.project_id=s.project_id
           AND a.automation_run_id=s.automation_run_id
           AND a.status='completed' AND a.accepted_at IS NOT NULL
           AND r.project_id=s.project_id AND r.id=s.automation_run_id
           AND j.project_id=r.project_id AND j.id=r.job_id
           AND j.state='running' AND j.lease_owner=$3 AND j.leased_until > now()",
    )
    .bind(project_id.as_uuid())
    .bind(automation_step_id.as_uuid())
    .bind(worker_id)
    .bind(accepted_attempt_id)
    .execute(pool)
    .await?
    .rows_affected();
    if changed != 1 {
        return Err(PostgresReviewError::WorkerOwnership);
    }
    Ok(())
}

pub async fn finalize_review_proposal(
    pool: &PgPool,
    run: &LeasedReviewRun,
    executed: ExecutedReviewTask,
    worker_id: &str,
) -> Result<ReviewFinalization, PostgresReviewError> {
    let project_id = run.snapshot.project_id;
    if executed.proposal.project_id != project_id {
        return Err(PostgresReviewError::FinalizationConflict);
    }
    let mut lease_check = pool.begin().await?;
    assert_worker_lease(&mut lease_check, project_id, run.snapshot.id, worker_id).await?;
    lease_check.commit().await?;
    if !subject_is_current(pool, project_id, &run.snapshot.subject).await? {
        block_review_run(
            pool,
            project_id,
            run.snapshot.id,
            ReviewBlockCode::SubjectChanged,
            "the review subject or published protocol changed before finalization",
        )
        .await?;
        return Ok(ReviewFinalization::Blocked);
    }
    let candidate_hash = ReviewHash::parse(hash_json(&serde_json::to_value(&executed.proposal)?)?)?;
    if let Some(proposal_id) = sqlx::query_scalar::<_, Uuid>(
        "SELECT proposal_id FROM review_proposal_finalizations
         WHERE project_id=$1 AND automation_run_id=$2 AND candidate_hash=$3",
    )
    .bind(project_id.as_uuid())
    .bind(run.snapshot.id.as_uuid())
    .bind(candidate_hash.as_str())
    .fetch_optional(pool)
    .await?
    {
        return Ok(ReviewFinalization::Completed { proposal_id });
    }
    let proposal = PostgresAiStore::new(pool)
        .create(AiProposal {
            id: Uuid::new_v4(),
            draft: executed.proposal,
            model_run_id: executed.model_run_id,
            status: ProposalStatus::Pending,
            resolved_at: None,
            resolved_by_actor_id: None,
        })
        .await?;
    let mut transaction = pool.begin().await?;
    sqlx::query(
        "INSERT INTO review_proposal_finalizations
         (project_id,automation_run_id,candidate_hash,proposal_id,model_run_id)
         VALUES ($1,$2,$3,$4,$5)
         ON CONFLICT (project_id,automation_run_id,candidate_hash) DO NOTHING",
    )
    .bind(project_id.as_uuid())
    .bind(run.snapshot.id.as_uuid())
    .bind(candidate_hash.as_str())
    .bind(proposal.id)
    .bind(executed.model_run_id)
    .execute(&mut *transaction)
    .await?;
    let persisted = sqlx::query(
        "SELECT proposal_id,model_run_id FROM review_proposal_finalizations
         WHERE project_id=$1 AND automation_run_id=$2 AND candidate_hash=$3
         FOR UPDATE",
    )
    .bind(project_id.as_uuid())
    .bind(run.snapshot.id.as_uuid())
    .bind(candidate_hash.as_str())
    .fetch_one(&mut *transaction)
    .await?;
    let proposal_id: Uuid = persisted.get("proposal_id");
    if proposal_id != proposal.id
        || persisted.get::<Uuid, _>("model_run_id") != executed.model_run_id
    {
        return Err(PostgresReviewError::FinalizationConflict);
    }
    let changed = sqlx::query(
        "UPDATE review_run_manifests
         SET state='completed',candidate_hash=$3,proposal_id=$4,finished_at=now(),
             state_code=NULL,state_message=NULL
         WHERE project_id=$1 AND automation_run_id=$2 AND state='running'",
    )
    .bind(project_id.as_uuid())
    .bind(run.snapshot.id.as_uuid())
    .bind(candidate_hash.as_str())
    .bind(proposal_id)
    .execute(&mut *transaction)
    .await?
    .rows_affected();
    if changed == 0 {
        let state = sqlx::query(
            "SELECT state,proposal_id FROM review_run_manifests
             WHERE project_id=$1 AND automation_run_id=$2 FOR UPDATE",
        )
        .bind(project_id.as_uuid())
        .bind(run.snapshot.id.as_uuid())
        .fetch_one(&mut *transaction)
        .await?;
        if state.get::<String, _>("state") != "completed"
            || state.get::<Option<Uuid>, _>("proposal_id") != Some(proposal_id)
        {
            return Err(PostgresReviewError::FinalizationConflict);
        }
    }
    transaction.commit().await?;
    Ok(ReviewFinalization::Completed { proposal_id })
}

async fn ensure_review_automation_definition(
    transaction: &mut Transaction<'_, Postgres>,
    project_id: ProjectId,
    recipe: BuiltInAutomationRecipe,
    actor: &deepref_domain::Actor,
) -> Result<Uuid, PostgresReviewError> {
    let row = sqlx::query(
        "SELECT id FROM configure_automation_definition($1,$2,'manual',$3,$4,'active',$5,$6)",
    )
    .bind(project_id.as_uuid())
    .bind(format!("Compiled review · {}", recipe.id()))
    .bind(recipe.id())
    .bind(recipe.version())
    .bind(actor.kind().as_str())
    .bind(actor.id())
    .fetch_one(&mut **transaction)
    .await?;
    Ok(row.get("id"))
}

const fn recipe_for(key: ReviewDefinitionKey) -> BuiltInAutomationRecipe {
    match key {
        ReviewDefinitionKey::Screening => BuiltInAutomationRecipe::ReviewScreeningV1,
        ReviewDefinitionKey::DuplicateDetection => {
            BuiltInAutomationRecipe::ReviewDuplicateDetectionV1
        }
        ReviewDefinitionKey::StudyClassification => {
            BuiltInAutomationRecipe::ReviewStudyClassificationV1
        }
        ReviewDefinitionKey::StudyGrouping => BuiltInAutomationRecipe::ReviewStudyGroupingV1,
        ReviewDefinitionKey::AppraisalPrefill => BuiltInAutomationRecipe::ReviewAppraisalPrefillV1,
        ReviewDefinitionKey::DataExtraction => BuiltInAutomationRecipe::ReviewDataExtractionV1,
    }
}

fn protocol_version_id(subject: &ReviewSubject) -> Option<deepref_domain::ProtocolVersionId> {
    match subject {
        ReviewSubject::Screening {
            protocol_version_id,
            ..
        } => Some(*protocol_version_id),
        _ => None,
    }
}

fn model_identity(route: ResolvedModel) -> Result<ReviewModelIdentity, PostgresReviewError> {
    Ok(ReviewModelIdentity {
        profile: route.profile,
        provider: route.provider,
        model: route.model,
        model_version: route.model_version,
        parameters_hash: ReviewHash::parse(hash_json(&serde_json::to_value(route.parameters)?)?)?,
    })
}

fn runtime_identity() -> ReviewRuntimeIdentity {
    let build_sha = option_env!("DEEPREF_BUILD_SHA").map_or_else(
        || {
            ReviewHash::digest_bytes(concat!(
                include_str!("review_runs.rs"),
                include_str!("review_preparation.rs"),
                include_str!("../../review/src/definition.rs"),
                include_str!("../../review/src/execution.rs"),
                include_str!("../../review/src/manifest.rs"),
                include_str!("../../review/src/task.rs"),
                include_str!("../../../services/worker/src/processor.rs"),
            ))
        },
        |build| ReviewHash::digest_bytes(build.as_bytes()),
    );
    ReviewRuntimeIdentity {
        build_sha,
        rust_version: option_env!("RUSTC_VERSION")
            .unwrap_or("workspace-toolchain")
            .to_owned(),
        target: format!("{}-{}", std::env::consts::ARCH, std::env::consts::OS),
    }
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
    let state = match state_name.as_str() {
        "queued" => ReviewRunState::Queued,
        "running" => ReviewRunState::Running,
        "blocked" => ReviewRunState::Blocked {
            code: ReviewBlockCode::parse(state_code.as_deref().unwrap_or_default()).ok_or_else(
                || PostgresReviewError::InvalidStoredValue("unknown review block code".to_owned()),
            )?,
            message: state_message.ok_or_else(|| {
                PostgresReviewError::InvalidStoredValue("blocked review has no message".to_owned())
            })?,
        },
        "failed" => ReviewRunState::Failed {
            code: state_code.ok_or_else(|| {
                PostgresReviewError::InvalidStoredValue("failed review has no code".to_owned())
            })?,
            message: state_message.ok_or_else(|| {
                PostgresReviewError::InvalidStoredValue("failed review has no message".to_owned())
            })?,
        },
        "completed" => ReviewRunState::Completed {
            proposal_id: row.get::<Option<Uuid>, _>("proposal_id").ok_or_else(|| {
                PostgresReviewError::InvalidStoredValue(
                    "completed review has no proposal".to_owned(),
                )
            })?,
        },
        _ => {
            return Err(PostgresReviewError::InvalidStoredValue(format!(
                "unknown review state {state_name}"
            )));
        }
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

async fn find_accepted_attempt(
    transaction: &mut Transaction<'_, Postgres>,
    project_id: ProjectId,
    node_id: &str,
    fingerprint: &ReviewHash,
) -> Result<Option<PgRow>, sqlx::Error> {
    sqlx::query(
        "SELECT a.id AS attempt_id,a.artifact_id,r.content_hash,r.payload
         FROM review_step_attempts AS a
         JOIN review_artifacts AS r
           ON r.project_id=a.project_id AND r.id=a.artifact_id
         WHERE a.project_id=$1 AND a.node_id=$2 AND a.input_fingerprint=$3
           AND a.status='completed' AND a.accepted_at IS NOT NULL
         ORDER BY a.accepted_at,a.id LIMIT 1",
    )
    .bind(project_id.as_uuid())
    .bind(node_id)
    .bind(fingerprint.as_str())
    .fetch_optional(&mut **transaction)
    .await
}

async fn assert_worker_lease(
    transaction: &mut Transaction<'_, Postgres>,
    project_id: ProjectId,
    run_id: ReviewRunId,
    worker_id: &str,
) -> Result<(), PostgresReviewError> {
    let owns_lease = sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS (
           SELECT 1
           FROM automation_runs AS r
           JOIN jobs AS j ON j.project_id=r.project_id AND j.id=r.job_id
           WHERE r.project_id=$1 AND r.id=$2
             AND j.kind='automation_run' AND j.state='running'
             AND j.lease_owner=$3 AND j.leased_until > now()
         )",
    )
    .bind(project_id.as_uuid())
    .bind(run_id.as_uuid())
    .bind(worker_id)
    .fetch_one(&mut **transaction)
    .await?;
    if !owns_lease {
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

fn accepted_attempt_from_row(
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

async fn subject_is_current(
    pool: &PgPool,
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
            .fetch_optional(pool)
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
            .fetch_optional(pool)
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
            .fetch_optional(pool)
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
        .fetch_one(pool)
        .await?),
        ReviewSubject::AppraisalPrefill { report_id, .. } => Ok(sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS(SELECT 1 FROM project_reports WHERE project_id=$1 AND report_id=$2)",
        )
        .bind(project_id.as_uuid())
        .bind(report_id.as_uuid())
        .fetch_one(pool)
        .await?),
        ReviewSubject::DataExtraction { study_id, .. } => Ok(sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS(SELECT 1 FROM studies WHERE project_id=$1 AND id=$2)",
        )
        .bind(project_id.as_uuid())
        .bind(study_id.as_uuid())
        .fetch_one(pool)
        .await?),
    }
}
