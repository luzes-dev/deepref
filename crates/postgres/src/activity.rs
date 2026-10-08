//! The activity feed: what the AI, automations and the assistant did, with
//! enough state to undo it. Undo always goes through the normal application
//! services and is itself audited.

use chrono::{DateTime, Utc};
use deepref_application::{RecordResolutionAction, ResolveRecordCommand, UndoScreeningCommand};
use deepref_domain::{Actor, ScreeningStage};
use serde_json::{Value, json};
use sqlx::{PgPool, Postgres, Row, Transaction};
use thiserror::Error;
use uuid::Uuid;

#[derive(Debug, Error)]
pub enum ActivityError {
    #[error("activity entry not found")]
    NotFound,
    #[error("this action was already undone")]
    AlreadyUndone,
    #[error("this action cannot be undone")]
    NotUndoable,
    #[error("{0}")]
    CannotUndo(String),
    #[error("database operation failed")]
    Database(#[from] sqlx::Error),
}

#[derive(Debug, Clone)]
pub struct NewActivity {
    pub project_id: Uuid,
    /// `ai`, `automation` or `assistant`.
    pub actor_type: &'static str,
    pub actor_label: String,
    pub actor: Actor,
    pub task: String,
    pub action: String,
    pub summary: String,
    pub affected: Value,
    pub before_state: Value,
    pub after_state: Value,
    pub undo_kind: Option<&'static str>,
    pub batch_id: Option<Uuid>,
    pub ai_run_id: Option<Uuid>,
    pub proposal_id: Option<Uuid>,
    pub model: Option<String>,
    pub prompt_version: Option<String>,
    pub evidence: Value,
}

impl NewActivity {
    pub fn new(
        project_id: Uuid,
        actor_type: &'static str,
        actor_label: impl Into<String>,
        actor: Actor,
        task: impl Into<String>,
        action: impl Into<String>,
        summary: impl Into<String>,
    ) -> Self {
        Self {
            project_id,
            actor_type,
            actor_label: actor_label.into(),
            actor,
            task: task.into(),
            action: action.into(),
            summary: summary.into(),
            affected: json!([]),
            before_state: json!({}),
            after_state: json!({}),
            undo_kind: None,
            batch_id: None,
            ai_run_id: None,
            proposal_id: None,
            model: None,
            prompt_version: None,
            evidence: json!([]),
        }
    }
}

#[derive(Debug, Clone)]
pub struct ActivityRecord {
    pub id: Uuid,
    pub project_id: Uuid,
    pub actor_type: String,
    pub actor_label: String,
    pub task: String,
    pub action: String,
    pub summary: String,
    pub affected: Value,
    pub undoable: bool,
    pub batch_id: Option<Uuid>,
    pub ai_run_id: Option<Uuid>,
    pub proposal_id: Option<Uuid>,
    pub model: Option<String>,
    pub prompt_version: Option<String>,
    pub evidence: Value,
    pub created_at: DateTime<Utc>,
    pub undone_at: Option<DateTime<Utc>>,
    pub undone_by: Option<String>,
}

const ACTIVITY_COLUMNS: &str = "id,project_id,actor_type,actor_label,task,action,summary,affected,
    undo_kind,batch_id,ai_run_id,proposal_id,model,prompt_version,evidence,created_at,
    undone_at,undone_by_id";

fn activity_from_row(row: &sqlx::postgres::PgRow) -> ActivityRecord {
    let undone_at: Option<DateTime<Utc>> = row.get("undone_at");
    ActivityRecord {
        id: row.get("id"),
        project_id: row.get("project_id"),
        actor_type: row.get("actor_type"),
        actor_label: row.get("actor_label"),
        task: row.get("task"),
        action: row.get("action"),
        summary: row.get("summary"),
        affected: row.get("affected"),
        // An undone entry cannot be undone again.
        undoable: row.get::<Option<String>, _>("undo_kind").is_some() && undone_at.is_none(),
        batch_id: row.get("batch_id"),
        ai_run_id: row.get("ai_run_id"),
        proposal_id: row.get("proposal_id"),
        model: row.get("model"),
        prompt_version: row.get("prompt_version"),
        evidence: row.get("evidence"),
        created_at: row.get("created_at"),
        undone_at,
        undone_by: row.get("undone_by_id"),
    }
}

pub async fn record_activity_in_transaction(
    tx: &mut Transaction<'_, Postgres>,
    entry: &NewActivity,
) -> Result<Uuid, sqlx::Error> {
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO ai_activity
         (id,project_id,actor_type,actor_label,actor_kind,actor_id,task,action,summary,affected,
          before_state,after_state,undo_kind,batch_id,ai_run_id,proposal_id,model,prompt_version,
          evidence)
         VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16,$17,$18,$19)",
    )
    .bind(id)
    .bind(entry.project_id)
    .bind(entry.actor_type)
    .bind(&entry.actor_label)
    .bind(entry.actor.kind().as_str())
    .bind(entry.actor.id())
    .bind(&entry.task)
    .bind(&entry.action)
    .bind(&entry.summary)
    .bind(&entry.affected)
    .bind(&entry.before_state)
    .bind(&entry.after_state)
    .bind(entry.undo_kind)
    .bind(entry.batch_id)
    .bind(entry.ai_run_id)
    .bind(entry.proposal_id)
    .bind(&entry.model)
    .bind(&entry.prompt_version)
    .bind(&entry.evidence)
    .execute(&mut **tx)
    .await?;
    Ok(id)
}

