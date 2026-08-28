//! Project-scoped reads used by the controlled agent tool adapter.
//!
//! The queries in this module deliberately accept a project scope on every
//! lookup. The HTTP application owns `AppState` and may call this scoped
//! adapter; the agent runtime and tool request never receive SQL, a `PgPool`,
//! or an unrestricted repository.

use chrono::{DateTime, Utc};
use serde::Serialize;
use serde_json::Value;
use sqlx::{PgPool, Row};
use thiserror::Error;
use uuid::Uuid;

#[derive(Debug, Error)]
pub enum AgentReadError {
    #[error("database operation failed")]
    Database(#[from] sqlx::Error),
    #[error("scoped resource was not found")]
    NotFound,
    #[error("stored agent read data is invalid")]
    InvalidData,
}

#[derive(Debug, Clone, Serialize)]
pub struct AgentReportIdentifier {
    pub scheme: String,
    pub value: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct AgentReportRecord {
    pub id: Uuid,
    pub project_id: Uuid,
    pub title: Option<String>,
    pub abstract_text: Option<String>,
    pub publication_year: Option<i32>,
    pub journal: Option<String>,
    pub url: Option<String>,
    pub identifiers: Vec<AgentReportIdentifier>,
}

#[derive(Debug, Clone, Serialize)]
pub struct AgentDocumentBlockRecord {
    pub id: Uuid,
    pub document_id: Uuid,
    pub page_number: i32,
    pub kind: String,
    pub section_path: Vec<String>,
    pub ordinal: i32,
    pub text: String,
    pub content_hash: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct AgentAppraisalRecord {
    pub id: Uuid,
    pub project_id: Uuid,
    pub report_id: Uuid,
    pub definition_id: String,
    pub definition_version: i32,
    pub responses: Value,
    pub judgments: Value,
    pub evidence: Vec<AgentAppraisalEvidence>,
    pub completed_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize)]
pub struct AgentAppraisalEvidence {
    pub question_id: String,
    pub document_id: Uuid,
    pub block_id: Uuid,
}

pub async fn project_exists(pool: &PgPool, project_id: Uuid) -> Result<bool, sqlx::Error> {
    sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM projects WHERE id=$1)")
        .bind(project_id)
        .fetch_one(pool)
        .await
}

pub async fn get_agent_report(
    pool: &PgPool,
    project_id: Uuid,
    report_id: Uuid,
) -> Result<AgentReportRecord, AgentReadError> {
    let row = sqlx::query(
        "SELECT r.id,pr.project_id,r.title,r.abstract_text,r.publication_year,r.journal,r.url
         FROM project_reports pr
         JOIN reports r ON r.id=pr.report_id
         WHERE pr.project_id=$1 AND pr.report_id=$2",
    )
    .bind(project_id)
    .bind(report_id)
    .fetch_optional(pool)
    .await?
    .ok_or(AgentReadError::NotFound)?;
    let identifiers = sqlx::query(
        "SELECT scheme,value FROM report_identifiers
         WHERE report_id=$1 ORDER BY scheme,value LIMIT 32",
    )
    .bind(report_id)
    .fetch_all(pool)
    .await?
    .into_iter()
    .map(|identifier| AgentReportIdentifier {
        scheme: identifier.get("scheme"),
        value: identifier.get("value"),
    })
    .collect();
    Ok(AgentReportRecord {
        id: row.get("id"),
        project_id: row.get("project_id"),
        title: row.get("title"),
        abstract_text: row.get("abstract_text"),
        publication_year: row.get("publication_year"),
        journal: row.get("journal"),
        url: row.get("url"),
        identifiers,
    })
}

pub async fn search_agent_reports(
    pool: &PgPool,
    project_id: Uuid,
    query: &str,
    limit: i64,
) -> Result<Vec<AgentReportRecord>, AgentReadError> {
    let rows = sqlx::query(
        "SELECT r.id,pr.project_id,r.title,r.abstract_text,r.publication_year,r.journal,r.url
         FROM project_reports pr
         JOIN reports r ON r.id=pr.report_id
         WHERE pr.project_id=$1
           AND lower(coalesce(r.title,'') || ' ' || coalesce(r.abstract_text,''))
               LIKE '%' || lower($2) || '%'
         ORDER BY r.updated_at DESC,r.id DESC LIMIT $3",
    )
    .bind(project_id)
    .bind(query)
    .bind(limit.clamp(1, 100))
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|row| AgentReportRecord {
            id: row.get("id"),
            project_id: row.get("project_id"),
            title: row.get("title"),
            abstract_text: row.get("abstract_text"),
            publication_year: row.get("publication_year"),
            journal: row.get("journal"),
            url: row.get("url"),
            identifiers: Vec::new(),
        })
        .collect())
}

pub async fn read_agent_document_blocks(
    pool: &PgPool,
    project_id: Uuid,
    document_id: Uuid,
    block_ids: &[Uuid],
) -> Result<Vec<AgentDocumentBlockRecord>, AgentReadError> {
    let document_exists = sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS(SELECT 1 FROM documents WHERE project_id=$1 AND id=$2)",
    )
    .bind(project_id)
    .bind(document_id)
    .fetch_one(pool)
    .await?;
    if !document_exists {
        return Err(AgentReadError::NotFound);
    }
    let rows = sqlx::query(
        "SELECT b.id,b.document_id,b.page_number,b.kind,b.section_path,b.ordinal,b.text,b.content_hash
         FROM document_blocks b
         JOIN documents d ON d.id=b.document_id
         WHERE d.project_id=$1 AND b.document_id=$2 AND b.id=ANY($3::uuid[])
           AND b.active AND b.parser_version=d.active_parser_version
         ORDER BY b.page_number,b.ordinal,b.id",
    )
    .bind(project_id)
    .bind(document_id)
    .bind(block_ids)
    .fetch_all(pool)
    .await?;
    if rows.len() != block_ids.len() {
        return Err(AgentReadError::NotFound);
    }
    Ok(rows
        .into_iter()
        .map(|row| AgentDocumentBlockRecord {
            id: row.get("id"),
            document_id: row.get("document_id"),
            page_number: row.get("page_number"),
            kind: row.get("kind"),
            section_path: row.get("section_path"),
            ordinal: row.get("ordinal"),
            text: row.get("text"),
            content_hash: row.get("content_hash"),
        })
        .collect())
}

