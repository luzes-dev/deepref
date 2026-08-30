use super::*;

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

pub(super) fn ai_run_from_row(row: sqlx::postgres::PgRow) -> Result<AiRunRecord, AiError> {
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

pub(super) fn proposal_from_row(row: sqlx::postgres::PgRow) -> Result<AiProposal, AiError> {
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
