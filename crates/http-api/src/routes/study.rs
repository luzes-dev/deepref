use std::collections::BTreeMap;

use axum::{
    Json,
    extract::{Path, Query, State},
    http::StatusCode,
};
use deepref_application::{
    AppraisalAssessmentInput, AssignReportToStudy, ClassifyStudy, CreateStudy, DefinitionId,
    DefinitionVersion, EvidenceReferenceInput, RemoveReportFromStudy, RenameStudy,
    all_appraisal_definitions, get_appraisal_definition,
};
use deepref_domain::{StudyDesign, StudyDesignContext, StudyReportRole, StudyTitle};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use utoipa::ToSchema;
use uuid::Uuid;

use crate::{
    error::{ApiError, ErrorResponse},
    state::AppState,
};

use super::review::extract_actor;

#[derive(Debug, Deserialize, ToSchema)]
pub(crate) struct StudyListParams {
    pub cursor: Option<Uuid>,
    pub limit: Option<i64>,
}

#[derive(Debug, Deserialize, ToSchema)]
pub(crate) struct CreateStudyRequest {
    pub title: String,
}

#[derive(Debug, Deserialize, ToSchema)]
pub(crate) struct UpdateStudyRequest {
    pub title: String,
    pub expected_revision: i64,
}

#[derive(Debug, Deserialize, ToSchema)]
pub(crate) struct StudyClassificationRequest {
    pub design: String,
    pub physiotherapy: Option<bool>,
    pub exposure: Option<bool>,
    pub prediction_or_ai: Option<bool>,
    pub expected_revision: i64,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub(crate) enum StudyReportRoleInput {
    ReportOfStudy,
    Protocol,
    PrimaryOutcome,
    SafetyAnalysis,
    EconomicAnalysis,
    FollowUp,
}

impl StudyReportRoleInput {
    fn domain(&self) -> StudyReportRole {
        match self {
            Self::ReportOfStudy => StudyReportRole::ReportOfStudy,
            Self::Protocol => StudyReportRole::Protocol,
            Self::PrimaryOutcome => StudyReportRole::PrimaryOutcome,
            Self::SafetyAnalysis => StudyReportRole::SafetyAnalysis,
            Self::EconomicAnalysis => StudyReportRole::EconomicAnalysis,
            Self::FollowUp => StudyReportRole::FollowUp,
        }
    }
}

#[derive(Debug, Deserialize, ToSchema)]
pub(crate) struct StudyMembershipRequest {
    pub study_id: Option<Uuid>,
    pub role: Option<StudyReportRoleInput>,
    pub expected_revision: i64,
    pub expected_previous_study_revision: Option<i64>,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub(crate) struct StudyReportDto {
    pub report_id: Uuid,
    pub title: Option<String>,
    pub abstract_text: Option<String>,
    pub publication_year: Option<i32>,
    pub role: String,
    pub assigned_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub(crate) struct StudyToolSuggestionDto {
    pub tool: String,
    pub rationale: String,
}

#[derive(Debug, Clone, Copy, Serialize, ToSchema)]
pub(crate) struct StudyDesignContextDto {
    pub physiotherapy: bool,
    pub exposure: bool,
    pub prediction_or_ai: bool,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub(crate) struct StudyDto {
    pub id: Uuid,
    pub project_id: Uuid,
    pub title: String,
    pub design: Option<String>,
    pub design_label: Option<String>,
    pub design_context: StudyDesignContextDto,
    pub revision: i64,
    pub reports: Vec<StudyReportDto>,
    pub tool_suggestions: Vec<StudyToolSuggestionDto>,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
    pub updated_by_actor_kind: String,
    pub updated_by_actor_id: String,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub(crate) struct StudyListDto {
    pub items: Vec<StudyDto>,
    pub next_cursor: Option<Uuid>,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub(crate) struct StudyMembershipDto {
    pub study_id: Uuid,
    pub role: String,
    pub study_revision: i64,
    pub study: Box<StudyDto>,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub(crate) struct StudyEventDto {
    pub id: Uuid,
    pub study_id: Uuid,
    pub report_id: Option<Uuid>,
    pub event_type: String,
    pub before_study_id: Option<Uuid>,
    pub result_study_id: Option<Uuid>,
    pub before_revision: i64,
    pub result_revision: i64,
    #[schema(value_type = Object)]
    pub before_snapshot: Value,
    #[schema(value_type = Object)]
    pub result_snapshot: Value,
    #[schema(value_type = Object)]
    pub payload: Value,
    pub actor_kind: String,
    pub actor_id: String,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub(crate) struct AppraisalAnswerOptionDto {
    pub value: String,
    pub label: String,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(crate) enum AppraisalAnswerSchemaDto {
    Enum {
        options: Vec<AppraisalAnswerOptionDto>,
    },
    Boolean,
    Scale {
        min: i64,
        max: i64,
        labels: BTreeMap<String, String>,
    },
    Text {
        max_length: u32,
    },
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub(crate) struct AppraisalJudgmentSchemaDto {
    pub options: Vec<AppraisalAnswerOptionDto>,
    pub allow_custom: bool,
    pub required: bool,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub(crate) struct AppraisalQuestionDto {
    pub id: String,
    pub label: String,
    pub help: Option<String>,
    pub answer_schema: AppraisalAnswerSchemaDto,
    pub required: bool,
    pub requires_evidence: bool,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub(crate) struct AppraisalDomainDto {
    pub id: String,
    pub label: String,
    pub description: Option<String>,
    pub questions: Vec<AppraisalQuestionDto>,
    pub judgment: AppraisalJudgmentSchemaDto,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub(crate) struct AppraisalApplicabilityDto {
    pub designs: Vec<String>,
    pub note: Option<String>,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub(crate) struct AppraisalDefinitionDto {
    pub id: String,
    pub version: u32,
    pub name: String,
    pub description: String,
    pub applicability: AppraisalApplicabilityDto,
    pub domains: Vec<AppraisalDomainDto>,
    pub overall_judgment: AppraisalJudgmentSchemaDto,
}

#[derive(Debug, Deserialize, ToSchema)]
pub(crate) struct AppraisalEvidenceRequest {
    pub question_id: String,
    pub document_id: Uuid,
    pub block_id: Uuid,
}

#[derive(Debug, Deserialize, ToSchema)]
pub(crate) struct CompleteAppraisalRequest {
    pub definition_id: String,
    pub definition_version: u32,
    #[schema(value_type = Object)]
    pub responses: Value,
    pub evidence: Vec<AppraisalEvidenceRequest>,
    pub domain_judgments: BTreeMap<String, String>,
    pub overall_judgment: Option<String>,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub(crate) struct AppraisalEvidenceDto {
    pub question_id: String,
    pub document_id: Uuid,
    pub block_id: Uuid,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub(crate) struct AppraisalAssessmentDto {
    pub id: Uuid,
    pub project_id: Uuid,
    pub report_id: Uuid,
    pub definition_id: String,
    pub definition_version: i32,
    #[schema(value_type = Object)]
    pub responses: Value,
    #[schema(value_type = Object)]
    pub judgments: Value,
    pub evidence: Vec<AppraisalEvidenceDto>,
    pub actor_kind: String,
    pub actor_id: String,
    pub completed_at: chrono::DateTime<chrono::Utc>,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

#[utoipa::path(
    get,
    path = "/projects/{project_id}/studies",
    operation_id = "listProjectStudies",
    tag = "studies",
    params(
        ("project_id" = Uuid, Path, description = "Project identifier"),
        ("cursor" = Option<Uuid>, Query, description = "Opaque study cursor"),
        ("limit" = Option<i64>, Query, description = "Maximum rows, 1 through 100")
    ),
    responses(
        (status = 200, body = StudyListDto),
        (status = 400, body = ErrorResponse),
        (status = 404, body = ErrorResponse),
        (status = 500, body = ErrorResponse)
    )
)]
pub(crate) async fn list_project_studies(
    State(state): State<AppState>,
    Path(project_id): Path<Uuid>,
    Query(params): Query<StudyListParams>,
) -> Result<Json<StudyListDto>, ApiError> {
    ensure_project(&state, project_id).await?;
    let limit = bounded_limit(params.limit)?;
    let studies = deepref_postgres::list_studies(&state.pool, project_id, params.cursor, limit)
        .await
        .map_err(map_study_error)?;
    Ok(Json(StudyListDto {
        items: studies
            .items
            .into_iter()
            .map(study_dto_from_summary)
            .collect(),
        next_cursor: studies.next_cursor,
    }))
}

#[utoipa::path(
    post,
    path = "/projects/{project_id}/studies",
    operation_id = "createProjectStudy",
    tag = "studies",
    params(("project_id" = Uuid, Path, description = "Project identifier")),
    request_body = CreateStudyRequest,
    responses((status = 201, body = StudyDto), (status = 400, body = ErrorResponse), (status = 404, body = ErrorResponse), (status = 500, body = ErrorResponse))
)]
pub(crate) async fn create_project_study(
    State(state): State<AppState>,
    Path(project_id): Path<Uuid>,
    headers: axum::http::HeaderMap,
    Json(input): Json<CreateStudyRequest>,
) -> Result<(StatusCode, Json<StudyDto>), ApiError> {
    let title =
        StudyTitle::new(input.title).map_err(|error| ApiError::BadRequest(error.to_string()))?;
    let command = CreateStudy {
        project_id: project_id.into(),
        study_id: Uuid::new_v4().into(),
        title,
        actor: extract_actor(&headers)?,
    };
    let result = deepref_postgres::create_study(&state.pool, command)
        .await
        .map_err(map_study_error)?;
    Ok((StatusCode::CREATED, Json(study_dto_from_record(result))))
}

#[utoipa::path(
    get,
    path = "/projects/{project_id}/studies/{study_id}",
    operation_id = "getProjectStudy",
    tag = "studies",
    params(("project_id" = Uuid, Path), ("study_id" = Uuid, Path)),
    responses((status = 200, body = StudyDto), (status = 404, body = ErrorResponse), (status = 500, body = ErrorResponse))
)]
pub(crate) async fn get_project_study(
    State(state): State<AppState>,
    Path((project_id, study_id)): Path<(Uuid, Uuid)>,
) -> Result<Json<StudyDto>, ApiError> {
    let result = deepref_postgres::get_study(&state.pool, project_id, study_id)
        .await
        .map_err(map_study_error)?;
    Ok(Json(study_dto_from_record(result)))
}

#[utoipa::path(
    get,
    path = "/projects/{project_id}/reports/{report_id}/study",
    operation_id = "getReportStudyMembership",
    tag = "studies",
    params(("project_id" = Uuid, Path), ("report_id" = Uuid, Path)),
    responses((status = 200, body = StudyMembershipDto), (status = 204, description = "Report is unassigned"), (status = 404, body = ErrorResponse), (status = 500, body = ErrorResponse))
)]
pub(crate) async fn get_report_study_membership(
    State(state): State<AppState>,
    Path((project_id, report_id)): Path<(Uuid, Uuid)>,
) -> Result<StudyMembershipLookupResponse, ApiError> {
    let membership = deepref_postgres::get_study_for_report(&state.pool, project_id, report_id)
        .await
        .map_err(map_study_error)?;
    match membership {
        Some(membership) => Ok(StudyMembershipLookupResponse::Membership(Json(
            study_membership_dto(membership),
        ))),
        None => Ok(StudyMembershipLookupResponse::NoContent(
            StatusCode::NO_CONTENT,
        )),
    }
}

#[utoipa::path(
    put,
    path = "/projects/{project_id}/studies/{study_id}",
    operation_id = "renameProjectStudy",
    tag = "studies",
    params(("project_id" = Uuid, Path), ("study_id" = Uuid, Path)),
    request_body = UpdateStudyRequest,
    responses((status = 200, body = StudyDto), (status = 400, body = ErrorResponse), (status = 404, body = ErrorResponse), (status = 409, body = ErrorResponse), (status = 500, body = ErrorResponse))
)]
pub(crate) async fn rename_project_study(
    State(state): State<AppState>,
    Path((project_id, study_id)): Path<(Uuid, Uuid)>,
    headers: axum::http::HeaderMap,
    Json(input): Json<UpdateStudyRequest>,
) -> Result<Json<StudyDto>, ApiError> {
    let title =
        StudyTitle::new(input.title).map_err(|error| ApiError::BadRequest(error.to_string()))?;
    let expected_revision = non_negative_revision(input.expected_revision)?;
    let result = deepref_postgres::rename_study(
        &state.pool,
        RenameStudy {
            project_id: project_id.into(),
            study_id: study_id.into(),
            title,
            expected_revision,
            actor: extract_actor(&headers)?,
        },
    )
    .await
    .map_err(map_study_error)?;
    Ok(Json(study_dto_from_record(result)))
}

#[utoipa::path(
    get,
    path = "/projects/{project_id}/studies/{study_id}/history",
    operation_id = "listProjectStudyHistory",
    tag = "studies",
    params(("project_id" = Uuid, Path), ("study_id" = Uuid, Path)),
    responses((status = 200, body = [StudyEventDto]), (status = 404, body = ErrorResponse), (status = 500, body = ErrorResponse))
)]
pub(crate) async fn list_project_study_history(
    State(state): State<AppState>,
    Path((project_id, study_id)): Path<(Uuid, Uuid)>,
) -> Result<Json<Vec<StudyEventDto>>, ApiError> {
    deepref_postgres::get_study(&state.pool, project_id, study_id)
        .await
        .map_err(map_study_error)?;
    let events = deepref_postgres::list_study_events(&state.pool, project_id, study_id)
        .await
        .map_err(map_study_error)?;
    Ok(Json(events.into_iter().map(study_event_dto).collect()))
}

#[utoipa::path(
    put,
    path = "/projects/{project_id}/studies/{study_id}/classification",
    operation_id = "classifyProjectStudy",
    tag = "studies",
    params(("project_id" = Uuid, Path), ("study_id" = Uuid, Path)),
    request_body = StudyClassificationRequest,
    responses((status = 200, body = StudyDto), (status = 400, body = ErrorResponse), (status = 404, body = ErrorResponse), (status = 409, body = ErrorResponse), (status = 500, body = ErrorResponse))
)]
pub(crate) async fn classify_project_study(
    State(state): State<AppState>,
    Path((project_id, study_id)): Path<(Uuid, Uuid)>,
    headers: axum::http::HeaderMap,
    Json(input): Json<StudyClassificationRequest>,
) -> Result<Json<StudyDto>, ApiError> {
    let design = StudyDesign::parse(input.design.trim()).ok_or_else(|| {
        ApiError::BadRequest("design is not in the normalized catalog".to_owned())
    })?;
    let result = deepref_postgres::classify_study(
        &state.pool,
        ClassifyStudy {
            project_id: project_id.into(),
            study_id: study_id.into(),
            design,
            context: StudyDesignContext {
                physiotherapy: input.physiotherapy.unwrap_or(false),
                exposure: input.exposure.unwrap_or(false),
                prediction_or_ai: input.prediction_or_ai.unwrap_or(false),
            },
            expected_revision: non_negative_revision(input.expected_revision)?,
            actor: extract_actor(&headers)?,
        },
    )
    .await
    .map_err(map_study_error)?;
    Ok(Json(study_dto_from_record(result)))
}

#[utoipa::path(
    put,
    path = "/projects/{project_id}/reports/{report_id}/study",
    operation_id = "putReportStudyMembership",
    tag = "studies",
    params(("project_id" = Uuid, Path), ("report_id" = Uuid, Path)),
    request_body = StudyMembershipRequest,
    responses((status = 200, body = StudyDto), (status = 204, description = "Report is unassigned"), (status = 400, body = ErrorResponse), (status = 404, body = ErrorResponse), (status = 409, body = ErrorResponse), (status = 500, body = ErrorResponse))
)]
pub(crate) async fn put_report_study_membership(
    State(state): State<AppState>,
    Path((project_id, report_id)): Path<(Uuid, Uuid)>,
    headers: axum::http::HeaderMap,
    Json(input): Json<StudyMembershipRequest>,
) -> Result<StudyMembershipResponse, ApiError> {
    let actor = extract_actor(&headers)?;
    let expected_revision = non_negative_revision(input.expected_revision)?;
    let result = if let Some(study_id) = input.study_id {
        let result = deepref_postgres::assign_report_to_study(
            &state.pool,
            AssignReportToStudy {
                project_id: project_id.into(),
                study_id: study_id.into(),
                report_id: report_id.into(),
                role: input
                    .role
                    .as_ref()
                    .map_or(StudyReportRole::ReportOfStudy, StudyReportRoleInput::domain),
                expected_revision,
                expected_previous_study_revision: input
                    .expected_previous_study_revision
                    .map(non_negative_revision)
                    .transpose()?,
                actor,
            },
        )
        .await
        .map_err(map_study_error)?;
        StudyMembershipResponse::Study(Box::new(Json(study_dto_from_record(result))))
    } else {
        let current = deepref_postgres::get_study_for_report(&state.pool, project_id, report_id)
            .await
            .map_err(map_study_error)?;
        let Some(membership) = current else {
            return Ok(StudyMembershipResponse::NoContent(StatusCode::NO_CONTENT));
        };
        let result = deepref_postgres::remove_report_from_study(
            &state.pool,
            RemoveReportFromStudy {
                project_id: project_id.into(),
                study_id: membership.study_id,
                report_id: report_id.into(),
                expected_revision,
                actor,
            },
        )
        .await
        .map_err(map_study_error)?;
        StudyMembershipResponse::Study(Box::new(Json(study_dto_from_record(result))))
    };
    Ok(result)
}

#[derive(Debug)]
pub(crate) enum StudyMembershipResponse {
    Study(Box<Json<StudyDto>>),
    NoContent(StatusCode),
}

#[derive(Debug)]
pub(crate) enum StudyMembershipLookupResponse {
    Membership(Json<StudyMembershipDto>),
    NoContent(StatusCode),
}

impl axum::response::IntoResponse for StudyMembershipLookupResponse {
    fn into_response(self) -> axum::response::Response {
        match self {
            Self::Membership(body) => body.into_response(),
            Self::NoContent(status) => status.into_response(),
        }
    }
}

impl axum::response::IntoResponse for StudyMembershipResponse {
    fn into_response(self) -> axum::response::Response {
        match self {
            Self::Study(body) => body.into_response(),
            Self::NoContent(status) => status.into_response(),
        }
    }
}

#[utoipa::path(
    get,
    path = "/projects/{project_id}/appraisal-definitions",
    operation_id = "listAppraisalDefinitions",
    tag = "appraisal",
    params(("project_id" = Uuid, Path)),
    responses((status = 200, body = [AppraisalDefinitionDto]), (status = 404, body = ErrorResponse), (status = 500, body = ErrorResponse))
)]
pub(crate) async fn list_appraisal_definitions(
    State(state): State<AppState>,
    Path(project_id): Path<Uuid>,
) -> Result<Json<Vec<AppraisalDefinitionDto>>, ApiError> {
    ensure_project(&state, project_id).await?;
    let definitions = all_appraisal_definitions()
        .map_err(|error| ApiError::DataIntegrity(error.to_string()))?
        .into_iter()
        .map(|definition| {
            validate_definition_for_response(&definition)?;
            Ok(appraisal_definition_dto(definition))
        })
        .collect::<Result<Vec<_>, ApiError>>()?;
    Ok(Json(definitions))
}

