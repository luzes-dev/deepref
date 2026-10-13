use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use chrono::{DateTime, Utc};
use deepref_application::{
    AutomationDomainEvent, GetScreeningQueueQuery, ScreenReportCommand, UndoScreeningCommand,
};
use deepref_domain::{
    Actor, CurrentScreeningState, ProjectId, ScreeningDecision, ScreeningStage, ScreeningTransition,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::{PgPool, Postgres, Row, Transaction};
use thiserror::Error;
use uuid::Uuid;

#[derive(Debug, Error)]
pub enum ScreeningError {
    #[error("database operation failed")]
    Database(#[from] sqlx::Error),
    #[error("AI-first screening operation failed")]
    AiFirst(#[source] Box<crate::AiFirstError>),
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
    pub title_abstract_status: deepref_domain::ScreeningStatus,
    pub full_text_status: deepref_domain::ScreeningStatus,
    pub full_text_exclusion_reason_id: Option<Uuid>,
    pub final_status: deepref_domain::ScreeningStatus,
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
    pub title_abstract_status: deepref_domain::ScreeningStatus,
    pub full_text_status: deepref_domain::ScreeningStatus,
    pub final_status: deepref_domain::ScreeningStatus,
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
    pub stage: ScreeningStage,
    pub decision: Option<ScreeningDecision>,
    pub notes: Option<String>,
    pub protocol_version_id: Uuid,
    pub actor_kind: String,
    pub actor_id: String,
    pub supersedes_event_id: Option<Uuid>,
    pub undoes_event_id: Option<Uuid>,
    pub created_at: DateTime<Utc>,
    pub previous_title_abstract_status: deepref_domain::ScreeningStatus,
    pub previous_full_text_status: deepref_domain::ScreeningStatus,
    pub previous_full_text_exclusion_reason_id: Option<Uuid>,
    pub previous_final_status: deepref_domain::ScreeningStatus,
    pub result_title_abstract_status: deepref_domain::ScreeningStatus,
    pub result_full_text_status: deepref_domain::ScreeningStatus,
    pub result_full_text_exclusion_reason_id: Option<Uuid>,
    pub result_final_status: deepref_domain::ScreeningStatus,
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

/// Records a title/abstract decision that a person confirmed as part of an
/// assistant plan. The actor is the confirming person; the event's `origin`
/// says the decision itself was chosen by the assistant, so PRISMA and the
/// ADR 0005 human reference stratum do not count it as a person's own
/// screening. Records in a blind AI-first audit are refused: only a fresh
/// person may label them.
pub async fn screen_report_from_assistant_plan(
    pool: &PgPool,
    command: ScreenReportCommand,
) -> Result<ScreeningStateSnapshot, ScreeningError> {
    let mut tx = pool.begin().await?;
    let next_snapshot =
        apply_screen_report(&mut tx, command, false, Some(ORIGIN_ASSISTANT_PLAN)).await?;
    tx.commit().await?;
    Ok(next_snapshot)
}

/// `screening_events.origin` for decisions applied from a confirmed
/// assistant plan.
pub const ORIGIN_ASSISTANT_PLAN: &str = "assistant_plan";

pub(crate) async fn screen_report_in_transaction(
    tx: &mut Transaction<'_, Postgres>,
    command: ScreenReportCommand,
) -> Result<ScreeningStateSnapshot, ScreeningError> {
    apply_screen_report(tx, command, false, None).await
}

pub(crate) async fn write_audit_reference(
    tx: &mut Transaction<'_, Postgres>,
    command: ScreenReportCommand,
) -> Result<ScreeningStateSnapshot, ScreeningError> {
    apply_screen_report(tx, command, true, None).await
}

async fn apply_screen_report(
    tx: &mut Transaction<'_, Postgres>,
    command: ScreenReportCommand,
    audit_reference: bool,
    origin: Option<&'static str>,
) -> Result<ScreeningStateSnapshot, ScreeningError> {
    sqlx::query("SELECT id FROM projects WHERE id=$1 FOR UPDATE")
        .bind(command.project_id.as_uuid())
        .fetch_optional(&mut **tx)
        .await?;
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
        .unwrap_or_else(|| default_state(command.project_id.into(), command.report_id.into()));
    if !audit_reference {
        let masked: bool = sqlx::query_scalar("SELECT ai_first_audit_masked($1,$2)")
            .bind(command.project_id.as_uuid())
            .bind(command.report_id.as_uuid())
            .fetch_one(&mut **tx)
            .await?;
        deepref_domain::validate_blind_screening(masked, origin.is_some(), command.stage)
            .map_err(|message| ScreeningError::InvalidTransition(message.into()))?;
        match crate::ai_first::record_audit_label(tx, &command)
            .await
            .map_err(|error| ScreeningError::AiFirst(Box::new(error)))?
        {
            crate::ai_first::audit::AuditLabelOutcome::NotAudit => {}
            crate::ai_first::audit::AuditLabelOutcome::Pending => {
                return Ok(default_state(
                    command.project_id.into(),
                    command.report_id.into(),
                ));
            }
        }
    }
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
    let actor_for_invalidation = command.actor.clone();
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
            origin,
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
    if !audit_reference && command.stage == ScreeningStage::TitleAbstract {
        crate::ai_first::cohorts::human_state_changed(
            tx,
            next_snapshot.project_id,
            next_snapshot.report_id,
            &actor_for_invalidation,
            false,
        )
        .await
        .map_err(|error| ScreeningError::AiFirst(Box::new(error)))?;
    }
    Ok(next_snapshot)
}

pub async fn undo_screening(
    pool: &PgPool,
    command: UndoScreeningCommand,
) -> Result<ScreeningStateSnapshot, ScreeningError> {
    let mut tx = pool.begin().await?;
    sqlx::query("SELECT id FROM projects WHERE id=$1 FOR UPDATE")
        .bind(command.project_id.as_uuid())
        .fetch_optional(&mut *tx)
        .await?;
    let masked: bool = sqlx::query_scalar("SELECT ai_first_audit_masked($1,$2)")
        .bind(command.project_id.as_uuid())
        .bind(command.report_id.as_uuid())
        .fetch_one(&mut *tx)
        .await?;
    if masked {
        return Err(ScreeningError::InvalidTransition(
            "recover the blind audit before undoing decisions".into(),
        ));
    }
    let actor = command.actor.clone();
    let stage = command.stage;
    let next = undo_screening_in_transaction(&mut tx, command).await?;
    if stage == ScreeningStage::TitleAbstract {
        crate::ai_first::cohorts::human_state_changed(
            &mut tx,
            next.project_id,
            next.report_id,
            &actor,
            true,
        )
        .await
        .map_err(|error| ScreeningError::AiFirst(Box::new(error)))?;
    }
    tx.commit().await?;
    Ok(next)
}

pub(crate) async fn undo_screening_in_transaction(
    tx: &mut Transaction<'_, Postgres>,
    command: UndoScreeningCommand,
) -> Result<ScreeningStateSnapshot, ScreeningError> {
    ensure_project_and_report(tx, command.project_id.into(), command.report_id.into()).await?;
    lock_screening_target(tx, command.project_id.into(), command.report_id.into()).await?;
    ensure_published_protocol(
        tx,
        command.project_id.into(),
        command.protocol_version_id.into(),
    )
    .await?;
    let current =
        load_state_for_update(tx, command.project_id.into(), command.report_id.into()).await?;
    let current_snapshot = current
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
    .fetch_optional(&mut **tx)
    .await?
    .ok_or(ScreeningError::NoHistory)?;
    let stage_value: String = event.try_get("stage")?;
    let event_stage = ScreeningStage::parse(&stage_value).ok_or_else(|| ScreeningError::InvalidData(format!("unknown screening stage {stage_value:?}")))?;
    if event_stage != command.stage {
        return Err(ScreeningError::UndoNotLatest {
            current: Box::new(current_snapshot),
        });
    }
    let restored_snapshot = ScreeningStateSnapshot {
        project_id: current_snapshot.project_id,
        report_id: current_snapshot.report_id,
        title_abstract_status: status_from_row(&event, "previous_title_abstract_status")?,
        full_text_status: status_from_row(&event, "previous_full_text_status")?,
        full_text_exclusion_reason_id: event.get("previous_full_text_exclusion_reason_id"),
        final_status: status_from_row(&event, "previous_final_status")?,
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
        tx,
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
            origin: None,
            previous: &current_snapshot,
            result: &result,
        },
    )
    .await?;
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
    let total = queue_total(
        pool,
        project_id,
        status,
        search,
        query.reviewer_id.as_deref(),
    )
    .await?;
    let progress = queue_progress(pool, project_id).await?;
    let items: Vec<_> = rows.iter().map(queue_item_from_row).collect::<Result<_, _>>()?;
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
        WHERE project_id=$1 AND report_id=$2 AND NOT ai_first_audit_masked(project_id,report_id)
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
        items: rows.into_iter().map(history_item_from_row).collect::<Result<_, _>>()?,
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
    origin: Option<&'static str>,
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
          result_full_text_exclusion_reason_id,result_final_status,origin
        ) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16,$17,$18,$19,$20,$21,$22)
        "#,
    )
    .bind(write.event_id)
    .bind(write.previous.project_id)
    .bind(write.previous.report_id)
    .bind(write.event_kind)
    .bind(write.stage.as_str())
    .bind(write.decision.map(ScreeningDecision::as_str))
    .bind(write.exclusion_reason_id)
    .bind(&write.notes)
    .bind(write.protocol_version_id)
    .bind(write.actor.kind().as_str())
    .bind(write.actor.id())
    .bind(write.supersedes_event_id)
    .bind(write.undoes_event_id)
    .bind(write.previous.title_abstract_status.as_str())
    .bind(write.previous.full_text_status.as_str())
    .bind(write.previous.full_text_exclusion_reason_id)
    .bind(write.previous.final_status.as_str())
    .bind(write.result.title_abstract_status.as_str())
    .bind(write.result.full_text_status.as_str())
    .bind(write.result.full_text_exclusion_reason_id)
    .bind(write.result.final_status.as_str())
    .bind(write.origin)
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
    .bind(write.result.title_abstract_status.as_str())
    .bind(write.result.full_text_status.as_str())
    .bind(write.result.full_text_exclusion_reason_id)
    .bind(write.result.final_status.as_str())
    .bind(write.result.revision)
    .bind(write.event_id)
    .fetch_one(&mut **tx)
    .await?;
    let next = state_snapshot_from_row(&row)?;
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

    if write.event_kind == "decision" {
        crate::dispatch_automation_domain_event(
            tx,
            &AutomationDomainEvent::ScreeningDecisionRecorded {
                project_id: write.result.project_id.into(),
                screening_event_id: write.event_id,
                actor: write.actor.clone(),
            },
        )
        .await?;
    }

    if write.event_kind == "decision"
        && write.previous.final_status != deepref_domain::ScreeningStatus::Include
        && write.result.final_status == deepref_domain::ScreeningStatus::Include
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
    .bind(stage.as_str())
    .fetch_optional(&mut **tx)
    .await?)
}

