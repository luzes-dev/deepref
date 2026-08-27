use std::collections::HashMap;

use chrono::{DateTime, Utc};
use deepref_ai::{
    AiError, AiFuture, AiProposal, AiRunRecord, AiRunStatus, AiRunStore, AiTaskKind, AuthorityTier,
    Embedding, EvidenceRef, EvidenceRetriever, GroundedBlock, ModelParameters, ModelProfile,
    ModelRouter, ProposalDraft, ProposalStatus, ProposalStore, ResolvedModel, RetrievalRequest,
};
use deepref_application::{
    AppraisalAssessmentInput, DefinitionId, DefinitionVersion, EvidenceReferenceInput,
    ResolveRecordCommand, ScreenReportCommand, get_appraisal_definition,
};
use deepref_domain::{Actor, ScreeningDecision, ScreeningStage, StudyReportRole, StudyTitle};
use pgvector::Vector;
use sqlx::{PgPool, Postgres, Row, Transaction};
use thiserror::Error;
use uuid::Uuid;

#[derive(Clone)]
pub struct PostgresAiStore {
    pool: PgPool,
}
impl PostgresAiStore {
    pub fn new(pool: &PgPool) -> Self {
        Self { pool: pool.clone() }
    }
    pub fn pool(&self) -> &PgPool {
        &self.pool
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AiScreeningTarget {
    pub title: Option<String>,
    pub abstract_text: Option<String>,
    pub expected_revision: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AiGroundingBlock {
    pub report_id: Uuid,
    pub document_id: Uuid,
    pub document_block_id: Uuid,
    pub page: u32,
    pub parser_version: String,
    pub section_path: Vec<String>,
    pub text: String,
    pub content_hash: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AiGroupingReport {
    pub report_id: Uuid,
    pub title: Option<String>,
    pub abstract_text: Option<String>,
    pub publication_year: Option<i32>,
    pub first_author: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AiGroupingStudy {
    pub study_id: Uuid,
    pub title: String,
    pub revision: i64,
    pub reports: Vec<AiGroupingReport>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AiStudyGroupingTarget {
    pub report: AiGroupingReport,
    pub current_study_id: Option<Uuid>,
    pub current_study_revision: Option<i64>,
    pub studies: Vec<AiGroupingStudy>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct AiDedupeTarget {
    pub source_title: Option<String>,
    pub candidate_title: Option<String>,
    pub source_year: Option<i32>,
    pub candidate_year: Option<i32>,
    pub source_author: Option<String>,
    pub candidate_author: Option<String>,
    pub source_title_hash: String,
    pub candidate_title_hash: String,
}

pub async fn get_ai_screening_target(
    pool: &PgPool,
    project_id: Uuid,
    report_id: Uuid,
) -> Result<AiScreeningTarget, AiProposalError> {
    sqlx::query(
        "SELECT r.title,r.abstract_text,coalesce(ss.revision,0)::bigint AS expected_revision
         FROM project_reports pr
         JOIN reports r ON r.id=pr.report_id
         LEFT JOIN screening_state ss ON ss.project_id=pr.project_id AND ss.report_id=pr.report_id
         WHERE pr.project_id=$1 AND pr.report_id=$2",
    )
    .bind(project_id)
    .bind(report_id)
    .fetch_optional(pool)
    .await?
    .map(|row| AiScreeningTarget {
        title: row.get("title"),
        abstract_text: row.get("abstract_text"),
        expected_revision: row.get("expected_revision"),
    })
    .ok_or(AiProposalError::NotFound)
}

pub async fn list_ai_grounding_blocks(
    pool: &PgPool,
    project_id: Uuid,
    report_id: Uuid,
    query: &str,
) -> Result<Vec<AiGroundingBlock>, AiProposalError> {
    let request = RetrievalRequest {
        project_id: project_id.into(),
        study_id: None,
        report_id: Some(report_id),
        document_id: None,
        query: sanitized_retrieval_query(query),
        embedding: None,
        section_prefix: None,
        kind: None,
        limit: 20,
    };
    retrieve_grounding_blocks(pool, request).await
}

pub async fn get_ai_study_grouping_target(
    pool: &PgPool,
    project_id: Uuid,
    report_id: Uuid,
) -> Result<AiStudyGroupingTarget, AiProposalError> {
    let report_row = sqlx::query(
        "SELECT r.id,r.title,r.abstract_text,r.publication_year,r.authors
         FROM project_reports pr JOIN reports r ON r.id=pr.report_id
         WHERE pr.project_id=$1 AND pr.report_id=$2",
    )
    .bind(project_id)
    .bind(report_id)
    .fetch_optional(pool)
    .await?
    .ok_or(AiProposalError::NotFound)?;
    let report = AiGroupingReport {
        report_id: report_row.get("id"),
        title: report_row.get("title"),
        abstract_text: report_row.get("abstract_text"),
        publication_year: report_row.get("publication_year"),
        first_author: first_author(report_row.get("authors")),
    };
    let membership = sqlx::query(
        "SELECT sr.study_id,s.study_revision
         FROM study_reports sr JOIN studies s ON s.project_id=sr.project_id AND s.id=sr.study_id
         WHERE sr.project_id=$1 AND sr.report_id=$2",
    )
    .bind(project_id)
    .bind(report_id)
    .fetch_optional(pool)
    .await?;
    let (current_study_id, current_study_revision) = membership
        .map(|row| (Some(row.get("study_id")), Some(row.get("study_revision"))))
        .unwrap_or((None, None));
    let rows = sqlx::query(
        "WITH shortlisted AS (
           SELECT s.id
           FROM studies s
           WHERE s.project_id=$1
           ORDER BY CASE WHEN s.id=$2 THEN 0 ELSE 1 END,
                    similarity(lower(s.title),lower(coalesce($3,''))) DESC,
                    s.updated_at DESC,s.id
           LIMIT 24
         ), bounded AS (
           SELECT id FROM shortlisted
           UNION
           SELECT $2::uuid WHERE $2 IS NOT NULL
         ), candidate_reports AS (
           SELECT sr.project_id,sr.study_id,sr.report_id,sr.created_at,
                  r.title AS report_title,r.abstract_text,
                  r.publication_year,r.authors,
                  row_number() OVER (
                    PARTITION BY sr.study_id
                    ORDER BY CASE WHEN sr.report_id=$4 THEN 0 ELSE 1 END,
                             sr.created_at NULLS LAST,sr.report_id
                  ) AS report_rank
           FROM study_reports sr
           JOIN reports r ON r.id=sr.report_id
           WHERE sr.project_id=$1 AND sr.study_id IN (SELECT id FROM bounded)
         )
         SELECT s.id AS study_id,s.title,s.study_revision,
                cr.report_id,cr.report_title,cr.abstract_text,
                cr.publication_year,cr.authors
         FROM studies s
         LEFT JOIN candidate_reports cr
           ON cr.project_id=s.project_id AND cr.study_id=s.id AND cr.report_rank <= 3
         WHERE s.project_id=$1 AND s.id IN (SELECT id FROM bounded)
         ORDER BY s.id,cr.report_rank,cr.report_id",
    )
    .bind(project_id)
    .bind(current_study_id)
    .bind(report.title.as_deref().unwrap_or(""))
    .bind(report_id)
    .fetch_all(pool)
    .await?;
    let mut studies = Vec::<AiGroupingStudy>::new();
    for row in rows {
        let study_id: Uuid = row.get("study_id");
        let index = studies.iter().position(|study| study.study_id == study_id);
        let report_id: Option<Uuid> = row.get("report_id");
        let report = report_id.map(|report_id| AiGroupingReport {
            report_id,
            title: row.get("report_title"),
            abstract_text: row.get("abstract_text"),
            publication_year: row.get("publication_year"),
            first_author: first_author(row.get("authors")),
        });
        if let Some(index) = index {
            if let Some(report) = report {
                studies[index].reports.push(report);
            }
        } else {
            studies.push(AiGroupingStudy {
                study_id,
                title: row.get("title"),
                revision: row.get("study_revision"),
                reports: report.into_iter().collect(),
            });
        }
    }
    Ok(AiStudyGroupingTarget {
        report,
        current_study_id,
        current_study_revision,
        studies,
    })
}

pub async fn list_ai_extraction_evidence(
    pool: &PgPool,
    project_id: Uuid,
    study_id: Uuid,
    query: &str,
) -> Result<Vec<AiGroundingBlock>, AiProposalError> {
    let request = RetrievalRequest {
        project_id: project_id.into(),
        study_id: Some(study_id),
        report_id: None,
        document_id: None,
        query: sanitized_retrieval_query(query),
        embedding: None,
        section_prefix: None,
        kind: None,
        limit: 32,
    };
    retrieve_grounding_blocks(pool, request).await
}

fn sanitized_retrieval_query(raw: &str) -> String {
    let terms = raw
        .split_whitespace()
        .take(32)
        .filter_map(|term| {
            let sanitized = term
                .chars()
                .filter(|character| character.is_alphanumeric() || *character == '-')
                .collect::<String>();
            (!sanitized.is_empty()).then_some(sanitized)
        })
        .collect::<Vec<_>>();
    if terms.is_empty() {
        "evidence".to_owned()
    } else {
        terms.join(" ")
    }
}

async fn retrieve_grounding_blocks(
    pool: &PgPool,
    request: RetrievalRequest,
) -> Result<Vec<AiGroundingBlock>, AiProposalError> {
    let retrieved = PostgresAiStore::new(pool)
        .retrieve(request.clone())
        .await
        .map_err(|error| AiProposalError::InvalidPayload(error.to_string()))?;
    if retrieved.is_empty() {
        return Ok(Vec::new());
    }
    let ids = retrieved
        .iter()
        .map(|block| block.evidence.document_block_id.as_uuid())
        .collect::<Vec<_>>();
    let rows = sqlx::query(
        "SELECT d.report_id,d.id AS document_id,b.id,b.page_number,b.section_path,b.text,
                b.content_hash,b.parser_version
         FROM documents d JOIN document_blocks b ON b.document_id=d.id
         JOIN document_pages p ON p.document_id=b.document_id
           AND p.parser_version=b.parser_version
           AND p.page_number=b.page_number AND p.active
         WHERE d.project_id=$1 AND b.id=ANY($2::uuid[])
           AND b.active AND d.active_parser_version=b.parser_version",
    )
    .bind(request.project_id.as_uuid())
    .bind(&ids)
    .fetch_all(pool)
    .await?;
    let mut by_id = HashMap::with_capacity(rows.len());
    for row in rows {
        let page = u32::try_from(row.get::<i32, _>("page_number"))
            .map_err(|_| AiProposalError::InvalidPayload("document page is invalid".to_owned()))?;
        let text: String = row.get("text");
        let content_hash: String = row.get("content_hash");
        if page == 0 || text.trim().is_empty() || !deepref_ai::is_sha256(&content_hash) {
            return Err(AiProposalError::InvalidPayload(
                "document grounding block is invalid".to_owned(),
            ));
        }
        let block_id: Uuid = row.get("id");
        by_id.insert(
            block_id,
            AiGroundingBlock {
                report_id: row.get("report_id"),
                document_id: row.get("document_id"),
                document_block_id: block_id,
                page,
                parser_version: row.get("parser_version"),
                section_path: row.get("section_path"),
                text,
                content_hash,
            },
        );
    }
    retrieved
        .into_iter()
        .map(|block| {
            let block_id = block.evidence.document_block_id.as_uuid();
            let grounded = by_id.remove(&block_id).ok_or_else(|| {
                AiProposalError::InvalidPayload("grounding changed during retrieval".to_owned())
            })?;
            if grounded.page != block.evidence.page
                || grounded.content_hash != block.evidence.content_hash
            {
                return Err(AiProposalError::InvalidPayload(
                    "grounding changed during retrieval".to_owned(),
                ));
            }
            Ok(grounded)
        })
        .collect()
}

pub async fn list_ai_exclusion_reasons(
    pool: &PgPool,
    project_id: Uuid,
    stage: ScreeningStage,
) -> Result<Vec<Uuid>, AiProposalError> {
    let stage = match stage {
        ScreeningStage::TitleAbstract => "title_abstract",
        ScreeningStage::FullText => "full_text",
    };
    Ok(sqlx::query_scalar(
        "SELECT id FROM exclusion_reasons WHERE project_id=$1 AND stage=$2 ORDER BY code,id",
    )
    .bind(project_id)
    .bind(stage)
    .fetch_all(pool)
    .await?)
}

pub async fn get_ai_dedupe_target(
    pool: &PgPool,
    project_id: Uuid,
    source_record_id: Uuid,
    candidate_report_id: Uuid,
) -> Result<AiDedupeTarget, AiProposalError> {
    let row = sqlx::query(
        "SELECT rec.title AS source_title,rec.publication_year AS source_year,rec.authors AS source_authors,
                r.title AS candidate_title,r.publication_year AS candidate_year,r.authors AS candidate_authors
         FROM records rec
         JOIN project_reports pr ON pr.project_id=rec.project_id AND pr.report_id=$3
         JOIN reports r ON r.id=pr.report_id
         WHERE rec.project_id=$1 AND rec.id=$2",
    )
    .bind(project_id)
    .bind(source_record_id)
    .bind(candidate_report_id)
    .fetch_optional(pool)
    .await?
    .ok_or(AiProposalError::NotFound)?;
    let source_title: Option<String> = row.get("source_title");
    let candidate_title: Option<String> = row.get("candidate_title");
    Ok(AiDedupeTarget {
        source_title_hash: deepref_ai::sha256_bytes(
            source_title.as_deref().unwrap_or("").as_bytes(),
        ),
        candidate_title_hash: deepref_ai::sha256_bytes(
            candidate_title.as_deref().unwrap_or("").as_bytes(),
        ),
        source_author: first_author(row.get("source_authors")),
        candidate_author: first_author(row.get("candidate_authors")),
        source_title,
        candidate_title,
        source_year: row.get("source_year"),
        candidate_year: row.get("candidate_year"),
    })
}

fn first_author(authors: serde_json::Value) -> Option<String> {
    authors
        .as_array()
        .and_then(|items| items.first())
        .and_then(|item| {
            item.get("literal")
                .or_else(|| item.get("family"))
                .or_else(|| item.get("name"))
                .and_then(serde_json::Value::as_str)
                .map(str::to_owned)
        })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AiProposalDecision {
    Accept,
    Reject,
}

#[derive(Debug, Clone, PartialEq)]
pub struct AiProposalDecisionRequest {
    pub project_id: Uuid,
    pub proposal_id: Uuid,
    pub decision: AiProposalDecision,
    pub reason: String,
    pub reviewed_payload: Option<ReviewedAiProposalPayload>,
    pub actor: Actor,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "kind", content = "payload", rename_all = "snake_case")]
pub enum ReviewedAiProposalPayload {
    AppraisalPrefill(deepref_ai::AppraisalPrefill),
    DataExtraction(deepref_ai::DataExtraction),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AiProposalResolution {
    pub proposal_id: Uuid,
    pub status: String,
    pub target_report_id: Option<Uuid>,
    pub target_record_id: Option<Uuid>,
    pub applied_revision: Option<i64>,
}

#[derive(Debug, Error)]
pub enum AiProposalError {
    #[error("database operation failed")]
    Database(#[from] sqlx::Error),
    #[error("AI proposal not found in this project")]
    NotFound,
    #[error("AI proposal is no longer pending")]
    NotPending,
    #[error("AI proposal payload is invalid: {0}")]
    InvalidPayload(String),
    #[error("AI proposal target is invalid: {0}")]
    InvalidTarget(String),
    #[error("screening command failed: {0}")]
    Screening(#[from] crate::ScreeningError),
    #[error("deduplication command failed: {0}")]
    Dedupe(#[from] crate::DedupeError),
    #[error("study command failed: {0}")]
    Study(#[from] crate::StudyError),
    #[error("appraisal command failed: {0}")]
    Appraisal(#[from] crate::AppraisalError),
    #[error("extraction command failed: {0}")]
    Extraction(#[from] crate::ExtractionError),
    #[error("actor is invalid")]
    InvalidActor,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct AiProposalRecord {
    pub id: Uuid,
    pub project_id: Uuid,
    pub task_kind: String,
    pub entity_type: String,
    pub entity_id: Option<Uuid>,
    pub operation: String,
    pub payload: serde_json::Value,
    pub authority_tier: String,
    pub model_run_id: Uuid,
    pub provider: String,
    pub model: String,
    pub model_version: String,
    pub prompt_version: String,
    pub schema_version: String,
    pub status: String,
    pub protocol_version_id: Option<Uuid>,
    pub expected_revision: Option<i64>,
    pub target_report_id: Option<Uuid>,
    pub target_record_id: Option<Uuid>,
    pub target_study_id: Option<Uuid>,
    pub prompt_hash: String,
    pub schema_hash: String,
    pub input_hash: String,
    pub evidence_hash: Option<String>,
    pub resolved_at: Option<DateTime<Utc>>,
    pub resolved_by_actor_kind: Option<String>,
    pub resolved_by_actor_id: Option<String>,
    pub resolution_reason: Option<String>,
    pub created_at: DateTime<Utc>,
}

impl ModelRouter for PostgresAiStore {
    fn resolve<'a>(&'a self, profile: ModelProfile) -> AiFuture<'a, ResolvedModel> {
        Box::pin(async move {
            let row = sqlx::query(
                "SELECT id,provider,model,model_version,parameters FROM ai_model_routes
                 WHERE profile=$1 AND enabled AND effective_from <= now()
                   AND (effective_until IS NULL OR effective_until > now())
                 ORDER BY effective_from DESC,id DESC LIMIT 1",
            )
            .bind(profile.as_str())
            .fetch_optional(&self.pool)
            .await
            .map_err(|_| AiError::Route("route lookup failed".to_owned()))?
            .ok_or_else(|| AiError::Route("no enabled route exists".to_owned()))?;
            let parameters = serde_json::from_value::<ModelParameters>(row.get("parameters"))
                .map_err(|_| AiError::Route("route parameters are invalid".to_owned()))?;
            let route = ResolvedModel {
                profile,
                provider: row.get("provider"),
                model: row.get("model"),
                model_version: row.get("model_version"),
                parameters,
                route_id: Some(row.get("id")),
            };
            route.validate()?;
            Ok(route)
        })
    }
}

impl AiRunStore for PostgresAiStore {
    fn find_reusable<'a>(&'a self, reuse_hash: &'a str) -> AiFuture<'a, Option<AiRunRecord>> {
        Box::pin(async move {
            let row = sqlx::query(
                "SELECT id,project_id,task_kind,profile,provider,model,model_version,parameters,
                        prompt_version,prompt_hash,schema_version,schema_hash,input_hash,reuse_hash,
                        protocol_hash,document_hash,evidence_hash,evidence_refs,input_tokens,
                        output_tokens,cost_micros,output,status,error_code,error_message,
                        parent_automation_run_id,created_at,completed_at
                 FROM ai_runs WHERE reuse_hash=$1 AND status='completed'
                 ORDER BY completed_at DESC NULLS LAST,created_at DESC,id DESC LIMIT 1",
            )
            .bind(reuse_hash)
            .fetch_optional(&self.pool)
            .await
            .map_err(|_| AiError::Persistence("completed run lookup failed".to_owned()))?;
            row.map(ai_run_from_row).transpose()
        })
    }

    fn save_run<'a>(&'a self, run: AiRunRecord) -> AiFuture<'a, ()> {
        Box::pin(async move {
            run.validate()?;
            let mut transaction = self
                .pool
                .begin()
                .await
                .map_err(|_| AiError::Persistence("run transaction failed".to_owned()))?;
            let parameters = serde_json::to_value(&run.route.parameters).map_err(|_| {
                AiError::Persistence("route parameters serialization failed".to_owned())
            })?;
            let evidence_refs = serde_json::to_value(&run.evidence_refs)
                .map_err(|_| AiError::Persistence("evidence serialization failed".to_owned()))?;
            let inserted = sqlx::query(
                "INSERT INTO ai_runs
                 (id,project_id,task_kind,profile,provider,model,model_version,parameters,
                  prompt_version,prompt_hash,schema_version,schema_hash,input_hash,reuse_hash,
                  protocol_hash,document_hash,evidence_hash,evidence_refs,input_tokens,output_tokens,
                  cost_micros,output,status,error_code,error_message,parent_automation_run_id,created_at,completed_at)
                 VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16,$17,$18,$19,$20,$21,$22,$23,$24,$25,$26,$27,$28)
                 ON CONFLICT (id) DO NOTHING",
            )
            .bind(run.id).bind(run.project_id.map(|id| id.as_uuid())).bind(run.task_kind.as_str())
            .bind(run.route.profile.as_str()).bind(&run.route.provider).bind(&run.route.model).bind(&run.route.model_version)
            .bind(parameters).bind(&run.prompt_version).bind(&run.prompt_hash).bind(&run.schema_version).bind(&run.schema_hash)
            .bind(&run.input_hash).bind(&run.reuse_hash).bind(&run.protocol_hash).bind(&run.document_hash).bind(&run.evidence_hash)
            .bind(evidence_refs).bind(i64::try_from(run.usage.input_tokens).map_err(|_| AiError::Persistence("input token count is too large".to_owned()))?)
            .bind(i64::try_from(run.usage.output_tokens).map_err(|_| AiError::Persistence("output token count is too large".to_owned()))?)
            .bind(run.cost_micros).bind(run.output.clone()).bind(run.status.as_str())
            .bind(run.error.as_ref().map(|error| error.code.clone())).bind(run.error.as_ref().map(|error| error.message.clone()))
            .bind(run.parent_automation_run_id).bind(run.created_at).bind(run.completed_at)
            .execute(&mut *transaction).await.map_err(|_| AiError::Persistence("AI run write failed".to_owned()))?;

            if inserted.rows_affected() == 1 {
                persist_run_evidence(&mut transaction, &run).await?;
            } else {
                let existing = sqlx::query(
                    "SELECT id,project_id,task_kind,profile,provider,model,model_version,parameters,
                            prompt_version,prompt_hash,schema_version,schema_hash,input_hash,reuse_hash,
                            protocol_hash,document_hash,evidence_hash,evidence_refs,input_tokens,
                            output_tokens,cost_micros,output,status,error_code,error_message,
                            parent_automation_run_id,created_at,completed_at
                     FROM ai_runs WHERE id=$1 FOR UPDATE",
                )
                .bind(run.id)
                .fetch_optional(&mut *transaction)
                .await
                .map_err(|_| AiError::Persistence("existing AI run lookup failed".to_owned()))?
                .ok_or_else(|| AiError::Persistence("AI run conflict disappeared".to_owned()))?;
                let existing = ai_run_from_row(existing)?;
                if !same_run_identity(&existing, &run) {
                    return Err(AiError::Persistence(
                        "AI run identity or evidence is immutable".to_owned(),
                    ));
                }
                match (existing.status, run.status) {
                    (AiRunStatus::Running, AiRunStatus::Running) => {
                        if !same_run_state(&existing, &run) {
                            return Err(AiError::Persistence(
                                "running AI run state is immutable except for terminal completion"
                                    .to_owned(),
                            ));
                        }
                    }
                    (AiRunStatus::Running, AiRunStatus::Completed)
                    | (AiRunStatus::Running, AiRunStatus::Failed)
                    | (AiRunStatus::Running, AiRunStatus::Abstained) => {
                        let updated = sqlx::query(
                            "UPDATE ai_runs
                             SET input_tokens=$2,output_tokens=$3,cost_micros=$4,output=$5,
                                 status=$6,error_code=$7,error_message=$8,completed_at=$9
                             WHERE id=$1 AND status='running'",
                        )
                        .bind(run.id)
                        .bind(i64::try_from(run.usage.input_tokens).map_err(|_| {
                            AiError::Persistence("input token count is too large".to_owned())
                        })?)
                        .bind(i64::try_from(run.usage.output_tokens).map_err(|_| {
                            AiError::Persistence("output token count is too large".to_owned())
                        })?)
                        .bind(run.cost_micros)
                        .bind(run.output)
                        .bind(run.status.as_str())
                        .bind(run.error.as_ref().map(|error| error.code.clone()))
                        .bind(run.error.as_ref().map(|error| error.message.clone()))
                        .bind(run.completed_at)
                        .execute(&mut *transaction)
                        .await
                        .map_err(|_| {
                            AiError::Persistence("AI run terminal transition failed".to_owned())
                        })?;
                        if updated.rows_affected() != 1 {
                            return Err(AiError::Persistence(
                                "AI run terminal transition affected no row".to_owned(),
                            ));
                        }
                    }
                    (_, _) => {
                        if !same_run_state(&existing, &run) {
                            return Err(AiError::Persistence(
                                "terminal AI run is immutable".to_owned(),
                            ));
                        }
                    }
                }
            }
            transaction
                .commit()
                .await
                .map_err(|_| AiError::Persistence("run transaction commit failed".to_owned()))?;
            Ok(())
        })
    }
}

async fn persist_run_evidence(
    transaction: &mut Transaction<'_, Postgres>,
    run: &AiRunRecord,
) -> Result<(), AiError> {
    if run.evidence_refs.is_empty() {
        return Ok(());
    }
    let project_id = run
        .project_id
        .ok_or_else(|| AiError::Persistence("evidence requires a project-scoped run".to_owned()))?;
    for (index, evidence) in run.evidence_refs.iter().enumerate() {
        let rank = if evidence.retrieval_rank == 0 {
            u32::try_from(index + 1)
                .map_err(|_| AiError::Persistence("evidence rank overflow".to_owned()))?
        } else {
            evidence.retrieval_rank
        };
        let inserted = sqlx::query(
            "INSERT INTO ai_run_evidence
             (ai_run_id,project_id,document_id,document_block_id,rank,retrieval_score,content_hash)
             SELECT $1,$2,d.id,b.id,$3,$4,$5
             FROM document_blocks b JOIN documents d ON d.id=b.document_id
             WHERE b.id=$6 AND d.project_id=$2 AND b.content_hash=$5
               AND b.active AND d.active_parser_version=b.parser_version",
        )
        .bind(run.id)
        .bind(project_id.as_uuid())
        .bind(
            i32::try_from(rank)
                .map_err(|_| AiError::Persistence("evidence rank overflow".to_owned()))?,
        )
        .bind(evidence.retrieval_score)
        .bind(&evidence.content_hash)
        .bind(evidence.document_block_id.as_uuid())
        .execute(&mut **transaction)
        .await
        .map_err(|_| AiError::Persistence("scoped evidence write failed".to_owned()))?;
        if inserted.rows_affected() != 1 {
            return Err(AiError::Persistence(
                "evidence is outside the run project or active parser".to_owned(),
            ));
        }
    }
    Ok(())
}

fn same_run_identity(existing: &AiRunRecord, incoming: &AiRunRecord) -> bool {
    existing.id == incoming.id
        && existing.project_id == incoming.project_id
        && existing.task_kind == incoming.task_kind
        && existing.route.profile == incoming.route.profile
        && existing.route.provider == incoming.route.provider
        && existing.route.model == incoming.route.model
        && existing.route.model_version == incoming.route.model_version
        && existing.route.parameters == incoming.route.parameters
        && existing.prompt_version == incoming.prompt_version
        && existing.prompt_hash == incoming.prompt_hash
        && existing.schema_version == incoming.schema_version
        && existing.schema_hash == incoming.schema_hash
        && existing.input_hash == incoming.input_hash
        && existing.reuse_hash == incoming.reuse_hash
        && existing.protocol_hash == incoming.protocol_hash
        && existing.document_hash == incoming.document_hash
        && existing.evidence_hash == incoming.evidence_hash
        && existing.evidence_refs == incoming.evidence_refs
        && existing.parent_automation_run_id == incoming.parent_automation_run_id
        && existing.created_at.timestamp_micros() == incoming.created_at.timestamp_micros()
}

fn same_run_state(existing: &AiRunRecord, incoming: &AiRunRecord) -> bool {
    same_run_identity(existing, incoming)
        && existing.status == incoming.status
        && existing.usage == incoming.usage
        && existing.cost_micros == incoming.cost_micros
        && existing.output == incoming.output
        && existing.error == incoming.error
        && existing.completed_at.map(|value| value.timestamp_micros())
            == incoming.completed_at.map(|value| value.timestamp_micros())
}

impl EvidenceRetriever for PostgresAiStore {
    fn retrieve<'a>(&'a self, request: RetrievalRequest) -> AiFuture<'a, Vec<GroundedBlock>> {
        Box::pin(async move {
            request.validate()?;
            let lexical = (!request.query.trim().is_empty()).then_some(request.query.clone());
            let vector = request
                .embedding
                .as_ref()
                .map(|embedding| Vector::from(embedding.as_slice().to_vec()));
            let rows = sqlx::query(
                "WITH candidates AS (
                   SELECT b.id,b.document_id,b.page_number,b.section_path,b.text,b.content_hash,b.ordinal,
                     GREATEST(CASE WHEN $5::text IS NULL THEN 0.0 ELSE ts_rank_cd(b.search_vector,websearch_to_tsquery('simple',$5)) END,0.0)
                     + CASE WHEN $6::vector IS NULL THEN 0.0
                            WHEN e.embedding IS NOT NULL AND e.dimension=vector_dims($6::vector)
                            THEN GREATEST(0.0,1.0-(e.embedding <=> $6::vector))
                            ELSE 0.0 END AS retrieval_score
                   FROM document_blocks b
                   JOIN documents d ON d.id=b.document_id
                   LEFT JOIN document_block_embeddings e ON e.document_block_id=b.id
                     AND e.is_current AND e.content_hash=b.content_hash
                   WHERE d.project_id=$1
                     AND ($2::uuid IS NULL OR EXISTS(
                       SELECT 1 FROM study_reports scoped_sr
                       WHERE scoped_sr.project_id=d.project_id
                         AND scoped_sr.study_id=$2 AND scoped_sr.report_id=d.report_id
                     ))
                     AND ($3::uuid IS NULL OR d.report_id=$3)
                     AND ($4::uuid IS NULL OR b.document_id=$4)
                     AND b.active AND b.parser_version=d.active_parser_version
                     AND ($7::text[] IS NULL OR cardinality($7::text[])=0
                          OR b.section_path[1:cardinality($7::text[])]= $7::text[])
                     AND ($8::text IS NULL OR b.kind=$8)
                     AND ($6::vector IS NULL OR
                          ($5::text IS NOT NULL AND b.search_vector @@ websearch_to_tsquery('simple',$5))
                          OR (e.embedding IS NOT NULL AND e.dimension=vector_dims($6::vector)))
                     AND (($5::text IS NOT NULL AND b.search_vector @@ websearch_to_tsquery('simple',$5))
                          OR ($6::vector IS NOT NULL AND e.embedding IS NOT NULL AND e.dimension=vector_dims($6::vector)))
                 ), ranked AS (
                   SELECT *,row_number() OVER (ORDER BY retrieval_score DESC,page_number,ordinal,id) AS retrieval_rank
                   FROM candidates
                 )
                 SELECT id,document_id,page_number,section_path,text,content_hash,retrieval_score,retrieval_rank
                 FROM ranked ORDER BY retrieval_rank LIMIT $9",
            )
            .bind(request.project_id.as_uuid())
            .bind(request.study_id)
            .bind(request.report_id)
            .bind(request.document_id.map(|id| id.as_uuid()))
            .bind(lexical)
            .bind(vector)
            .bind(request.section_prefix)
            .bind(request.kind)
            .bind(i64::from(request.limit))
            .fetch_all(&self.pool).await.map_err(|_| AiError::Persistence("hybrid retrieval failed".to_owned()))?;
            rows.into_iter()
                .map(|row| {
                    let rank = row.get::<i64, _>("retrieval_rank") as u32;
                    let score = row.get::<f64, _>("retrieval_score");
                    let evidence = EvidenceRef::new(
                        deepref_domain::DocumentBlockId::new(row.get("id")),
                        row.get::<i32, _>("page_number") as u32,
                        row.get::<String, _>("content_hash"),
                    )?
                    .with_section_path(row.get("section_path"))
                    .with_retrieval(rank, score)?;
                    Ok(GroundedBlock {
                        evidence,
                        text: row.get("text"),
                        retrieval_rank: rank,
                        retrieval_score: score,
                    })
                })
                .collect()
        })
    }
}

impl ProposalStore for PostgresAiStore {
    fn find_for_run<'a>(&'a self, run_id: Uuid) -> AiFuture<'a, Option<AiProposal>> {
        Box::pin(async move {
            let row = sqlx::query("SELECT id,project_id,entity_type,entity_id,operation,payload,authority_tier,model_run_id,status,resolved_at,resolved_by_actor_id FROM ai_proposals WHERE model_run_id=$1 ORDER BY created_at DESC,id DESC LIMIT 1")
                .bind(run_id).fetch_optional(&self.pool).await.map_err(|_| AiError::Proposal("proposal lookup failed".to_owned()))?;
            row.map(proposal_from_row).transpose()
        })
    }
    fn create<'a>(&'a self, proposal: AiProposal) -> AiFuture<'a, AiProposal> {
        Box::pin(async move {
            let candidate = proposal.draft.payload.get("candidate");
            let target_report_id = payload_uuid(&proposal.draft.payload, "report_id")
                .or_else(|| candidate.and_then(|value| value_uuid(value, "candidate_report_id")))
                .or_else(|| {
                    (proposal.draft.entity_type == "screening_report")
                        .then_some(proposal.draft.entity_id)
                        .flatten()
                });
            let target_record_id = payload_uuid(&proposal.draft.payload, "source_record_id")
                .or_else(|| candidate.and_then(|value| value_uuid(value, "source_record_id")))
                .or_else(|| {
                    (proposal.draft.entity_type == "dedupe_record")
                        .then_some(proposal.draft.entity_id)
                        .flatten()
                });
            let target_study_id = payload_uuid(&proposal.draft.payload, "study_id")
                .or_else(|| {
                    proposal
                        .draft
                        .payload
                        .get("choice")
                        .and_then(|choice| value_uuid(choice, "study_id"))
                })
                .or_else(|| {
                    (proposal.draft.entity_type == "extraction_study")
                        .then_some(proposal.draft.entity_id)
                        .flatten()
                });
            let protocol_version_id = payload_uuid(&proposal.draft.payload, "protocol_version_id");
            let expected_revision = proposal
                .draft
                .payload
                .get("expected_revision")
                .and_then(serde_json::Value::as_i64);
            let operation_task_kind = match proposal.draft.operation.as_str() {
                "study_grouping_suggestion" => "study_grouping",
                operation => operation,
            };
            let task_kind = proposal
                .draft
                .payload
                .get("task_kind")
                .and_then(serde_json::Value::as_str)
                .unwrap_or(operation_task_kind);
            let mut transaction = self
                .pool
                .begin()
                .await
                .map_err(|_| AiError::Proposal("proposal transaction failed".to_owned()))?;
            let inserted = sqlx::query(
                "INSERT INTO ai_proposals
                 (id,project_id,ai_run_id,proposal_type,payload,status,entity_type,entity_id,operation,
                  model_run_id,authority_tier,task_kind,target_report_id,target_record_id,target_study_id,
                  protocol_version_id,expected_revision)
                 VALUES ($1,$2,$3,$4,$5,'pending',$6,$7,$8,$3,$9,$10,$11,$12,$13,$14,$15)
                 ON CONFLICT (model_run_id) DO NOTHING",
            ).bind(proposal.id).bind(proposal.draft.project_id.as_uuid()).bind(proposal.model_run_id)
            .bind(&proposal.draft.operation).bind(&proposal.draft.payload).bind(&proposal.draft.entity_type).bind(proposal.draft.entity_id)
            .bind(&proposal.draft.operation).bind(proposal.draft.authority.as_str()).bind(task_kind)
            .bind(target_report_id).bind(target_record_id).bind(target_study_id)
            .bind(protocol_version_id).bind(expected_revision)
            .execute(&mut *transaction).await.map_err(|_| AiError::Proposal("proposal write failed".to_owned()))?;
            let existing = sqlx::query(
                "SELECT id,project_id,entity_type,entity_id,operation,payload,authority_tier,
                        model_run_id,status,resolved_at,resolved_by_actor_id
                 FROM ai_proposals WHERE model_run_id=$1 ORDER BY created_at DESC,id DESC LIMIT 1",
            )
            .bind(proposal.model_run_id)
            .fetch_optional(&mut *transaction)
            .await
            .map_err(|_| AiError::Proposal("proposal lookup failed".to_owned()))?
            .map(proposal_from_row)
            .transpose()?
            .ok_or_else(|| AiError::Proposal("proposal write was not visible".to_owned()))?;
            if inserted.rows_affected() == 0
                && (existing.draft.project_id != proposal.draft.project_id
                    || existing.draft.entity_type != proposal.draft.entity_type
                    || existing.draft.entity_id != proposal.draft.entity_id
                    || existing.draft.operation != proposal.draft.operation
                    || existing.draft.payload != proposal.draft.payload
                    || existing.draft.authority != proposal.draft.authority
                    || existing.model_run_id != proposal.model_run_id)
            {
                return Err(AiError::Proposal(
                    "model run proposal idempotency conflict".to_owned(),
                ));
            }
            if inserted.rows_affected() == 1 {
                persist_typed_proposal_projection(&mut transaction, &existing).await?;
            }
            transaction
                .commit()
                .await
                .map_err(|_| AiError::Proposal("proposal transaction commit failed".to_owned()))?;
            Ok(existing)
        })
    }
}

fn payload_uuid(payload: &serde_json::Value, field: &str) -> Option<Uuid> {
    payload
        .get(field)
        .and_then(serde_json::Value::as_str)
        .and_then(|value| Uuid::parse_str(value).ok())
}

async fn persist_typed_proposal_projection(
    transaction: &mut Transaction<'_, Postgres>,
    proposal: &AiProposal,
) -> Result<(), AiError> {
    let Some(criteria) = proposal
        .draft
        .payload
        .get("criteria")
        .and_then(serde_json::Value::as_array)
    else {
        return Ok(());
    };
    let protocol_version_id = payload_uuid(&proposal.draft.payload, "protocol_version_id")
        .ok_or_else(|| AiError::Proposal("protocol version projection is invalid".to_owned()))?;
    let target_report_id = payload_uuid(&proposal.draft.payload, "report_id")
        .ok_or_else(|| AiError::Proposal("screening report projection is invalid".to_owned()))?;
    for (ordinal, criterion) in criteria.iter().enumerate() {
        let Some(criterion_id) = criterion
            .get("criterion_id")
            .and_then(serde_json::Value::as_str)
            .and_then(|value| Uuid::parse_str(value).ok())
        else {
            return Err(AiError::Proposal(
                "criterion projection is invalid".to_owned(),
            ));
        };
        let judgment = criterion
            .get("judgment")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| AiError::Proposal("criterion judgment is invalid".to_owned()))?;
        let rationale = criterion
            .get("rationale")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| AiError::Proposal("criterion rationale is invalid".to_owned()))?;
        sqlx::query(
            "INSERT INTO ai_proposal_criterion_judgments
             (proposal_id,project_id,criterion_id,protocol_version_id,ordinal,judgment,rationale,evidence)
             VALUES ($1,$2,$3,$4,$5,$6,$7,$8)",
        )
        .bind(proposal.id)
        .bind(proposal.draft.project_id.as_uuid())
        .bind(criterion_id)
        .bind(protocol_version_id)
        .bind(
            i32::try_from(ordinal)
                .map_err(|_| AiError::Proposal("criterion ordinal overflow".to_owned()))?,
        )
        .bind(judgment)
        .bind(rationale)
        .bind(
            criterion
                .get("evidence")
                .cloned()
                .unwrap_or_else(|| serde_json::json!([])),
        )
        .execute(&mut **transaction)
        .await
        .map_err(|_| AiError::Proposal("criterion projection write failed".to_owned()))?;

        if let Some(evidence) = criterion
            .get("evidence")
            .and_then(serde_json::Value::as_array)
        {
            for (evidence_ordinal, reference) in evidence.iter().enumerate() {
                let kind = reference.get("kind").and_then(serde_json::Value::as_str);
                let content_hash = reference
                    .get("content_hash")
                    .and_then(serde_json::Value::as_str)
                    .ok_or_else(|| AiError::Proposal("evidence hash is invalid".to_owned()))?;
                let (evidence_kind, report_id, document_id, document_block_id, page, source_field) =
                    match kind {
                        Some("report_metadata") => {
                            let report_id =
                                payload_uuid(reference, "report_id").ok_or_else(|| {
                                    AiError::Proposal(
                                        "metadata evidence report is invalid".to_owned(),
                                    )
                                })?;
                            if report_id != target_report_id {
                                return Err(AiError::Proposal(
                                    "metadata evidence is outside the proposal report".to_owned(),
                                ));
                            }
                            let source_field = reference
                                .get("field")
                                .and_then(serde_json::Value::as_str)
                                .ok_or_else(|| {
                                    AiError::Proposal(
                                        "metadata evidence field is invalid".to_owned(),
                                    )
                                })?;
                            (
                                "report_metadata",
                                report_id,
                                None,
                                None,
                                None,
                                Some(source_field),
                            )
                        }
                        Some("document_block") => {
                            let document_block_id = payload_uuid(reference, "document_block_id")
                                .ok_or_else(|| {
                                    AiError::Proposal(
                                        "document evidence block is invalid".to_owned(),
                                    )
                                })?;
                            let page = reference
                                .get("page")
                                .and_then(serde_json::Value::as_i64)
                                .and_then(|value| i32::try_from(value).ok())
                                .filter(|value| *value > 0)
                                .ok_or_else(|| {
                                    AiError::Proposal(
                                        "document evidence page is invalid".to_owned(),
                                    )
                                })?;
                            let document_id = sqlx::query_scalar::<_, Uuid>(
                                "SELECT d.id
                             FROM document_blocks b
                             JOIN documents d ON d.id=b.document_id
                             WHERE b.id=$1 AND d.project_id=$2 AND d.report_id=$3
                               AND b.content_hash=$4 AND b.active
                               AND d.active_parser_version=b.parser_version",
                            )
                            .bind(document_block_id)
                            .bind(proposal.draft.project_id.as_uuid())
                            .bind(target_report_id)
                            .bind(content_hash)
                            .fetch_optional(&mut **transaction)
                            .await
                            .map_err(|_| {
                                AiError::Proposal("document evidence lookup failed".to_owned())
                            })?
                            .ok_or_else(|| {
                                AiError::Proposal(
                                    "document evidence is outside the active proposal report"
                                        .to_owned(),
                                )
                            })?;
                            (
                                "document_block",
                                target_report_id,
                                Some(document_id),
                                Some(document_block_id),
                                Some(page),
                                None,
                            )
                        }
                        _ => return Err(AiError::Proposal("evidence kind is invalid".to_owned())),
                    };
                sqlx::query(
                    "INSERT INTO ai_proposal_evidence
                     (proposal_id,project_id,ordinal,evidence_kind,report_id,document_id,document_block_id,page,source_field,content_hash)
                     VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10)",
                )
                .bind(proposal.id)
                .bind(proposal.draft.project_id.as_uuid())
                .bind(i32::try_from(ordinal.saturating_mul(1000).saturating_add(evidence_ordinal)).map_err(|_| AiError::Proposal("evidence ordinal overflow".to_owned()))?)
                .bind(evidence_kind)
                .bind(report_id)
                .bind(document_id)
                .bind(document_block_id)
                .bind(page)
                .bind(source_field)
                .bind(content_hash)
                .execute(&mut **transaction)
                .await
                .map_err(|_| AiError::Proposal("evidence projection write failed".to_owned()))?;
            }
        }
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AiProposalCursor {
    pub created_at: DateTime<Utc>,
    pub id: Uuid,
}

#[derive(Debug, Clone, Copy)]
pub struct AiProposalFilters<'a> {
    pub status: Option<&'a str>,
    pub task_kind: Option<&'a str>,
    pub target_report_id: Option<Uuid>,
    pub target_record_id: Option<Uuid>,
    pub candidate_report_id: Option<Uuid>,
    pub target_study_id: Option<Uuid>,
}