#[utoipa::path(
    get,
    path = "/projects/{project_id}/appraisal-definitions/{definition_id}/{version}",
    operation_id = "getAppraisalDefinition",
    tag = "appraisal",
    params(("project_id" = Uuid, Path), ("definition_id" = String, Path), ("version" = u32, Path)),
    responses((status = 200, body = AppraisalDefinitionDto), (status = 404, body = ErrorResponse), (status = 500, body = ErrorResponse))
)]
pub(crate) async fn get_appraisal_definition_route(
    State(state): State<AppState>,
    Path((project_id, definition_id, version)): Path<(Uuid, String, u32)>,
) -> Result<Json<AppraisalDefinitionDto>, ApiError> {
    ensure_project(&state, project_id).await?;
    let definition = get_appraisal_definition(&definition_id, version)
        .map_err(|error| ApiError::NotFound(error.to_string()))?;
    Ok(Json(appraisal_definition_dto(definition)))
}

#[utoipa::path(
    get,
    path = "/projects/{project_id}/reports/{report_id}/appraisals",
    operation_id = "listReportAppraisals",
    tag = "appraisal",
    params(("project_id" = Uuid, Path), ("report_id" = Uuid, Path)),
    responses((status = 200, body = [AppraisalAssessmentDto]), (status = 404, body = ErrorResponse), (status = 500, body = ErrorResponse))
)]
pub(crate) async fn list_report_appraisals(
    State(state): State<AppState>,
    Path((project_id, report_id)): Path<(Uuid, Uuid)>,
) -> Result<Json<Vec<AppraisalAssessmentDto>>, ApiError> {
    let assessments = deepref_postgres::list_appraisals(&state.pool, project_id, report_id)
        .await
        .map_err(map_appraisal_error)?;
    Ok(Json(assessments.into_iter().map(appraisal_dto).collect()))
}