async fn ensure_project_and_report_pool(
    pool: &PgPool,
    project_id: Uuid,
    report_id: Uuid,
) -> Result<(), ScreeningError> {
    let project_exists = sqlx::query_scalar!(
        r#"SELECT EXISTS(SELECT 1 FROM projects WHERE id=$1) as "exists!""#,
        project_id,
    )
    .fetch_one(pool)
    .await?;
    if !project_exists {
        return Err(ScreeningError::ProjectNotFound);
    }
    let report_exists = sqlx::query_scalar!(
        r#"SELECT EXISTS(SELECT 1 FROM project_reports WHERE project_id=$1 AND report_id=$2) as "exists!""#,
        project_id,
        report_id,
    )
    .fetch_one(pool)
    .await?;
    if !report_exists {
        return Err(ScreeningError::ReportNotInProject);
    }
    Ok(())
}

async fn ensure_project(pool: &PgPool, project_id: Uuid) -> Result<(), ScreeningError> {
    let exists = sqlx::query_scalar!(
        r#"SELECT EXISTS(SELECT 1 FROM projects WHERE id=$1) as "exists!""#,
        project_id,
    )
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
    let exists = sqlx::query_scalar!(
        r#"SELECT EXISTS(SELECT 1 FROM protocol_versions WHERE id=$1 AND project_id=$2 AND status='published') as "exists!""#,
        protocol_version_id,
        project_id,
    )
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
    let reason_stage = sqlx::query_scalar!(
        "SELECT stage FROM exclusion_reasons WHERE id=$1 AND project_id=$2",
        reason_id,
        project_id,
    )
    .fetch_optional(&mut **tx)
    .await?;
    let value = reason_stage.ok_or(ScreeningError::ExclusionReasonNotFound)?;
    let reason_stage = ScreeningStage::parse(&value).ok_or_else(|| ScreeningError::InvalidData(format!("unknown exclusion reason stage {value:?}")))?;
    if reason_stage == stage {
        Ok(())
    } else {
        Err(ScreeningError::ExclusionReasonWrongStage)
    }
}

