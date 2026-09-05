use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use chrono::{DateTime, Utc};
use deepref_application::{
    AutomationDomainEvent, GetScreeningQueueQuery, ScreenReportCommand, UndoScreeningCommand,
};
use deepref_domain::{
    Actor, CurrentScreeningState, ScreeningDecision, ScreeningStage, ScreeningTransition,
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sqlx::{PgPool, Postgres, Row, Transaction};
use thiserror::Error;
use uuid::Uuid;

#[derive(Debug, Error)]
pub enum ScreeningError {
    #[error("database operation failed")]
    Database(#[from] sqlx::Error),
    #[error("project not found")]
    ProjectNotFound,
    #[error("report is not part of this project")]
    ReportNotInProject,
    #[error("published protocol not found")]
    ProtocolNotFound,
    #[error("exclusion reason not found in this project")]
    ExclusionReasonNotFound,
    #[error("exclusion reason is for a different screening stage")]
    ExclusionReasonWrongStage,
    #[error("screening revision conflict")]
    RevisionConflict {
        current: Box<ScreeningStateSnapshot>,
    },
    #[error("screening decision is already current")]
    Repeated {
        current: Box<ScreeningStateSnapshot>,
    },
    #[error("screening history has no event to undo")]
    NoHistory,
    #[error("only the latest screening event can be undone")]
    UndoNotLatest {
        current: Box<ScreeningStateSnapshot>,
    },
    #[error("screening history is unavailable for this report")]
    HistoryNotFound,
    #[error("screening queue is empty")]
    QueueEmpty,
    #[error("invalid screening cursor")]
    InvalidCursor,
    #[error("invalid screening data: {0}")]
    InvalidData(String),
    #[error("invalid screening transition: {0}")]
    InvalidTransition(String),
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ScreeningStateSnapshot {
    pub project_id: Uuid,
    pub report_id: Uuid,
    pub title_abstract_status: String,
    pub full_text_status: String,
    pub full_text_exclusion_reason_id: Option<Uuid>,
    pub final_status: String,
    pub revision: i64,
    pub last_event_id: Option<Uuid>,
    pub updated_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct ScreeningQueueItem {
    pub report_id: Uuid,
    pub title: Option<String>,
    pub abstract_text: Option<String>,
    pub doi: Option<String>,
    pub publication_year: Option<i32>,
    pub title_abstract_status: String,
    pub full_text_status: String,
    pub final_status: String,
    pub revision: i64,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct ScreeningProgress {
    pub total: i64,
    pub screened: i64,
    pub unscreened: i64,
    pub included: i64,
    pub excluded: i64,
    pub maybe: i64,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct ScreeningQueue {
    pub items: Vec<ScreeningQueueItem>,
    pub status: String,
    pub sort: String,
    pub total: i64,
    pub next_cursor: Option<String>,
    pub progress: ScreeningProgress,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct ScreeningHistoryItem {
    pub id: Uuid,
    pub event_kind: String,
    pub stage: String,
    pub decision: Option<String>,
    pub notes: Option<String>,
    pub protocol_version_id: Uuid,
    pub actor_kind: String,
    pub actor_id: String,
    pub supersedes_event_id: Option<Uuid>,
    pub undoes_event_id: Option<Uuid>,
    pub created_at: DateTime<Utc>,
    pub previous_title_abstract_status: String,
    pub previous_full_text_status: String,
    pub previous_full_text_exclusion_reason_id: Option<Uuid>,
    pub previous_final_status: String,
    pub result_title_abstract_status: String,
    pub result_full_text_status: String,
    pub result_full_text_exclusion_reason_id: Option<Uuid>,
    pub result_final_status: String,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct ScreeningHistory {
    pub project_id: Uuid,
    pub report_id: Uuid,
    pub items: Vec<ScreeningHistoryItem>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ScreeningCursor {
    sort: String,
    report_id: Uuid,
    created_at: DateTime<Utc>,
    title: String,
    year: i32,
}

pub async fn screen_report(
    pool: &PgPool,
    command: ScreenReportCommand,
) -> Result<ScreeningStateSnapshot, ScreeningError> {
    let mut tx = pool.begin().await?;
    let next_snapshot = screen_report_in_transaction(&mut tx, command).await?;
    tx.commit().await?;
    Ok(next_snapshot)
}

pub(crate) async fn screen_report_in_transaction(
    tx: &mut Transaction<'_, Postgres>,
    command: ScreenReportCommand,
) -> Result<ScreeningStateSnapshot, ScreeningError> {
    ensure_project_and_report(tx, command.project_id.into(), command.report_id.into()).await?;
    lock_screening_target(tx, command.project_id.into(), command.report_id.into()).await?;
    ensure_published_protocol(
        tx,
        command.project_id.into(),
        command.protocol_version_id.into(),
    )
    .await?;
    ensure_exclusion_reason(
        tx,
        command.project_id.into(),
        command.stage,
        command.exclusion_reason_id.map(Into::into),
    )
    .await?;

    let current =
        load_state_for_update(tx, command.project_id.into(), command.report_id.into()).await?;
    let current_snapshot = current
        .clone()
        .unwrap_or_else(|| default_state(command.project_id.into(), command.report_id.into()));
    if current_snapshot.revision != command.expected_revision {
        return Err(ScreeningError::RevisionConflict {
            current: Box::new(current_snapshot),
        });
    }
    let current_domain = domain_state(&current_snapshot)?;
    let supersedes_event_id = latest_stage_event_id(
        tx,
        command.project_id.into(),
        command.report_id.into(),
        command.stage,
    )
    .await?;
    let next_domain = match command
        .validate(current_domain)
        .map_err(|error| ScreeningError::InvalidTransition(error.to_string()))?
    {
        ScreeningTransition::Applied(next) => next,
        ScreeningTransition::Repeated => {
            return Err(ScreeningError::Repeated {
                current: Box::new(current_snapshot),
            });
        }
    };
    let event_id = Uuid::new_v4();
    let next_snapshot = persist_event_and_state(
        tx,
        &EventWrite {
            event_id,
            event_kind: "decision",
            stage: command.stage,
            decision: Some(command.decision),
            exclusion_reason_id: command.exclusion_reason_id.map(Into::into),
            notes: command.notes,
            protocol_version_id: command.protocol_version_id.into(),
            actor: command.actor,
            supersedes_event_id,
            undoes_event_id: None,
            previous: &current_snapshot,
            result: &state_snapshot_from_domain(
                command.project_id.into(),
                command.report_id.into(),
                current_snapshot.revision + 1,
                event_id,
                next_domain,
            ),
        },
    )
    .await?;
    Ok(next_snapshot)
}

pub async fn undo_screening(
    pool: &PgPool,
    command: UndoScreeningCommand,
) -> Result<ScreeningStateSnapshot, ScreeningError> {
    let mut tx = pool.begin().await?;
    ensure_project_and_report(&mut tx, command.project_id.into(), command.report_id.into()).await?;
    lock_screening_target(&mut tx, command.project_id.into(), command.report_id.into()).await?;
    ensure_published_protocol(
        &mut tx,
        command.project_id.into(),
        command.protocol_version_id.into(),
    )
    .await?;
    let current =
        load_state_for_update(&mut tx, command.project_id.into(), command.report_id.into()).await?;
    let current_snapshot = current
        .clone()
        .unwrap_or_else(|| default_state(command.project_id.into(), command.report_id.into()));
    if current_snapshot.revision != command.expected_revision {
        return Err(ScreeningError::RevisionConflict {
            current: Box::new(current_snapshot),
        });
    }
    let Some(last_event_id) = current_snapshot.last_event_id else {
        return Err(ScreeningError::NoHistory);
    };
    let event = sqlx::query(
        "SELECT id, event_kind, stage, previous_title_abstract_status, previous_full_text_status, previous_full_text_exclusion_reason_id, previous_final_status FROM screening_events WHERE id=$1 AND project_id=$2 AND report_id=$3 FOR SHARE",
    )
    .bind(last_event_id)
    .bind(Uuid::from(command.project_id))
    .bind(Uuid::from(command.report_id))
    .fetch_optional(&mut *tx)
    .await?
    .ok_or(ScreeningError::NoHistory)?;
    if event.get::<String, _>("stage") != stage_name(command.stage) {
        return Err(ScreeningError::UndoNotLatest {
            current: Box::new(current_snapshot),
        });
    }
    let restored_snapshot = ScreeningStateSnapshot {
        project_id: current_snapshot.project_id,
        report_id: current_snapshot.report_id,
        title_abstract_status: event.get("previous_title_abstract_status"),
        full_text_status: event.get("previous_full_text_status"),
        full_text_exclusion_reason_id: event.get("previous_full_text_exclusion_reason_id"),
        final_status: event.get("previous_final_status"),
        revision: current_snapshot.revision + 1,
        last_event_id: None,
        updated_at: None,
    };
    let next_domain = match command
        .validate(
            domain_state(&current_snapshot)?,
            domain_state(&restored_snapshot)?,
        )
        .map_err(|error| ScreeningError::InvalidTransition(error.to_string()))?
    {
        ScreeningTransition::Applied(next) => next,
        ScreeningTransition::Repeated => {
            return Err(ScreeningError::Repeated {
                current: Box::new(current_snapshot),
            });
        }
    };
    let event_id = Uuid::new_v4();
    let result = state_snapshot_from_domain(
        current_snapshot.project_id,
        current_snapshot.report_id,
        current_snapshot.revision + 1,
        event_id,
        next_domain,
    );
    let next_snapshot = persist_event_and_state(
        &mut tx,
        &EventWrite {
            event_id,
            event_kind: "undo",
            stage: command.stage,
            decision: None,
            exclusion_reason_id: None,
            notes: command.notes,
            protocol_version_id: command.protocol_version_id.into(),
            actor: command.actor,
            supersedes_event_id: current_snapshot.last_event_id,
            undoes_event_id: current_snapshot.last_event_id,
            previous: &current_snapshot,
            result: &result,
        },
    )
    .await?;
    tx.commit().await?;
    Ok(next_snapshot)
}

pub async fn get_screening_queue(
    pool: &PgPool,
    query: GetScreeningQueueQuery,
) -> Result<ScreeningQueue, ScreeningError> {
    let project_id = Uuid::from(query.project_id);
    ensure_project(pool, project_id).await?;
    let cursor = query.cursor.as_deref().map(decode_cursor).transpose()?;
    let limit = query.limit.clamp(1, 100);
    let status = query.status.as_str();
    let search = query
        .search
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty());
    if let Some(cursor) = cursor.as_ref() {
        if cursor.sort != query.sort.as_str() {
            return Err(ScreeningError::InvalidCursor);
        }
        validate_cursor(pool, &query, status, search, cursor).await?;
    }
    let mut rows = queue_rows(pool, &query, status, search, cursor.as_ref(), limit + 1).await?;
    let has_next = rows.len() > limit as usize;
    if has_next {
        rows.truncate(limit as usize);
    }
    let total = queue_total(pool, project_id, status, search).await?;
    let progress = queue_progress(pool, project_id).await?;
    let items: Vec<_> = rows.iter().map(queue_item_from_row).collect();
    let next_cursor = if has_next {
        rows.last()
            .map(|row| encode_cursor(&query, row))
            .transpose()?
    } else {
        None
    };
    Ok(ScreeningQueue {
        items,
        status: status.to_owned(),
        sort: query.sort.as_str().to_owned(),
        total,
        next_cursor,
        progress,
    })
}

pub async fn get_next_screening_item(
    pool: &PgPool,
    mut query: GetScreeningQueueQuery,
) -> Result<ScreeningQueueItem, ScreeningError> {
    query.limit = 1;
    get_screening_queue(pool, query)
        .await?
        .items
        .into_iter()
        .next()
        .ok_or(ScreeningError::QueueEmpty)
}

pub async fn get_screening_history(
    pool: &PgPool,
    project_id: Uuid,
    report_id: Uuid,
) -> Result<ScreeningHistory, ScreeningError> {
    ensure_project_and_report_pool(pool, project_id, report_id).await?;
    let rows = sqlx::query(
        r#"
        SELECT id,event_kind,stage,decision,notes,protocol_version_id,actor_kind,actor_id,
               supersedes_event_id,undoes_event_id,created_at,
               previous_title_abstract_status,previous_full_text_status,
               previous_full_text_exclusion_reason_id,previous_final_status,
               result_title_abstract_status,result_full_text_status,
               result_full_text_exclusion_reason_id,result_final_status
        FROM screening_events
        WHERE project_id=$1 AND report_id=$2
        ORDER BY created_at ASC, id ASC
        "#,
    )
    .bind(project_id)
    .bind(report_id)
    .fetch_all(pool)
    .await?;
    Ok(ScreeningHistory {
        project_id,
        report_id,
        items: rows.into_iter().map(history_item_from_row).collect(),
    })
}

struct EventWrite<'a> {
    event_id: Uuid,
    event_kind: &'static str,
    stage: ScreeningStage,
    decision: Option<ScreeningDecision>,
    exclusion_reason_id: Option<Uuid>,
    notes: Option<String>,
    protocol_version_id: Uuid,
    actor: Actor,
    supersedes_event_id: Option<Uuid>,
    undoes_event_id: Option<Uuid>,
    previous: &'a ScreeningStateSnapshot,
    result: &'a ScreeningStateSnapshot,
}

async fn persist_event_and_state(
    tx: &mut Transaction<'_, Postgres>,
    write: &EventWrite<'_>,
) -> Result<ScreeningStateSnapshot, ScreeningError> {
    sqlx::query(
        r#"
        INSERT INTO screening_events (
          id,project_id,report_id,event_kind,stage,decision,exclusion_reason_id,notes,
          protocol_version_id,actor_kind,actor_id,supersedes_event_id,undoes_event_id,
          previous_title_abstract_status,previous_full_text_status,
          previous_full_text_exclusion_reason_id,previous_final_status,
          result_title_abstract_status,result_full_text_status,
          result_full_text_exclusion_reason_id,result_final_status
        ) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16,$17,$18,$19,$20,$21)
        "#,
    )
    .bind(write.event_id)
    .bind(write.previous.project_id)
    .bind(write.previous.report_id)
    .bind(write.event_kind)
    .bind(stage_name(write.stage))
    .bind(write.decision.map(decision_name))
    .bind(write.exclusion_reason_id)
    .bind(&write.notes)
    .bind(write.protocol_version_id)
    .bind(write.actor.kind().as_str())
    .bind(write.actor.id())
    .bind(write.supersedes_event_id)
    .bind(write.undoes_event_id)
    .bind(&write.previous.title_abstract_status)
    .bind(&write.previous.full_text_status)
    .bind(write.previous.full_text_exclusion_reason_id)
    .bind(&write.previous.final_status)
    .bind(&write.result.title_abstract_status)
    .bind(&write.result.full_text_status)
    .bind(write.result.full_text_exclusion_reason_id)
    .bind(&write.result.final_status)
    .execute(&mut **tx)
    .await?;

    let row = sqlx::query(
        r#"
        INSERT INTO screening_state (
          project_id,report_id,title_abstract_status,full_text_status,
          full_text_exclusion_reason_id,final_status,revision,last_event_id,updated_at
        ) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,now())
        ON CONFLICT (project_id,report_id) DO UPDATE SET
          title_abstract_status=EXCLUDED.title_abstract_status,
          full_text_status=EXCLUDED.full_text_status,
          full_text_exclusion_reason_id=EXCLUDED.full_text_exclusion_reason_id,
          final_status=EXCLUDED.final_status,
          revision=EXCLUDED.revision,
          last_event_id=EXCLUDED.last_event_id,
          updated_at=now()
        RETURNING project_id,report_id,title_abstract_status,full_text_status,
          full_text_exclusion_reason_id,final_status,revision,last_event_id,updated_at
        "#,
    )
    .bind(write.result.project_id)
    .bind(write.result.report_id)
    .bind(&write.result.title_abstract_status)
    .bind(&write.result.full_text_status)
    .bind(write.result.full_text_exclusion_reason_id)
    .bind(&write.result.final_status)
    .bind(write.result.revision)
    .bind(write.event_id)
    .fetch_one(&mut **tx)
    .await?;
    let next = state_snapshot_from_row(&row);
    let lifecycle = lifecycle_for_status(&next.final_status);
    sqlx::query(
        "UPDATE project_reports SET lifecycle_status=$3 WHERE project_id=$1 AND report_id=$2",
    )
    .bind(next.project_id)
    .bind(next.report_id)
    .bind(lifecycle)
    .execute(&mut **tx)
    .await?;

    let event_type = if write.event_kind == "undo" {
        "screening_decision_undone"
    } else {
        "report_screened"
    };
    sqlx::query(
        "INSERT INTO review_events (id,project_id,event_type,aggregate_type,aggregate_id,payload,actor_kind,actor_id) VALUES ($1,$2,$3,'report',$4,$5,$6,$7)",
    )
    .bind(Uuid::new_v4())
    .bind(next.project_id)
    .bind(event_type)
    .bind(next.report_id)
    .bind(json!({
        "screening_event_id": write.event_id,
        "event_kind": write.event_kind,
        "stage": stage_name(write.stage),
        "decision": write.decision.map(decision_name),
        "revision": next.revision,
        "protocol_version_id": write.protocol_version_id,
        "supersedes_event_id": write.supersedes_event_id,
        "undoes_event_id": write.undoes_event_id,
    }))
    .bind(write.actor.kind().as_str())
    .bind(write.actor.id())
    .execute(&mut **tx)
    .await?;

    if write.event_kind == "decision"
        && write.previous.final_status != "include"
        && write.result.final_status == "include"
    {
        crate::dispatch_automation_domain_event(
            tx,
            &AutomationDomainEvent::ReportIncluded {
                project_id: write.result.project_id.into(),
                screening_event_id: write.event_id,
                actor: write.actor.clone(),
            },
        )
        .await?;
    }

    Ok(next)
}

async fn ensure_project_and_report(
    tx: &mut Transaction<'_, Postgres>,
    project_id: Uuid,
    report_id: Uuid,
) -> Result<(), ScreeningError> {
    let project_exists: bool =
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM projects WHERE id=$1)")
            .bind(project_id)
            .fetch_one(&mut **tx)
            .await?;
    if !project_exists {
        return Err(ScreeningError::ProjectNotFound);
    }
    let report_exists: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM project_reports WHERE project_id=$1 AND report_id=$2)",
    )
    .bind(project_id)
    .bind(report_id)
    .fetch_one(&mut **tx)
    .await?;
    if !report_exists {
        return Err(ScreeningError::ReportNotInProject);
    }
    Ok(())
}

async fn lock_screening_target(
    tx: &mut Transaction<'_, Postgres>,
    project_id: Uuid,
    report_id: Uuid,
) -> Result<(), ScreeningError> {
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 0))")
        .bind(format!("screening:{project_id}:{report_id}"))
        .execute(&mut **tx)
        .await?;
    Ok(())
}