#[utoipa::path(
    post,
    path = "/projects/{project_id}/reports/{report_id}/appraisals",
    operation_id = "completeReportAppraisal",
    tag = "appraisal",
    params(("project_id" = Uuid, Path), ("report_id" = Uuid, Path)),
    request_body = CompleteAppraisalRequest,
    responses((status = 201, body = AppraisalAssessmentDto), (status = 400, body = ErrorResponse), (status = 404, body = ErrorResponse), (status = 500, body = ErrorResponse))
)]
pub(crate) async fn complete_report_appraisal(
    State(state): State<AppState>,
    Path((project_id, report_id)): Path<(Uuid, Uuid)>,
    headers: axum::http::HeaderMap,
    Json(input): Json<CompleteAppraisalRequest>,
) -> Result<(StatusCode, Json<AppraisalAssessmentDto>), ApiError> {
    let definition_id = DefinitionId::new(input.definition_id)
        .map_err(|error| ApiError::BadRequest(error.to_string()))?;
    let definition_version = DefinitionVersion::new(input.definition_version)
        .ok_or_else(|| ApiError::BadRequest("definition_version must be positive".to_owned()))?;
    let evidence = input
        .evidence
        .into_iter()
        .map(|evidence| EvidenceReferenceInput {
            question_id: evidence.question_id,
            document_id: evidence.document_id,
            block_id: evidence.block_id,
        })
        .collect();
    let assessment = deepref_postgres::complete_appraisal(
        &state.pool,
        project_id.into(),
        report_id.into(),
        AppraisalAssessmentInput {
            definition_id,
            definition_version,
            responses: input.responses,
            evidence,
            domain_judgments: input.domain_judgments,
            overall_judgment: input.overall_judgment,
        },
        extract_actor(&headers)?,
    )
    .await
    .map_err(map_appraisal_error)?;
    Ok((StatusCode::CREATED, Json(appraisal_dto(assessment))))
}

