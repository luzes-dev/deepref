use chrono::{DateTime, Utc};
use deepref_application::{
    AppraisalAssessmentInput, AppraisalCompleted, AppraisalDefinitionError,
    AppraisalValidationError, EvidenceReferenceInput, get_appraisal_definition,
    validate_assessment_input,
};
use deepref_domain::{Actor, ProjectId, ReportId};
use serde_json::{Value, json};
use sqlx::{PgPool, Row, postgres::PgRow};
use thiserror::Error;
use uuid::Uuid;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppraisalEvidenceRecord {
    pub question_id: String,
    pub document_id: Uuid,
    pub block_id: Uuid,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppraisalAssessmentRecord {
    pub id: Uuid,
    pub project_id: ProjectId,
    pub report_id: ReportId,
    pub definition_id: String,
    pub definition_version: i32,
    pub responses: Value,
    pub judgments: Value,
    pub evidence: Vec<AppraisalEvidenceRecord>,
    pub actor_kind: String,
    pub actor_id: String,
    pub completed_at: DateTime<Utc>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Error)]
pub enum AppraisalError {
    #[error("database operation failed")]
    Database(#[from] sqlx::Error),
    #[error("report is not part of the project")]
    ReportNotInProject,
    #[error("appraisal definition is invalid: {0}")]
    Definition(#[from] AppraisalDefinitionError),
    #[error("appraisal responses are invalid: {0}")]
    Validation(#[from] AppraisalValidationError),
    #[error("evidence block does not belong to the same project and report")]
    EvidenceNotInReport,
    #[error("appraisal assessment not found")]
    AssessmentNotFound,
    #[error("appraisal data integrity failure: {0}")]
    DataIntegrity(String),
}

pub async fn list_appraisals(
    pool: &PgPool,
    project_id: Uuid,
    report_id: Uuid,
) -> Result<Vec<AppraisalAssessmentRecord>, AppraisalError> {
    ensure_project_report(pool, project_id, report_id).await?;
    let rows = sqlx::query(
        "SELECT id, project_id, report_id, definition_id, definition_version,
                responses, judgments, actor_kind, actor_id, completed_at, created_at
         FROM appraisal_assessments
         WHERE project_id=$1 AND report_id=$2
         ORDER BY created_at DESC, id DESC",
    )
    .bind(project_id)
    .bind(report_id)
    .fetch_all(pool)
    .await?;
    let mut assessments = Vec::with_capacity(rows.len());
    for row in rows {
        assessments.push(assessment_from_row(row, pool).await?);
    }
    Ok(assessments)
}

pub async fn complete_appraisal(
    pool: &PgPool,
    project_id: ProjectId,
    report_id: ReportId,
    input: AppraisalAssessmentInput,
    actor: Actor,
) -> Result<AppraisalAssessmentRecord, AppraisalError> {
    let definition =
        get_appraisal_definition(input.definition_id.as_str(), input.definition_version.get())?;
    validate_assessment_input(&definition, &input)?;
    let mut transaction = pool.begin().await?;
    ensure_project_report_tx(&mut transaction, project_id.into(), report_id.into()).await?;
    for evidence in &input.evidence {
        if !evidence_belongs_to_report(
            &mut transaction,
            project_id.into(),
            report_id.into(),
            evidence,
        )
        .await?
        {
            return Err(AppraisalError::EvidenceNotInReport);
        }
    }

    let assessment_id = Uuid::new_v4();
    let judgments = json!({
        "domains": input.domain_judgments,
        "overall": input.overall_judgment,
    });
    sqlx::query(
        "INSERT INTO appraisal_assessments
          (id,project_id,report_id,definition_id,definition_version,responses,judgments,
           actor_kind,actor_id)
         VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9)",
    )
    .bind(assessment_id)
    .bind(project_id.as_uuid())
    .bind(report_id.as_uuid())
    .bind(input.definition_id.as_str())
    .bind(i32::try_from(input.definition_version.get()).map_err(|_| {
        AppraisalError::DataIntegrity("definition version exceeds database range".to_owned())
    })?)
    .bind(&input.responses)
    .bind(&judgments)
    .bind(actor.kind().as_str())
    .bind(actor.id())
    .execute(&mut *transaction)
    .await?;
    for evidence in &input.evidence {
        insert_evidence(
            &mut transaction,
            assessment_id,
            project_id,
            report_id,
            evidence,
        )
        .await?;
    }
    let event = AppraisalCompleted {
        assessment_id,
        project_id,
        report_id,
        definition_id: input.definition_id.clone(),
        definition_version: input.definition_version,
        actor: actor.clone(),
    };
    sqlx::query(
        "INSERT INTO appraisal_events
          (id,assessment_id,project_id,report_id,event_type,payload,actor_kind,actor_id)
         VALUES ($1,$2,$3,$4,'appraisal_completed',$5,$6,$7)",
    )
    .bind(Uuid::new_v4())
    .bind(assessment_id)
    .bind(project_id.as_uuid())
    .bind(report_id.as_uuid())
    .bind(
        serde_json::to_value(&event)
            .map_err(|error| AppraisalError::DataIntegrity(error.to_string()))?,
    )
    .bind(actor.kind().as_str())
    .bind(actor.id())
    .execute(&mut *transaction)
    .await?;
    transaction.commit().await?;
    get_appraisal(pool, project_id.into(), report_id.into(), assessment_id).await
}

pub async fn get_appraisal(
    pool: &PgPool,
    project_id: Uuid,
    report_id: Uuid,
    assessment_id: Uuid,
) -> Result<AppraisalAssessmentRecord, AppraisalError> {
    let row = sqlx::query(
        "SELECT id, project_id, report_id, definition_id, definition_version,
                responses, judgments, actor_kind, actor_id, completed_at, created_at
         FROM appraisal_assessments
         WHERE id=$1 AND project_id=$2 AND report_id=$3",
    )
    .bind(assessment_id)
    .bind(project_id)
    .bind(report_id)
    .fetch_optional(pool)
    .await?
    .ok_or(AppraisalError::AssessmentNotFound)?;
    assessment_from_row(row, pool).await
}

async fn assessment_from_row(
    row: PgRow,
    pool: &PgPool,
) -> Result<AppraisalAssessmentRecord, AppraisalError> {
    let assessment_id: Uuid = row.get("id");
    let evidence_rows = sqlx::query(
        "SELECT question_id, document_id, block_id
         FROM appraisal_assessment_evidence
         WHERE assessment_id=$1 ORDER BY question_id",
    )
    .bind(assessment_id)
    .fetch_all(pool)
    .await?;
    Ok(AppraisalAssessmentRecord {
        id: assessment_id,
        project_id: row.get::<Uuid, _>("project_id").into(),
        report_id: row.get::<Uuid, _>("report_id").into(),
        definition_id: row.get("definition_id"),
        definition_version: row.get("definition_version"),
        responses: row.get("responses"),
        judgments: row.get("judgments"),
        evidence: evidence_rows
            .into_iter()
            .map(|evidence| AppraisalEvidenceRecord {
                question_id: evidence.get("question_id"),
                document_id: evidence.get("document_id"),
                block_id: evidence.get("block_id"),
            })
            .collect(),
        actor_kind: row.get("actor_kind"),
        actor_id: row.get("actor_id"),
        completed_at: row.get("completed_at"),
        created_at: row.get("created_at"),
    })
}

async fn ensure_project_report(
    pool: &PgPool,
    project_id: Uuid,
    report_id: Uuid,
) -> Result<(), AppraisalError> {
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
        Err(AppraisalError::ReportNotInProject)
    }
}

async fn ensure_project_report_tx(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    project_id: Uuid,
    report_id: Uuid,
) -> Result<(), AppraisalError> {
    let exists = sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS(SELECT 1 FROM project_reports WHERE project_id=$1 AND report_id=$2)",
    )
    .bind(project_id)
    .bind(report_id)
    .fetch_one(&mut **transaction)
    .await?;
    if exists {
        Ok(())
    } else {
        Err(AppraisalError::ReportNotInProject)
    }
}