async fn latest_stage_event_id(
    tx: &mut Transaction<'_, Postgres>,
    project_id: Uuid,
    report_id: Uuid,
    stage: ScreeningStage,
) -> Result<Option<Uuid>, ScreeningError> {
    Ok(sqlx::query_scalar(
        "SELECT id FROM screening_events WHERE project_id=$1 AND report_id=$2 AND stage=$3 ORDER BY created_at DESC, id DESC LIMIT 1",
    )
    .bind(project_id)
    .bind(report_id)
    .bind(stage_name(stage))
    .fetch_optional(&mut **tx)
    .await?)
}

async fn ensure_project_and_report_pool(
    pool: &PgPool,
    project_id: Uuid,
    report_id: Uuid,
) -> Result<(), ScreeningError> {
    let project_exists: bool =
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM projects WHERE id=$1)")
            .bind(project_id)
            .fetch_one(pool)
            .await?;
    if !project_exists {
        return Err(ScreeningError::ProjectNotFound);
    }
    let report_exists: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM project_reports WHERE project_id=$1 AND report_id=$2)",
    )
    .bind(project_id)
    .bind(report_id)
    .fetch_one(pool)
    .await?;
    if !report_exists {
        return Err(ScreeningError::ReportNotInProject);
    }
    Ok(())
}