async fn ensure_project(state: &AppState, project_id: Uuid) -> Result<(), ApiError> {
    let exists = sqlx::query_scalar::<_, bool>("SELECT EXISTS(SELECT 1 FROM projects WHERE id=$1)")
        .bind(project_id)
        .fetch_one(&state.pool)
        .await?;
    if exists {
        Ok(())
    } else {
        Err(ApiError::NotFound("project not found".to_owned()))
    }
}

fn bounded_limit(limit: Option<i64>) -> Result<i64, ApiError> {
    let limit = limit.unwrap_or(25);
    if !(1..=100).contains(&limit) {
        return Err(ApiError::BadRequest(
            "limit must be between 1 and 100".to_owned(),
        ));
    }
    Ok(limit)
}

fn non_negative_revision(value: i64) -> Result<u64, ApiError> {
    u64::try_from(value)
        .map_err(|_| ApiError::BadRequest("expected_revision must be non-negative".to_owned()))
}

fn study_dto_from_record(record: deepref_postgres::StudyDetailRecord) -> StudyDto {
    let study = record.study;
    StudyDto {
        id: study.id.into(),
        project_id: study.project_id,
        title: study.title,
        design: study.design.map(StudyDesign::as_str).map(str::to_owned),
        design_label: study.design.map(StudyDesign::label).map(str::to_owned),
        design_context: StudyDesignContextDto {
            physiotherapy: study.design_context.physiotherapy,
            exposure: study.design_context.exposure,
            prediction_or_ai: study.design_context.prediction_or_ai,
        },
        revision: study.revision,
        reports: record
            .reports
            .into_iter()
            .map(|report| StudyReportDto {
                report_id: report.report_id.into(),
                title: report.title,
                abstract_text: report.abstract_text,
                publication_year: report.publication_year,
                role: report.role.as_str().to_owned(),
                assigned_at: report.assigned_at,
            })
            .collect(),
        tool_suggestions: record
            .tool_suggestions
            .into_iter()
            .map(|suggestion| StudyToolSuggestionDto {
                tool: suggestion.tool,
                rationale: suggestion.rationale,
            })
            .collect(),
        created_at: study.created_at,
        updated_at: study.updated_at,
        updated_by_actor_kind: study.updated_by_actor_kind,
        updated_by_actor_id: study.updated_by_actor_id,
    }
}

