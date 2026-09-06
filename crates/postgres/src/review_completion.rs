use chrono::Utc;
use deepref_ai::{AiProposal, hash_json};
use deepref_domain::ProjectId;
use deepref_review::{
    ReviewBlockCode, ReviewRunState,
    worker::{AcceptedArtifactInput, ReviewHash},
};
use serde_json::Value;
use sqlx::{PgPool, Postgres, Row, Transaction};
use uuid::Uuid;

use super::review_runs::{
    AcceptedReviewAttempt, LeasedReviewRun, PostgresReviewError, ReviewAttemptCompletion,
    ReviewFinalization, accepted_attempt_from_row, assert_worker_lease, find_accepted_attempt,
    subject_is_current_in_transaction,
};

pub enum ReviewOutcome<'a> {
    Candidate {
        proposal: AiProposal,
    },
    Blocked {
        code: ReviewBlockCode,
        message: &'a str,
    },
}

pub struct ReviewOutcomeCompletion<'a> {
    pub final_attempt_id: Uuid,
    pub predecessors: &'a [AcceptedArtifactInput],
    pub outcome: ReviewOutcome<'a>,
    pub automation_step_id: deepref_application::AutomationStepRunId,
    pub worker_id: &'a str,
}

pub async fn complete_review_attempt(
    pool: &PgPool,
    run: &LeasedReviewRun,
    completion: ReviewAttemptCompletion<'_>,
) -> Result<AcceptedReviewAttempt, PostgresReviewError> {
    let mut transaction = pool.begin().await?;
    let accepted =
        complete_review_attempt_in_transaction(&mut transaction, run, completion).await?;
    transaction.commit().await?;
    Ok(accepted)
}