async fn ensure_project(pool: &PgPool, project_id: Uuid) -> Result<(), ScreeningError> {
    let exists: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM projects WHERE id=$1)")
        .bind(project_id)
        .fetch_one(pool)
        .await?;
    if exists {
        Ok(())
    } else {
        Err(ScreeningError::ProjectNotFound)
    }
}

async fn ensure_published_protocol(
    tx: &mut Transaction<'_, Postgres>,
    project_id: Uuid,
    protocol_version_id: Uuid,
) -> Result<(), ScreeningError> {
    let exists: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM protocol_versions WHERE id=$1 AND project_id=$2 AND status='published')",
    )
    .bind(protocol_version_id)
    .bind(project_id)
    .fetch_one(&mut **tx)
    .await?;
    if exists {
        Ok(())
    } else {
        Err(ScreeningError::ProtocolNotFound)
    }
}

async fn ensure_exclusion_reason(
    tx: &mut Transaction<'_, Postgres>,
    project_id: Uuid,
    stage: ScreeningStage,
    reason_id: Option<Uuid>,
) -> Result<(), ScreeningError> {
    let Some(reason_id) = reason_id else {
        return Ok(());
    };
    let stage_name = stage_name(stage);
    let reason_stage: Option<String> =
        sqlx::query_scalar("SELECT stage FROM exclusion_reasons WHERE id=$1 AND project_id=$2")
            .bind(reason_id)
            .bind(project_id)
            .fetch_optional(&mut **tx)
            .await?;
    match reason_stage {
        None => Err(ScreeningError::ExclusionReasonNotFound),
        Some(value) if value == stage_name => Ok(()),
        Some(_) => Err(ScreeningError::ExclusionReasonWrongStage),
    }
}