const AI_PROPOSAL_SELECT: &str =
    "SELECT p.id,p.project_id,p.task_kind,p.entity_type,p.entity_id,p.operation,p.payload,
            p.authority_tier,p.model_run_id,r.provider,r.model,r.model_version,r.prompt_version,
            r.schema_version,r.prompt_hash,r.schema_hash,r.input_hash,r.evidence_hash,
            p.status,p.protocol_version_id,p.expected_revision,p.target_report_id,
            p.target_record_id,p.target_study_id,p.resolved_at,p.resolved_by_actor_kind,p.resolved_by_actor_id,
            p.resolution_reason,p.created_at
     FROM ai_proposals p JOIN ai_runs r ON r.id=p.model_run_id
     WHERE p.project_id=$1";

pub async fn get_ai_proposal(
    pool: &PgPool,
    project_id: Uuid,
    proposal_id: Uuid,
) -> Result<AiProposalRecord, AiProposalError> {
    let query = format!("{AI_PROPOSAL_SELECT} AND p.id=$2");
    let row = sqlx::query(sqlx::AssertSqlSafe(query))
        .bind(project_id)
        .bind(proposal_id)
        .fetch_optional(pool)
        .await?
        .ok_or(AiProposalError::NotFound)?;
    proposal_record_from_row(row).map_err(AiProposalError::InvalidPayload)
}