async fn load_state_for_update(
    tx: &mut Transaction<'_, Postgres>,
    project_id: Uuid,
    report_id: Uuid,
) -> Result<Option<ScreeningStateSnapshot>, ScreeningError> {
    sqlx::query(
        "SELECT project_id,report_id,title_abstract_status,full_text_status,full_text_exclusion_reason_id,final_status,revision,last_event_id,updated_at FROM screening_state WHERE project_id=$1 AND report_id=$2 FOR UPDATE",
    )
    .bind(project_id)
    .bind(report_id)
    .fetch_optional(&mut **tx)
    .await?
    .map(|row| state_snapshot_from_row(&row)).transpose()
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
          CASE WHEN ai_first_audit_masked(pr.project_id,pr.report_id) THEN 'unscreened' ELSE coalesce(ss.title_abstract_status,'unscreened') END AS title_abstract_status,
          CASE WHEN ai_first_audit_masked(pr.project_id,pr.report_id) THEN 'not_required' ELSE coalesce(ss.full_text_status,'not_required') END AS full_text_status,
          CASE WHEN ai_first_audit_masked(pr.project_id,pr.report_id) THEN 'unscreened' ELSE coalesce(ss.final_status,'unscreened') END AS final_status,
          CASE WHEN ai_first_audit_masked(pr.project_id,pr.report_id) THEN 0 ELSE coalesce(ss.revision,0) END::bigint AS revision,
          ai_first_queue_time(pr.project_id,pr.report_id,pr.created_at) AS queue_created_at
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
          AND ($2='all' OR (CASE WHEN ai_first_audit_masked(pr.project_id,pr.report_id) THEN 'unscreened' ELSE coalesce(ss.title_abstract_status,'unscreened') END=$2 AND ai_first_queue_visible(pr.project_id,pr.report_id,COALESCE($11,'local-user')))) AND (NOT ai_first_audit_masked(pr.project_id,pr.report_id) OR ai_first_queue_visible(pr.project_id,pr.report_id,COALESCE($11,'local-user')))
          AND ($3::text IS NULL OR lower(coalesce(r.title,'') || ' ' || coalesce(r.abstract_text,'')) LIKE '%' || lower($3) || '%')
          AND (
            NOT $4::boolean
            OR CASE $5
              WHEN 'created_asc' THEN (ai_first_queue_time(pr.project_id,pr.report_id,pr.created_at), r.id) > ($6::timestamptz, $7::uuid)
              WHEN 'created_desc' THEN (ai_first_queue_time(pr.project_id,pr.report_id,pr.created_at), r.id) < ($6::timestamptz, $7::uuid)
              WHEN 'title_asc' THEN (lower(coalesce(r.title,'')), r.id) > (lower(coalesce($8,'')), $7::uuid)
              WHEN 'title_desc' THEN (lower(coalesce(r.title,'')), r.id) < (lower(coalesce($8,'')), $7::uuid)
              WHEN 'year_asc' THEN (coalesce(r.publication_year,-2147483648), r.id) > (coalesce($9::int,-2147483648), $7::uuid)
              WHEN 'year_desc' THEN (coalesce(r.publication_year,-2147483648), r.id) < (coalesce($9::int,-2147483648), $7::uuid)
              ELSE false
            END
          )
        ORDER BY
          CASE WHEN $5='created_asc' THEN ai_first_queue_time(pr.project_id,pr.report_id,pr.created_at) END ASC,
          CASE WHEN $5='created_desc' THEN ai_first_queue_time(pr.project_id,pr.report_id,pr.created_at) END DESC,
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
    .bind(query.reviewer_id.as_deref())
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
        SELECT ai_first_queue_time(pr.project_id,pr.report_id,pr.created_at) AS queue_created_at,
               lower(coalesce(r.title, '')) AS normalized_title,
               coalesce(r.publication_year, -2147483648) AS normalized_year
        FROM project_reports pr
        JOIN reports r ON r.id = pr.report_id
        LEFT JOIN screening_state ss ON ss.project_id = pr.project_id AND ss.report_id = pr.report_id
        WHERE pr.project_id = $1 AND pr.report_id = $2
          AND ($3='all' OR (CASE WHEN ai_first_audit_masked(pr.project_id,pr.report_id) THEN 'unscreened' ELSE coalesce(ss.title_abstract_status,'unscreened') END=$3 AND ai_first_queue_visible(pr.project_id,pr.report_id,COALESCE($5,'local-user')))) AND (NOT ai_first_audit_masked(pr.project_id,pr.report_id) OR ai_first_queue_visible(pr.project_id,pr.report_id,COALESCE($5,'local-user')))
          AND ($4::text IS NULL OR lower(coalesce(r.title, '') || ' ' || coalesce(r.abstract_text, '')) LIKE '%' || lower($4) || '%')
        "#,
    )
    .bind(Uuid::from(query.project_id))
    .bind(cursor.report_id)
    .bind(status)
    .bind(search)
    .bind(query.reviewer_id.as_deref())
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
    reviewer: Option<&str>,
) -> Result<i64, ScreeningError> {
    Ok(sqlx::query_scalar(
        "SELECT count(*)::bigint FROM project_reports pr JOIN reports r ON r.id=pr.report_id LEFT JOIN screening_state ss ON ss.project_id=pr.project_id AND ss.report_id=pr.report_id WHERE pr.project_id=$1 AND ($2='all' OR (CASE WHEN ai_first_audit_masked(pr.project_id,pr.report_id) THEN 'unscreened' ELSE coalesce(ss.title_abstract_status,'unscreened') END=$2 AND ai_first_queue_visible(pr.project_id,pr.report_id,COALESCE($4,'local-user')))) AND (NOT ai_first_audit_masked(pr.project_id,pr.report_id) OR ai_first_queue_visible(pr.project_id,pr.report_id,COALESCE($4,'local-user'))) AND ($3::text IS NULL OR lower(coalesce(r.title,'') || ' ' || coalesce(r.abstract_text,'')) LIKE '%' || lower($3) || '%')",
    )
    .bind(project_id)
    .bind(status)
    .bind(search)
    .bind(reviewer)
    .fetch_one(pool)
    .await?)
}