pub async fn record_activity(pool: &PgPool, entry: &NewActivity) -> Result<Uuid, sqlx::Error> {
    let mut tx = pool.begin().await?;
    let id = record_activity_in_transaction(&mut tx, entry).await?;
    tx.commit().await?;
    Ok(id)
}

#[derive(Debug, Clone, Copy, Default)]
pub struct ActivityFilters<'a> {
    pub task: Option<&'a str>,
    pub actor_type: Option<&'a str>,
    pub batch_id: Option<Uuid>,
    pub undone: Option<bool>,
}

pub async fn list_activity(
    pool: &PgPool,
    project_id: Uuid,
    filters: ActivityFilters<'_>,
    cursor: Option<(DateTime<Utc>, Uuid)>,
    limit: i64,
) -> Result<Vec<ActivityRecord>, ActivityError> {
    let query = format!(
        "SELECT {ACTIVITY_COLUMNS} FROM ai_activity
         WHERE project_id=$1
           AND ($2::text IS NULL OR task=$2)
           AND ($3::text IS NULL OR actor_type=$3)
           AND ($4::uuid IS NULL OR batch_id=$4)
           AND ($5::boolean IS NULL OR (undone_at IS NOT NULL)=$5)
           AND ($6::timestamptz IS NULL OR (created_at,id)<($6,$7))
         ORDER BY created_at DESC,id DESC LIMIT $8"
    );
    let rows = sqlx::query(sqlx::AssertSqlSafe(query))
        .bind(project_id)
        .bind(filters.task)
        .bind(filters.actor_type)
        .bind(filters.batch_id)
        .bind(filters.undone)
        .bind(cursor.map(|value| value.0))
        .bind(cursor.map(|value| value.1))
        .bind(limit + 1)
        .fetch_all(pool)
        .await?;
    Ok(rows.iter().map(activity_from_row).collect())
}

#[derive(Debug, Clone, Copy, Default)]
pub struct ActivityOverview {
    pub to_verify: i64,
    pub open_conflicts: i64,
    pub undoable: i64,
}

pub async fn activity_overview(
    pool: &PgPool,
    project_id: Uuid,
) -> Result<ActivityOverview, sqlx::Error> {
    let to_verify: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM extraction_values
         WHERE project_id=$1 AND needs_verification AND superseded_at IS NULL",
    )
    .bind(project_id)
    .fetch_one(pool)
    .await?;
    let undoable: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM ai_activity
         WHERE project_id=$1 AND undo_kind IS NOT NULL AND undone_at IS NULL",
    )
    .bind(project_id)
    .fetch_one(pool)
    .await?;
    let open_conflicts = crate::ai_reviewer::count_open_conflicts(pool, project_id).await?;
    Ok(ActivityOverview {
        to_verify,
        open_conflicts,
        undoable,
    })
}

struct Claimed {
    id: Uuid,
    undo_kind: String,
    after_state: Value,
}

