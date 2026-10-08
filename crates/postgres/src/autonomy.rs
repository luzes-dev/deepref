//! Per-project AI autonomy settings. This is the single resolver used by the
//! worker's AI jobs, the workflow `AutonomyGate` and the scheduling checks.

use deepref_application::workflows::{
    AutonomyError, AutonomyFuture, AutonomyGate, AutonomyLevel, AutonomyTask,
};
use deepref_domain::{Actor, ProjectId};
use sqlx::{PgPool, Postgres, Row, Transaction};
use thiserror::Error;
use uuid::Uuid;

#[derive(Debug, Error)]
pub enum AutonomyStoreError {
    #[error("project not found")]
    ProjectNotFound,
    #[error("this level is not available for this task")]
    LevelNotAllowed,
    #[error("database operation failed")]
    Database(#[from] sqlx::Error),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AutonomySetting {
    pub task: AutonomyTask,
    pub level: AutonomyLevel,
    pub is_default: bool,
}

/// All configurable tasks with their effective level (stored or default).
pub async fn get_autonomy_settings(
    pool: &PgPool,
    project_id: Uuid,
) -> Result<Vec<AutonomySetting>, AutonomyStoreError> {
    let exists: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM projects WHERE id=$1)")
        .bind(project_id)
        .fetch_one(pool)
        .await?;
    if !exists {
        return Err(AutonomyStoreError::ProjectNotFound);
    }
    let rows = sqlx::query("SELECT task, level FROM project_ai_autonomy WHERE project_id=$1")
        .bind(project_id)
        .fetch_all(pool)
        .await?;
    let stored: Vec<(AutonomyTask, AutonomyLevel)> = rows
        .iter()
        .filter_map(|row| {
            Some((
                AutonomyTask::parse(&row.get::<String, _>("task"))?,
                AutonomyLevel::parse(&row.get::<String, _>("level"))?,
            ))
        })
        .collect();
    Ok(AutonomyTask::ALL
        .into_iter()
        .map(|task| {
            let found = stored.iter().find(|(stored_task, _)| *stored_task == task);
            AutonomySetting {
                task,
                level: found.map_or(task.default_level(), |(_, level)| *level),
                is_default: found.is_none(),
            }
        })
        .collect())
}

pub async fn set_autonomy_level(
    pool: &PgPool,
    project_id: Uuid,
    task: AutonomyTask,
    level: AutonomyLevel,
    actor: &Actor,
) -> Result<(), AutonomyStoreError> {
    if !task.allows(level) {
        return Err(AutonomyStoreError::LevelNotAllowed);
    }
    let mut tx = pool.begin().await?;
    let exists: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM projects WHERE id=$1)")
        .bind(project_id)
        .fetch_one(&mut *tx)
        .await?;
    if !exists {
        return Err(AutonomyStoreError::ProjectNotFound);
    }
    sqlx::query(
        "INSERT INTO project_ai_autonomy (project_id,task,level,updated_by_kind,updated_by_id)
         VALUES ($1,$2,$3,$4,$5)
         ON CONFLICT (project_id,task) DO UPDATE
         SET level=EXCLUDED.level,updated_by_kind=EXCLUDED.updated_by_kind,
             updated_by_id=EXCLUDED.updated_by_id,updated_at=now()",
    )
    .bind(project_id)
    .bind(task.as_str())
    .bind(level.as_str())
    .bind(actor.kind().as_str())
    .bind(actor.id())
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        "INSERT INTO review_events
         (id,project_id,event_type,aggregate_type,aggregate_id,payload,actor_kind,actor_id)
         VALUES ($1,$2,'ai_autonomy_changed','project',$2,$3,$4,$5)",
    )
    .bind(Uuid::new_v4())
    .bind(project_id)
    .bind(serde_json::json!({"task": task.as_str(), "level": level.as_str()}))
    .bind(actor.kind().as_str())
    .bind(actor.id())
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(())
}

/// The effective level for one task. Unknown projects resolve to the default.
pub async fn resolve_autonomy_level(
    pool: &PgPool,
    project_id: Uuid,
    task: AutonomyTask,
) -> Result<AutonomyLevel, sqlx::Error> {
    let stored: Option<String> =
        sqlx::query_scalar("SELECT level FROM project_ai_autonomy WHERE project_id=$1 AND task=$2")
            .bind(project_id)
            .bind(task.as_str())
            .fetch_optional(pool)
            .await?;
    Ok(stored
        .as_deref()
        .and_then(AutonomyLevel::parse)
        .filter(|level| task.allows(*level))
        .unwrap_or_else(|| task.default_level()))
}

pub(crate) async fn resolve_autonomy_level_in_transaction(
    tx: &mut Transaction<'_, Postgres>,
    project_id: Uuid,
    task: AutonomyTask,
) -> Result<AutonomyLevel, sqlx::Error> {
    let stored: Option<String> =
        sqlx::query_scalar("SELECT level FROM project_ai_autonomy WHERE project_id=$1 AND task=$2")
            .bind(project_id)
            .bind(task.as_str())
            .fetch_optional(&mut **tx)
            .await?;
    Ok(stored
        .as_deref()
        .and_then(AutonomyLevel::parse)
        .filter(|level| task.allows(*level))
        .unwrap_or_else(|| task.default_level()))
}

/// Workflow gate backed by the project settings.
#[derive(Clone)]
pub struct ProjectAutonomyGate {
    pool: PgPool,
}

impl ProjectAutonomyGate {
    pub fn new(pool: &PgPool) -> Self {
        Self { pool: pool.clone() }
    }
}

impl AutonomyGate for ProjectAutonomyGate {
    fn decide<'a>(
        &'a self,
        task: AutonomyTask,
        project: ProjectId,
    ) -> AutonomyFuture<'a, Result<AutonomyLevel, AutonomyError>> {
        Box::pin(async move {
            resolve_autonomy_level(&self.pool, project.as_uuid(), task)
                .await
                .map_err(|error| AutonomyError::Unavailable(error.to_string()))
        })
    }
}

/// Which autonomy task an AI proposal belongs to, if any.
pub(crate) fn task_for_proposal(
    operation: &str,
    payload: &serde_json::Value,
) -> Option<AutonomyTask> {
    match operation {
        "screening_suggestion" => Some(
            if payload.get("stage").and_then(serde_json::Value::as_str) == Some("full_text") {
                AutonomyTask::FullTextScreening
            } else {
                AutonomyTask::TitleAbstractScreening
            },
        ),
        "dedupe_suggestion" => Some(AutonomyTask::FuzzyDuplicates),
        "data_extraction" => Some(AutonomyTask::Extraction),
        "appraisal_prefill" => Some(AutonomyTask::Appraisal),
        "study_grouping_suggestion" => Some(AutonomyTask::StudyGrouping),
        _ => None,
    }
}