fn study_membership_dto(record: deepref_postgres::StudyMembershipRecord) -> StudyMembershipDto {
    StudyMembershipDto {
        study_id: record.study_id.into(),
        role: record.role.as_str().to_owned(),
        study_revision: record.study_revision,
        study: Box::new(study_dto_from_record(record.study)),
    }
}

fn study_dto_from_summary(study: deepref_postgres::StudyRecord) -> StudyDto {
    let suggestions = study
        .design
        .map(|design| deepref_domain::suggest_appraisal_tools(design, study.design_context))
        .unwrap_or_default();
    StudyDto {
        id: study.id.into(),
        project_id: study.project_id,
        title: study.title,
        design: study.design.map(StudyDesign::as_str).map(str::to_owned),
        design_label: study.design.map(StudyDesign::label).map(str::to_owned),
        design_context: StudyDesignContextDto {
            physiotherapy: study.design_context.physiotherapy,
            exposure: study.design_context.exposure,
            prediction_or_ai: study.design_context.prediction_or_ai,
        },
        revision: study.revision,
        reports: Vec::new(),
        tool_suggestions: suggestions
            .into_iter()
            .map(|suggestion| StudyToolSuggestionDto {
                tool: suggestion.tool,
                rationale: suggestion.rationale,
            })
            .collect(),
        created_at: study.created_at,
        updated_at: study.updated_at,
        updated_by_actor_kind: study.updated_by_actor_kind,
        updated_by_actor_id: study.updated_by_actor_id,
    }
}