async fn queue_progress(
    pool: &PgPool,
    project_id: Uuid,
) -> Result<ScreeningProgress, ScreeningError> {
    let row = sqlx::query(
        "SELECT count(*)::bigint AS total, count(*) FILTER (WHERE coalesce(ss.title_abstract_status,'unscreened') <> 'unscreened')::bigint AS screened, count(*) FILTER (WHERE coalesce(ss.title_abstract_status,'unscreened')='unscreened')::bigint AS unscreened, count(*) FILTER (WHERE ss.title_abstract_status='include')::bigint AS included, count(*) FILTER (WHERE ss.title_abstract_status='exclude')::bigint AS excluded, count(*) FILTER (WHERE ss.title_abstract_status='maybe')::bigint AS maybe FROM project_reports pr LEFT JOIN screening_state raw ON raw.project_id=pr.project_id AND raw.report_id=pr.report_id LEFT JOIN LATERAL (SELECT CASE WHEN ai_first_audit_masked(pr.project_id,pr.report_id) THEN 'unscreened' ELSE raw.title_abstract_status END AS title_abstract_status) ss ON true WHERE pr.project_id=$1",
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
        title_abstract_status: deepref_domain::ScreeningStatus::Unscreened,
        full_text_status: deepref_domain::ScreeningStatus::NotRequired,
        full_text_exclusion_reason_id: None,
        final_status: deepref_domain::ScreeningStatus::Unscreened,
        revision: 0,
        last_event_id: None,
        updated_at: None,
    }
}

