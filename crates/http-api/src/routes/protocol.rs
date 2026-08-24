use std::collections::BTreeMap;

use axum::{Json, extract::Path, extract::State, http::HeaderMap};
use chrono::{DateTime, Utc};
use deepref_application::{
    ProtocolCriterionCommand, PublishProtocolCommand, SaveProtocolDraftCommand,
};
use deepref_domain::{
    CriterionDimension, CriterionKind, CriterionStage, FrameworkKind, ProjectId, ProtocolStatus,
};
use garde::Validate;
use serde::{Deserialize, Serialize};
use serde_json::json;
use utoipa::ToSchema;
use uuid::Uuid;

use super::review::{Actor, extract_actor};
use crate::{
    error::{ApiError, ErrorResponse},
    state::AppState,
};

#[derive(Debug, Clone, Copy, Deserialize, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub(crate) enum FrameworkKindInput {
    Pico,
    Picos,
    Peco,
    Peo,
    Pcc,
    Spider,
    Custom,
}

impl From<FrameworkKindInput> for FrameworkKind {
    fn from(value: FrameworkKindInput) -> Self {
        match value {
            FrameworkKindInput::Pico => Self::Pico,
            FrameworkKindInput::Picos => Self::Picos,
            FrameworkKindInput::Peco => Self::Peco,
            FrameworkKindInput::Peo => Self::Peo,
            FrameworkKindInput::Pcc => Self::Pcc,
            FrameworkKindInput::Spider => Self::Spider,
            FrameworkKindInput::Custom => Self::Custom,
        }
    }
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub(crate) enum CriterionKindInput {
    Inclusion,
    Exclusion,
}

impl From<CriterionKindInput> for CriterionKind {
    fn from(value: CriterionKindInput) -> Self {
        match value {
            CriterionKindInput::Inclusion => Self::Inclusion,
            CriterionKindInput::Exclusion => Self::Exclusion,
        }
    }
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub(crate) enum CriterionStageInput {
    TitleAbstract,
    FullText,
    Both,
}

impl From<CriterionStageInput> for CriterionStage {
    fn from(value: CriterionStageInput) -> Self {
        match value {
            CriterionStageInput::TitleAbstract => Self::TitleAbstract,
            CriterionStageInput::FullText => Self::FullText,
            CriterionStageInput::Both => Self::Both,
        }
    }
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub(crate) enum CriterionDimensionInput {
    Population,
    Intervention,
    Comparator,
    Outcome,
    Design,
    Setting,
    Language,
    Date,
    Other,
}

impl From<CriterionDimensionInput> for CriterionDimension {
    fn from(value: CriterionDimensionInput) -> Self {
        match value {
            CriterionDimensionInput::Population => Self::Population,
            CriterionDimensionInput::Intervention => Self::Intervention,
            CriterionDimensionInput::Comparator => Self::Comparator,
            CriterionDimensionInput::Outcome => Self::Outcome,
            CriterionDimensionInput::Design => Self::Design,
            CriterionDimensionInput::Setting => Self::Setting,
            CriterionDimensionInput::Language => Self::Language,
            CriterionDimensionInput::Date => Self::Date,
            CriterionDimensionInput::Other => Self::Other,
        }
    }
}

#[derive(Debug, Clone, Deserialize, ToSchema)]
pub(crate) struct ProtocolFrameworkInput {
    pub kind: FrameworkKindInput,
    pub fields: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Deserialize, ToSchema)]
pub(crate) struct ProtocolCriterionInput {
    pub id: Option<Uuid>,
    pub kind: CriterionKindInput,
    pub stage: CriterionStageInput,
    pub dimension: CriterionDimensionInput,
    pub label: String,
    pub description: String,
}

#[derive(Debug, Clone, Deserialize, Validate, ToSchema)]
pub(crate) struct SaveProtocolRequest {
    #[garde(length(min = 1, max = 200))]
    pub name: String,
    #[garde(length(min = 1, max = 10_000))]
    pub objective: String,
    #[garde(length(min = 1, max = 10_000))]
    pub question: String,
    #[garde(skip)]
    pub framework: ProtocolFrameworkInput,
    #[garde(skip)]
    pub criteria: Vec<ProtocolCriterionInput>,
    #[garde(skip)]
    pub protocol_version_id: Option<Uuid>,
    #[garde(range(min = 0))]
    pub expected_revision: i64,
}

#[derive(Debug, Clone, Deserialize, Validate, ToSchema)]
pub(crate) struct PublishProtocolRequest {
    #[garde(skip)]
    pub protocol_version_id: Uuid,
    #[garde(range(min = 1))]
    pub expected_revision: i64,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub(crate) struct EligibilityCriterionDto {
    pub id: Uuid,
    pub kind: CriterionKindInput,
    pub stage: CriterionStageInput,
    pub dimension: CriterionDimensionInput,
    pub label: String,
    pub description: String,
    pub ordinal: i32,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub(crate) struct ProtocolDto {
    pub id: Uuid,
    pub project_id: Uuid,
    pub version: i32,
    pub name: String,
    pub status: ProtocolStatusDto,
    pub framework_kind: FrameworkKindInput,
    pub framework_fields: BTreeMap<String, String>,
    pub objective: String,
    pub question: String,
    pub criteria: Vec<EligibilityCriterionDto>,
    pub revision: i64,
    pub amendment_of: Option<Uuid>,
    pub published_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ProtocolStatusDto {
    Draft,
    Published,
    Superseded,
}

#[utoipa::path(
    get,
    path = "/projects/{project_id}/protocol",
    operation_id = "getProjectProtocol",
    tag = "review",
    params(("project_id" = Uuid, Path, description = "Project identifier")),
    responses(
        (status = 200, description = "Latest published protocol", body = ProtocolDto),
        (status = 404, description = "Project or published protocol not found", body = ErrorResponse),
        (status = 500, description = "Internal server error", body = ErrorResponse)
    )
)]
pub(crate) async fn get_published_protocol(
    State(state): State<AppState>,
    Path(project_id): Path<Uuid>,
) -> Result<Json<ProtocolDto>, ApiError> {
    let protocol = deepref_postgres::get_published_protocol(&state.pool, project_id)
        .await
        .map_err(map_protocol_error)?;
    Ok(Json(protocol_dto(protocol)))
}

#[utoipa::path(
    get,
    path = "/projects/{project_id}/review/protocol",
    operation_id = "getProjectReviewProtocol",
    tag = "review",
    params(("project_id" = Uuid, Path, description = "Project identifier")),
    responses(
        (status = 200, description = "Current protocol editor aggregate", body = ProtocolDto),
        (status = 404, description = "Project or protocol not found", body = ErrorResponse),
        (status = 500, description = "Internal server error", body = ErrorResponse)
    )
)]
pub(crate) async fn get_protocol_editor(
    State(state): State<AppState>,
    Path(project_id): Path<Uuid>,
) -> Result<Json<ProtocolDto>, ApiError> {
    let protocol = deepref_postgres::get_protocol_editor(&state.pool, project_id)
        .await
        .map_err(map_protocol_error)?;
    Ok(Json(protocol_dto(protocol)))
}

#[utoipa::path(
    put,
    path = "/projects/{project_id}/review/protocol",
    operation_id = "saveProjectReviewProtocol",
    tag = "review",
    params(("project_id" = Uuid, Path, description = "Project identifier")),
    request_body = SaveProtocolRequest,
    responses(
        (status = 200, description = "Saved protocol draft", body = ProtocolDto),
        (status = 400, description = "Invalid protocol", body = ErrorResponse),
        (status = 404, description = "Project or protocol not found", body = ErrorResponse),
        (status = 409, description = "Protocol revision conflict", body = ErrorResponse),
        (status = 500, description = "Internal server error", body = ErrorResponse)
    )
)]
pub(crate) async fn save_protocol_draft(
    State(state): State<AppState>,
    Path(project_id): Path<Uuid>,
    headers: HeaderMap,
    Json(input): Json<SaveProtocolRequest>,
) -> Result<Json<ProtocolDto>, ApiError> {
    input
        .validate()
        .map_err(|error| ApiError::BadRequest(error.to_string()))?;
    let actor = extract_actor(&headers)?;
    let command = save_command(project_id, input)?;
    let protocol =
        deepref_postgres::save_protocol_draft(&state.pool, &command, &protocol_actor(&actor))
            .await
            .map_err(map_protocol_error)?;
    Ok(Json(protocol_dto(protocol)))
}