pub async fn list_ai_proposals(
    pool: &PgPool,
    project_id: Uuid,
    filters: AiProposalFilters<'_>,
    cursor: Option<AiProposalCursor>,
    limit: i64,
) -> Result<Vec<AiProposalRecord>, AiProposalError> {
    let status = filters.status.unwrap_or("pending");
    if !matches!(status, "pending" | "accepted" | "rejected" | "expired") {
        return Err(AiProposalError::InvalidPayload(
            "status is invalid".to_owned(),
        ));
    }
    let query = format!(
        "{AI_PROPOSAL_SELECT}
         AND p.status=$2 AND ($3::text IS NULL OR p.task_kind=$3)
         AND ($4::uuid IS NULL OR p.target_report_id=$4)
         AND ($5::uuid IS NULL OR p.target_record_id=$5)
         AND ($6::uuid IS NULL OR (p.task_kind='duplicate_candidate_detection'
                                   AND p.target_report_id=$6))
         AND ($7::uuid IS NULL OR p.target_study_id=$7)
         AND ($8::timestamptz IS NULL OR (p.created_at,p.id)<($8,$9))
         ORDER BY p.created_at DESC,p.id DESC LIMIT $10"
    );
    let rows = sqlx::query(sqlx::AssertSqlSafe(query))
        .bind(project_id)
        .bind(status)
        .bind(filters.task_kind)
        .bind(filters.target_report_id)
        .bind(filters.target_record_id)
        .bind(filters.candidate_report_id)
        .bind(filters.target_study_id)
        .bind(cursor.as_ref().map(|value| value.created_at))
        .bind(cursor.as_ref().map(|value| value.id))
        .bind(limit + 1)
        .fetch_all(pool)
        .await?;
    rows.into_iter()
        .map(proposal_record_from_row)
        .map(|result| result.map_err(AiProposalError::InvalidPayload))
        .collect()
}

