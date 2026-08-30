use chrono::{DateTime, Utc};
use deepref_application::{
    AssignReportToStudy, AutomationDomainEvent, ClassifyStudy, CreateStudy, RemoveReportFromStudy,
    RenameStudy,
};
use deepref_domain::{
    AppraisalToolSuggestion, ReportAssignedToStudy, ReportId, ReportRemovedFromStudy,
    StudyClassified, StudyCreated, StudyDesign, StudyDesignContext, StudyEvent, StudyId,
    StudyRenamed, StudyReportRole,
};
use serde_json::{Value, json};
use sqlx::{PgPool, Row, postgres::PgRow};
use thiserror::Error;
use uuid::Uuid;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StudyReportRecord {
    pub report_id: ReportId,
    pub title: Option<String>,
    pub abstract_text: Option<String>,
    pub publication_year: Option<i32>,
    pub role: StudyReportRole,
    pub assigned_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StudyRecord {
    pub id: StudyId,
    pub project_id: Uuid,
    pub title: String,
    pub design: Option<StudyDesign>,
    pub design_context: StudyDesignContext,
    pub revision: i64,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub updated_by_actor_kind: String,
    pub updated_by_actor_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StudyDetailRecord {
    pub study: StudyRecord,
    pub reports: Vec<StudyReportRecord>,
    pub tool_suggestions: Vec<AppraisalToolSuggestion>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StudyMembershipRecord {
    pub study_id: StudyId,
    pub role: StudyReportRole,
    pub study_revision: i64,
    pub study: StudyDetailRecord,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StudyListRecord {
    pub items: Vec<StudyRecord>,
    pub next_cursor: Option<Uuid>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StudyEventRecord {
    pub id: Uuid,
    pub study_id: StudyId,
    pub report_id: Option<ReportId>,
    pub event_type: String,
    pub before_study_id: Option<StudyId>,
    pub result_study_id: Option<StudyId>,
    pub before_revision: i64,
    pub result_revision: i64,
    pub before_snapshot: Value,
    pub result_snapshot: Value,
    pub payload: Value,
    pub actor_kind: String,
    pub actor_id: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Error)]
pub enum StudyError {
    #[error("database operation failed")]
    Database(#[from] sqlx::Error),
    #[error("project not found")]
    ProjectNotFound,
    #[error("study not found")]
    StudyNotFound,
    #[error("report is not part of the project")]
    ReportNotInProject,
    #[error("report is already assigned to the target study")]
    AlreadyMember,
    #[error("report is not assigned to the study")]
    NotMember,
    #[error("study revision conflict")]
    RevisionConflict { current: Box<StudyDetailRecord> },
    #[error("study data integrity failure: {0}")]
    DataIntegrity(String),
}

pub async fn list_studies(
    pool: &PgPool,
    project_id: Uuid,
    cursor: Option<Uuid>,
    limit: i64,
) -> Result<StudyListRecord, StudyError> {
    let rows = sqlx::query(
        "SELECT s.id, s.project_id, s.title, s.design, s.design_context,
                s.study_revision, s.created_at, s.updated_at,
                s.updated_by_actor_kind, s.updated_by_actor_id
         FROM studies s
         WHERE s.project_id = $1
           AND ($2::uuid IS NULL OR (s.created_at, s.id) >
             (SELECT created_at, id FROM studies WHERE project_id = $1 AND id = $2))
         ORDER BY s.created_at, s.id
         LIMIT $3",
    )
    .bind(project_id)
    .bind(cursor)
    .bind(limit + 1)
    .fetch_all(pool)
    .await?;
    let mut items = rows
        .into_iter()
        .map(study_from_row)
        .collect::<Result<Vec<_>, _>>()?;
    let next_cursor = if items.len() > limit as usize {
        items.pop().map(|item| item.id.into())
    } else {
        None
    };
    Ok(StudyListRecord { items, next_cursor })
}

pub async fn get_study(
    pool: &PgPool,
    project_id: Uuid,
    study_id: Uuid,
) -> Result<StudyDetailRecord, StudyError> {
    let mut connection = pool.acquire().await?;
    get_study_with_connection(&mut connection, project_id, study_id).await
}

pub async fn list_study_events(
    pool: &PgPool,
    project_id: Uuid,
    study_id: Uuid,
) -> Result<Vec<StudyEventRecord>, StudyError> {
    let rows = sqlx::query(
        "SELECT id, study_id, report_id, event_type, before_study_id, result_study_id,
                before_revision, result_revision, before_snapshot, result_snapshot, payload,
                actor_kind, actor_id, created_at
         FROM study_events
         WHERE project_id=$1 AND (study_id=$2 OR before_study_id=$2 OR result_study_id=$2)
         ORDER BY created_at, id",
    )
    .bind(project_id)
    .bind(study_id)
    .fetch_all(pool)
    .await?;
    rows.into_iter().map(event_from_row).collect()
}

pub async fn get_study_for_report(
    pool: &PgPool,
    project_id: Uuid,
    report_id: Uuid,
) -> Result<Option<StudyMembershipRecord>, StudyError> {
    ensure_project_report_pool(pool, project_id, report_id).await?;
    let row = sqlx::query(
        "SELECT sr.study_id, sr.relationship, s.study_revision
         FROM study_reports sr
         JOIN studies s ON s.project_id=sr.project_id AND s.id=sr.study_id
         WHERE sr.project_id=$1 AND sr.report_id=$2",
    )
    .bind(project_id)
    .bind(report_id)
    .fetch_optional(pool)
    .await?;
    let Some(row) = row else { return Ok(None) };
    let role_value: String = row.get("relationship");
    let role = StudyReportRole::parse(&role_value)
        .ok_or_else(|| StudyError::DataIntegrity("study report has an unknown role".to_owned()))?;
    let study_id: Uuid = row.get("study_id");
    let study = get_study(pool, project_id, study_id).await?;
    Ok(Some(StudyMembershipRecord {
        study_id: study_id.into(),
        role,
        study_revision: row.get("study_revision"),
        study,
    }))
}

pub async fn create_study(
    pool: &PgPool,
    command: CreateStudy,
) -> Result<StudyDetailRecord, StudyError> {
    let mut transaction = pool.begin().await?;
    ensure_project(&mut transaction, command.project_id.into()).await?;
    sqlx::query(
        "INSERT INTO studies
          (id, project_id, title, design, design_context,
           study_revision, updated_by_actor_kind, updated_by_actor_id)
         VALUES ($1,$2,$3,NULL,'{}'::jsonb,0,$4,$5)",
    )
    .bind(command.study_id.as_uuid())
    .bind(command.project_id.as_uuid())
    .bind(command.title.as_str())
    .bind(command.actor.kind().as_str())
    .bind(command.actor.id())
    .execute(&mut *transaction)
    .await?;
    let study_event_id = insert_study_event(
        &mut transaction,
        command.project_id.into(),
        command.study_id.as_uuid(),
        None,
        "study_created",
        None,
        Some(command.study_id.as_uuid()),
        0,
        0,
        json!({}),
        json!({ "title": command.title.as_str(), "revision": 0 }),
        study_event_payload(StudyEvent::StudyCreated(StudyCreated {
            study_id: command.study_id,
            title: command.title.clone(),
            actor: command.actor.clone(),
        }))?,
        &command.actor,
    )
    .await?;
    crate::dispatch_automation_domain_event(
        &mut transaction,
        &AutomationDomainEvent::StudyCreated {
            project_id: command.project_id,
            study_event_id,
            actor: command.actor.clone(),
        },
    )
    .await?;
    transaction.commit().await?;
    get_study(pool, command.project_id.into(), command.study_id.into()).await
}

pub async fn rename_study(
    pool: &PgPool,
    command: RenameStudy,
) -> Result<StudyDetailRecord, StudyError> {
    let mut transaction = pool.begin().await?;
    let current = lock_study(
        &mut transaction,
        command.project_id.into(),
        command.study_id.into(),
    )
    .await?;
    if current.revision != command.expected_revision as i64 {
        return Err(StudyError::RevisionConflict {
            current: Box::new(
                get_study_with_connection(
                    &mut transaction,
                    command.project_id.into(),
                    command.study_id.into(),
                )
                .await?,
            ),
        });
    }
    let result_revision = current.revision + 1;
    sqlx::query(
        "UPDATE studies SET title=$1, study_revision=$2,
             updated_by_actor_kind=$3, updated_by_actor_id=$4, updated_at=now()
         WHERE project_id=$5 AND id=$6",
    )
    .bind(command.title.as_str())
    .bind(result_revision)
    .bind(command.actor.kind().as_str())
    .bind(command.actor.id())
    .bind(command.project_id.as_uuid())
    .bind(command.study_id.as_uuid())
    .execute(&mut *transaction)
    .await?;
    insert_study_event(
        &mut transaction,
        command.project_id.into(),
        command.study_id.as_uuid(),
        None,
        "study_renamed",
        Some(command.study_id.as_uuid()),
        Some(command.study_id.as_uuid()),
        current.revision,
        result_revision,
        json!({ "title": current.title, "revision": current.revision }),
        json!({ "title": command.title.as_str(), "revision": result_revision }),
        study_event_payload(StudyEvent::StudyRenamed(StudyRenamed {
            study_id: command.study_id,
            title: command.title.clone(),
            before_revision: current.revision as u64,
            result_revision: result_revision as u64,
            actor: command.actor.clone(),
        }))?,
        &command.actor,
    )
    .await?;
    transaction.commit().await?;
    get_study(pool, command.project_id.into(), command.study_id.into()).await
}

pub async fn classify_study(
    pool: &PgPool,
    command: ClassifyStudy,
) -> Result<StudyDetailRecord, StudyError> {
    let mut transaction = pool.begin().await?;
    classify_study_in_transaction(&mut transaction, command.clone()).await?;
    transaction.commit().await?;
    get_study(pool, command.project_id.into(), command.study_id.into()).await
}

pub(crate) async fn classify_study_in_transaction(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    command: ClassifyStudy,
) -> Result<i64, StudyError> {
    let current = lock_study(
        transaction,
        command.project_id.into(),
        command.study_id.into(),
    )
    .await?;
    apply_classification_in_transaction(transaction, command, current).await
}

pub(crate) async fn apply_classification_in_transaction(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    command: ClassifyStudy,
    current: LockedStudy,
) -> Result<i64, StudyError> {
    if current.revision != command.expected_revision as i64 {
        return Err(StudyError::RevisionConflict {
            current: Box::new(
                get_study_with_connection(
                    transaction,
                    command.project_id.into(),
                    command.study_id.into(),
                )
                .await?,
            ),
        });
    }
    let result_revision = current.revision + 1;
    let context = serde_json::to_value(command.context)
        .map_err(|error| StudyError::DataIntegrity(error.to_string()))?;
    sqlx::query(
        "UPDATE studies SET design=$1, design_context=$2,
             study_revision=$3, updated_by_actor_kind=$4, updated_by_actor_id=$5,
             classified_at=now(), updated_at=now()
         WHERE project_id=$6 AND id=$7",
    )
    .bind(command.design.as_str())
    .bind(context)
    .bind(result_revision)
    .bind(command.actor.kind().as_str())
    .bind(command.actor.id())
    .bind(command.project_id.as_uuid())
    .bind(command.study_id.as_uuid())
    .execute(&mut **transaction)
    .await?;
    insert_study_event(
        transaction,
        command.project_id.into(),
        command.study_id.as_uuid(),
        None,
        "study_classified",
        Some(command.study_id.as_uuid()),
        Some(command.study_id.as_uuid()),
        current.revision,
        result_revision,
        json!({ "design": current.design.map(StudyDesign::as_str), "context": current.design_context }),
        json!({ "design": command.design.as_str(), "context": command.context, "revision": result_revision }),
        study_event_payload(StudyEvent::StudyClassified(StudyClassified {
            study_id: command.study_id,
            previous_design: current.design,
            design: command.design,
            context: command.context,
            before_revision: current.revision as u64,
            result_revision: result_revision as u64,
            actor: command.actor.clone(),
        }))?,
        &command.actor,
    )
    .await?;
    Ok(result_revision)
}

pub async fn assign_report_to_study(
    pool: &PgPool,
    command: AssignReportToStudy,
) -> Result<StudyDetailRecord, StudyError> {
    let mut transaction = pool.begin().await?;
    assign_report_to_study_in_transaction(&mut transaction, command.clone()).await?;
    transaction.commit().await?;
    get_study(pool, command.project_id.into(), command.study_id.into()).await
}

pub async fn assign_report_to_study_in_transaction(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    command: AssignReportToStudy,
) -> Result<(), StudyError> {
    ensure_project_report(
        transaction,
        command.project_id.into(),
        command.report_id.into(),
    )
    .await?;
    let membership = sqlx::query(
        "SELECT study_id, relationship FROM study_reports
         WHERE project_id=$1 AND report_id=$2 FOR UPDATE",
    )
    .bind(command.project_id.as_uuid())
    .bind(command.report_id.as_uuid())
    .fetch_optional(&mut **transaction)
    .await?;
    let previous_study_id = membership
        .as_ref()
        .map(|row| row.get::<Uuid, _>("study_id"));
    if command
        .expected_previous_study_id
        .map(Into::into)
        .is_some_and(|id| previous_study_id != Some(id))
        || (previous_study_id.is_none() && command.expected_previous_study_revision.is_some())
    {
        return Err(StudyError::RevisionConflict {
            current: Box::new(
                get_study_with_connection(
                    transaction,
                    command.project_id.into(),
                    command.study_id.into(),
                )
                .await?,
            ),
        });
    }
    if previous_study_id == Some(command.study_id.as_uuid()) {
        return Err(StudyError::AlreadyMember);
    }
    let mut study_ids = vec![command.study_id.as_uuid()];
    if let Some(previous_id) = previous_study_id {
        study_ids.push(previous_id);
    }
    study_ids.sort_unstable();
    study_ids.dedup();
    let mut locked_studies = Vec::with_capacity(study_ids.len());
    for study_id in study_ids {
        locked_studies.push(lock_study(transaction, command.project_id.into(), study_id).await?);
    }
    let target = locked_studies
        .iter()
        .find(|study| study.id.as_uuid() == command.study_id.as_uuid())
        .cloned()
        .ok_or(StudyError::StudyNotFound)?;
    if target.revision != command.expected_revision as i64 {
        return Err(StudyError::RevisionConflict {
            current: Box::new(
                get_study_with_connection(
                    transaction,
                    command.project_id.into(),
                    command.study_id.into(),
                )
                .await?,
            ),
        });
    }
    let previous = if let Some(previous_id) = previous_study_id {
        let previous = locked_studies
            .iter()
            .find(|study| study.id.as_uuid() == previous_id)
            .cloned()
            .ok_or(StudyError::StudyNotFound)?;
        if command.expected_previous_study_revision != Some(previous.revision as u64) {
            return Err(StudyError::RevisionConflict {
                current: Box::new(
                    get_study_with_connection(transaction, command.project_id.into(), previous_id)
                        .await?,
                ),
            });
        }
        Some(previous)
    } else {
        None
    };
    let target_revision = target.revision + 1;
    if let Some(previous) = &previous {
        sqlx::query("DELETE FROM study_reports WHERE project_id=$1 AND report_id=$2")
            .bind(command.project_id.as_uuid())
            .bind(command.report_id.as_uuid())
            .execute(&mut **transaction)
            .await?;
        sqlx::query(
            "UPDATE studies SET study_revision=$1, updated_by_actor_kind=$2,
                 updated_by_actor_id=$3, updated_at=now()
             WHERE project_id=$4 AND id=$5",
        )
        .bind(previous.revision + 1)
        .bind(command.actor.kind().as_str())
        .bind(command.actor.id())
        .bind(command.project_id.as_uuid())
        .bind(previous.id.as_uuid())
        .execute(&mut **transaction)
        .await?;
    }
    sqlx::query(
        "INSERT INTO study_reports(project_id,study_id,report_id,relationship)
         VALUES ($1,$2,$3,$4)",
    )
    .bind(command.project_id.as_uuid())
    .bind(command.study_id.as_uuid())
    .bind(command.report_id.as_uuid())
    .bind(command.role.as_str())
    .execute(&mut **transaction)
    .await?;
    sqlx::query(
        "UPDATE studies SET study_revision=$1, updated_by_actor_kind=$2,
             updated_by_actor_id=$3, updated_at=now()
         WHERE project_id=$4 AND id=$5",
    )
    .bind(target_revision)
    .bind(command.actor.kind().as_str())
    .bind(command.actor.id())
    .bind(command.project_id.as_uuid())
    .bind(command.study_id.as_uuid())
    .execute(&mut **transaction)
    .await?;
    let event_type = if previous.is_some() {
        "report_moved"
    } else {
        "report_assigned"
    };
    insert_study_event(
        transaction,
        command.project_id.into(),
        command.study_id.as_uuid(),
        Some(command.report_id.as_uuid()),
        event_type,
        previous_study_id,
        Some(command.study_id.as_uuid()),
        target.revision,
        target_revision,
        json!({ "role": membership.as_ref().map(|row| row.get::<String, _>("relationship")), "source_revision": previous.as_ref().map(|study| study.revision) }),
        json!({ "role": command.role.as_str(), "target_revision": target_revision }),
        study_event_payload(StudyEvent::ReportAssignedToStudy(ReportAssignedToStudy {
            study_id: command.study_id,
            report_id: command.report_id,
            previous_study_id: previous_study_id.map(Into::into),
            role: command.role,
            before_revision: target.revision as u64,
            result_revision: target_revision as u64,
            actor: command.actor.clone(),
        }))?,
        &command.actor,
    )
    .await?;
    Ok(())
}

pub async fn create_study_and_assign_report_in_transaction(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    command: CreateStudy,
    report_id: ReportId,
    role: StudyReportRole,
    expected_previous_study_id: Option<StudyId>,
    expected_previous_study_revision: Option<u64>,
) -> Result<(), StudyError> {
    ensure_project(transaction, command.project_id.into()).await?;
    sqlx::query(
        "INSERT INTO studies
          (id, project_id, title, design, design_context,
           study_revision, updated_by_actor_kind, updated_by_actor_id)
         VALUES ($1,$2,$3,NULL,'{}'::jsonb,0,$4,$5)",
    )
    .bind(command.study_id.as_uuid())
    .bind(command.project_id.as_uuid())
    .bind(command.title.as_str())
    .bind(command.actor.kind().as_str())
    .bind(command.actor.id())
    .execute(&mut **transaction)
    .await?;
    let study_event_id = insert_study_event(
        transaction,
        command.project_id.into(),
        command.study_id.as_uuid(),
        None,
        "study_created",
        None,
        Some(command.study_id.as_uuid()),
        0,
        0,
        json!({}),
        json!({ "title": command.title.as_str(), "revision": 0 }),
        study_event_payload(StudyEvent::StudyCreated(StudyCreated {
            study_id: command.study_id,
            title: command.title.clone(),
            actor: command.actor.clone(),
        }))?,
        &command.actor,
    )
    .await?;
    crate::dispatch_automation_domain_event(
        transaction,
        &AutomationDomainEvent::StudyCreated {
            project_id: command.project_id,
            study_event_id,
            actor: command.actor.clone(),
        },
    )
    .await?;
    assign_report_to_study_in_transaction(
        transaction,
        AssignReportToStudy {
            project_id: command.project_id,
            study_id: command.study_id,
            report_id,
            role,
            expected_revision: 0,
            expected_previous_study_id,
            expected_previous_study_revision,
            actor: command.actor,
        },
    )
    .await
}

pub async fn remove_report_from_study(
    pool: &PgPool,
    command: RemoveReportFromStudy,
) -> Result<StudyDetailRecord, StudyError> {
    let mut transaction = pool.begin().await?;
    let current = lock_study(
        &mut transaction,
        command.project_id.into(),
        command.study_id.into(),
    )
    .await?;
    if current.revision != command.expected_revision as i64 {
        return Err(StudyError::RevisionConflict {
            current: Box::new(
                get_study_with_connection(
                    &mut transaction,
                    command.project_id.into(),
                    command.study_id.into(),
                )
                .await?,
            ),
        });
    }
    let deleted = sqlx::query(
        "DELETE FROM study_reports WHERE project_id=$1 AND study_id=$2 AND report_id=$3",
    )
    .bind(command.project_id.as_uuid())
    .bind(command.study_id.as_uuid())
    .bind(command.report_id.as_uuid())
    .execute(&mut *transaction)
    .await?;
    if deleted.rows_affected() == 0 {
        return Err(StudyError::NotMember);
    }
    let result_revision = current.revision + 1;
    sqlx::query(
        "UPDATE studies SET study_revision=$1, updated_by_actor_kind=$2,
             updated_by_actor_id=$3, updated_at=now()
         WHERE project_id=$4 AND id=$5",
    )
    .bind(result_revision)
    .bind(command.actor.kind().as_str())
    .bind(command.actor.id())
    .bind(command.project_id.as_uuid())
    .bind(command.study_id.as_uuid())
    .execute(&mut *transaction)
    .await?;
    insert_study_event(
        &mut transaction,
        command.project_id.into(),
        command.study_id.as_uuid(),
        Some(command.report_id.as_uuid()),
        "report_unassigned",
        Some(command.study_id.as_uuid()),
        None,
        current.revision,
        result_revision,
        json!({ "report_id": command.report_id, "revision": current.revision }),
        json!({ "report_id": command.report_id, "revision": result_revision }),
        study_event_payload(StudyEvent::ReportRemovedFromStudy(ReportRemovedFromStudy {
            study_id: command.study_id,
            report_id: command.report_id,
            result_revision: result_revision as u64,
            actor: command.actor.clone(),
        }))?,
        &command.actor,
    )
    .await?;
    transaction.commit().await?;
    get_study(pool, command.project_id.into(), command.study_id.into()).await
}

async fn ensure_project(
    connection: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    project_id: Uuid,
) -> Result<(), StudyError> {
    let exists = sqlx::query_scalar::<_, bool>("SELECT EXISTS(SELECT 1 FROM projects WHERE id=$1)")
        .bind(project_id)
        .fetch_one(&mut **connection)
        .await?;
    if exists {
        Ok(())
    } else {
        Err(StudyError::ProjectNotFound)
    }
}

async fn ensure_project_report(
    connection: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    project_id: Uuid,
    report_id: Uuid,
) -> Result<(), StudyError> {
    let exists = sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS(SELECT 1 FROM project_reports WHERE project_id=$1 AND report_id=$2)",
    )
    .bind(project_id)
    .bind(report_id)
    .fetch_one(&mut **connection)
    .await?;
    if exists {
        Ok(())
    } else {
        Err(StudyError::ReportNotInProject)
    }
}

async fn ensure_project_report_pool(
    pool: &PgPool,
    project_id: Uuid,
    report_id: Uuid,
) -> Result<(), StudyError> {
    let exists = sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS(SELECT 1 FROM project_reports WHERE project_id=$1 AND report_id=$2)",
    )
    .bind(project_id)
    .bind(report_id)
    .fetch_one(pool)
    .await?;
    if exists {
        Ok(())
    } else {
        Err(StudyError::ReportNotInProject)
    }
}

#[derive(Debug, Clone)]
pub(crate) struct LockedStudy {
    pub(crate) id: StudyId,
    pub(crate) title: String,
    pub(crate) design: Option<StudyDesign>,
    pub(crate) design_context: StudyDesignContext,
    pub(crate) revision: i64,
}

pub(crate) async fn lock_study(
    connection: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    project_id: Uuid,
    study_id: Uuid,
) -> Result<LockedStudy, StudyError> {
    let row = sqlx::query(
        "SELECT id, title, design, design_context, study_revision
         FROM studies WHERE project_id=$1 AND id=$2 FOR UPDATE",
    )
    .bind(project_id)
    .bind(study_id)
    .fetch_optional(&mut **connection)
    .await?
    .ok_or(StudyError::StudyNotFound)?;
    locked_study_from_row(row)
}

async fn get_study_with_connection(
    connection: &mut sqlx::PgConnection,
    project_id: Uuid,
    study_id: Uuid,
) -> Result<StudyDetailRecord, StudyError> {
    let study_row = sqlx::query(
        "SELECT id, project_id, title, design, design_context,
                study_revision, created_at, updated_at,
                updated_by_actor_kind, updated_by_actor_id
         FROM studies WHERE project_id=$1 AND id=$2",
    )
    .bind(project_id)
    .bind(study_id)
    .fetch_optional(&mut *connection)
    .await?
    .ok_or(StudyError::StudyNotFound)?;
    let report_rows = sqlx::query(
        "SELECT sr.report_id, r.title, r.abstract_text, r.publication_year,
                sr.relationship, sr.created_at
         FROM study_reports sr
         JOIN reports r ON r.id=sr.report_id
         WHERE sr.project_id=$1 AND sr.study_id=$2
         ORDER BY sr.created_at, sr.report_id",
    )
    .bind(project_id)
    .bind(study_id)
    .fetch_all(&mut *connection)
    .await?;
    detail_from_rows(study_row, report_rows)
}

fn study_from_row(row: PgRow) -> Result<StudyRecord, StudyError> {
    let id: Uuid = row.get("id");
    let design = parse_design(row.try_get::<Option<String>, _>("design")?)?;
    let design_context = parse_context(row.try_get("design_context")?)?;
    Ok(StudyRecord {
        id: id.into(),
        project_id: row.get("project_id"),
        title: row.get("title"),
        design,
        design_context,
        revision: row.get("study_revision"),
        created_at: row.get("created_at"),
        updated_at: row.get("updated_at"),
        updated_by_actor_kind: row.get("updated_by_actor_kind"),
        updated_by_actor_id: row.get("updated_by_actor_id"),
    })
}

fn locked_study_from_row(row: PgRow) -> Result<LockedStudy, StudyError> {
    Ok(LockedStudy {
        id: row.get::<Uuid, _>("id").into(),
        title: row.get("title"),
        design: parse_design(row.try_get::<Option<String>, _>("design")?)?,
        design_context: parse_context(row.try_get("design_context")?)?,
        revision: row.get("study_revision"),
    })
}

fn detail_from_rows(
    study_row: PgRow,
    report_rows: Vec<PgRow>,
) -> Result<StudyDetailRecord, StudyError> {
    let study = study_from_row(study_row)?;
    let reports = report_rows
        .into_iter()
        .map(|row| {
            let role_value: String = row.get("relationship");
            let role = StudyReportRole::parse(&role_value).ok_or_else(|| {
                StudyError::DataIntegrity("study report has an unknown role".to_owned())
            })?;
            Ok(StudyReportRecord {
                report_id: row.get::<Uuid, _>("report_id").into(),
                title: row.get("title"),
                abstract_text: row.get("abstract_text"),
                publication_year: row.get("publication_year"),
                role,
                assigned_at: row.get("created_at"),
            })
        })
        .collect::<Result<Vec<_>, StudyError>>()?;
    let tool_suggestions = study
        .design
        .map(|design| deepref_domain::suggest_appraisal_tools(design, study.design_context))
        .unwrap_or_default();
    Ok(StudyDetailRecord {
        study,
        reports,
        tool_suggestions,
    })
}

fn parse_design(value: Option<String>) -> Result<Option<StudyDesign>, StudyError> {
    value
        .map(|value| {
            StudyDesign::parse(&value)
                .ok_or_else(|| StudyError::DataIntegrity(format!("unknown study design {value}")))
        })
        .transpose()
}

fn parse_context(value: Value) -> Result<StudyDesignContext, StudyError> {
    serde_json::from_value(value).map_err(|error| StudyError::DataIntegrity(error.to_string()))
}

#[allow(clippy::too_many_arguments)]
async fn insert_study_event(
    connection: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    project_id: Uuid,
    study_id: Uuid,
    report_id: Option<Uuid>,
    event_type: &str,
    before_study_id: Option<Uuid>,
    result_study_id: Option<Uuid>,
    before_revision: i64,
    result_revision: i64,
    before_snapshot: Value,
    result_snapshot: Value,
    payload: Value,
    actor: &deepref_domain::Actor,
) -> Result<Uuid, StudyError> {
    let event_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO study_events
          (id,project_id,study_id,report_id,event_type,before_study_id,result_study_id,
           before_revision,result_revision,before_snapshot,result_snapshot,payload,actor_kind,actor_id)
         VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14)",
    )
    .bind(event_id)
    .bind(project_id)
    .bind(study_id)
    .bind(report_id)
    .bind(event_type)
    .bind(before_study_id)
    .bind(result_study_id)
    .bind(before_revision)
    .bind(result_revision)
    .bind(before_snapshot)
    .bind(result_snapshot)
    .bind(payload)
    .bind(actor.kind().as_str())
    .bind(actor.id())
    .execute(&mut **connection)
    .await?;
    Ok(event_id)
}

fn study_event_payload(event: StudyEvent) -> Result<Value, StudyError> {
    serde_json::to_value(event).map_err(|error| StudyError::DataIntegrity(error.to_string()))
}

fn event_from_row(row: PgRow) -> Result<StudyEventRecord, StudyError> {
    Ok(StudyEventRecord {
        id: row.get("id"),
        study_id: row.get::<Uuid, _>("study_id").into(),
        report_id: row.try_get::<Option<Uuid>, _>("report_id")?.map(Into::into),
        event_type: row.get("event_type"),
        before_study_id: row
            .try_get::<Option<Uuid>, _>("before_study_id")?
            .map(Into::into),
        result_study_id: row
            .try_get::<Option<Uuid>, _>("result_study_id")?
            .map(Into::into),
        before_revision: row.get("before_revision"),
        result_revision: row.get("result_revision"),
        before_snapshot: row.get("before_snapshot"),
        result_snapshot: row.get("result_snapshot"),
        payload: row.get("payload"),
        actor_kind: row.get("actor_kind"),
        actor_id: row.get("actor_id"),
        created_at: row.get("created_at"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_study_design_rows_are_not_silently_accepted() {
        assert!(parse_design(Some("rct".to_owned())).unwrap().is_some());
        assert!(parse_design(Some("legacy-text".to_owned())).is_err());
    }
}