pub async fn search_agent_document(
    pool: &PgPool,
    project_id: Uuid,
    document_id: Uuid,
    query: &str,
    limit: i64,
) -> Result<Vec<AgentDocumentBlockRecord>, AgentReadError> {
    let document_exists = sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS(SELECT 1 FROM documents WHERE project_id=$1 AND id=$2)",
    )
    .bind(project_id)
    .bind(document_id)
    .fetch_one(pool)
    .await?;
    if !document_exists {
        return Err(AgentReadError::NotFound);
    }
    let rows = sqlx::query(
        "SELECT b.id,b.document_id,b.page_number,b.kind,b.section_path,b.ordinal,b.text,b.content_hash
         FROM document_blocks b
         JOIN documents d ON d.id=b.document_id
         WHERE d.project_id=$1 AND b.document_id=$2
           AND b.active AND b.parser_version=d.active_parser_version
           AND b.search_vector @@ websearch_to_tsquery('simple',$3)
         ORDER BY ts_rank_cd(b.search_vector,websearch_to_tsquery('simple',$3)) DESC,
                  b.page_number,b.ordinal,b.id LIMIT $4",
    )
    .bind(project_id)
    .bind(document_id)
    .bind(query)
    .bind(limit.clamp(1, 100))
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|row| AgentDocumentBlockRecord {
            id: row.get("id"),
            document_id: row.get("document_id"),
            page_number: row.get("page_number"),
            kind: row.get("kind"),
            section_path: row.get("section_path"),
            ordinal: row.get("ordinal"),
            text: row.get("text"),
            content_hash: row.get("content_hash"),
        })
        .collect())
}

pub async fn get_agent_screening_state(
    pool: &PgPool,
    project_id: Uuid,
    report_id: Uuid,
) -> Result<crate::screening::ScreeningStateSnapshot, AgentReadError> {
    let row = sqlx::query(
        "SELECT pr.project_id,pr.report_id,coalesce(ss.title_abstract_status,'unscreened') AS title_abstract_status,
                coalesce(ss.full_text_status,'not_required') AS full_text_status,
                ss.full_text_exclusion_reason_id,
                coalesce(ss.final_status,'unscreened') AS final_status,
                coalesce(ss.revision,0)::bigint AS revision,ss.last_event_id,ss.updated_at
         FROM project_reports pr
         LEFT JOIN screening_state ss ON ss.project_id=pr.project_id AND ss.report_id=pr.report_id
         WHERE pr.project_id=$1 AND pr.report_id=$2",
    )
    .bind(project_id)
    .bind(report_id)
    .fetch_optional(pool)
    .await?
    .ok_or(AgentReadError::NotFound)?;
    Ok(crate::screening::ScreeningStateSnapshot {
        project_id: row.get("project_id"),
        report_id: row.get("report_id"),
        title_abstract_status: row.get("title_abstract_status"),
        full_text_status: row.get("full_text_status"),
        full_text_exclusion_reason_id: row.get("full_text_exclusion_reason_id"),
        final_status: row.get("final_status"),
        revision: row.get("revision"),
        last_event_id: row.get("last_event_id"),
        updated_at: row.get("updated_at"),
    })
}

pub async fn get_latest_agent_appraisal(
    pool: &PgPool,
    project_id: Uuid,
    report_id: Uuid,
    definition_id: &str,
    definition_version: i32,
) -> Result<AgentAppraisalRecord, AgentReadError> {
    let row = sqlx::query(
        "SELECT a.id,a.project_id,a.report_id,a.definition_id,a.definition_version,
                a.responses,a.judgments,a.completed_at
         FROM appraisal_assessments a
         JOIN project_reports pr ON pr.project_id=a.project_id AND pr.report_id=a.report_id
         WHERE a.project_id=$1 AND a.report_id=$2 AND a.definition_id=$3
           AND a.definition_version=$4 AND a.completed_at IS NOT NULL
         ORDER BY a.completed_at DESC,a.id DESC LIMIT 1",
    )
    .bind(project_id)
    .bind(report_id)
    .bind(definition_id)
    .bind(definition_version)
    .fetch_optional(pool)
    .await?
    .ok_or(AgentReadError::NotFound)?;
    let assessment_id: Uuid = row.get("id");
    let evidence = sqlx::query(
        "SELECT question_id,document_id,block_id
         FROM appraisal_assessment_evidence WHERE assessment_id=$1 ORDER BY question_id",
    )
    .bind(assessment_id)
    .fetch_all(pool)
    .await?
    .into_iter()
    .map(|evidence| AgentAppraisalEvidence {
        question_id: evidence.get("question_id"),
        document_id: evidence.get("document_id"),
        block_id: evidence.get("block_id"),
    })
    .collect();
    Ok(AgentAppraisalRecord {
        id: assessment_id,
        project_id: row.get("project_id"),
        report_id: row.get("report_id"),
        definition_id: row.get("definition_id"),
        definition_version: row.get("definition_version"),
        responses: row.get("responses"),
        judgments: row.get("judgments"),
        evidence,
        completed_at: row.get("completed_at"),
    })
}
