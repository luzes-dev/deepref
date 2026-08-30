use super::*;

#[utoipa::path(
    post,
    path = "/projects/{project_id}/reports/{report_id}/ai/screening",
    operation_id = "generateScreeningSuggestion",
    tag = "ai",
    params(
        ("project_id" = Uuid, Path, description = "Project identifier"),
        ("report_id" = Uuid, Path, description = "Report identifier")
    ),
    request_body = GenerateScreeningRequest,
    responses(
        (status = 200, description = "AI screening proposal", body = AiProposalDto),
        (status = 400, description = "Invalid AI request", body = ErrorResponse),
        (status = 404, description = "Project, report, or protocol not found", body = ErrorResponse),
        (status = 409, description = "A current proposal or revision conflicts", body = ErrorResponse),
        (status = 503, description = "AI provider is unavailable", body = ErrorResponse),
        (status = 500, description = "Internal server error", body = ErrorResponse)
    )
)]
pub(crate) async fn generate_screening_suggestion(
    State(state): State<AppState>,
    Path((project_id, report_id)): Path<(Uuid, Uuid)>,
    Json(input): Json<GenerateScreeningRequest>,
) -> Result<Json<AiProposalDto>, ApiError> {
    let proposal = AiReviewService::new(&state)
        .screening(ScreeningReviewCommand {
            project_id,
            report_id,
            input,
        })
        .await?;
    Ok(Json(proposal_dto(proposal)?))
}

pub(crate) async fn create_screening_proposal(
    state: &AppState,
    project_id: Uuid,
    report_id: Uuid,
    input: GenerateScreeningRequest,
) -> Result<AiProposalRecord, ApiError> {
    let stage = input.stage.ai();
    let protocol = deepref_postgres::get_published_protocol(&state.pool, project_id)
        .await
        .map_err(map_protocol_error)?;
    if input
        .protocol_version_id
        .is_some_and(|id| id != protocol.id)
    {
        return Err(ApiError::Conflict {
            code: "ai_protocol_changed".to_owned(),
            message: "the requested protocol is not the current published version".to_owned(),
            details: json!({"protocolVersionId": protocol.id}),
        });
    }
    let target = deepref_postgres::get_ai_screening_target(&state.pool, project_id, report_id)
        .await
        .map_err(map_ai_proposal_error)?;
    let expected_revision = input.expected_revision.unwrap_or(target.expected_revision);
    let allowed_reasons =
        deepref_postgres::list_ai_exclusion_reasons(&state.pool, project_id, input.stage.domain())
            .await
            .map_err(map_ai_proposal_error)?;
    let allowed_evidence = metadata_evidence(report_id, &target);
    let criteria = protocol.criteria.clone();
    let task = ScreeningTask::new(ScreeningTaskConfig {
        project_id: project_id.into(),
        report_id: report_id.into(),
        stage,
        protocol_version_id: protocol.id.into(),
        expected_revision,
        criteria: criteria.clone(),
        allowed_evidence,
        allowed_exclusion_reasons: allowed_reasons.into_iter().collect(),
    });
    let prompts = criteria.iter().map(criterion_prompt).collect::<Vec<_>>();
    let ai_input = ScreeningInput {
        project_id: project_id.into(),
        report_id: report_id.into(),
        stage,
        protocol_version_id: protocol.id.into(),
        expected_revision,
        title: target.title.clone(),
        abstract_text: target.abstract_text.clone(),
        document_hash: None,
        retrieval_query: (stage == ScreeningStage::FullText)
            .then(|| screening_retrieval_query(&target, &criteria)),
        criteria: prompts,
    };
    let proposal = run_task(state, &task, ai_input).await?;
    Ok(proposal)
}

#[utoipa::path(
    post,
    path = "/projects/{project_id}/reports/{report_id}/ai/study-grouping",
    operation_id = "generateStudyGroupingSuggestion",
    tag = "ai",
    params(("project_id" = Uuid, Path), ("report_id" = Uuid, Path)),
    responses(
        (status = 200, body = AiProposalDto),
        (status = 400, body = ErrorResponse),
        (status = 404, body = ErrorResponse),
        (status = 409, body = ErrorResponse),
        (status = 503, body = ErrorResponse),
        (status = 500, body = ErrorResponse)
    )
)]
pub(crate) async fn generate_study_grouping_suggestion(
    State(state): State<AppState>,
    Path((project_id, report_id)): Path<(Uuid, Uuid)>,
) -> Result<Json<AiProposalDto>, ApiError> {
    let proposal = AiReviewService::new(&state)
        .study_grouping(StudyGroupingReviewCommand {
            project_id,
            report_id,
        })
        .await?;
    Ok(Json(proposal_dto(proposal)?))
}