async fn complete_review_attempt_in_transaction(
    transaction: &mut Transaction<'_, Postgres>,
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
    assert_worker_lease(transaction, project_id, run.snapshot.id, worker_id).await?;
    let attempt = sqlx::query(
        "SELECT node_id,input_fingerprint,status
         FROM review_step_attempts
         WHERE id=$1 AND project_id=$2 AND automation_run_id=$3
         FOR UPDATE",
    )
    .bind(attempt_id)
    .bind(project_id.as_uuid())
    .bind(run.snapshot.id.as_uuid())
    .fetch_optional(&mut **transaction)
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
        .execute(&mut **transaction)
        .await?;
    if let Some(row) =
        find_accepted_attempt(transaction, project_id, &node_id, &input_fingerprint).await?
    {
        sqlx::query(
            "UPDATE review_step_attempts
             SET status='failed',finished_at=now(),error_code='superseded_attempt',
                 error_message='an exact accepted attempt completed first'
             WHERE id=$1 AND status='running'",
        )
        .bind(attempt_id)
        .execute(&mut **transaction)
        .await?;
        let accepted = accepted_attempt_from_row(&row, true)?;
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
    .execute(&mut **transaction)
    .await?;
    let row = sqlx::query(
        "SELECT id,payload FROM review_artifacts
         WHERE project_id=$1 AND content_hash=$2",
    )
    .bind(project_id.as_uuid())
    .bind(content_hash.as_str())
    .fetch_one(&mut **transaction)
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
        .execute(&mut **transaction)
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
    .execute(&mut **transaction)
    .await?;
    Ok(AcceptedReviewAttempt {
        attempt_id,
        artifact_id: persisted_artifact_id,
        artifact_hash: content_hash,
        payload,
        reused: false,
    })
}

pub async fn bind_review_step_acceptance(
    pool: &PgPool,
    project_id: ProjectId,
    automation_step_id: deepref_application::AutomationStepRunId,
    accepted_attempt_id: Uuid,
    worker_id: &str,
) -> Result<(), PostgresReviewError> {
    let mut transaction = pool.begin().await?;
    bind_review_step_acceptance_in_transaction(
        &mut transaction,
        project_id,
        automation_step_id,
        accepted_attempt_id,
        worker_id,
    )
    .await?;
    transaction.commit().await?;
    Ok(())
}

async fn bind_review_step_acceptance_in_transaction(
    transaction: &mut Transaction<'_, Postgres>,
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
    .execute(&mut **transaction)
    .await?
    .rows_affected();
    if changed != 1 {
        let existing = sqlx::query(
            "SELECT status,accepted_attempt_id
             FROM automation_step_runs
             WHERE project_id=$1 AND id=$2
             FOR UPDATE",
        )
        .bind(project_id.as_uuid())
        .bind(automation_step_id.as_uuid())
        .fetch_optional(&mut **transaction)
        .await?
        .ok_or(PostgresReviewError::WorkerOwnership)?;
        if existing.get::<String, _>("status") == "completed"
            && existing.get::<Option<Uuid>, _>("accepted_attempt_id") == Some(accepted_attempt_id)
        {
            return Ok(());
        }
        return Err(PostgresReviewError::WorkerOwnership);
    }
    Ok(())
}

pub async fn complete_review_step(
    pool: &PgPool,
    project_id: ProjectId,
    automation_step_id: deepref_application::AutomationStepRunId,
    accepted_attempt_id: Uuid,
    worker_id: &str,
    output: Option<Value>,
) -> Result<(), PostgresReviewError> {
    let mut transaction = pool.begin().await?;
    bind_review_step_acceptance_in_transaction(
        &mut transaction,
        project_id,
        automation_step_id,
        accepted_attempt_id,
        worker_id,
    )
    .await?;
    crate::automations::complete_automation_step_with_output_in_transaction(
        &mut transaction,
        project_id,
        automation_step_id,
        worker_id,
        output,
    )
    .await
    .map_err(map_automation_error)?;
    transaction.commit().await?;
    Ok(())
}

fn map_automation_error(error: crate::automations::AutomationError) -> PostgresReviewError {
    match error {
        crate::automations::AutomationError::Database(error) => {
            PostgresReviewError::Database(error)
        }
        crate::automations::AutomationError::Serialization(error) => {
            PostgresReviewError::Serialization(error)
        }
        crate::automations::AutomationError::WorkerOwnership => {
            PostgresReviewError::WorkerOwnership
        }
        other => PostgresReviewError::InvalidState(other.to_string()),
    }
}

pub async fn complete_review_outcome(
    pool: &PgPool,
    run: &LeasedReviewRun,
    outcome: ReviewOutcomeCompletion<'_>,
) -> Result<ReviewFinalization, PostgresReviewError> {
    let ReviewOutcomeCompletion {
        final_attempt_id,
        predecessors,
        outcome,
        automation_step_id,
        worker_id,
    } = outcome;
    let (proposal, blocked, model_run_id) = match outcome {
        ReviewOutcome::Candidate { proposal } => {
            let model_run_id = Some(proposal.model_run_id);
            (Some(proposal), None, model_run_id)
        }
        ReviewOutcome::Blocked { code, message } => (None, Some((code, message)), None),
    };
    let mut transaction = pool.begin().await?;
    assert_worker_lease(
        &mut transaction,
        run.snapshot.project_id,
        run.snapshot.id,
        worker_id,
    )
    .await?;

    let subject_current = if blocked.is_some() {
        true
    } else {
        subject_is_current_in_transaction(
            &mut transaction,
            run.snapshot.project_id,
            &run.snapshot.subject,
        )
        .await?
    };
    let (state, proposal_id, payload, candidate_hash) = if !subject_current {
        (
            ReviewRunState::Blocked {
                code: ReviewBlockCode::SubjectChanged,
                message: "the review subject or published protocol changed before finalization"
                    .to_owned(),
            },
            None,
            serde_json::json!({
                "state": "blocked",
                "code": ReviewBlockCode::SubjectChanged.as_str(),
                "message": "the review subject or published protocol changed before finalization"
            }),
            None,
        )
    } else if let Some((code, message)) = blocked {
        if message.trim().is_empty() || message.len() > 4096 {
            return Err(PostgresReviewError::InvalidState(
                "terminal review error metadata is invalid".to_owned(),
            ));
        }
        let state = ReviewRunState::Blocked {
            code,
            message: message.to_owned(),
        };
        (
            state,
            None,
            serde_json::json!({
                "state": "blocked",
                "code": code.as_str(),
                "message": message
            }),
            None,
        )
    } else {
        let proposal = proposal.ok_or_else(|| {
            PostgresReviewError::InvalidState("completed review outcome has no proposal".to_owned())
        })?;
        let candidate_hash =
            ReviewHash::parse(hash_json(&serde_json::to_value(&proposal.draft)?)?)?;
        let proposal_id = if let Some(row) = sqlx::query(
            "SELECT proposal_id,model_run_id
             FROM review_proposal_finalizations
             WHERE project_id=$1 AND automation_run_id=$2 AND candidate_hash=$3
             FOR UPDATE",
        )
        .bind(run.snapshot.project_id.as_uuid())
        .bind(run.snapshot.id.as_uuid())
        .bind(candidate_hash.as_str())
        .fetch_optional(&mut *transaction)
        .await?
        {
            if row.get::<Uuid, _>("model_run_id") != proposal.model_run_id {
                return Err(PostgresReviewError::FinalizationConflict);
            }
            row.get("proposal_id")
        } else {
            let persisted =
                crate::ai::create_proposal_in_transaction(&mut transaction, proposal).await?;
            sqlx::query(
                "INSERT INTO review_proposal_finalizations
                 (project_id,automation_run_id,candidate_hash,proposal_id,model_run_id)
                 VALUES ($1,$2,$3,$4,$5)
                 ON CONFLICT (project_id,automation_run_id,candidate_hash) DO NOTHING",
            )
            .bind(run.snapshot.project_id.as_uuid())
            .bind(run.snapshot.id.as_uuid())
            .bind(candidate_hash.as_str())
            .bind(persisted.id)
            .bind(persisted.model_run_id)
            .execute(&mut *transaction)
            .await?;
            let row = sqlx::query(
                "SELECT proposal_id,model_run_id
                 FROM review_proposal_finalizations
                 WHERE project_id=$1 AND automation_run_id=$2 AND candidate_hash=$3
                 FOR UPDATE",
            )
            .bind(run.snapshot.project_id.as_uuid())
            .bind(run.snapshot.id.as_uuid())
            .bind(candidate_hash.as_str())
            .fetch_one(&mut *transaction)
            .await?;
            if row.get::<Uuid, _>("model_run_id") != persisted.model_run_id {
                return Err(PostgresReviewError::FinalizationConflict);
            }
            row.get("proposal_id")
        };
        (
            ReviewRunState::Completed { proposal_id },
            Some(proposal_id),
            serde_json::json!({"state":"completed","proposal_id":proposal_id}),
            Some(candidate_hash),
        )
    };

    let (state_name, state_code, state_message) = match &state {
        ReviewRunState::Completed { .. } => ("completed", None, None),
        ReviewRunState::Blocked { code, message } => {
            ("blocked", Some(code.as_str()), Some(message.as_str()))
        }
        ReviewRunState::Queued | ReviewRunState::Running | ReviewRunState::Failed { .. } => {
            unreachable!("terminal completion only produces terminal states")
        }
    };
    let changed = sqlx::query(
        "UPDATE review_run_manifests
         SET state=$3,finished_at=now(),state_code=$4,state_message=$5,proposal_id=$6,
             candidate_hash=COALESCE($7,candidate_hash)
         WHERE project_id=$1 AND automation_run_id=$2 AND state='running'",
    )
    .bind(run.snapshot.project_id.as_uuid())
    .bind(run.snapshot.id.as_uuid())
    .bind(state_name)
    .bind(state_code)
    .bind(state_message)
    .bind(proposal_id)
    .bind(candidate_hash.as_ref().map(ReviewHash::as_str))
    .execute(&mut *transaction)
    .await?
    .rows_affected();
    if changed != 1 {
        return Err(PostgresReviewError::InvalidState(
            "review run is not running".to_owned(),
        ));
    }

    let accepted = complete_review_attempt_in_transaction(
        &mut transaction,
        run,
        ReviewAttemptCompletion {
            attempt_id: final_attempt_id,
            payload,
            media_type: "application/vnd.deepref.review-finalization+json",
            predecessors,
            model_run_id,
            worker_id,
        },
    )
    .await?;
    bind_review_step_acceptance_in_transaction(
        &mut transaction,
        run.snapshot.project_id,
        automation_step_id,
        accepted.attempt_id,
        worker_id,
    )
    .await?;
    let mut snapshot = run.snapshot.clone();
    snapshot.state = state;
    snapshot.finished_at = Some(Utc::now());
    crate::automations::complete_automation_step_with_output_in_transaction(
        &mut transaction,
        run.snapshot.project_id,
        automation_step_id,
        worker_id,
        Some(serde_json::to_value(snapshot)?),
    )
    .await
    .map_err(map_automation_error)?;
    transaction.commit().await?;
    Ok(match proposal_id {
        Some(proposal_id) => ReviewFinalization::Completed { proposal_id },
        None => ReviewFinalization::Blocked,
    })
}