fn study_event_dto(event: deepref_postgres::StudyEventRecord) -> StudyEventDto {
    StudyEventDto {
        id: event.id,
        study_id: event.study_id.into(),
        report_id: event.report_id.map(Into::into),
        event_type: event.event_type,
        before_study_id: event.before_study_id.map(Into::into),
        result_study_id: event.result_study_id.map(Into::into),
        before_revision: event.before_revision,
        result_revision: event.result_revision,
        before_snapshot: event.before_snapshot,
        result_snapshot: event.result_snapshot,
        payload: event.payload,
        actor_kind: event.actor_kind,
        actor_id: event.actor_id,
        created_at: event.created_at,
    }
}

fn appraisal_definition_dto(
    definition: deepref_application::AppraisalDefinition,
) -> AppraisalDefinitionDto {
    AppraisalDefinitionDto {
        id: definition.id.as_str().to_owned(),
        version: definition.version.get(),
        name: definition.name,
        description: definition.description,
        applicability: AppraisalApplicabilityDto {
            designs: definition
                .applicability
                .designs
                .into_iter()
                .map(|design| design.as_str().to_owned())
                .collect(),
            note: definition.applicability.note,
        },
        domains: definition
            .domains
            .into_iter()
            .map(|domain| AppraisalDomainDto {
                id: domain.id,
                label: domain.label,
                description: domain.description,
                questions: domain
                    .questions
                    .into_iter()
                    .map(|question| AppraisalQuestionDto {
                        id: question.id,
                        label: question.label,
                        help: question.help,
                        answer_schema: answer_schema_dto(question.answer_schema),
                        required: question.required,
                        requires_evidence: question.requires_evidence,
                    })
                    .collect(),
                judgment: judgment_schema_dto(domain.judgment),
            })
            .collect(),
        overall_judgment: judgment_schema_dto(definition.overall_judgment),
    }
}