#[utoipa::path(
    post,
    path = "/projects/{project_id}/review/protocol/publish",
    operation_id = "publishProjectReviewProtocol",
    tag = "review",
    params(("project_id" = Uuid, Path, description = "Project identifier")),
    request_body = PublishProtocolRequest,
    responses(
        (status = 200, description = "Published protocol", body = ProtocolDto),
        (status = 400, description = "Invalid protocol publication", body = ErrorResponse),
        (status = 404, description = "Project or protocol draft not found", body = ErrorResponse),
        (status = 409, description = "Protocol revision conflict", body = ErrorResponse),
        (status = 500, description = "Internal server error", body = ErrorResponse)
    )
)]
pub(crate) async fn publish_protocol(
    State(state): State<AppState>,
    Path(project_id): Path<Uuid>,
    headers: HeaderMap,
    Json(input): Json<PublishProtocolRequest>,
) -> Result<Json<ProtocolDto>, ApiError> {
    input
        .validate()
        .map_err(|error| ApiError::BadRequest(error.to_string()))?;
    let actor = extract_actor(&headers)?;
    let protocol = deepref_postgres::publish_protocol(
        &state.pool,
        &PublishProtocolCommand {
            project_id: ProjectId::from(project_id),
            protocol_version_id: input.protocol_version_id,
            expected_revision: input.expected_revision,
        },
        &protocol_actor(&actor),
    )
    .await
    .map_err(map_protocol_error)?;
    Ok(Json(protocol_dto(protocol)))
}

