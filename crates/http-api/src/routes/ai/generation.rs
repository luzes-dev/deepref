use super::*;
use deepref_review::ReviewScheduler as _;

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
        (status = 202, description = "Compiled screening review scheduled", body = ReviewRunDto),
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
    headers: HeaderMap,
    Json(input): Json<GenerateScreeningRequest>,
) -> Result<AcceptedReviewRun, ApiError> {
    let snapshot = deepref_postgres::schedule_screening_review(
        &state.pool,
        project_id,
        report_id,
        input.stage.ai(),
        input.protocol_version_id,
        input.expected_revision,
        extract_actor(&headers)?,
    )
    .await
    .map_err(map_review_preparation_error)?;
    accepted_review_run(snapshot)
}

#[utoipa::path(
    post,
    path = "/projects/{project_id}/reports/{report_id}/ai/study-grouping",
    operation_id = "generateStudyGroupingSuggestion",
    tag = "ai",
    params(("project_id" = Uuid, Path), ("report_id" = Uuid, Path)),
    responses(
        (status = 202, body = ReviewRunDto),
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
    headers: HeaderMap,
) -> Result<AcceptedReviewRun, ApiError> {
    let snapshot = deepref_postgres::schedule_study_grouping_review(
        &state.pool,
        project_id,
        report_id,
        extract_actor(&headers)?,
    )
    .await
    .map_err(map_review_preparation_error)?;
    accepted_review_run(snapshot)
}

#[utoipa::path(
    post,
    path = "/projects/{project_id}/reports/{report_id}/ai/appraisal-prefill",
    operation_id = "generateAppraisalPrefillSuggestion",
    tag = "ai",
    params(("project_id" = Uuid, Path), ("report_id" = Uuid, Path)),
    request_body = GenerateAppraisalPrefillRequest,
    responses(
        (status = 202, body = ReviewRunDto),
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
    headers: HeaderMap,
    Json(input): Json<GenerateAppraisalPrefillRequest>,
) -> Result<AcceptedReviewRun, ApiError> {
    let snapshot = deepref_postgres::schedule_appraisal_prefill_review(
        &state.pool,
        project_id,
        report_id,
        &input.definition_id,
        input.definition_version,
        extract_actor(&headers)?,
    )
    .await
    .map_err(map_review_preparation_error)?;
    accepted_review_run(snapshot)
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
        (status = 202, description = "Compiled duplicate review scheduled", body = ReviewRunDto),
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
    headers: HeaderMap,
    Json(input): Json<GenerateDuplicateRequest>,
) -> Result<AcceptedReviewRun, ApiError> {
    let snapshot = deepref_postgres::schedule_duplicate_detection_review(
        &state.pool,
        project_id,
        record_id,
        input.candidate_report_id,
        extract_actor(&headers)?,
    )
    .await
    .map_err(map_review_preparation_error)?;
    accepted_review_run(snapshot)
}

#[utoipa::path(
    get,
    path = "/projects/{project_id}/review-runs/{run_id}",
    operation_id = "getReviewRun",
    tag = "ai",
    params(
        ("project_id" = Uuid, Path, description = "Project identifier"),
        ("run_id" = Uuid, Path, description = "Review run identifier")
    ),
    responses(
        (status = 200, description = "Compiled review run status and result linkage", body = ReviewRunDto),
        (status = 404, description = "Review run not found", body = ErrorResponse),
        (status = 500, description = "Internal server error", body = ErrorResponse)
    )
)]
pub(crate) async fn get_review_run(
    State(state): State<AppState>,
    Path((project_id, run_id)): Path<(Uuid, Uuid)>,
) -> Result<Json<ReviewRunDto>, ApiError> {
    let run_id = deepref_review::ReviewRunId::new(run_id)
        .map_err(|error| ApiError::BadRequest(error.to_string()))?;
    let snapshot = deepref_postgres::PostgresReviewScheduler::new(&state.pool)
        .get(project_id.into(), run_id)
        .await
        .map_err(map_review_preparation_error)?;
    Ok(Json(review_run_dto(snapshot)?))
}

pub(crate) fn accepted_review_run(
    snapshot: deepref_review::ReviewRunSnapshot,
) -> Result<AcceptedReviewRun, ApiError> {
    let location = format!(
        "/projects/{}/review-runs/{}",
        snapshot.project_id.as_uuid(),
        snapshot.id.as_uuid()
    );
    let mut headers = HeaderMap::new();
    headers.insert(
        axum::http::header::LOCATION,
        location.parse().map_err(|error| {
            ApiError::Internal(anyhow::anyhow!("invalid run location: {error}"))
        })?,
    );
    Ok((
        axum::http::StatusCode::ACCEPTED,
        headers,
        Json(review_run_dto(snapshot)?),
    ))
}