fn answer_schema_dto(schema: deepref_application::AnswerSchema) -> AppraisalAnswerSchemaDto {
    match schema {
        deepref_application::AnswerSchema::Enum { options } => AppraisalAnswerSchemaDto::Enum {
            options: options.into_iter().map(answer_option_dto).collect(),
        },
        deepref_application::AnswerSchema::Boolean => AppraisalAnswerSchemaDto::Boolean,
        deepref_application::AnswerSchema::Scale { min, max, labels } => {
            AppraisalAnswerSchemaDto::Scale {
                min,
                max,
                labels: labels
                    .into_iter()
                    .map(|(key, value)| (key.to_string(), value))
                    .collect(),
            }
        }
        deepref_application::AnswerSchema::Text { max_length } => {
            AppraisalAnswerSchemaDto::Text { max_length }
        }
    }
}

fn judgment_schema_dto(schema: deepref_application::JudgmentSchema) -> AppraisalJudgmentSchemaDto {
    AppraisalJudgmentSchemaDto {
        options: schema.options.into_iter().map(answer_option_dto).collect(),
        allow_custom: schema.allow_custom,
        required: schema.required,
    }
}

fn answer_option_dto(
    option: deepref_application::appraisal::AnswerOption,
) -> AppraisalAnswerOptionDto {
    AppraisalAnswerOptionDto {
        value: option.value,
        label: option.label,
    }
}