pub async fn decide_ai_proposal(
    pool: &PgPool,
    request: AiProposalDecisionRequest,
) -> Result<AiProposalResolution, AiProposalError> {
    if request.reason.trim().is_empty() {
        return Err(AiProposalError::InvalidPayload(
            "resolution reason must not be empty".to_owned(),
        ));
    }
    if request.actor.id().trim().is_empty() {
        return Err(AiProposalError::InvalidActor);
    }
    let mut tx = pool.begin().await?;
    let query = format!("{AI_PROPOSAL_SELECT} AND p.id=$2 FOR UPDATE");
    let row = sqlx::query(sqlx::AssertSqlSafe(query))
        .bind(request.project_id)
        .bind(request.proposal_id)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or(AiProposalError::NotFound)?;
    let proposal = proposal_record_from_row(row).map_err(AiProposalError::InvalidPayload)?;
    if proposal.status != "pending" {
        return Err(AiProposalError::NotPending);
    }

    let applied_payload = if request.decision == AiProposalDecision::Accept {
        reviewed_payload_value(&proposal, request.reviewed_payload.as_ref())?
    } else {
        if request.reviewed_payload.is_some() {
            return Err(AiProposalError::InvalidPayload(
                "reviewed payload is only valid when accepting a proposal".to_owned(),
            ));
        }
        serde_json::Value::Null
    };
    let mut applied_revision = None;
    if request.decision == AiProposalDecision::Accept {
        match proposal.operation.as_str() {
            "screening_suggestion" => {
                let report_id = proposal.target_report_id.ok_or_else(|| {
                    AiProposalError::InvalidTarget("screening report is missing".to_owned())
                })?;
                let protocol_version_id = proposal.protocol_version_id.ok_or_else(|| {
                    AiProposalError::InvalidTarget("protocol version is missing".to_owned())
                })?;
                let expected_revision = proposal.expected_revision.ok_or_else(|| {
                    AiProposalError::InvalidTarget("screening revision is missing".to_owned())
                })?;
                let stage = parse_screening_stage(
                    applied_payload
                        .get("stage")
                        .and_then(serde_json::Value::as_str),
                )?;
                let decision = parse_screening_decision(&applied_payload, stage)?;
                let reason_id = screening_reason_id(&applied_payload)?;
                let actor = request.actor.clone();
                let snapshot = crate::screening::screen_report_in_transaction(
                    &mut tx,
                    ScreenReportCommand {
                        project_id: proposal.project_id.into(),
                        report_id: report_id.into(),
                        stage,
                        decision,
                        exclusion_reason_id: reason_id.map(Into::into),
                        protocol_version_id: protocol_version_id.into(),
                        expected_revision,
                        notes: Some("Accepted AI screening proposal".to_owned()),
                        actor,
                    },
                )
                .await?;
                applied_revision = Some(snapshot.revision);
            }
            "dedupe_suggestion" => {
                let candidate = applied_payload.get("candidate").ok_or_else(|| {
                    AiProposalError::InvalidTarget("duplicate candidate is missing".to_owned())
                })?;
                let source_record_id =
                    value_uuid(candidate, "source_record_id").ok_or_else(|| {
                        AiProposalError::InvalidTarget("source record is missing".to_owned())
                    })?;
                let candidate_report_id =
                    value_uuid(candidate, "candidate_report_id").ok_or_else(|| {
                        AiProposalError::InvalidTarget("candidate report is missing".to_owned())
                    })?;
                let decision = applied_payload
                    .get("decision")
                    .and_then(serde_json::Value::as_str)
                    .ok_or_else(|| {
                        AiProposalError::InvalidPayload("duplicate decision is missing".to_owned())
                    })?;
                if decision != "match" {
                    return Err(AiProposalError::InvalidPayload(
                        "only a duplicate match can be accepted; reject abstentions or no-match suggestions"
                            .to_owned(),
                    ));
                }
                crate::deduplication::resolve_record_in_transaction(
                    &mut tx,
                    ResolveRecordCommand {
                        project_id: proposal.project_id.into(),
                        record_id: source_record_id.into(),
                        action: deepref_application::RecordResolutionAction::Link,
                        report_id: Some(candidate_report_id.into()),
                        proposal_id: None,
                        reason: request.reason.clone(),
                        actor_kind: request.actor.kind().as_str().to_owned(),
                        actor_id: request.actor.id().to_owned(),
                    },
                )
                .await?;
            }
            "study_grouping_suggestion" => {
                apply_study_grouping(&mut tx, &proposal, &applied_payload, &request.actor).await?;
            }
            "appraisal_prefill" => {
                apply_appraisal_prefill(&mut tx, &proposal, &applied_payload, &request.actor)
                    .await?;
            }
            "data_extraction" => {
                let extraction: deepref_ai::DataExtraction =
                    serde_json::from_value(applied_payload.clone()).map_err(|error| {
                        AiProposalError::InvalidPayload(format!(
                            "data extraction payload is invalid: {error}"
                        ))
                    })?;
                let study_id = proposal.target_study_id.ok_or_else(|| {
                    AiProposalError::InvalidTarget("extraction study is missing".to_owned())
                })?;
                crate::extraction::apply_data_extraction_in_transaction(
                    &mut tx,
                    proposal.project_id.into(),
                    study_id,
                    proposal.id,
                    &extraction,
                    &request.actor,
                )
                .await?;
            }
            operation => {
                return Err(AiProposalError::InvalidTarget(format!(
                    "operation {operation} cannot be accepted"
                )));
            }
        }
    }

    let status = match request.decision {
        AiProposalDecision::Accept => "accepted",
        AiProposalDecision::Reject => "rejected",
    };
    let updated = sqlx::query(
        "UPDATE ai_proposals
         SET status=$3,resolved_at=now(),resolved_by_actor_kind=$4,resolved_by_actor_id=$5,
             resolution_reason=$6,decided_by=$5,decided_at=now()
         WHERE project_id=$1 AND id=$2 AND status='pending'",
    )
    .bind(request.project_id)
    .bind(request.proposal_id)
    .bind(status)
    .bind(request.actor.kind().as_str())
    .bind(request.actor.id())
    .bind(&request.reason)
    .execute(&mut *tx)
    .await?;
    if updated.rows_affected() != 1 {
        return Err(AiProposalError::NotPending);
    }
    sqlx::query(
        "INSERT INTO review_events
         (id,project_id,event_type,aggregate_type,aggregate_id,payload,actor_kind,actor_id)
         VALUES ($1,$2,'ai_proposal_resolved','ai_proposal',$3,$4,$5,$6)",
    )
    .bind(Uuid::new_v4())
    .bind(request.project_id)
    .bind(request.proposal_id)
    .bind(serde_json::json!({
        "status": status,
        "operation": proposal.operation,
        "applied_revision": applied_revision,
        "applied_payload": applied_payload,
    }))
    .bind(request.actor.kind().as_str())
    .bind(request.actor.id())
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(AiProposalResolution {
        proposal_id: request.proposal_id,
        status: status.to_owned(),
        target_report_id: proposal.target_report_id,
        target_record_id: proposal.target_record_id,
        applied_revision,
    })
}