pub(super) fn review_run_dto(
    snapshot: deepref_review::ReviewRunSnapshot,
) -> Result<ReviewRunDto, ApiError> {
    let state = match snapshot.state {
        deepref_review::ReviewRunState::Queued => ReviewRunStateDto::Queued,
        deepref_review::ReviewRunState::Running => ReviewRunStateDto::Running,
        deepref_review::ReviewRunState::Blocked { code, message } => ReviewRunStateDto::Blocked {
            code: code.as_str().to_owned(),
            message,
        },
        deepref_review::ReviewRunState::Failed { code, message } => {
            ReviewRunStateDto::Failed { code, message }
        }
        deepref_review::ReviewRunState::Completed { proposal_id } => {
            ReviewRunStateDto::Completed { proposal_id }
        }
    };
    Ok(ReviewRunDto {
        id: snapshot.id.as_uuid(),
        project_id: snapshot.project_id.as_uuid(),
        definition: snapshot.definition.as_str().to_owned(),
        subject: serde_json::to_value(snapshot.subject)
            .map_err(|error| ApiError::Internal(error.into()))?,
        origin: serde_json::to_value(snapshot.origin)
            .map_err(|error| ApiError::Internal(error.into()))?,
        state,
        created_at: snapshot.created_at,
        started_at: snapshot.started_at,
        finished_at: snapshot.finished_at,
    })
}

pub(crate) fn map_review_preparation_error(
    error: deepref_postgres::ReviewPreparationError,
) -> ApiError {
    match error {
        deepref_postgres::ReviewPreparationError::Review(error) => map_postgres_review_error(error),
        deepref_postgres::ReviewPreparationError::Protocol(error) => map_protocol_error(error),
        deepref_postgres::ReviewPreparationError::AiProposal(error) => map_ai_proposal_error(error),
        deepref_postgres::ReviewPreparationError::Extraction(error) => match error {
            deepref_postgres::ExtractionError::Database(error) => ApiError::Database(error),
            deepref_postgres::ExtractionError::DefinitionNotFound
            | deepref_postgres::ExtractionError::StudyNotFound => {
                ApiError::NotFound(error.to_string())
            }
            _ => ApiError::BadRequest(error.to_string()),
        },
        deepref_postgres::ReviewPreparationError::Study(error) => {
            super::super::study::map_study_error(error)
        }
        deepref_postgres::ReviewPreparationError::InvalidInput(message)
            if message.contains("changed") =>
        {
            ApiError::Conflict {
                code: "review_subject_changed".to_owned(),
                message,
                details: Value::Null,
            }
        }
        deepref_postgres::ReviewPreparationError::InvalidInput(message) => {
            ApiError::BadRequest(message)
        }
    }
}

fn map_postgres_review_error(error: deepref_postgres::PostgresReviewError) -> ApiError {
    match error {
        deepref_postgres::PostgresReviewError::Database(error) => ApiError::Database(error),
        deepref_postgres::PostgresReviewError::Serialization(error) => {
            ApiError::Internal(error.into())
        }
        deepref_postgres::PostgresReviewError::Review(error) => {
            ApiError::Configuration(error.to_string())
        }
        deepref_postgres::PostgresReviewError::Ai(error) => map_ai_error(error),
        deepref_postgres::PostgresReviewError::RunNotFound => {
            ApiError::NotFound("review run not found".to_owned())
        }
        deepref_postgres::PostgresReviewError::InvalidState(message) => ApiError::Conflict {
            code: "review_run_state_conflict".to_owned(),
            message,
            details: Value::Null,
        },
        deepref_postgres::PostgresReviewError::InvalidStoredValue(message) => {
            ApiError::Internal(anyhow::anyhow!(message))
        }
        deepref_postgres::PostgresReviewError::WorkerOwnership => {
            ApiError::Internal(anyhow::anyhow!("review worker lease is not owned"))
        }
        deepref_postgres::PostgresReviewError::FinalizationConflict => ApiError::Conflict {
            code: "review_finalization_conflict".to_owned(),
            message: "review proposal finalization conflicts with persisted state".to_owned(),
            details: Value::Null,
        },
        deepref_postgres::PostgresReviewError::CalibrationMissing => ApiError::Conflict {
            code: "calibration_missing".to_owned(),
            message: error.to_string(),
            details: Value::Null,
        },
        deepref_postgres::PostgresReviewError::CalibrationFailed => ApiError::Conflict {
            code: "calibration_failed".to_owned(),
            message: error.to_string(),
            details: Value::Null,
        },
        deepref_postgres::PostgresReviewError::CalibrationStale => ApiError::Conflict {
            code: "calibration_stale".to_owned(),
            message: error.to_string(),
            details: Value::Null,
        },
    }
}
