use chrono::{DateTime, Utc};
use deepref_ai::{
    AiError, AiFuture, AiProposal, AiRunRecord, AiRunStatus, AiRunStore, AiTaskKind, AuthorityTier,
    Embedding, EvidenceRef, EvidenceRetriever, GroundedBlock, ModelParameters, ModelProfile,
    ModelRouter, ProposalDraft, ProposalStatus, ProposalStore, ResolvedModel, RetrievalRequest,
};
use pgvector::Vector;
use sqlx::{PgPool, Postgres, Row, Transaction};
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
                     GREATEST(CASE WHEN $3::text IS NULL THEN 0.0 ELSE ts_rank_cd(b.search_vector,websearch_to_tsquery('simple',$3)) END,0.0)
                     + CASE WHEN $4::vector IS NULL THEN 0.0
                            WHEN e.embedding IS NOT NULL AND e.dimension=vector_dims($4::vector)
                            THEN GREATEST(0.0,1.0-(e.embedding <=> $4::vector))
                            ELSE 0.0 END AS retrieval_score
                   FROM document_blocks b
                   JOIN documents d ON d.id=b.document_id
                   LEFT JOIN document_block_embeddings e ON e.document_block_id=b.id
                     AND e.is_current AND e.content_hash=b.content_hash
                   WHERE d.project_id=$1 AND ($2::uuid IS NULL OR b.document_id=$2)
                     AND b.active AND b.parser_version=d.active_parser_version
                     AND ($5::text[] IS NULL OR cardinality($5::text[])=0
                          OR b.section_path[1:cardinality($5::text[])]= $5::text[])
                     AND ($6::text IS NULL OR b.kind=$6)
                     AND ($4::vector IS NULL OR
                          ($3::text IS NOT NULL AND b.search_vector @@ websearch_to_tsquery('simple',$3))
                          OR (e.embedding IS NOT NULL AND e.dimension=vector_dims($4::vector)))
                     AND (($3::text IS NOT NULL AND b.search_vector @@ websearch_to_tsquery('simple',$3))
                          OR ($4::vector IS NOT NULL AND e.embedding IS NOT NULL AND e.dimension=vector_dims($4::vector)))
                 ), ranked AS (
                   SELECT *,row_number() OVER (ORDER BY retrieval_score DESC,page_number,ordinal,id) AS retrieval_rank
                   FROM candidates
                 )
                 SELECT id,document_id,page_number,section_path,text,content_hash,retrieval_score,retrieval_rank
                 FROM ranked ORDER BY retrieval_rank LIMIT $7",
            )
            .bind(request.project_id.as_uuid()).bind(request.document_id.map(|id| id.as_uuid())).bind(lexical).bind(vector)
            .bind(request.section_prefix).bind(request.kind).bind(i64::from(request.limit))
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
            let inserted = sqlx::query(
                "INSERT INTO ai_proposals (id,project_id,ai_run_id,proposal_type,payload,status,entity_type,entity_id,operation,model_run_id,authority_tier)
                 VALUES ($1,$2,$3,$4,$5,'pending',$6,$7,$8,$3,$9)
                 ON CONFLICT (model_run_id) DO NOTHING",
            ).bind(proposal.id).bind(proposal.draft.project_id.as_uuid()).bind(proposal.model_run_id)
            .bind(&proposal.draft.operation).bind(&proposal.draft.payload).bind(&proposal.draft.entity_type).bind(proposal.draft.entity_id)
            .bind(&proposal.draft.operation).bind(proposal.draft.authority.as_str())
            .execute(&self.pool).await.map_err(|_| AiError::Proposal("proposal write failed".to_owned()))?;
            let existing = self
                .find_for_run(proposal.model_run_id)
                .await?
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
            Ok(existing)
        })
    }
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