pub(crate) fn state_snapshot_from_row(row: &sqlx::postgres::PgRow) -> Result<ScreeningStateSnapshot, ScreeningError> {
    Ok(ScreeningStateSnapshot {
        project_id: row.get("project_id"),
        report_id: row.get("report_id"),
        title_abstract_status: status_from_row(row, "title_abstract_status")?,
        full_text_status: status_from_row(row, "full_text_status")?,
        full_text_exclusion_reason_id: row.get("full_text_exclusion_reason_id"),
        final_status: status_from_row(row, "final_status")?,
        revision: row.get("revision"),
        last_event_id: row.get("last_event_id"),
        updated_at: row.get("updated_at"),
    })
}

fn state_snapshot_from_domain(
    project_id: Uuid,
    report_id: Uuid,
    revision: i64,
    event_id: Uuid,
    state: CurrentScreeningState,
) -> ScreeningStateSnapshot {
    use deepref_domain::ScreeningStatus;
    let title = state.title_abstract.map(status_from_decision).unwrap_or(ScreeningStatus::Unscreened);
    let full_text = state.full_text.map(status_from_decision).unwrap_or(ScreeningStatus::NotRequired);
    let final_status = match (state.title_abstract, state.full_text) {
        (Some(ScreeningDecision::Include), Some(decision)) => status_from_decision(decision),
        (Some(ScreeningDecision::Include), None) => ScreeningStatus::PendingFullText,
        (Some(ScreeningDecision::Exclude), _) => ScreeningStatus::Exclude,
        (Some(ScreeningDecision::Maybe), _) => ScreeningStatus::Maybe,
        (None, _) => ScreeningStatus::Unscreened,
    };
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

fn queue_item_from_row(row: &sqlx::postgres::PgRow) -> Result<ScreeningQueueItem, ScreeningError> {
    Ok(ScreeningQueueItem {
        report_id: row.get("report_id"),
        title: row.get("title"),
        abstract_text: row.get("abstract_text"),
        doi: row.get("doi"),
        publication_year: row.get("publication_year"),
        title_abstract_status: status_from_row(row, "title_abstract_status")?,
        full_text_status: status_from_row(row, "full_text_status")?,
        final_status: status_from_row(row, "final_status")?,
        revision: row.get("revision"),
    })
}

fn history_item_from_row(row: sqlx::postgres::PgRow) -> Result<ScreeningHistoryItem, ScreeningError> {
    let stage: String = row.try_get("stage")?;
    let decision: Option<String> = row.try_get("decision")?;
    Ok(ScreeningHistoryItem {
        id: row.get("id"),
        event_kind: row.get("event_kind"),
        stage: ScreeningStage::parse(&stage).ok_or_else(|| ScreeningError::InvalidData(format!("unknown screening stage {stage:?}")))?,
        decision: decision.map(|value| ScreeningDecision::parse(&value).ok_or_else(|| ScreeningError::InvalidData(format!("unknown screening decision {value:?}")))).transpose()?,
        notes: row.get("notes"),
        protocol_version_id: row.get("protocol_version_id"),
        actor_kind: row.get("actor_kind"),
        actor_id: row.get("actor_id"),
        supersedes_event_id: row.get("supersedes_event_id"),
        undoes_event_id: row.get("undoes_event_id"),
        created_at: row.get("created_at"),
        previous_title_abstract_status: status_from_row(&row, "previous_title_abstract_status")?,
        previous_full_text_status: status_from_row(&row, "previous_full_text_status")?,
        previous_full_text_exclusion_reason_id: row.get("previous_full_text_exclusion_reason_id"),
        previous_final_status: status_from_row(&row, "previous_final_status")?,
        result_title_abstract_status: status_from_row(&row, "result_title_abstract_status")?,
        result_full_text_status: status_from_row(&row, "result_full_text_status")?,
        result_full_text_exclusion_reason_id: row.get("result_full_text_exclusion_reason_id"),
        result_final_status: status_from_row(&row, "result_final_status")?,
    })
}

pub(crate) fn status_from_row(row: &sqlx::postgres::PgRow, column: &str) -> Result<deepref_domain::ScreeningStatus, ScreeningError> {
    let value: String = row.try_get(column)?;
    parse_status(&value, column)
}

pub(crate) fn parse_status(value: &str, column: &str) -> Result<deepref_domain::ScreeningStatus, ScreeningError> {
    let status = deepref_domain::ScreeningStatus::parse(value).ok_or_else(|| ScreeningError::InvalidData(format!("unknown {column} {value:?}")))?;
    let valid = if column.ends_with("title_abstract_status") {
        !matches!(status, deepref_domain::ScreeningStatus::NotRequired | deepref_domain::ScreeningStatus::PendingFullText)
    } else if column.ends_with("full_text_status") {
        status != deepref_domain::ScreeningStatus::PendingFullText
    } else {
        status != deepref_domain::ScreeningStatus::NotRequired
    };
    if !valid {
        return Err(ScreeningError::InvalidData(format!("invalid {column} {value:?}")));
    }
    Ok(status)
}

fn status_from_decision(decision: ScreeningDecision) -> deepref_domain::ScreeningStatus {
    match decision {
        ScreeningDecision::Include => deepref_domain::ScreeningStatus::Include,
        ScreeningDecision::Exclude => deepref_domain::ScreeningStatus::Exclude,
        ScreeningDecision::Maybe => deepref_domain::ScreeningStatus::Maybe,
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

fn decision_from_status(status: &deepref_domain::ScreeningStatus) -> Result<Option<ScreeningDecision>, ScreeningError> {
    use deepref_domain::ScreeningStatus;
    match status {
        ScreeningStatus::Include => Ok(Some(ScreeningDecision::Include)),
        ScreeningStatus::Exclude => Ok(Some(ScreeningDecision::Exclude)),
        ScreeningStatus::Maybe => Ok(Some(ScreeningDecision::Maybe)),
        ScreeningStatus::Unscreened | ScreeningStatus::NotRequired => Ok(None),
        ScreeningStatus::PendingFullText => Err(ScreeningError::InvalidData("pending_full_text is not a screening decision".into())),
    }
}


fn lifecycle_for_status(status: &deepref_domain::ScreeningStatus) -> &'static str {
    use deepref_domain::ScreeningStatus;
    match status {
        ScreeningStatus::Include => "included",
        ScreeningStatus::Exclude => "excluded",
        ScreeningStatus::Maybe => "maybe",
        ScreeningStatus::Unscreened | ScreeningStatus::NotRequired | ScreeningStatus::PendingFullText => "screening",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use deepref_application::{ScreeningQueueSort, ScreeningQueueStatus};

    #[test]
    fn stored_statuses_reject_unknown_and_inapplicable_states() {
        use deepref_domain::ScreeningStatus;
        assert_eq!(parse_status("include", "title_abstract_status").unwrap(), ScreeningStatus::Include);
        assert_eq!(parse_status("not_required", "full_text_status").unwrap(), ScreeningStatus::NotRequired);
        assert_eq!(parse_status("pending_full_text", "final_status").unwrap(), ScreeningStatus::PendingFullText);
        for (value, column) in [
            ("unknown", "final_status"),
            ("not_required", "title_abstract_status"),
            ("pending_full_text", "title_abstract_status"),
            ("pending_full_text", "full_text_status"),
            ("not_required", "final_status"),
        ] {
            assert!(matches!(parse_status(value, column), Err(ScreeningError::InvalidData(_))));
        }
    }

    #[test]
    fn cursor_round_trip_uses_an_opaque_url_safe_value() {
        let query = GetScreeningQueueQuery {
            reviewer_id: None,
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

/// Every change made by a confirmed plan lands in the activity feed, grouped
/// by plan so the whole plan can be undone at once.
#[allow(clippy::too_many_arguments)]
pub async fn record_assistant_screening_activity(
    pool: &PgPool,
    project_id: ProjectId,
    plan_id: Uuid,
    model: &str,
    prompt_version: &str,
    actor: &Actor,
    report_id: Uuid,
    decision: ScreeningDecision,
    event_id: Option<Uuid>,
    protocol_version_id: Uuid,
    evidence: &Value,
) {
    let Some(event_id) = event_id else {
        return;
    };
    let title = sqlx::query_scalar::<_, Option<String>>("SELECT title FROM reports WHERE id=$1")
        .bind(report_id)
        .fetch_optional(pool)
        .await
        .ok()
        .flatten()
        .flatten()
        .map(|title| crate::decode_html_entities(&title))
        .unwrap_or_else(|| "Untitled record".to_owned());
    let verb = match decision {
        ScreeningDecision::Include => "included",
        ScreeningDecision::Exclude => "excluded",
        ScreeningDecision::Maybe => "marked as maybe",
    };
    let shown: String = title.chars().take(120).collect();
    let mut entry = crate::NewActivity::new(
        project_id.as_uuid(),
        "assistant",
        "Assistant",
        actor.clone(),
        "title_abstract_screening",
        "screening_decision",
        format!("The assistant {verb} “{shown}” after you confirmed its plan."),
    );
    entry.affected = json!([{"type": "report", "id": report_id, "label": title}]);
    entry.after_state = json!({
        "report_id": report_id,
        "stage": "title_abstract",
        "event_id": event_id,
        "protocol_version_id": protocol_version_id,
    });
    entry.undo_kind = Some("screening_event");
    entry.batch_id = Some(plan_id);
    entry.model = Some(model.to_owned());
    entry.prompt_version = Some(prompt_version.to_owned());
    entry.evidence = evidence.clone();
    if let Err(error) = crate::record_activity(pool, &entry).await {
        tracing::warn!(%error, "failed to record assistant activity");
    }
}


pub async fn get_agent_screening_state(
    pool: &PgPool,
    project_id: Uuid,
    report_id: Uuid,
) -> Result<crate::screening::ScreeningStateSnapshot, crate::AgentReadError> {
    let row = sqlx::query(
        "SELECT pr.project_id,pr.report_id,CASE WHEN ai_first_audit_masked(pr.project_id,pr.report_id) THEN 'unscreened' ELSE coalesce(ss.title_abstract_status,'unscreened') END AS title_abstract_status,
                CASE WHEN ai_first_audit_masked(pr.project_id,pr.report_id) THEN 'not_required' ELSE coalesce(ss.full_text_status,'not_required') END AS full_text_status,
                CASE WHEN ai_first_audit_masked(pr.project_id,pr.report_id) THEN NULL ELSE ss.full_text_exclusion_reason_id END AS full_text_exclusion_reason_id,
                CASE WHEN ai_first_audit_masked(pr.project_id,pr.report_id) THEN 'unscreened' ELSE coalesce(ss.final_status,'unscreened') END AS final_status,
                CASE WHEN ai_first_audit_masked(pr.project_id,pr.report_id) THEN 0 ELSE coalesce(ss.revision,0) END::bigint AS revision, CASE WHEN ai_first_audit_masked(pr.project_id,pr.report_id) THEN NULL ELSE ss.last_event_id END AS last_event_id, CASE WHEN ai_first_audit_masked(pr.project_id,pr.report_id) THEN NULL ELSE ss.updated_at END AS updated_at
         FROM project_reports pr
         LEFT JOIN screening_state ss ON ss.project_id=pr.project_id AND ss.report_id=pr.report_id
         WHERE pr.project_id=$1 AND pr.report_id=$2",
    )
    .bind(project_id)
    .bind(report_id)
    .fetch_optional(pool)
    .await?
    .ok_or(crate::AgentReadError::NotFound)?;
    state_snapshot_from_row(&row).map_err(|error| match error {
        ScreeningError::Database(error) => crate::AgentReadError::Database(error),
        error => crate::AgentReadError::InvalidStoredValue(error.to_string()),
    })
}


/// One record of a planned `screen_reports` action as it is now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScreeningPlanSnapshot {
    pub report_id: Uuid,
    pub title: String,
    pub title_abstract_status: deepref_domain::ScreeningStatus,
    pub revision: i64,
    /// The record sits in a blind AI-first audit; only a person may screen it.
    pub audit_masked: bool,
}

const MAX_SNAPSHOT_TITLE_CHARS: usize = 300;

/// Reads the current title, title/abstract status and screening revision of
/// each report, in the order given. Reports that are not in the project are
/// missing from the result. Blind-audit records read as unscreened, exactly
/// like every other assistant read.
pub async fn snapshot_screening_plan_items(
    pool: &PgPool,
    project_id: Uuid,
    report_ids: &[Uuid],
) -> Result<Vec<ScreeningPlanSnapshot>, crate::AssistantError> {
    let rows = sqlx::query(
        "SELECT ids.report_id, r.title,
                ai_first_audit_masked(pr.project_id, pr.report_id) AS masked,
                COALESCE(s.title_abstract_status, 'unscreened') AS status,
                COALESCE(s.revision, 0) AS revision
         FROM unnest($2::uuid[]) WITH ORDINALITY AS ids(report_id, ordinal)
         JOIN project_reports pr ON pr.project_id = $1 AND pr.report_id = ids.report_id
         JOIN reports r ON r.id = ids.report_id
         LEFT JOIN screening_state s ON s.project_id = pr.project_id AND s.report_id = pr.report_id
         ORDER BY ids.ordinal",
    )
    .bind(project_id)
    .bind(report_ids)
    .fetch_all(pool)
    .await?;
    rows
        .iter()
        .map(|row| {
            let masked: bool = row.get("masked");
            let stored_status: String = row.try_get("status")?;
            let status = deepref_domain::ScreeningStatus::parse(&stored_status).ok_or_else(|| {
                crate::AssistantError::InvalidInput(format!("unknown screening status {stored_status}"))
            })?;
            let title = row
                .get::<Option<String>, _>("title")
                .map(|title| crate::decode_html_entities(&title))
                .filter(|title| !title.trim().is_empty())
                .unwrap_or_else(|| "Untitled record".to_owned());
            Ok(ScreeningPlanSnapshot {
                report_id: row.get("report_id"),
                title: title.chars().take(MAX_SNAPSHOT_TITLE_CHARS).collect(),
                title_abstract_status: if masked {
                    deepref_domain::ScreeningStatus::Unscreened
                } else {
                    status
                },
                revision: row.get("revision"),
                audit_masked: masked,
            })
        })
        .collect()
}


/// Lists reports with their title/abstract screening status. `status` is one
/// of `unscreened`, `include`, `exclude`, `maybe`, or `None` for all.
pub async fn list_agent_reports_by_screening(
    pool: &PgPool,
    project_id: Uuid,
    status: Option<&str>,
    limit: i64,
) -> Result<Value, crate::AgentReadError> {
    if !matches!(
        status,
        None | Some("unscreened" | "include" | "exclude" | "maybe")
    ) {
        return Err(crate::AgentReadError::InvalidData);
    }
    let rows = sqlx::query(
        "SELECT r.id, r.title, r.publication_year, r.journal,
                CASE WHEN ai_first_audit_masked(pr.project_id,pr.report_id) THEN 'unscreened' ELSE COALESCE(s.title_abstract_status,'unscreened') END AS status,
                CASE WHEN ai_first_audit_masked(pr.project_id,pr.report_id) THEN 'unscreened' ELSE COALESCE(s.final_status,'unscreened') END AS final_status
         FROM project_reports pr
         JOIN reports r ON r.id=pr.report_id
         LEFT JOIN screening_state s ON s.project_id=pr.project_id AND s.report_id=pr.report_id
         WHERE pr.project_id=$1
           AND ($2::text IS NULL OR CASE WHEN ai_first_audit_masked(pr.project_id,pr.report_id) THEN 'unscreened' ELSE COALESCE(s.title_abstract_status,'unscreened') END=$2)
         ORDER BY pr.created_at, r.id LIMIT $3",
    )
    .bind(project_id)
    .bind(status)
    .bind(limit.clamp(1, 50))
    .fetch_all(pool)
    .await?;
    Ok(Value::Array(
        rows.into_iter()
            .map(|row| {
                let status = deepref_domain::ScreeningStatus::parse(row.try_get("status")?)
                    .ok_or(crate::AgentReadError::InvalidData)?;
                let final_status = deepref_domain::ScreeningStatus::parse(row.try_get("final_status")?)
                    .ok_or(crate::AgentReadError::InvalidData)?;
                Ok(serde_json::json!({
                    "report_id": row.get::<Uuid, _>("id"),
                    "title": row.get::<Option<String>, _>("title").map(|title| crate::decode_html_entities(&title)),
                    "year": row.get::<Option<i32>, _>("publication_year"),
                    "journal": row.get::<Option<String>, _>("journal"),
                    "title_abstract_status": status.as_str(),
                    "final_status": final_status.as_str(),
                }))
            })
            .collect::<Result<Vec<_>, crate::AgentReadError>>()?,
    ))
}