async fn load_state_for_update(
    tx: &mut Transaction<'_, Postgres>,
    project_id: Uuid,
    report_id: Uuid,
) -> Result<Option<ScreeningStateSnapshot>, ScreeningError> {
    Ok(sqlx::query(
        "SELECT project_id,report_id,title_abstract_status,full_text_status,full_text_exclusion_reason_id,final_status,revision,last_event_id,updated_at FROM screening_state WHERE project_id=$1 AND report_id=$2 FOR UPDATE",
    )
    .bind(project_id)
    .bind(report_id)
    .fetch_optional(&mut **tx)
    .await?
    .map(|row| state_snapshot_from_row(&row)))
}

async fn queue_rows(
    pool: &PgPool,
    query: &GetScreeningQueueQuery,
    status: &str,
    search: Option<&str>,
    cursor: Option<&ScreeningCursor>,
    limit: i64,
) -> Result<Vec<sqlx::postgres::PgRow>, ScreeningError> {
    let project_id = Uuid::from(query.project_id);
    let cursor_created_at = cursor.map(|value| value.created_at);
    let cursor_title = cursor.map(|value| value.title.as_str());
    let cursor_year = cursor.map(|value| value.year);
    let cursor_id = cursor.map(|value| value.report_id);
    let rows = sqlx::query(
        r#"
        SELECT r.id AS report_id, r.title, r.abstract_text, r.publication_year,
          doi.value AS doi,
          coalesce(ss.title_abstract_status,'unscreened') AS title_abstract_status,
          coalesce(ss.full_text_status,'not_required') AS full_text_status,
          coalesce(ss.final_status,'unscreened') AS final_status,
          coalesce(ss.revision,0)::bigint AS revision,
          pr.created_at AS queue_created_at
        FROM project_reports pr
        JOIN reports r ON r.id=pr.report_id
        LEFT JOIN LATERAL (
          SELECT value FROM report_identifiers
          WHERE report_id=r.id AND scheme='doi'
          ORDER BY created_at,id
          LIMIT 1
        ) doi ON true
        LEFT JOIN screening_state ss ON ss.project_id=pr.project_id AND ss.report_id=pr.report_id
        WHERE pr.project_id=$1
          AND ($2='all' OR coalesce(ss.title_abstract_status,'unscreened')=$2)
          AND ($3::text IS NULL OR lower(coalesce(r.title,'') || ' ' || coalesce(r.abstract_text,'')) LIKE '%' || lower($3) || '%')
          AND (
            NOT $4::boolean
            OR CASE $5
              WHEN 'created_asc' THEN (pr.created_at, r.id) > ($6::timestamptz, $7::uuid)
              WHEN 'created_desc' THEN (pr.created_at, r.id) < ($6::timestamptz, $7::uuid)
              WHEN 'title_asc' THEN (lower(coalesce(r.title,'')), r.id) > (lower(coalesce($8,'')), $7::uuid)
              WHEN 'title_desc' THEN (lower(coalesce(r.title,'')), r.id) < (lower(coalesce($8,'')), $7::uuid)
              WHEN 'year_asc' THEN (coalesce(r.publication_year,-2147483648), r.id) > (coalesce($9::int,-2147483648), $7::uuid)
              WHEN 'year_desc' THEN (coalesce(r.publication_year,-2147483648), r.id) < (coalesce($9::int,-2147483648), $7::uuid)
              ELSE false
            END
          )
        ORDER BY
          CASE WHEN $5='created_asc' THEN pr.created_at END ASC,
          CASE WHEN $5='created_desc' THEN pr.created_at END DESC,
          CASE WHEN $5='title_asc' THEN lower(coalesce(r.title,'')) END ASC,
          CASE WHEN $5='title_desc' THEN lower(coalesce(r.title,'')) END DESC,
          CASE WHEN $5='year_asc' THEN coalesce(r.publication_year,-2147483648) END ASC,
          CASE WHEN $5='year_desc' THEN coalesce(r.publication_year,-2147483648) END DESC,
          CASE WHEN $5 IN ('created_asc','title_asc','year_asc') THEN r.id END ASC,
          CASE WHEN $5 IN ('created_desc','title_desc','year_desc') THEN r.id END DESC
        LIMIT $10
        "#,
    )
    .bind(project_id)
    .bind(status)
    .bind(search)
    .bind(cursor.is_some())
    .bind(query.sort.as_str())
    .bind(cursor_created_at)
    .bind(cursor_id)
    .bind(cursor_title)
    .bind(cursor_year)
    .bind(limit)
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

async fn validate_cursor(
    pool: &PgPool,
    query: &GetScreeningQueueQuery,
    status: &str,
    search: Option<&str>,
    cursor: &ScreeningCursor,
) -> Result<(), ScreeningError> {
    let row = sqlx::query(
        r#"
        SELECT pr.created_at AS queue_created_at,
               lower(coalesce(r.title, '')) AS normalized_title,
               coalesce(r.publication_year, -2147483648) AS normalized_year
        FROM project_reports pr
        JOIN reports r ON r.id = pr.report_id
        LEFT JOIN screening_state ss ON ss.project_id = pr.project_id AND ss.report_id = pr.report_id
        WHERE pr.project_id = $1 AND pr.report_id = $2
          AND ($3 = 'all' OR coalesce(ss.title_abstract_status, 'unscreened') = $3)
          AND ($4::text IS NULL OR lower(coalesce(r.title, '') || ' ' || coalesce(r.abstract_text, '')) LIKE '%' || lower($4) || '%')
        "#,
    )
    .bind(Uuid::from(query.project_id))
    .bind(cursor.report_id)
    .bind(status)
    .bind(search)
    .fetch_optional(pool)
    .await?
    .ok_or(ScreeningError::InvalidCursor)?;

    let created_at: DateTime<Utc> = row.get("queue_created_at");
    let title: String = row.get("normalized_title");
    let year: i32 = row.get("normalized_year");
    if cursor.created_at != created_at || cursor.title != title || cursor.year != year {
        return Err(ScreeningError::InvalidCursor);
    }
    Ok(())
}

async fn queue_total(
    pool: &PgPool,
    project_id: Uuid,
    status: &str,
    search: Option<&str>,
) -> Result<i64, ScreeningError> {
    Ok(sqlx::query_scalar(
        "SELECT count(*)::bigint FROM project_reports pr JOIN reports r ON r.id=pr.report_id LEFT JOIN screening_state ss ON ss.project_id=pr.project_id AND ss.report_id=pr.report_id WHERE pr.project_id=$1 AND ($2='all' OR coalesce(ss.title_abstract_status,'unscreened')=$2) AND ($3::text IS NULL OR lower(coalesce(r.title,'') || ' ' || coalesce(r.abstract_text,'')) LIKE '%' || lower($3) || '%')",
    )
    .bind(project_id)
    .bind(status)
    .bind(search)
    .fetch_one(pool)
    .await?)
}

async fn queue_progress(
    pool: &PgPool,
    project_id: Uuid,
) -> Result<ScreeningProgress, ScreeningError> {
    let row = sqlx::query(
        "SELECT count(*)::bigint AS total, count(*) FILTER (WHERE coalesce(ss.title_abstract_status,'unscreened') <> 'unscreened')::bigint AS screened, count(*) FILTER (WHERE coalesce(ss.title_abstract_status,'unscreened')='unscreened')::bigint AS unscreened, count(*) FILTER (WHERE ss.title_abstract_status='include')::bigint AS included, count(*) FILTER (WHERE ss.title_abstract_status='exclude')::bigint AS excluded, count(*) FILTER (WHERE ss.title_abstract_status='maybe')::bigint AS maybe FROM project_reports pr LEFT JOIN screening_state ss ON ss.project_id=pr.project_id AND ss.report_id=pr.report_id WHERE pr.project_id=$1",
    )
    .bind(project_id)
    .fetch_one(pool)
    .await?;
    Ok(ScreeningProgress {
        total: row.get("total"),
        screened: row.get("screened"),
        unscreened: row.get("unscreened"),
        included: row.get("included"),
        excluded: row.get("excluded"),
        maybe: row.get("maybe"),
    })
}

fn default_state(project_id: Uuid, report_id: Uuid) -> ScreeningStateSnapshot {
    ScreeningStateSnapshot {
        project_id,
        report_id,
        title_abstract_status: "unscreened".to_owned(),
        full_text_status: "not_required".to_owned(),
        full_text_exclusion_reason_id: None,
        final_status: "unscreened".to_owned(),
        revision: 0,
        last_event_id: None,
        updated_at: None,
    }
}

fn state_snapshot_from_row(row: &sqlx::postgres::PgRow) -> ScreeningStateSnapshot {
    ScreeningStateSnapshot {
        project_id: row.get("project_id"),
        report_id: row.get("report_id"),
        title_abstract_status: row.get("title_abstract_status"),
        full_text_status: row.get("full_text_status"),
        full_text_exclusion_reason_id: row.get("full_text_exclusion_reason_id"),
        final_status: row.get("final_status"),
        revision: row.get("revision"),
        last_event_id: row.get("last_event_id"),
        updated_at: row.get("updated_at"),
    }
}

fn state_snapshot_from_domain(
    project_id: Uuid,
    report_id: Uuid,
    revision: i64,
    event_id: Uuid,
    state: CurrentScreeningState,
) -> ScreeningStateSnapshot {
    let title = state
        .title_abstract
        .map(decision_name)
        .unwrap_or("unscreened")
        .to_owned();
    let full_text = state
        .full_text
        .map(decision_name)
        .unwrap_or("not_required")
        .to_owned();
    let final_status = match (state.title_abstract, state.full_text) {
        (Some(ScreeningDecision::Include), Some(decision)) => decision_name(decision),
        (Some(ScreeningDecision::Include), None) => "pending_full_text",
        (Some(ScreeningDecision::Exclude), _) => "exclude",
        (Some(ScreeningDecision::Maybe), _) => "maybe",
        (None, _) => "unscreened",
    }
    .to_owned();
    ScreeningStateSnapshot {
        project_id,
        report_id,
        title_abstract_status: title,
        full_text_status: full_text,
        full_text_exclusion_reason_id: state.full_text_exclusion_reason_id.map(Into::into),
        final_status,
        revision,
        last_event_id: Some(event_id),
        updated_at: None,
    }
}

fn domain_state(state: &ScreeningStateSnapshot) -> Result<CurrentScreeningState, ScreeningError> {
    Ok(CurrentScreeningState {
        title_abstract: decision_from_status(&state.title_abstract_status)?,
        full_text: decision_from_status(&state.full_text_status)?,
        full_text_exclusion_reason_id: state.full_text_exclusion_reason_id.map(Into::into),
    })
}

fn queue_item_from_row(row: &sqlx::postgres::PgRow) -> ScreeningQueueItem {
    ScreeningQueueItem {
        report_id: row.get("report_id"),
        title: row.get("title"),
        abstract_text: row.get("abstract_text"),
        doi: row.get("doi"),
        publication_year: row.get("publication_year"),
        title_abstract_status: row.get("title_abstract_status"),
        full_text_status: row.get("full_text_status"),
        final_status: row.get("final_status"),
        revision: row.get("revision"),
    }
}

fn history_item_from_row(row: sqlx::postgres::PgRow) -> ScreeningHistoryItem {
    ScreeningHistoryItem {
        id: row.get("id"),
        event_kind: row.get("event_kind"),
        stage: row.get("stage"),
        decision: row.get("decision"),
        notes: row.get("notes"),
        protocol_version_id: row.get("protocol_version_id"),
        actor_kind: row.get("actor_kind"),
        actor_id: row.get("actor_id"),
        supersedes_event_id: row.get("supersedes_event_id"),
        undoes_event_id: row.get("undoes_event_id"),
        created_at: row.get("created_at"),
        previous_title_abstract_status: row.get("previous_title_abstract_status"),
        previous_full_text_status: row.get("previous_full_text_status"),
        previous_full_text_exclusion_reason_id: row.get("previous_full_text_exclusion_reason_id"),
        previous_final_status: row.get("previous_final_status"),
        result_title_abstract_status: row.get("result_title_abstract_status"),
        result_full_text_status: row.get("result_full_text_status"),
        result_full_text_exclusion_reason_id: row.get("result_full_text_exclusion_reason_id"),
        result_final_status: row.get("result_final_status"),
    }
}

fn encode_cursor(
    query: &GetScreeningQueueQuery,
    row: &sqlx::postgres::PgRow,
) -> Result<String, ScreeningError> {
    let value = ScreeningCursor {
        sort: query.sort.as_str().to_owned(),
        report_id: row.get("report_id"),
        created_at: row.get("queue_created_at"),
        title: row
            .get::<Option<String>, _>("title")
            .unwrap_or_default()
            .to_lowercase(),
        year: row
            .get::<Option<i32>, _>("publication_year")
            .unwrap_or(i32::MIN),
    };
    let bytes = serde_json::to_vec(&value).map_err(|_| ScreeningError::InvalidCursor)?;
    Ok(URL_SAFE_NO_PAD.encode(bytes))
}

fn decode_cursor(value: &str) -> Result<ScreeningCursor, ScreeningError> {
    let bytes = URL_SAFE_NO_PAD
        .decode(value)
        .map_err(|_| ScreeningError::InvalidCursor)?;
    let cursor: ScreeningCursor =
        serde_json::from_slice(&bytes).map_err(|_| ScreeningError::InvalidCursor)?;
    if cursor.sort.is_empty() {
        return Err(ScreeningError::InvalidCursor);
    }
    Ok(cursor)
}

fn decision_from_status(status: &str) -> Result<Option<ScreeningDecision>, ScreeningError> {
    match status {
        "include" => Ok(Some(ScreeningDecision::Include)),
        "exclude" => Ok(Some(ScreeningDecision::Exclude)),
        "maybe" => Ok(Some(ScreeningDecision::Maybe)),
        "unscreened" | "not_required" => Ok(None),
        other => Err(ScreeningError::InvalidData(format!(
            "unknown screening status {other:?}"
        ))),
    }
}

fn stage_name(stage: ScreeningStage) -> &'static str {
    match stage {
        ScreeningStage::TitleAbstract => "title_abstract",
        ScreeningStage::FullText => "full_text",
    }
}