async fn claim_undo(
    pool: &PgPool,
    project_id: Uuid,
    activity_id: Uuid,
    actor: &Actor,
) -> Result<Claimed, ActivityError> {
    let mut tx = pool.begin().await?;
    let row = sqlx::query(
        "SELECT id,undo_kind,after_state,undone_at FROM ai_activity
         WHERE project_id=$1 AND id=$2 FOR UPDATE",
    )
    .bind(project_id)
    .bind(activity_id)
    .fetch_optional(&mut *tx)
    .await?
    .ok_or(ActivityError::NotFound)?;
    if row.get::<Option<DateTime<Utc>>, _>("undone_at").is_some() {
        return Err(ActivityError::AlreadyUndone);
    }
    let undo_kind: Option<String> = row.get("undo_kind");
    let undo_kind = undo_kind.ok_or(ActivityError::NotUndoable)?;
    sqlx::query(
        "UPDATE ai_activity SET undone_at=now(),undone_by_kind=$3,undone_by_id=$4
         WHERE project_id=$1 AND id=$2",
    )
    .bind(project_id)
    .bind(activity_id)
    .bind(actor.kind().as_str())
    .bind(actor.id())
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(Claimed {
        id: activity_id,
        undo_kind,
        after_state: row.get("after_state"),
    })
}

async fn release_claim(pool: &PgPool, project_id: Uuid, activity_id: Uuid) {
    let _ = sqlx::query(
        "UPDATE ai_activity SET undone_at=NULL,undone_by_kind=NULL,undone_by_id=NULL
         WHERE project_id=$1 AND id=$2",
    )
    .bind(project_id)
    .bind(activity_id)
    .execute(pool)
    .await;
}

fn state_uuid(state: &Value, key: &str) -> Option<Uuid> {
    state
        .get(key)
        .and_then(Value::as_str)
        .and_then(|value| Uuid::parse_str(value).ok())
}