fn value_uuid(value: &serde_json::Value, field: &str) -> Option<Uuid> {
    value
        .get(field)
        .and_then(serde_json::Value::as_str)
        .and_then(|value| Uuid::parse_str(value).ok())
}

fn reviewed_payload_value(
    proposal: &AiProposalRecord,
    reviewed: Option<&ReviewedAiProposalPayload>,
) -> Result<serde_json::Value, AiProposalError> {
    let Some(reviewed) = reviewed else {
        return Ok(proposal.payload.clone());
    };
    match (proposal.operation.as_str(), reviewed) {
        ("appraisal_prefill", ReviewedAiProposalPayload::AppraisalPrefill(reviewed)) => {
            let original: deepref_ai::AppraisalPrefill =
                serde_json::from_value(proposal.payload.clone()).map_err(|error| {
                    AiProposalError::InvalidPayload(format!(
                        "stored appraisal prefill payload is invalid: {error}"
                    ))
                })?;
            if original.report_id != reviewed.report_id
                || original.definition_id != reviewed.definition_id
                || original.definition_version != reviewed.definition_version
            {
                return Err(AiProposalError::InvalidPayload(
                    "reviewed appraisal must retain the original report and definition version"
                        .to_owned(),
                ));
            }
            serde_json::to_value(reviewed).map_err(|error| {
                AiProposalError::InvalidPayload(format!(
                    "reviewed appraisal is not serializable: {error}"
                ))
            })
        }
        ("data_extraction", ReviewedAiProposalPayload::DataExtraction(reviewed)) => {
            let original: deepref_ai::DataExtraction =
                serde_json::from_value(proposal.payload.clone()).map_err(|error| {
                    AiProposalError::InvalidPayload(format!(
                        "stored extraction payload is invalid: {error}"
                    ))
                })?;
            if original.study_id != reviewed.study_id
                || extraction_field_set(&original) != extraction_field_set(reviewed)
            {
                return Err(AiProposalError::InvalidPayload(
                    "reviewed extraction must retain the original study and field versions"
                        .to_owned(),
                ));
            }
            serde_json::to_value(reviewed).map_err(|error| {
                AiProposalError::InvalidPayload(format!(
                    "reviewed extraction is not serializable: {error}"
                ))
            })
        }
        _ => Err(AiProposalError::InvalidPayload(
            "reviewed payload variant does not match this proposal operation".to_owned(),
        )),
    }
}

