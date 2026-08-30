use super::*;

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
            "study_design_classification_suggestion" => {
                applied_revision = Some(
                    apply_study_classification(
                        &mut tx,
                        &proposal,
                        &applied_payload,
                        &request.actor,
                    )
                    .await?,
                );
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