fn save_command(
    project_id: Uuid,
    input: SaveProtocolRequest,
) -> Result<SaveProtocolDraftCommand, ApiError> {
    let criteria = input
        .criteria
        .into_iter()
        .map(|criterion| ProtocolCriterionCommand {
            id: criterion.id,
            kind: criterion.kind.into(),
            stage: criterion.stage.into(),
            dimension: criterion.dimension.into(),
            label: criterion.label,
            description: criterion.description,
        })
        .collect();
    Ok(SaveProtocolDraftCommand {
        project_id: ProjectId::from(project_id),
        protocol_version_id: input.protocol_version_id,
        name: input.name,
        objective: input.objective,
        question: input.question,
        framework_kind: input.framework.kind.into(),
        framework_fields: input.framework.fields,
        criteria,
        expected_revision: input.expected_revision,
    })
}

fn protocol_actor(actor: &Actor) -> deepref_postgres::ProtocolActor {
    deepref_postgres::ProtocolActor {
        kind: actor.kind.clone(),
        id: actor.id.clone(),
    }
}

fn protocol_dto(protocol: deepref_postgres::ProtocolDocument) -> ProtocolDto {
    ProtocolDto {
        id: protocol.id,
        project_id: protocol.project_id,
        version: protocol.version,
        name: protocol.name,
        status: protocol_status_dto(protocol.status),
        framework_kind: framework_kind_input(protocol.framework.kind),
        framework_fields: protocol.framework.fields,
        objective: protocol.objective,
        question: protocol.question,
        criteria: protocol.criteria.into_iter().map(criterion_dto).collect(),
        revision: protocol.revision,
        amendment_of: protocol.amendment_of,
        published_at: protocol.published_at,
        created_at: protocol.created_at,
        updated_at: protocol.updated_at,
    }
}

fn framework_kind_input(kind: FrameworkKind) -> FrameworkKindInput {
    match kind {
        FrameworkKind::Pico => FrameworkKindInput::Pico,
        FrameworkKind::Picos => FrameworkKindInput::Picos,
        FrameworkKind::Peco => FrameworkKindInput::Peco,
        FrameworkKind::Peo => FrameworkKindInput::Peo,
        FrameworkKind::Pcc => FrameworkKindInput::Pcc,
        FrameworkKind::Spider => FrameworkKindInput::Spider,
        FrameworkKind::Custom => FrameworkKindInput::Custom,
    }
}

fn criterion_dto(criterion: deepref_domain::EligibilityCriterion) -> EligibilityCriterionDto {
    EligibilityCriterionDto {
        id: criterion.id,
        kind: match criterion.kind {
            CriterionKind::Inclusion => CriterionKindInput::Inclusion,
            CriterionKind::Exclusion => CriterionKindInput::Exclusion,
        },
        stage: match criterion.stage {
            CriterionStage::TitleAbstract => CriterionStageInput::TitleAbstract,
            CriterionStage::FullText => CriterionStageInput::FullText,
            CriterionStage::Both => CriterionStageInput::Both,
        },
        dimension: match criterion.dimension {
            CriterionDimension::Population => CriterionDimensionInput::Population,
            CriterionDimension::Intervention => CriterionDimensionInput::Intervention,
            CriterionDimension::Comparator => CriterionDimensionInput::Comparator,
            CriterionDimension::Outcome => CriterionDimensionInput::Outcome,
            CriterionDimension::Design => CriterionDimensionInput::Design,
            CriterionDimension::Setting => CriterionDimensionInput::Setting,
            CriterionDimension::Language => CriterionDimensionInput::Language,
            CriterionDimension::Date => CriterionDimensionInput::Date,
            CriterionDimension::Other => CriterionDimensionInput::Other,
        },
        label: criterion.label,
        description: criterion.description,
        ordinal: criterion.ordinal,
    }
}

fn protocol_status_dto(status: ProtocolStatus) -> ProtocolStatusDto {
    match status {
        ProtocolStatus::Draft => ProtocolStatusDto::Draft,
        ProtocolStatus::Published => ProtocolStatusDto::Published,
        ProtocolStatus::Superseded => ProtocolStatusDto::Superseded,
    }
}

fn map_protocol_error(error: deepref_postgres::ProtocolError) -> ApiError {
    match error {
        deepref_postgres::ProtocolError::ProjectNotFound
        | deepref_postgres::ProtocolError::NotFound => {
            ApiError::NotFound("project or protocol not found".to_owned())
        }
        deepref_postgres::ProtocolError::DraftAlreadyExists
        | deepref_postgres::ProtocolError::NotEditable => ApiError::BadRequest(error.to_string()),
        deepref_postgres::ProtocolError::Conflict {
            code,
            message,
            current_revision,
        } => ApiError::Conflict {
            code: code.to_owned(),
            message: message.to_owned(),
            details: json!({ "currentRevision": current_revision }),
        },
        deepref_postgres::ProtocolError::Invalid(message) => ApiError::BadRequest(message),
        deepref_postgres::ProtocolError::DataIntegrity(message) => ApiError::DataIntegrity(message),
        deepref_postgres::ProtocolError::Database(error) => ApiError::Database(error),
        deepref_postgres::ProtocolError::Serialization(error) => ApiError::Internal(error.into()),
    }
}