fn extraction_field_set(
    extraction: &deepref_ai::DataExtraction,
) -> std::collections::BTreeSet<(Uuid, u32)> {
    extraction
        .fields
        .iter()
        .map(|field| match field {
            deepref_ai::ExtractedField::Value {
                field_id,
                field_version,
                ..
            }
            | deepref_ai::ExtractedField::InsufficientEvidence {
                field_id,
                field_version,
                ..
            } => (*field_id, *field_version),
        })
        .collect()
}

async fn validate_study_grouping_provenance(
    tx: &mut Transaction<'_, Postgres>,
    project_id: Uuid,
    target_report_id: Uuid,
    grouping: &deepref_ai::StudyGroupingProposal,
) -> Result<(), AiProposalError> {
    let mut seen = std::collections::BTreeSet::new();
    if grouping.provenance.len() > 256 {
        return Err(AiProposalError::InvalidPayload(
            "grouping provenance exceeds the acceptance bound".to_owned(),
        ));
    }
    for evidence in &grouping.provenance {
        let key = serde_json::to_string(evidence).map_err(|error| {
            AiProposalError::InvalidPayload(format!("grouping provenance is invalid: {error}"))
        })?;
        if !seen.insert(key) {
            return Err(AiProposalError::InvalidPayload(
                "grouping provenance contains a duplicate entry".to_owned(),
            ));
        }
        let (current_hash, scope_is_valid) = match evidence {
            deepref_ai::StudyGroupingEvidence::ReportMetadata {
                report_id, field, ..
            } => {
                let row = sqlx::query(
                    "SELECT r.title,r.abstract_text,r.publication_year,r.authors
                     FROM project_reports pr JOIN reports r ON r.id=pr.report_id
                     WHERE pr.project_id=$1 AND pr.report_id=$2",
                )
                .bind(project_id)
                .bind(report_id)
                .fetch_optional(&mut **tx)
                .await?;
                let current_hash = row.and_then(|row| {
                    grouping_metadata_hash(
                        *field,
                        row.get("title"),
                        row.get("abstract_text"),
                        row.get("publication_year"),
                        first_author(row.get("authors")),
                    )
                });
                (current_hash, *report_id == target_report_id)
            }
            deepref_ai::StudyGroupingEvidence::StudyMetadata {
                study_id, field, ..
            } => {
                let row = sqlx::query("SELECT title FROM studies WHERE project_id=$1 AND id=$2")
                    .bind(project_id)
                    .bind(study_id)
                    .fetch_optional(&mut **tx)
                    .await?;
                let current_hash = row.and_then(|row| {
                    grouping_metadata_hash(
                        *field,
                        Some(row.get::<String, _>("title")),
                        None,
                        None,
                        None,
                    )
                });
                (current_hash, true)
            }
            deepref_ai::StudyGroupingEvidence::StudyReportMetadata {
                study_id,
                report_id,
                field,
                ..
            } => {
                let row = sqlx::query(
                    "SELECT r.title,r.abstract_text,r.publication_year,r.authors
                     FROM study_reports sr
                     JOIN reports r ON r.id=sr.report_id
                     WHERE sr.project_id=$1 AND sr.study_id=$2 AND sr.report_id=$3",
                )
                .bind(project_id)
                .bind(study_id)
                .bind(report_id)
                .fetch_optional(&mut **tx)
                .await?;
                let current_hash = row.and_then(|row| {
                    grouping_metadata_hash(
                        *field,
                        row.get("title"),
                        row.get("abstract_text"),
                        row.get("publication_year"),
                        first_author(row.get("authors")),
                    )
                });
                (current_hash, true)
            }
        };
        let expected_hash = match evidence {
            deepref_ai::StudyGroupingEvidence::ReportMetadata { content_hash, .. }
            | deepref_ai::StudyGroupingEvidence::StudyMetadata { content_hash, .. }
            | deepref_ai::StudyGroupingEvidence::StudyReportMetadata { content_hash, .. } => {
                content_hash
            }
        };
        if !scope_is_valid
            || !deepref_ai::is_sha256(expected_hash)
            || current_hash.as_deref() != Some(expected_hash)
        {
            return Err(AiProposalError::InvalidPayload(
                "grouping provenance is stale or outside the target project".to_owned(),
            ));
        }
    }
    Ok(())
}

fn grouping_metadata_hash(
    field: deepref_ai::StudyGroupingField,
    title: Option<String>,
    abstract_text: Option<String>,
    publication_year: Option<i32>,
    first_author: Option<String>,
) -> Option<String> {
    let value = match field {
        deepref_ai::StudyGroupingField::Title => title,
        deepref_ai::StudyGroupingField::Abstract => abstract_text,
        deepref_ai::StudyGroupingField::PublicationYear => {
            publication_year.map(|year| year.to_string())
        }
        deepref_ai::StudyGroupingField::FirstAuthor => first_author,
    }?;
    Some(deepref_ai::sha256_bytes(value.as_bytes()))
}