fn appraisal_dto(record: deepref_postgres::AppraisalAssessmentRecord) -> AppraisalAssessmentDto {
    AppraisalAssessmentDto {
        id: record.id,
        project_id: record.project_id.into(),
        report_id: record.report_id.into(),
        definition_id: record.definition_id,
        definition_version: record.definition_version,
        responses: record.responses,
        judgments: record.judgments,
        evidence: record
            .evidence
            .into_iter()
            .map(|evidence| AppraisalEvidenceDto {
                question_id: evidence.question_id,
                document_id: evidence.document_id,
                block_id: evidence.block_id,
            })
            .collect(),
        actor_kind: record.actor_kind,
        actor_id: record.actor_id,
        completed_at: record.completed_at,
        created_at: record.created_at,
    }
}

fn validate_definition_for_response(
    definition: &deepref_application::AppraisalDefinition,
) -> Result<(), ApiError> {
    deepref_application::validate_definition_resource(definition)
        .map_err(|error| ApiError::DataIntegrity(error.to_string()))
}

fn map_study_error(error: deepref_postgres::StudyError) -> ApiError {
    match error {
        deepref_postgres::StudyError::Database(error) => ApiError::Database(error),
        deepref_postgres::StudyError::ProjectNotFound
        | deepref_postgres::StudyError::StudyNotFound
        | deepref_postgres::StudyError::ReportNotInProject => ApiError::NotFound(error.to_string()),
        deepref_postgres::StudyError::AlreadyMember | deepref_postgres::StudyError::NotMember => {
            ApiError::BadRequest(error.to_string())
        }
        deepref_postgres::StudyError::RevisionConflict { current } => ApiError::Conflict {
            code: "STUDY_REVISION_CONFLICT".to_owned(),
            message: "study changed since it was read".to_owned(),
            details: json!({ "current": study_dto_from_record(*current) }),
        },
        deepref_postgres::StudyError::DataIntegrity(message) => ApiError::DataIntegrity(message),
    }
}

fn map_appraisal_error(error: deepref_postgres::AppraisalError) -> ApiError {
    match error {
        deepref_postgres::AppraisalError::Database(error) => ApiError::Database(error),
        deepref_postgres::AppraisalError::ReportNotInProject
        | deepref_postgres::AppraisalError::AssessmentNotFound => {
            ApiError::NotFound(error.to_string())
        }
        deepref_postgres::AppraisalError::Definition(error) => {
            ApiError::BadRequest(error.to_string())
        }
        deepref_postgres::AppraisalError::Validation(error) => {
            ApiError::BadRequest(error.to_string())
        }
        deepref_postgres::AppraisalError::EvidenceNotInReport => {
            ApiError::BadRequest(error.to_string())
        }
        deepref_postgres::AppraisalError::DataIntegrity(message) => {
            ApiError::DataIntegrity(message)
        }
    }
}