pub(crate) async fn create_study_grouping_proposal(
    state: &AppState,
    project_id: Uuid,
    report_id: Uuid,
) -> Result<AiProposalRecord, ApiError> {
    let target = deepref_postgres::get_ai_study_grouping_target(&state.pool, project_id, report_id)
        .await
        .map_err(map_ai_proposal_error)?;
    let grounded_evidence = grouping_evidence(&target);
    let task_input = StudyGroupingInput {
        project_id: project_id.into(),
        report_id: report_id.into(),
        report_title: target.report.title.clone(),
        report_abstract: target.report.abstract_text.clone(),
        publication_year: target.report.publication_year,
        first_author: target.report.first_author.clone(),
        current_study_id: target.current_study_id.map(Into::into),
        current_study_revision: target.current_study_revision,
        candidates: target
            .studies
            .iter()
            .map(|study| StudyGroupingCandidate {
                study_id: study.study_id,
                title: study.title.clone(),
                revision: study.revision,
                report_ids: study
                    .reports
                    .iter()
                    .map(|report| report.report_id)
                    .collect(),
            })
            .collect(),
        grounded_evidence,
    };
    let task = StudyGroupingTask::new(&task_input).map_err(map_ai_error)?;
    let proposal = run_task(state, &task, task_input).await?;
    Ok(proposal)
}

#[utoipa::path(
    post,
    path = "/projects/{project_id}/reports/{report_id}/ai/appraisal-prefill",
    operation_id = "generateAppraisalPrefillSuggestion",
    tag = "ai",
    params(("project_id" = Uuid, Path), ("report_id" = Uuid, Path)),
    request_body = GenerateAppraisalPrefillRequest,
    responses(
        (status = 200, body = AiProposalDto),
        (status = 400, body = ErrorResponse),
        (status = 404, body = ErrorResponse),
        (status = 409, body = ErrorResponse),
        (status = 503, body = ErrorResponse),
        (status = 500, body = ErrorResponse)
    )
)]
pub(crate) async fn generate_appraisal_prefill_suggestion(
    State(state): State<AppState>,
    Path((project_id, report_id)): Path<(Uuid, Uuid)>,
    Json(input): Json<GenerateAppraisalPrefillRequest>,
) -> Result<Json<AiProposalDto>, ApiError> {
    let proposal = AiReviewService::new(&state)
        .appraisal_prefill(AppraisalPrefillReviewCommand {
            project_id,
            report_id,
            input,
        })
        .await?;
    Ok(Json(proposal_dto(proposal)?))
}