async fn apply_study_grouping(
    tx: &mut Transaction<'_, Postgres>,
    proposal: &AiProposalRecord,
    payload: &serde_json::Value,
    actor: &Actor,
) -> Result<(), AiProposalError> {
    let grouping: deepref_ai::StudyGroupingProposal = serde_json::from_value(payload.clone())
        .map_err(|error| {
            AiProposalError::InvalidPayload(format!("study grouping payload is invalid: {error}"))
        })?;
    let report_id = proposal
        .target_report_id
        .ok_or_else(|| AiProposalError::InvalidTarget("grouping report is missing".to_owned()))?;
    if grouping.report_id != report_id
        || grouping.rationale.trim().is_empty()
        || grouping.provenance.is_empty()
        || grouping.provenance.iter().any(|evidence| {
            !deepref_ai::is_sha256(match evidence {
                deepref_ai::StudyGroupingEvidence::ReportMetadata { content_hash, .. }
                | deepref_ai::StudyGroupingEvidence::StudyMetadata { content_hash, .. }
                | deepref_ai::StudyGroupingEvidence::StudyReportMetadata { content_hash, .. } => {
                    content_hash
                }
            })
        })
    {
        return Err(AiProposalError::InvalidPayload(
            "grouping rationale or provenance is invalid".to_owned(),
        ));
    }
    validate_study_grouping_provenance(tx, proposal.project_id, report_id, &grouping).await?;
    let previous_id = grouping.expected_previous_study_id.map(Into::into);
    let previous_revision = grouping
        .expected_previous_study_revision
        .map(|revision| {
            u64::try_from(revision).map_err(|_| {
                AiProposalError::InvalidPayload("previous study revision is invalid".to_owned())
            })
        })
        .transpose()?;
    match grouping.choice {
        deepref_ai::StudyGroupingChoice::ExistingStudy {
            study_id,
            expected_revision,
        } => {
            let expected_revision = u64::try_from(expected_revision).map_err(|_| {
                AiProposalError::InvalidPayload("target study revision is invalid".to_owned())
            })?;
            let study_id = study_id.into();
            if Some(study_id) == previous_id {
                return Err(AiProposalError::InvalidTarget(
                    "grouping proposal does not change study membership".to_owned(),
                ));
            }
            crate::study::assign_report_to_study_in_transaction(
                tx,
                deepref_application::AssignReportToStudy {
                    project_id: proposal.project_id.into(),
                    study_id,
                    report_id: report_id.into(),
                    role: StudyReportRole::ReportOfStudy,
                    expected_revision,
                    expected_previous_study_id: previous_id,
                    expected_previous_study_revision: previous_revision,
                    actor: actor.clone(),
                },
            )
            .await?;
        }
        deepref_ai::StudyGroupingChoice::NewStudy { title } => {
            let title = StudyTitle::new(title)
                .map_err(|error| AiProposalError::InvalidPayload(error.to_string()))?;
            crate::study::create_study_and_assign_report_in_transaction(
                tx,
                deepref_application::CreateStudy {
                    project_id: proposal.project_id.into(),
                    study_id: Uuid::new_v4().into(),
                    title,
                    actor: actor.clone(),
                },
                report_id.into(),
                StudyReportRole::ReportOfStudy,
                previous_id,
                previous_revision,
            )
            .await?;
        }
    }
    Ok(())
}

async fn apply_appraisal_prefill(
    tx: &mut Transaction<'_, Postgres>,
    proposal: &AiProposalRecord,
    payload: &serde_json::Value,
    actor: &Actor,
) -> Result<(), AiProposalError> {
    let prefill: deepref_ai::AppraisalPrefill =
        serde_json::from_value(payload.clone()).map_err(|error| {
            AiProposalError::InvalidPayload(format!(
                "appraisal prefill payload is invalid: {error}"
            ))
        })?;
    let report_id = proposal
        .target_report_id
        .ok_or_else(|| AiProposalError::InvalidTarget("appraisal report is missing".to_owned()))?;
    if prefill.report_id != report_id || prefill.answers.is_empty() {
        return Err(AiProposalError::InvalidTarget(
            "appraisal payload targets another report".to_owned(),
        ));
    }
    let definition_id = DefinitionId::new(prefill.definition_id.clone())
        .map_err(|error| AiProposalError::InvalidPayload(error.to_string()))?;
    let definition_version =
        DefinitionVersion::new(prefill.definition_version).ok_or_else(|| {
            AiProposalError::InvalidPayload("appraisal definition version is invalid".to_owned())
        })?;
    let definition = get_appraisal_definition(definition_id.as_str(), definition_version.get())
        .map_err(|error| AiProposalError::InvalidPayload(error.to_string()))?;
    let questions = definition
        .domains
        .iter()
        .flat_map(|domain| domain.questions.iter())
        .map(|question| deepref_ai::AppraisalPrefillQuestion {
            id: question.id.clone(),
            answer_schema: match &question.answer_schema {
                deepref_application::AnswerSchema::Enum { options } => {
                    deepref_ai::AppraisalAnswerSchema::Enum {
                        options: options.iter().map(|option| option.value.clone()).collect(),
                    }
                }
                deepref_application::AnswerSchema::Boolean => {
                    deepref_ai::AppraisalAnswerSchema::Boolean
                }
                deepref_application::AnswerSchema::Scale { min, max, .. } => {
                    deepref_ai::AppraisalAnswerSchema::Scale {
                        min: *min,
                        max: *max,
                    }
                }
                deepref_application::AnswerSchema::Text { max_length } => {
                    deepref_ai::AppraisalAnswerSchema::Text {
                        max_length: *max_length,
                    }
                }
            },
            required: question.required,
            requires_evidence: question.requires_evidence,
        })
        .collect::<Vec<_>>();
    let domains = definition
        .domains
        .iter()
        .map(|domain| deepref_ai::AppraisalPrefillDomain {
            id: domain.id.clone(),
            allowed_judgments: domain
                .judgment
                .options
                .iter()
                .map(|option| option.value.clone())
                .collect(),
            required: domain.judgment.required,
        })
        .collect::<Vec<_>>();
    let overall_allowed_judgments = definition
        .overall_judgment
        .options
        .iter()
        .map(|option| option.value.clone())
        .collect::<Vec<_>>();
    let grounded_evidence = prefill
        .answers
        .iter()
        .flat_map(|answer| answer.evidence.iter().cloned())
        .collect::<Vec<_>>();
    let task_input = deepref_ai::AppraisalPrefillInput {
        project_id: proposal.project_id.into(),
        report_id: report_id.into(),
        definition_id: definition_id.as_str().to_owned(),
        definition_version: definition_version.get(),
        questions,
        domains,
        overall_allowed_judgments,
        report_title: None,
        report_abstract: None,
        grounded_evidence,
    };
    let task = deepref_ai::AppraisalPrefillTask::new(&task_input)
        .map_err(|error| AiProposalError::InvalidPayload(error.to_string()))?;
    <deepref_ai::AppraisalPrefillTask as deepref_ai::AiTask>::semantic_validate(&task, &prefill)
        .map_err(|error| AiProposalError::InvalidPayload(error.to_string()))?;
    let mut responses = serde_json::Map::new();
    let mut evidence = Vec::new();
    for answer in &prefill.answers {
        let response = match &answer.answer {
            deepref_ai::AppraisalAnswerValue::Enum { value }
            | deepref_ai::AppraisalAnswerValue::Text { value } => {
                serde_json::Value::String(value.clone())
            }
            deepref_ai::AppraisalAnswerValue::Boolean { value } => serde_json::Value::Bool(*value),
            deepref_ai::AppraisalAnswerValue::Scale { value } => {
                serde_json::json!(value)
            }
        };
        if responses
            .insert(answer.question_id.clone(), response)
            .is_some()
        {
            return Err(AiProposalError::InvalidPayload(
                "appraisal payload contains a duplicate question".to_owned(),
            ));
        }
        for source in &answer.evidence {
            let page = source.page;
            if !deepref_ai::is_sha256(&source.content_hash)
                || source.parser_version.trim().is_empty()
            {
                return Err(AiProposalError::InvalidPayload(
                    "appraisal evidence provenance is invalid".to_owned(),
                ));
            }
            evidence.push(EvidenceReferenceInput {
                question_id: answer.question_id.clone(),
                document_id: source.document_id,
                block_id: source.document_block_id,
                page: Some(page),
                parser_version: Some(source.parser_version.clone()),
                content_hash: Some(source.content_hash.clone()),
            });
        }
    }
    let input = AppraisalAssessmentInput {
        definition_id,
        definition_version,
        responses: serde_json::Value::Object(responses),
        evidence,
        domain_judgments: prefill.domain_judgments,
        overall_judgment: Some(prefill.overall_judgment),
    };
    crate::appraisal::complete_appraisal_in_transaction(
        tx,
        proposal.project_id.into(),
        report_id.into(),
        input,
        actor.clone(),
    )
    .await?;
    Ok(())
}

fn parse_screening_stage(value: Option<&str>) -> Result<ScreeningStage, AiProposalError> {
    match value {
        Some("title_abstract") => Ok(ScreeningStage::TitleAbstract),
        Some("full_text") => Ok(ScreeningStage::FullText),
        _ => Err(AiProposalError::InvalidPayload(
            "screening stage is invalid".to_owned(),
        )),
    }
}

fn parse_screening_decision(
    payload: &serde_json::Value,
    stage: ScreeningStage,
) -> Result<ScreeningDecision, AiProposalError> {
    let kind = payload
        .get("suggested_decision")
        .and_then(|value| value.get("kind"))
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| {
            AiProposalError::InvalidPayload("suggested decision is missing".to_owned())
        })?;
    match (stage, kind) {
        (_, "include") => Ok(ScreeningDecision::Include),
        (_, "maybe") => Ok(ScreeningDecision::Maybe),
        (_, "exclude") => Ok(ScreeningDecision::Exclude),
        (_, "insufficient_evidence") => Err(AiProposalError::InvalidPayload(
            "insufficient evidence must be reviewed rather than accepted as a decision".to_owned(),
        )),
        _ => Err(AiProposalError::InvalidPayload(
            "suggested decision is invalid".to_owned(),
        )),
    }
}

fn screening_reason_id(payload: &serde_json::Value) -> Result<Option<Uuid>, AiProposalError> {
    let Some(decision) = payload.get("suggested_decision") else {
        return Err(AiProposalError::InvalidPayload(
            "suggested decision is missing".to_owned(),
        ));
    };
    if decision.get("kind").and_then(serde_json::Value::as_str) != Some("exclude") {
        return Ok(None);
    }
    decision
        .get("exclusion_reason_id")
        .and_then(serde_json::Value::as_str)
        .map(|value| {
            Uuid::parse_str(value).map_err(|_| {
                AiProposalError::InvalidPayload("exclusion reason is invalid".to_owned())
            })
        })
        .transpose()
}

