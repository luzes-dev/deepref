use std::collections::HashMap;

use chrono::{DateTime, Utc};
use deepref_ai::{
    AiError, AiFuture, AiProposal, AiRunRecord, AiRunStatus, AiRunStore, AiTask, AiTaskKind,
    AuthorityTier, ClassificationReportField, Embedding, EvidenceRef, EvidenceRetriever,
    GroundedBlock, ModelParameters, ModelProfile, ModelRouter, ProposalDraft, ProposalStatus,
    ProposalStore, ResolvedModel, RetrievalRequest, StudyDesignClassification,
    StudyDesignClassificationInput, StudyDesignEvidence, StudyDesignLabel, StudyMetadataField,
};
use deepref_application::{
    AppraisalAssessmentInput, ClassifyStudy, DefinitionId, DefinitionVersion,
    EvidenceReferenceInput, ResolveRecordCommand, ScreenReportCommand, get_appraisal_definition,
};
use deepref_domain::{
    Actor, ProjectId, ScreeningDecision, ScreeningStage, StudyReportRole, StudyTitle,
};
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
    fn find_reusable<'a>(
        &'a self,
        project_id: Option<ProjectId>,
        reuse_hash: &'a str,
    ) -> AiFuture<'a, Option<AiRunRecord>> {
        Box::pin(async move {
            let row = sqlx::query(
                "SELECT id,project_id,task_kind,profile,provider,model,model_version,parameters,
                        prompt_version,prompt_hash,schema_version,schema_hash,input_hash,reuse_hash,
                        protocol_hash,document_hash,evidence_hash,evidence_refs,input_tokens,
                        output_tokens,cost_micros,output,status,error_code,error_message,
                        parent_automation_run_id,created_at,completed_at
                 FROM ai_runs
                 WHERE project_id IS NOT DISTINCT FROM $1
                   AND reuse_hash=$2 AND status='completed'
                 ORDER BY completed_at DESC NULLS LAST,created_at DESC,id DESC LIMIT 1",
            )
            .bind(project_id.map(|id| id.as_uuid()))
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
                "study_design_classification_suggestion" => "study_design_classification",
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

mod acceptance;
mod persistence;
mod proposals;

use acceptance::*;
use persistence::{ai_run_from_row, proposal_from_row};
pub use persistence::{insert_model_route, persist_document_block_embedding, resolve_ai_proposal};
pub use proposals::{
    AiProposalCursor, AiProposalFilters, decide_ai_proposal, get_ai_proposal, list_ai_proposals,
};
