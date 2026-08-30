use super::*;

#[utoipa::path(
    get,
    path = "/projects/{project_id}/ai/proposals",
    operation_id = "listAiProposals",
    tag = "ai",
    params(
        ("project_id" = Uuid, Path, description = "Project identifier"),
        ("cursor" = Option<String>, Query, description = "Opaque pagination cursor"),
        ("limit" = Option<i64>, Query, description = "Page size from 1 through 100"),
        ("status" = Option<String>, Query, description = "pending, accepted, rejected, or expired"),
        ("task_kind" = Option<String>, Query, description = "AI task kind filter"),
        ("target_report_id" = Option<Uuid>, Query, description = "Screening report target"),
        ("target_record_id" = Option<Uuid>, Query, description = "Dedupe source record target"),
        ("candidate_report_id" = Option<Uuid>, Query, description = "Dedupe candidate report target"),
        ("target_study_id" = Option<Uuid>, Query, description = "Study-scoped proposal target")
    ),
    responses(
        (status = 200, description = "Project-scoped AI proposals", body = PaginatedResponse<AiProposalDto>),
        (status = 400, description = "Invalid pagination or filter", body = ErrorResponse),
        (status = 500, description = "Internal server error", body = ErrorResponse)
    )
)]
pub(crate) async fn list_ai_proposals(
    State(state): State<AppState>,
    Path(project_id): Path<Uuid>,
    Query(params): Query<AiProposalListParams>,
) -> Result<Json<PaginatedResponse<AiProposalDto>>, ApiError> {
    let pagination = PaginationParams {
        cursor: params.cursor,
        limit: params.limit,
    };
    let limit = pagination.limit()?;
    let cursor = pagination.decode::<(DateTime<Utc>, Uuid)>()?;
    let proposals = deepref_postgres::list_ai_proposals(
        &state.pool,
        project_id,
        deepref_postgres::AiProposalFilters {
            status: params.status.as_deref(),
            task_kind: params.task_kind.as_deref(),
            target_report_id: params.target_report_id,
            target_record_id: params.target_record_id,
            candidate_report_id: params.candidate_report_id,
            target_study_id: params.target_study_id,
        },
        cursor.map(|(created_at, id)| deepref_postgres::AiProposalCursor { created_at, id }),
        limit,
    )
    .await
    .map_err(map_ai_proposal_error)?;
    let items = proposals
        .into_iter()
        .map(proposal_dto)
        .collect::<Result<Vec<_>, _>>()?;
    Ok(Json(page(items, limit as usize, |item| {
        (item.created_at, item.id)
    })?))
}

#[utoipa::path(
    get,
    path = "/projects/{project_id}/ai/proposals/{proposal_id}",
    operation_id = "getAiProposal",
    tag = "ai",
    params(
        ("project_id" = Uuid, Path, description = "Project identifier"),
        ("proposal_id" = Uuid, Path, description = "AI proposal identifier")
    ),
    responses(
        (status = 200, description = "AI proposal", body = AiProposalDto),
        (status = 404, description = "AI proposal not found", body = ErrorResponse),
        (status = 500, description = "Internal server error", body = ErrorResponse)
    )
)]
pub(crate) async fn get_ai_proposal(
    State(state): State<AppState>,
    Path((project_id, proposal_id)): Path<(Uuid, Uuid)>,
) -> Result<Json<AiProposalDto>, ApiError> {
    Ok(Json(proposal_dto(
        deepref_postgres::get_ai_proposal(&state.pool, project_id, proposal_id)
            .await
            .map_err(map_ai_proposal_error)?,
    )?))
}

#[utoipa::path(
    post,
    path = "/projects/{project_id}/ai/proposals/{proposal_id}/decision",
    operation_id = "decideAiProposal",
    tag = "ai",
    params(
        ("project_id" = Uuid, Path, description = "Project identifier"),
        ("proposal_id" = Uuid, Path, description = "AI proposal identifier")
    ),
    request_body = DecideAiProposalRequest,
    responses(
        (status = 200, description = "AI proposal resolution", body = AiProposalDecisionDto),
        (status = 400, description = "Invalid decision", body = ErrorResponse),
        (status = 404, description = "AI proposal target not found", body = ErrorResponse),
        (status = 409, description = "Proposal or domain revision conflict", body = ErrorResponse),
        (status = 500, description = "Internal server error", body = ErrorResponse)
    )
)]
pub(crate) async fn decide_ai_proposal(
    State(state): State<AppState>,
    Path((project_id, proposal_id)): Path<(Uuid, Uuid)>,
    headers: HeaderMap,
    Json(input): Json<DecideAiProposalRequest>,
) -> Result<Json<AiProposalDecisionDto>, ApiError> {
    if input.reason.trim().is_empty() || input.reason.len() > 2_000 {
        return Err(ApiError::BadRequest(
            "reason must be between 1 and 2000 characters".to_owned(),
        ));
    }
    let actor = extract_actor(&headers)?;
    let decision = match input.decision {
        AiProposalDecisionInput::Accept => AiProposalDecision::Accept,
        AiProposalDecisionInput::Reject => AiProposalDecision::Reject,
    };
    let resolution = deepref_postgres::decide_ai_proposal(
        &state.pool,
        AiProposalDecisionRequest {
            project_id,
            proposal_id,
            decision,
            reason: input.reason,
            reviewed_payload: input
                .reviewed_payload
                .map(reviewed_payload_to_internal)
                .transpose()?,
            actor,
        },
    )
    .await
    .map_err(map_ai_proposal_error)?;
    let proposal = deepref_postgres::get_ai_proposal(&state.pool, project_id, proposal_id)
        .await
        .map_err(map_ai_proposal_error)?;
    Ok(Json(AiProposalDecisionDto {
        proposal: proposal_dto(proposal)?,
        applied_revision: resolution.applied_revision,
    }))
}

fn reviewed_payload_to_internal(
    payload: AiReviewedProposalPayload,
) -> Result<ReviewedAiProposalPayload, ApiError> {
    match payload {
        AiReviewedProposalPayload::AppraisalPrefill {
            report_id,
            definition_id,
            definition_version,
            answers,
            domain_judgments,
            overall_judgment,
        } => Ok(ReviewedAiProposalPayload::AppraisalPrefill(
            deepref_ai::AppraisalPrefill {
                report_id,
                definition_id,
                definition_version,
                answers: serde_json::from_value(serde_json::to_value(answers).map_err(
                    |error| ApiError::BadRequest(format!("reviewed payload is invalid: {error}")),
                )?)
                .map_err(|error| {
                    ApiError::BadRequest(format!("reviewed payload is invalid: {error}"))
                })?,
                domain_judgments,
                overall_judgment,
            },
        )),
        AiReviewedProposalPayload::DataExtraction { study_id, fields } => Ok(
            ReviewedAiProposalPayload::DataExtraction(deepref_ai::DataExtraction {
                study_id,
                fields: serde_json::from_value(serde_json::to_value(fields).map_err(|error| {
                    ApiError::BadRequest(format!("reviewed payload is invalid: {error}"))
                })?)
                .map_err(|error| {
                    ApiError::BadRequest(format!("reviewed payload is invalid: {error}"))
                })?,
            }),
        ),
    }
}