fn proposal_record_from_row(row: sqlx::postgres::PgRow) -> Result<AiProposalRecord, String> {
    Ok(AiProposalRecord {
        id: row.get("id"),
        project_id: row.get("project_id"),
        task_kind: row.get("task_kind"),
        entity_type: row.get("entity_type"),
        entity_id: row.get("entity_id"),
        operation: row.get("operation"),
        payload: row.get("payload"),
        authority_tier: row.get("authority_tier"),
        model_run_id: row.get("model_run_id"),
        provider: row.get("provider"),
        model: row.get("model"),
        model_version: row.get("model_version"),
        prompt_version: row.get("prompt_version"),
        schema_version: row.get("schema_version"),
        status: row.get("status"),
        protocol_version_id: row.get("protocol_version_id"),
        expected_revision: row.get("expected_revision"),
        target_report_id: row.get("target_report_id"),
        target_record_id: row.get("target_record_id"),
        target_study_id: row.get("target_study_id"),
        prompt_hash: row.get("prompt_hash"),
        schema_hash: row.get("schema_hash"),
        input_hash: row.get("input_hash"),
        evidence_hash: row.get("evidence_hash"),
        resolved_at: row.get("resolved_at"),
        resolved_by_actor_kind: row.get("resolved_by_actor_kind"),
        resolved_by_actor_id: row.get("resolved_by_actor_id"),
        resolution_reason: row.get("resolution_reason"),
        created_at: row.get("created_at"),
    })
}

pub async fn insert_model_route(
    pool: &PgPool,
    route: &ResolvedModel,
    effective_from: DateTime<Utc>,
) -> anyhow::Result<Uuid> {
    route.validate().map_err(|error| anyhow::anyhow!(error))?;
    let id = route.route_id.unwrap_or_else(Uuid::new_v4);
    let parameters = serde_json::to_value(&route.parameters)?;
    let mut transaction = pool.begin().await?;
    let inserted = sqlx::query("INSERT INTO ai_model_routes (id,profile,provider,model,model_version,parameters,effective_from) VALUES ($1,$2,$3,$4,$5,$6,$7) ON CONFLICT (id) DO NOTHING")
        .bind(id).bind(route.profile.as_str()).bind(&route.provider).bind(&route.model).bind(&route.model_version).bind(&parameters).bind(effective_from).execute(&mut *transaction).await?;
    if inserted.rows_affected() == 0 {
        let existing = sqlx::query("SELECT profile,provider,model,model_version,parameters,effective_from FROM ai_model_routes WHERE id=$1 FOR UPDATE")
            .bind(id).fetch_optional(&mut *transaction).await?
            .ok_or_else(|| anyhow::anyhow!("model route conflict disappeared"))?;
        let matches = existing.get::<String, _>("profile") == route.profile.as_str()
            && existing.get::<String, _>("provider") == route.provider
            && existing.get::<String, _>("model") == route.model
            && existing.get::<String, _>("model_version") == route.model_version
            && existing.get::<serde_json::Value, _>("parameters") == parameters
            && existing
                .get::<DateTime<Utc>, _>("effective_from")
                .timestamp_micros()
                == effective_from.timestamp_micros();
        if !matches {
            return Err(anyhow::anyhow!("model route identifiers are immutable"));
        }
    }
    transaction.commit().await?;
    Ok(id)
}

pub async fn resolve_ai_proposal(
    pool: &PgPool,
    proposal_id: Uuid,
    accepted: bool,
    actor_kind: &str,
    actor_id: &str,
    reason: Option<&str>,
) -> anyhow::Result<bool> {
    if !matches!(actor_kind, "user" | "automation" | "system") || actor_id.trim().is_empty() {
        anyhow::bail!("proposal resolution requires a valid actor");
    }
    let status = if accepted { "accepted" } else { "rejected" };
    let result = sqlx::query("UPDATE ai_proposals SET status=$2,resolved_at=now(),resolved_by_actor_kind=$3,resolved_by_actor_id=$4,resolution_reason=$5,decided_by=$4,decided_at=now() WHERE id=$1 AND status='pending'")
        .bind(proposal_id).bind(status).bind(actor_kind).bind(actor_id).bind(reason).execute(pool).await?;
    Ok(result.rows_affected() == 1)
}

pub async fn persist_document_block_embedding(
    pool: &PgPool,
    document_block_id: Uuid,
    content_hash: &str,
    model_identifier: &str,
    generation: &str,
    embedding: &Embedding,
) -> anyhow::Result<bool> {
    if !deepref_ai::is_sha256(content_hash)
        || model_identifier.trim().is_empty()
        || generation.trim().is_empty()
    {
        anyhow::bail!("embedding metadata is invalid");
    }
    let mut transaction = pool.begin().await?;
    sqlx::query("SELECT id FROM document_blocks WHERE id=$1 FOR UPDATE")
        .bind(document_block_id)
        .fetch_optional(&mut *transaction)
        .await?
        .ok_or_else(|| anyhow::anyhow!("document block does not exist"))?;
    let eligible: bool = sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM document_blocks b JOIN documents d ON d.id=b.document_id WHERE b.id=$1 AND b.content_hash=$2 AND b.active AND b.parser_version=d.active_parser_version)")
        .bind(document_block_id).bind(content_hash).fetch_one(&mut *transaction).await?;
    if !eligible {
        transaction.rollback().await?;
        return Ok(false);
    }
    let vector = Vector::from(embedding.as_slice().to_vec());
    if let Some(existing) = sqlx::query("SELECT dimension,embedding FROM document_block_embeddings WHERE document_block_id=$1 AND model_identifier=$2 AND generation=$3 AND content_hash=$4 FOR UPDATE")
        .bind(document_block_id).bind(model_identifier).bind(generation).bind(content_hash).fetch_optional(&mut *transaction).await? {
        let existing_dimension = existing.get::<i32, _>("dimension");
        let existing_vector = existing.get::<Vector, _>("embedding");
        if existing_dimension != i32::try_from(embedding.dimension())?
            || existing_vector.as_slice() != vector.as_slice()
        {
            return Err(anyhow::anyhow!("embedding generation is immutable"));
        }
        transaction.commit().await?;
        return Ok(true);
    }
    sqlx::query("UPDATE document_block_embeddings SET is_current=false WHERE document_block_id=$1 AND is_current")
        .bind(document_block_id).execute(&mut *transaction).await?;
    let inserted = sqlx::query("INSERT INTO document_block_embeddings (document_block_id,model_identifier,generation,content_hash,dimension,embedding,is_current) VALUES ($1,$2,$3,$4,$5,$6,true)")
        .bind(document_block_id).bind(model_identifier).bind(generation).bind(content_hash).bind(i32::try_from(embedding.dimension())?).bind(vector).execute(&mut *transaction).await?;
    if inserted.rows_affected() != 1 {
        return Err(anyhow::anyhow!(
            "embedding generation insert affected no row"
        ));
    }
    transaction.commit().await?;
    Ok(true)
}

fn ai_run_from_row(row: sqlx::postgres::PgRow) -> Result<AiRunRecord, AiError> {
    let task_kind = AiTaskKind::parse(&row.get::<String, _>("task_kind"))
        .ok_or_else(|| AiError::Persistence("unknown AI task kind".to_owned()))?;
    let profile = ModelProfile::parse(&row.get::<String, _>("profile"))
        .ok_or_else(|| AiError::Persistence("unknown AI profile".to_owned()))?;
    let status = AiRunStatus::parse(&row.get::<String, _>("status"))
        .ok_or_else(|| AiError::Persistence("unknown AI run status".to_owned()))?;
    let parameters = serde_json::from_value::<ModelParameters>(row.get("parameters"))
        .map_err(|_| AiError::Persistence("stored route parameters are invalid".to_owned()))?;
    let evidence_refs = serde_json::from_value::<Vec<EvidenceRef>>(row.get("evidence_refs"))
        .map_err(|_| AiError::Persistence("stored evidence is invalid".to_owned()))?;
    Ok(AiRunRecord {
        id: row.get("id"),
        project_id: row
            .get::<Option<Uuid>, _>("project_id")
            .map(deepref_domain::ProjectId::new),
        task_kind,
        route: ResolvedModel {
            profile,
            provider: row.get("provider"),
            model: row.get("model"),
            model_version: row.get("model_version"),
            parameters,
            route_id: None,
        },
        prompt_version: row.get("prompt_version"),
        prompt_hash: row.get("prompt_hash"),
        schema_version: row.get("schema_version"),
        schema_hash: row.get("schema_hash"),
        input_hash: row.get("input_hash"),
        reuse_hash: row.get("reuse_hash"),
        protocol_hash: row.get("protocol_hash"),
        document_hash: row.get("document_hash"),
        evidence_hash: row.get("evidence_hash"),
        evidence_refs,
        usage: deepref_ai::TokenUsage {
            input_tokens: u64::try_from(row.get::<i64, _>("input_tokens"))
                .map_err(|_| AiError::Persistence("input tokens are invalid".to_owned()))?,
            output_tokens: u64::try_from(row.get::<i64, _>("output_tokens"))
                .map_err(|_| AiError::Persistence("output tokens are invalid".to_owned()))?,
        },
        cost_micros: row.get("cost_micros"),
        output: row.get("output"),
        status,
        error: row
            .get::<Option<String>, _>("error_code")
            .zip(row.get::<Option<String>, _>("error_message"))
            .map(|(code, message)| deepref_ai::SafeErrorMetadata { code, message }),
        parent_automation_run_id: row.get("parent_automation_run_id"),
        created_at: row.get("created_at"),
        completed_at: row.get("completed_at"),
    })
}

fn proposal_from_row(row: sqlx::postgres::PgRow) -> Result<AiProposal, AiError> {
    let authority = AuthorityTier::parse(&row.get::<String, _>("authority_tier"))
        .ok_or_else(|| AiError::Proposal("stored proposal authority is invalid".to_owned()))?;
    let status = match row.get::<String, _>("status").as_str() {
        "pending" => ProposalStatus::Pending,
        "accepted" => ProposalStatus::Accepted,
        "rejected" => ProposalStatus::Rejected,
        "expired" => ProposalStatus::Expired,
        _ => {
            return Err(AiError::Proposal(
                "stored proposal status is invalid".to_owned(),
            ));
        }
    };
    Ok(AiProposal {
        id: row.get("id"),
        draft: ProposalDraft {
            project_id: deepref_domain::ProjectId::new(row.get("project_id")),
            entity_type: row.get("entity_type"),
            entity_id: row.get("entity_id"),
            operation: row.get("operation"),
            payload: row.get("payload"),
            authority,
        },
        model_run_id: row.get("model_run_id"),
        status,
        resolved_at: row.get("resolved_at"),
        resolved_by_actor_id: row.get("resolved_by_actor_id"),
    })
}