pub(crate) async fn create_appraisal_prefill_proposal(
    state: &AppState,
    project_id: Uuid,
    report_id: Uuid,
    input: GenerateAppraisalPrefillRequest,
) -> Result<AiProposalRecord, ApiError> {
    let definition = deepref_application::get_appraisal_definition(
        &input.definition_id,
        input.definition_version,
    )
    .map_err(|error| ApiError::BadRequest(error.to_string()))?;
    let target = deepref_postgres::get_ai_screening_target(&state.pool, project_id, report_id)
        .await
        .map_err(map_ai_proposal_error)?;
    let query = definition
        .domains
        .iter()
        .flat_map(|domain| {
            domain.questions.iter().map(|question| {
                format!(
                    "{} {}",
                    question.label,
                    question.help.as_deref().unwrap_or("")
                )
            })
        })
        .collect::<Vec<_>>()
        .join(" ");
    let blocks =
        deepref_postgres::list_ai_grounding_blocks(&state.pool, project_id, report_id, &query)
            .await
            .map_err(map_ai_proposal_error)?;
    let questions = definition
        .domains
        .iter()
        .flat_map(|domain| domain.questions.iter())
        .map(|question| AppraisalPrefillQuestion {
            id: question.id.clone(),
            answer_schema: appraisal_answer_schema(&question.answer_schema),
            required: question.required,
            requires_evidence: question.requires_evidence,
        })
        .collect::<Vec<_>>();
    let domains = definition
        .domains
        .iter()
        .map(|domain| AppraisalPrefillDomain {
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
    let grounded_evidence = blocks
        .iter()
        .map(|block| AppraisalPrefillEvidence {
            document_id: block.document_id,
            document_block_id: block.document_block_id,
            page: block.page,
            parser_version: block.parser_version.clone(),
            content_hash: block.content_hash.clone(),
        })
        .collect();
    let task_input = AppraisalPrefillInput {
        project_id: project_id.into(),
        report_id: report_id.into(),
        definition_id: definition.id.as_str().to_owned(),
        definition_version: definition.version.get(),
        questions,
        domains,
        overall_allowed_judgments: definition
            .overall_judgment
            .options
            .iter()
            .map(|option| option.value.clone())
            .collect(),
        report_title: target.title,
        report_abstract: target.abstract_text,
        grounded_evidence,
    };
    let task = deepref_ai::AppraisalPrefillTask::new(&task_input).map_err(map_ai_error)?;
    let proposal = run_task(state, &task, task_input).await?;
    Ok(proposal)
}

#[utoipa::path(
    post,
    path = "/projects/{project_id}/records/{record_id}/ai/deduplication",
    operation_id = "generateDuplicateSuggestion",
    tag = "ai",
    params(
        ("project_id" = Uuid, Path, description = "Project identifier"),
        ("record_id" = Uuid, Path, description = "Source record identifier")
    ),
    request_body = GenerateDuplicateRequest,
    responses(
        (status = 200, description = "AI duplicate assistance proposal", body = AiProposalDto),
        (status = 400, description = "Invalid AI request", body = ErrorResponse),
        (status = 404, description = "Record or candidate report not found", body = ErrorResponse),
        (status = 409, description = "A proposal conflicts", body = ErrorResponse),
        (status = 503, description = "AI provider is unavailable", body = ErrorResponse),
        (status = 500, description = "Internal server error", body = ErrorResponse)
    )
)]
pub(crate) async fn generate_duplicate_suggestion(
    State(state): State<AppState>,
    Path((project_id, record_id)): Path<(Uuid, Uuid)>,
    Json(input): Json<GenerateDuplicateRequest>,
) -> Result<Json<AiProposalDto>, ApiError> {
    let proposal = AiReviewService::new(&state)
        .duplicate(DuplicateReviewCommand {
            project_id,
            record_id,
            candidate_report_id: input.candidate_report_id,
        })
        .await?;
    Ok(Json(proposal_dto(proposal)?))
}

pub(crate) async fn create_duplicate_proposal(
    state: &AppState,
    project_id: Uuid,
    record_id: Uuid,
    candidate_report_id: Uuid,
) -> Result<AiProposalRecord, ApiError> {
    let target = deepref_postgres::get_ai_dedupe_target(
        &state.pool,
        project_id,
        record_id,
        candidate_report_id,
    )
    .await
    .map_err(map_ai_proposal_error)?;
    let source_id = record_id;
    let candidate_id = candidate_report_id;
    let provenance = dedupe_provenance(source_id, candidate_id, &target);
    let signals = dedupe_signals(candidate_id, &target);
    let task = DedupeTask::new(
        project_id.into(),
        record_id.into(),
        candidate_id.into(),
        provenance.clone(),
        signals.clone(),
    );
    let ai_input = DedupeInput {
        project_id: project_id.into(),
        source_record_id: record_id.into(),
        candidate_report_id: candidate_id.into(),
        source_title: target.source_title.clone(),
        candidate_title: target.candidate_title.clone(),
        source_year: target.source_year,
        candidate_year: target.candidate_year,
        source_author: target.source_author.clone(),
        candidate_author: target.candidate_author.clone(),
        source_title_hash: target.source_title_hash.clone(),
        candidate_title_hash: target.candidate_title_hash.clone(),
        grounded_signals: signals,
        grounded_provenance: provenance,
    };
    let proposal = run_task(state, &task, ai_input).await?;
    Ok(proposal)
}
