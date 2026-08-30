use axum::{
    Json,
    extract::{Path, State},
    http::HeaderMap,
};
use deepref_application::{ExtractionFieldDefinition, ExtractionFieldType, ExtractionValue};
use deepref_domain::ProjectId;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use utoipa::ToSchema;
use uuid::Uuid;

use super::{
    ai::{AcceptedReviewRun, ReviewRunDto, accepted_review_run},
    review::extract_actor,
};
use crate::{
    error::{ApiError, ErrorResponse},
    state::AppState,
};

#[derive(Debug, Clone, Serialize, ToSchema)]
pub(crate) struct ExtractionFieldDto {
    pub id: Uuid,
    pub project_id: Uuid,
    pub version: u32,
    pub field_key: String,
    pub label: String,
    pub value_type: String,
    pub required: bool,
}

#[derive(Debug, Deserialize, ToSchema)]
pub(crate) struct CreateExtractionFieldRequest {
    pub id: Option<Uuid>,
    pub version: u32,
    pub field_key: String,
    pub label: String,
    pub value_type: String,
    pub required: bool,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub(crate) struct ExtractionValueDto {
    pub id: Uuid,
    pub study_id: Uuid,
    pub report_id: Uuid,
    pub field_definition_id: Uuid,
    pub field_definition_version: i32,
    pub value: ExtractionValueDtoValue,
    pub rationale: String,
    pub source_document_id: Uuid,
    pub source_block_id: Uuid,
    pub source_page: i32,
    pub source_parser_version: String,
    pub source_content_hash: String,
    pub approved_by_actor_kind: String,
    pub approved_by_actor_id: String,
    pub approved_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(crate) enum ExtractionValueDtoValue {
    Text { value: String },
    Number { value: f64 },
    Boolean { value: bool },
    Date { value: String },
}

#[utoipa::path(
    get,
    path = "/projects/{project_id}/extraction/fields",
    operation_id = "listExtractionFields",
    tag = "extraction",
    params(("project_id" = Uuid, Path)),
    responses((status = 200, body = [ExtractionFieldDto]), (status = 404, body = ErrorResponse), (status = 500, body = ErrorResponse))
)]
pub(crate) async fn list_extraction_fields(
    State(state): State<AppState>,
    Path(project_id): Path<Uuid>,
) -> Result<Json<Vec<ExtractionFieldDto>>, ApiError> {
    let definitions = deepref_postgres::list_field_definitions(&state.pool, project_id)
        .await
        .map_err(map_extraction_error)?;
    Ok(Json(definitions.into_iter().map(field_dto).collect()))
}

#[utoipa::path(
    post,
    path = "/projects/{project_id}/extraction/fields",
    operation_id = "createExtractionField",
    tag = "extraction",
    params(("project_id" = Uuid, Path)),
    request_body = CreateExtractionFieldRequest,
    responses((status = 201, body = ExtractionFieldDto), (status = 400, body = ErrorResponse), (status = 404, body = ErrorResponse), (status = 409, body = ErrorResponse), (status = 500, body = ErrorResponse))
)]
pub(crate) async fn create_extraction_field(
    State(state): State<AppState>,
    Path(project_id): Path<Uuid>,
    Json(input): Json<CreateExtractionFieldRequest>,
) -> Result<(axum::http::StatusCode, Json<ExtractionFieldDto>), ApiError> {
    let value_type = ExtractionFieldType::parse(&input.value_type)
        .ok_or_else(|| ApiError::BadRequest("value_type is invalid".to_owned()))?;
    let definition = ExtractionFieldDefinition {
        id: input.id.unwrap_or_else(Uuid::new_v4),
        project_id: ProjectId::new(project_id),
        version: input.version,
        field_key: input.field_key,
        label: input.label,
        value_type,
        required: input.required,
    };
    let definition = deepref_postgres::create_field_definition(&state.pool, definition)
        .await
        .map_err(map_extraction_error)?;
    Ok((axum::http::StatusCode::CREATED, Json(field_dto(definition))))
}