async fn perform_undo(
    pool: &PgPool,
    project_id: Uuid,
    claimed: &Claimed,
    actor: &Actor,
) -> Result<(), ActivityError> {
    let state = &claimed.after_state;
    match claimed.undo_kind.as_str() {
        "extraction_values" => {
            let ids: Vec<Uuid> = state
                .get("value_ids")
                .and_then(Value::as_array)
                .map(|items| {
                    items
                        .iter()
                        .filter_map(|item| item.as_str().and_then(|id| Uuid::parse_str(id).ok()))
                        .collect()
                })
                .unwrap_or_default();
            let removed = sqlx::query(
                "UPDATE extraction_values
                 SET superseded_at=now(),superseded_by_actor_kind=$3,superseded_by_actor_id=$4
                 WHERE project_id=$1 AND id=ANY($2) AND superseded_at IS NULL
                   AND needs_verification",
            )
            .bind(project_id)
            .bind(&ids)
            .bind(actor.kind().as_str())
            .bind(actor.id())
            .execute(pool)
            .await?
            .rows_affected();
            if removed == 0 && !ids.is_empty() {
                return Err(ActivityError::CannotUndo(
                    "These values were already confirmed or changed by a person, so they were left as they are."
                        .to_owned(),
                ));
            }
        }
        "duplicate_link" => {
            let record_id = state_uuid(state, "record_id").ok_or(ActivityError::NotUndoable)?;
            crate::deduplication::resolve_record(
                pool,
                ResolveRecordCommand {
                    project_id: project_id.into(),
                    record_id: record_id.into(),
                    action: RecordResolutionAction::Revert,
                    report_id: None,
                    proposal_id: None,
                    reason: "Undo of an automatic action from the activity feed".to_owned(),
                    actor_kind: actor.kind().as_str().to_owned(),
                    actor_id: actor.id().to_owned(),
                },
            )
            .await
            .map_err(|error| match error {
                crate::DedupeError::RevertConflict => ActivityError::CannotUndo(
                    "The record was changed after this action, so it cannot be undone safely."
                        .to_owned(),
                ),
                crate::DedupeError::Database(error) => ActivityError::Database(error),
                other => ActivityError::CannotUndo(other.to_string()),
            })?;
        }
        "screening_event" => {
            let report_id = state_uuid(state, "report_id").ok_or(ActivityError::NotUndoable)?;
            let event_id = state_uuid(state, "event_id").ok_or(ActivityError::NotUndoable)?;
            let protocol =
                state_uuid(state, "protocol_version_id").ok_or(ActivityError::NotUndoable)?;
            let stage = match state.get("stage").and_then(Value::as_str) {
                Some("full_text") => ScreeningStage::FullText,
                _ => ScreeningStage::TitleAbstract,
            };
            let row = sqlx::query(
                "SELECT revision,last_event_id FROM screening_state
                 WHERE project_id=$1 AND report_id=$2",
            )
            .bind(project_id)
            .bind(report_id)
            .fetch_optional(pool)
            .await?;
            let Some(row) = row else {
                return Err(ActivityError::CannotUndo(
                    "The screening decision no longer exists.".to_owned(),
                ));
            };
            if row.get::<Option<Uuid>, _>("last_event_id") != Some(event_id) {
                return Err(ActivityError::CannotUndo(
                    "The screening decision was changed after this action, so it cannot be undone safely."
                        .to_owned(),
                ));
            }
            crate::screening::undo_screening(
                pool,
                UndoScreeningCommand {
                    project_id: project_id.into(),
                    report_id: report_id.into(),
                    stage,
                    protocol_version_id: protocol.into(),
                    expected_revision: row.get("revision"),
                    notes: Some("Undone from the activity feed".to_owned()),
                    actor: actor.clone(),
                },
            )
            .await
            .map_err(|error| match error {
                crate::ScreeningError::Database(error) => ActivityError::Database(error),
                other => ActivityError::CannotUndo(other.to_string()),
            })?;
        }
        "reviewer_decision" => {
            let decision_id = state_uuid(state, "decision_id").ok_or(ActivityError::NotUndoable)?;
            sqlx::query(
                "UPDATE ai_reviewer_decisions SET voided_at=now()
                 WHERE project_id=$1 AND id=$2 AND voided_at IS NULL",
            )
            .bind(project_id)
            .bind(decision_id)
            .execute(pool)
            .await?;
        }
        _ => return Err(ActivityError::NotUndoable),
    }
    sqlx::query(
        "INSERT INTO review_events
         (id,project_id,event_type,aggregate_type,aggregate_id,payload,actor_kind,actor_id)
         VALUES ($1,$2,'ai_activity_undone','ai_activity',$3,$4,$5,$6)",
    )
    .bind(Uuid::new_v4())
    .bind(project_id)
    .bind(claimed.id)
    .bind(json!({"undo_kind": claimed.undo_kind}))
    .bind(actor.kind().as_str())
    .bind(actor.id())
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn undo_activity(
    pool: &PgPool,
    project_id: Uuid,
    activity_id: Uuid,
    actor: &Actor,
) -> Result<ActivityRecord, ActivityError> {
    let claimed = claim_undo(pool, project_id, activity_id, actor).await?;
    if let Err(error) = perform_undo(pool, project_id, &claimed, actor).await {
        release_claim(pool, project_id, activity_id).await;
        return Err(error);
    }
    get_activity(pool, project_id, activity_id).await
}

pub async fn get_activity(
    pool: &PgPool,
    project_id: Uuid,
    activity_id: Uuid,
) -> Result<ActivityRecord, ActivityError> {
    let query = format!("SELECT {ACTIVITY_COLUMNS} FROM ai_activity WHERE project_id=$1 AND id=$2");
    let row = sqlx::query(sqlx::AssertSqlSafe(query))
        .bind(project_id)
        .bind(activity_id)
        .fetch_optional(pool)
        .await?
        .ok_or(ActivityError::NotFound)?;
    Ok(activity_from_row(&row))
}

#[derive(Debug, Clone, Default)]
pub struct BatchUndoResult {
    pub undone: u32,
    pub failed: Vec<(Uuid, String)>,
}

/// Undo every still-undoable entry of a batch, newest first. Entries that
/// cannot be undone safely are reported and left alone.
pub async fn undo_batch(
    pool: &PgPool,
    project_id: Uuid,
    batch_id: Uuid,
    actor: &Actor,
) -> Result<BatchUndoResult, ActivityError> {
    let ids: Vec<Uuid> = sqlx::query_scalar(
        "SELECT id FROM ai_activity
         WHERE project_id=$1 AND batch_id=$2 AND undo_kind IS NOT NULL AND undone_at IS NULL
         ORDER BY created_at DESC,id DESC",
    )
    .bind(project_id)
    .bind(batch_id)
    .fetch_all(pool)
    .await?;
    if ids.is_empty() {
        let exists: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM ai_activity WHERE project_id=$1 AND batch_id=$2)",
        )
        .bind(project_id)
        .bind(batch_id)
        .fetch_one(pool)
        .await?;
        return Err(if exists {
            ActivityError::AlreadyUndone
        } else {
            ActivityError::NotFound
        });
    }
    let mut result = BatchUndoResult::default();
    for id in ids {
        match undo_activity(pool, project_id, id, actor).await {
            Ok(_) => result.undone += 1,
            Err(ActivityError::Database(error)) => return Err(ActivityError::Database(error)),
            Err(error) => result.failed.push((id, error.to_string())),
        }
    }
    Ok(result)
}