fn decision_name(decision: ScreeningDecision) -> &'static str {
    match decision {
        ScreeningDecision::Include => "include",
        ScreeningDecision::Exclude => "exclude",
        ScreeningDecision::Maybe => "maybe",
    }
}

fn lifecycle_for_status(status: &str) -> &'static str {
    match status {
        "include" => "included",
        "exclude" => "excluded",
        "maybe" => "maybe",
        _ => "screening",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use deepref_application::{ScreeningQueueSort, ScreeningQueueStatus};

    #[test]
    fn cursor_round_trip_uses_an_opaque_url_safe_value() {
        let query = GetScreeningQueueQuery {
            project_id: Uuid::new_v4().into(),
            status: ScreeningQueueStatus::Unscreened,
            search: None,
            sort: ScreeningQueueSort::CreatedAscending,
            cursor: None,
            limit: 50,
        };
        let cursor = ScreeningCursor {
            sort: query.sort.as_str().to_owned(),
            report_id: Uuid::new_v4(),
            created_at: Utc::now(),
            title: "a report".to_owned(),
            year: 2024,
        };
        let value = URL_SAFE_NO_PAD.encode(serde_json::to_vec(&cursor).expect("cursor serializes"));
        let parsed = decode_cursor(&value).expect("cursor decodes");
        assert_eq!(parsed.report_id, cursor.report_id);
        assert_eq!(parsed.sort, "created_asc");
    }

    #[test]
    fn queue_status_and_sort_parsers_are_closed() {
        assert_eq!(
            ScreeningQueueStatus::parse("all"),
            Some(ScreeningQueueStatus::All)
        );
        assert_eq!(
            ScreeningQueueSort::parse("year_desc"),
            Some(ScreeningQueueSort::YearDescending)
        );
        assert_eq!(ScreeningQueueSort::parse("random"), None);
    }
}