#[utoipa::path(
    get,
    path = "/projects/{project_id}/studies/{study_id}/extraction",
    operation_id = "listStudyExtractionValues",
    tag = "extraction",
    params(("project_id" = Uuid, Path), ("study_id" = Uuid, Path)),
    responses((status = 200, body = [ExtractionValueDto]), (status = 404, body = ErrorResponse), (status = 500, body = ErrorResponse))
)]
pub(crate) async fn list_study_extraction_values(
    State(state): State<AppState>,
    Path((project_id, study_id)): Path<(Uuid, Uuid)>,
) -> Result<Json<Vec<ExtractionValueDto>>, ApiError> {
    let values = deepref_postgres::list_values(&state.pool, project_id, study_id)
        .await
        .map_err(map_extraction_error)?;
    Ok(Json(values.into_iter().map(value_dto).collect()))
}

#[utoipa::path(
    post,
    path = "/projects/{project_id}/studies/{study_id}/ai/extraction",
    operation_id = "generateDataExtractionSuggestion",
    tag = "ai",
    params(("project_id" = Uuid, Path), ("study_id" = Uuid, Path)),
    responses((status = 202, body = ReviewRunDto), (status = 400, body = ErrorResponse), (status = 404, body = ErrorResponse), (status = 503, body = ErrorResponse), (status = 500, body = ErrorResponse))
)]
pub(crate) async fn generate_data_extraction_suggestion(
    State(state): State<AppState>,
    Path((project_id, study_id)): Path<(Uuid, Uuid)>,
    headers: HeaderMap,
) -> Result<AcceptedReviewRun, ApiError> {
    let snapshot = deepref_postgres::schedule_data_extraction_review(
        &state.pool,
        project_id,
        study_id,
        extract_actor(&headers)?,
    )
    .await
    .map_err(super::ai::map_review_preparation_error)?;
    accepted_review_run(snapshot)
}

fn field_dto(definition: ExtractionFieldDefinition) -> ExtractionFieldDto {
    ExtractionFieldDto {
        id: definition.id,
        project_id: definition.project_id.as_uuid(),
        version: definition.version,
        field_key: definition.field_key,
        label: definition.label,
        value_type: definition.value_type.as_str().to_owned(),
        required: definition.required,
    }
}

fn value_dto(value: deepref_postgres::ExtractionValueRecord) -> ExtractionValueDto {
    let typed_value = match value.value {
        ExtractionValue::Text { value } => ExtractionValueDtoValue::Text { value },
        ExtractionValue::Number { value } => ExtractionValueDtoValue::Number { value },
        ExtractionValue::Boolean { value } => ExtractionValueDtoValue::Boolean { value },
        ExtractionValue::Date { value } => ExtractionValueDtoValue::Date {
            value: value.to_string(),
        },
    };
    ExtractionValueDto {
        id: value.id,
        study_id: value.study_id,
        report_id: value.report_id,
        field_definition_id: value.field_definition_id,
        field_definition_version: value.field_definition_version,
        value: typed_value,
        rationale: value.rationale,
        source_document_id: value.source_document_id,
        source_block_id: value.source_block_id,
        source_page: value.source_page,
        source_parser_version: value.source_parser_version,
        source_content_hash: value.source_content_hash,
        approved_by_actor_kind: value.approved_by_actor_kind,
        approved_by_actor_id: value.approved_by_actor_id,
        approved_at: value.approved_at,
    }
}

fn map_extraction_error(error: deepref_postgres::ExtractionError) -> ApiError {
    match error {
        deepref_postgres::ExtractionError::Database(error) => ApiError::Database(error),
        deepref_postgres::ExtractionError::ImmutableDefinition
        | deepref_postgres::ExtractionError::ValueAlreadyApproved => ApiError::Conflict {
            code: "extraction_conflict".to_owned(),
            message: error.to_string(),
            details: Value::Null,
        },
        deepref_postgres::ExtractionError::DefinitionNotFound
        | deepref_postgres::ExtractionError::StudyNotFound => ApiError::NotFound(error.to_string()),
        deepref_postgres::ExtractionError::StaleDefinitionVersion => ApiError::Conflict {
            code: "extraction_definition_changed".to_owned(),
            message: error.to_string(),
            details: Value::Null,
        },
        deepref_postgres::ExtractionError::InvalidDefinition(message)
        | deepref_postgres::ExtractionError::InvalidValue(message) => ApiError::BadRequest(message),
        deepref_postgres::ExtractionError::EvidenceNotInStudy
        | deepref_postgres::ExtractionError::RequiredFieldInsufficient => ApiError::Conflict {
            code: "extraction_evidence_conflict".to_owned(),
            message: error.to_string(),
            details: Value::Null,
        },
    }
}
