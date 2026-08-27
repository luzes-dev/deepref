use chrono::{DateTime, Utc};
use deepref_ai::{
    AiError, AiFuture, AiProposal, AiRunRecord, AiRunStatus, AiRunStore, AiTaskKind, AuthorityTier,
    Embedding, EvidenceRef, EvidenceRetriever, GroundedBlock, ModelParameters, ModelProfile,
    ModelRouter, ProposalDraft, ProposalStatus, ProposalStore, ResolvedModel, RetrievalRequest,
};
use deepref_application::{ResolveRecordCommand, ScreenReportCommand};
use deepref_domain::{Actor, ScreeningDecision, ScreeningStage};
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
    pub document_block_id: Uuid,
    pub page: u32,
    pub section_path: Vec<String>,
    pub text: String,
    pub content_hash: String,
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
) -> Result<Vec<AiGroundingBlock>, AiProposalError> {
    let rows = sqlx::query(
        "SELECT b.id,b.page_number,b.section_path,b.text,b.content_hash
         FROM project_reports pr
         JOIN documents d ON d.report_id=pr.report_id AND d.project_id=pr.project_id
         JOIN document_blocks b ON b.document_id=d.id
         WHERE pr.project_id=$1 AND pr.report_id=$2 AND d.active_parser_version=d.parser_version
           AND b.active AND b.parser_version=d.active_parser_version
         ORDER BY b.page_number,b.ordinal,b.id
         LIMIT 20",
    )
    .bind(project_id)
    .bind(report_id)
    .fetch_all(pool)
    .await?;
    rows.into_iter()
        .map(|row| {
            let page = row.get::<i32, _>("page_number");
            let text = row.get::<String, _>("text");
            let content_hash = row.get::<String, _>("content_hash");
            let page = u32::try_from(page).map_err(|_| {
                AiProposalError::InvalidPayload("document page is invalid".to_owned())
            })?;
            if page == 0 || text.trim().is_empty() || !deepref_ai::is_sha256(&content_hash) {
                return Err(AiProposalError::InvalidPayload(
                    "document grounding block is invalid".to_owned(),
                ));
            }
            Ok(AiGroundingBlock {
                document_block_id: row.get("id"),
                page,
                section_path: row.get("section_path"),
                text,
                content_hash,
            })
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AiProposalDecisionRequest {
    pub project_id: Uuid,
    pub proposal_id: Uuid,
    pub decision: AiProposalDecision,
    pub reason: String,
    pub actor: Actor,
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
                     GREATEST(CASE WHEN $4::text IS NULL THEN 0.0 ELSE ts_rank_cd(b.search_vector,websearch_to_tsquery('simple',$4)) END,0.0)
                     + CASE WHEN $5::vector IS NULL THEN 0.0
                            WHEN e.embedding IS NOT NULL AND e.dimension=vector_dims($5::vector)
                            THEN GREATEST(0.0,1.0-(e.embedding <=> $5::vector))
                            ELSE 0.0 END AS retrieval_score
                   FROM document_blocks b
                   JOIN documents d ON d.id=b.document_id
                   LEFT JOIN document_block_embeddings e ON e.document_block_id=b.id
                     AND e.is_current AND e.content_hash=b.content_hash
                   WHERE d.project_id=$1 AND ($2::uuid IS NULL OR d.report_id=$2)
                     AND ($3::uuid IS NULL OR b.document_id=$3)
                     AND b.active AND b.parser_version=d.active_parser_version
                     AND ($6::text[] IS NULL OR cardinality($6::text[])=0
                          OR b.section_path[1:cardinality($6::text[])]= $6::text[])
                     AND ($7::text IS NULL OR b.kind=$7)
                     AND ($5::vector IS NULL OR
                          ($4::text IS NOT NULL AND b.search_vector @@ websearch_to_tsquery('simple',$4))
                          OR (e.embedding IS NOT NULL AND e.dimension=vector_dims($5::vector)))
                     AND (($4::text IS NOT NULL AND b.search_vector @@ websearch_to_tsquery('simple',$4))
                          OR ($5::vector IS NOT NULL AND e.embedding IS NOT NULL AND e.dimension=vector_dims($5::vector)))
                 ), ranked AS (
                   SELECT *,row_number() OVER (ORDER BY retrieval_score DESC,page_number,ordinal,id) AS retrieval_rank
                   FROM candidates
                 )
                 SELECT id,document_id,page_number,section_path,text,content_hash,retrieval_score,retrieval_rank
                 FROM ranked ORDER BY retrieval_rank LIMIT $8",
            )
            .bind(request.project_id.as_uuid())
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
            let protocol_version_id = payload_uuid(&proposal.draft.payload, "protocol_version_id");
            let expected_revision = proposal
                .draft
                .payload
                .get("expected_revision")
                .and_then(serde_json::Value::as_i64);
            let task_kind = proposal
                .draft
                .payload
                .get("task_kind")
                .and_then(serde_json::Value::as_str)
                .unwrap_or(&proposal.draft.operation);
            let mut transaction = self
                .pool
                .begin()
                .await
                .map_err(|_| AiError::Proposal("proposal transaction failed".to_owned()))?;
            let inserted = sqlx::query(
                "INSERT INTO ai_proposals
                 (id,project_id,ai_run_id,proposal_type,payload,status,entity_type,entity_id,operation,
                  model_run_id,authority_tier,task_kind,target_report_id,target_record_id,protocol_version_id,expected_revision)
                 VALUES ($1,$2,$3,$4,$5,'pending',$6,$7,$8,$3,$9,$10,$11,$12,$13,$14)
                 ON CONFLICT (model_run_id) DO NOTHING",
            ).bind(proposal.id).bind(proposal.draft.project_id.as_uuid()).bind(proposal.model_run_id)
            .bind(&proposal.draft.operation).bind(&proposal.draft.payload).bind(&proposal.draft.entity_type).bind(proposal.draft.entity_id)
            .bind(&proposal.draft.operation).bind(proposal.draft.authority.as_str()).bind(task_kind)
            .bind(target_report_id).bind(target_record_id).bind(protocol_version_id).bind(expected_revision)
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
}

const AI_PROPOSAL_SELECT: &str =
    "SELECT p.id,p.project_id,p.task_kind,p.entity_type,p.entity_id,p.operation,p.payload,
            p.authority_tier,p.model_run_id,r.provider,r.model,r.model_version,r.prompt_version,
            r.schema_version,p.status,p.protocol_version_id,p.expected_revision,p.target_report_id,
            p.target_record_id,p.resolved_at,p.resolved_by_actor_kind,p.resolved_by_actor_id,
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
         AND ($7::timestamptz IS NULL OR (p.created_at,p.id)<($7,$8))
         ORDER BY p.created_at DESC,p.id DESC LIMIT $9"
    );
    let rows = sqlx::query(sqlx::AssertSqlSafe(query))
        .bind(project_id)
        .bind(status)
        .bind(filters.task_kind)
        .bind(filters.target_report_id)
        .bind(filters.target_record_id)
        .bind(filters.candidate_report_id)
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
                    proposal
                        .payload
                        .get("stage")
                        .and_then(serde_json::Value::as_str),
                )?;
                let decision = parse_screening_decision(&proposal.payload, stage)?;
                let reason_id = screening_reason_id(&proposal.payload)?;
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
                let candidate = proposal.payload.get("candidate").ok_or_else(|| {
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
                let decision = proposal
                    .payload
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
            operation => {
                return Err(AiProposalError::InvalidTarget(format!(
                    "operation {operation} cannot be accepted in PR12"
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
    .bind(serde_json::json!({"status": status, "operation": proposal.operation, "applied_revision": applied_revision}))
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