async fn evidence_belongs_to_report(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    project_id: Uuid,
    report_id: Uuid,
    evidence: &EvidenceReferenceInput,
) -> Result<bool, AppraisalError> {
    let exists = sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS(
           SELECT 1 FROM document_blocks b
           JOIN documents d ON d.id=b.document_id
           WHERE b.id=$1 AND b.document_id=$2 AND b.active
             AND d.project_id=$3 AND d.report_id=$4
         )",
    )
    .bind(evidence.block_id)
    .bind(evidence.document_id)
    .bind(project_id)
    .bind(report_id)
    .fetch_one(&mut **transaction)
    .await?;
    Ok(exists)
}

async fn insert_evidence(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    assessment_id: Uuid,
    project_id: ProjectId,
    report_id: ReportId,
    evidence: &EvidenceReferenceInput,
) -> Result<(), AppraisalError> {
    sqlx::query(
        "INSERT INTO appraisal_assessment_evidence
          (id,assessment_id,project_id,report_id,question_id,document_id,block_id)
         VALUES ($1,$2,$3,$4,$5,$6,$7)",
    )
    .bind(Uuid::new_v4())
    .bind(assessment_id)
    .bind(project_id.as_uuid())
    .bind(report_id.as_uuid())
    .bind(&evidence.question_id)
    .bind(evidence.document_id)
    .bind(evidence.block_id)
    .execute(&mut **transaction)
    .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn assessment_records_keep_project_and_report_identity() {
        let project_id = ProjectId::new(Uuid::new_v4());
        let report_id = ReportId::new(Uuid::new_v4());
        assert_ne!(project_id.as_uuid(), report_id.as_uuid());
    }
}
