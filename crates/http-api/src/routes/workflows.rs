//! Visual workflow builder API: catalog, templates, workflow CRUD, versions,
//! runs with per-block logs, test runs, endpoint credentials and the public
//! webhook / inbound e-mail hooks.

use std::{
    collections::HashMap,
    sync::{Mutex, OnceLock},
    time::{Duration, Instant},
};

use axum::{
    Json,
    body::{Body, to_bytes},
    extract::{FromRequest, Multipart, Path, Query, Request, State},
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse, Response},
};
use chrono::{DateTime, Utc};
use deepref_application::workflows::{
    self as wf, ConfigFieldKind, InboundEmail, NodeTypeDef, ValidationReport, WorkflowGraph,
};
use deepref_domain::ProjectId;
use deepref_postgres::{
    NodeRunRecord, RotateTarget, StartRun, UpdateWorkflow, WebhookConfig, WorkflowError,
    WorkflowRecord, WorkflowRunRecord, WorkflowVersionRecord,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::PgPool;
use utoipa::{IntoParams, ToSchema};
use utoipa_axum::{router::OpenApiRouter, routes};
use uuid::Uuid;

use super::actor::extract_actor;
use crate::{
    error::{ApiError, ErrorResponse},
    state::AppState,
};

/// Largest body accepted by the public hooks.
pub(crate) const MAX_HOOK_BODY_BYTES: usize = 256 * 1024;
const HOOK_RATE_LIMIT_PER_MINUTE: u32 = 60;
const MAX_TEST_WAIT_MS: u64 = 20_000;
const SIGNATURE_HEADER: &str = "x-deepref-signature";

pub(crate) fn router() -> OpenApiRouter<AppState> {
    let authored = OpenApiRouter::new()
        .routes(routes!(get_catalog))
        .routes(routes!(list_templates))
        .routes(routes!(list_workflows, create_workflow))
        .routes(routes!(create_workflow_from_template))
        .routes(routes!(get_workflow, update_workflow, delete_workflow))
        .routes(routes!(validate_workflow))
        .routes(routes!(get_block_details))
        .routes(routes!(publish_workflow))
        .routes(routes!(list_versions))
        .routes(routes!(get_version))
        .routes(routes!(enable_workflow))
        .routes(routes!(disable_workflow))
        .routes(routes!(list_workflow_runs, start_workflow_run))
        .routes(routes!(start_test_run))
        .routes(routes!(get_endpoints, update_endpoints))
        .routes(routes!(rotate_webhook))
        .routes(routes!(rotate_webhook_secret))
        .routes(routes!(rotate_email))
        .routes(routes!(list_project_workflow_runs))
        .routes(routes!(get_workflow_run))
        .routes(routes!(cancel_workflow_run))
        .routes(routes!(download_workflow_file));
    let hooks = OpenApiRouter::new()
        .routes(routes!(receive_webhook))
        .routes(routes!(receive_inbound_email))
        .layer(axum::extract::DefaultBodyLimit::max(MAX_HOOK_BODY_BYTES));
    authored.merge(hooks)
}

// ---------------------------------------------------------------------------
// DTOs

#[derive(Debug, Clone, Serialize, ToSchema)]
pub(crate) struct PortDto {
    pub id: String,
    pub label: String,
    /// `trigger`, `records`, `record`, `report`, `study`, `document`, `text` or `json`.
    #[serde(rename = "type")]
    pub port_type: String,
    pub required: bool,
    pub multiple: bool,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub(crate) struct ConfigOptionDto {
    pub value: String,
    pub label: String,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub(crate) struct ConfigFieldDto {
    pub key: String,
    pub label: String,
    pub help: Option<String>,
    /// `text`, `long_text`, `number`, `select`, `boolean`, `project_field`,
    /// `secret`, `duration`, `schedule`, `condition`, `publication_query` or `field_list`.
    pub kind: String,
    pub required: bool,
    pub default: Option<Value>,
    pub options: Vec<ConfigOptionDto>,
    pub placeholder: Option<String>,
    pub min: Option<f64>,
    pub max: Option<f64>,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub(crate) struct NodeTypeDto {
    pub id: String,
    /// `trigger`, `data`, `action`, `ai`, `logic` or `integration`.
    pub category: String,
    pub label: String,
    pub description: String,
    pub inputs: Vec<PortDto>,
    pub outputs: Vec<PortDto>,
    pub config: Vec<ConfigFieldDto>,
    /// Test runs never run these for real.
    pub has_side_effects: bool,
    /// Several outputs of which only one carries data on each run.
    pub branches: bool,
}

fn field_kind_name(kind: ConfigFieldKind) -> String {
    serde_json::to_value(kind)
        .ok()
        .and_then(|value| value.as_str().map(str::to_owned))
        .unwrap_or_default()
}

fn node_type_dto(def: &NodeTypeDef) -> NodeTypeDto {
    let port = |port: &wf::PortDef| PortDto {
        id: port.id.clone(),
        label: port.label.clone(),
        port_type: port.port_type.as_str().to_owned(),
        required: port.required,
        multiple: port.multiple,
    };
    NodeTypeDto {
        id: def.id.clone(),
        category: def.category.as_str().to_owned(),
        label: def.label.clone(),
        description: def.description.clone(),
        inputs: def.inputs.iter().map(port).collect(),
        outputs: def.outputs.iter().map(port).collect(),
        config: def
            .config
            .iter()
            .map(|field| ConfigFieldDto {
                key: field.key.clone(),
                label: field.label.clone(),
                help: field.help.clone(),
                kind: field_kind_name(field.kind),
                required: field.required,
                default: field.default.clone(),
                options: field
                    .options
                    .iter()
                    .map(|option| ConfigOptionDto {
                        value: option.value.clone(),
                        label: option.label.clone(),
                    })
                    .collect(),
                placeholder: field.placeholder.clone(),
                min: field.min,
                max: field.max,
            })
            .collect(),
        has_side_effects: def.has_side_effects,
        branches: def.branches,
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, ToSchema)]
pub(crate) struct PositionDto {
    pub x: f64,
    pub y: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub(crate) struct WorkflowGraphNodeDto {
    pub id: String,
    /// A block id from the catalog, for example `logic.filter`.
    #[serde(rename = "type")]
    pub node_type: String,
    pub position: PositionDto,
    #[serde(default)]
    pub label: Option<String>,
    /// Values for the block's configuration fields, keyed by field key.
    #[serde(default)]
    pub config: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub(crate) struct EndpointDto {
    pub node: String,
    pub port: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub(crate) struct WorkflowGraphEdgeDto {
    #[serde(default)]
    pub id: Option<String>,
    pub from: EndpointDto,
    pub to: EndpointDto,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub(crate) struct WorkflowGraphDto {
    pub nodes: Vec<WorkflowGraphNodeDto>,
    pub edges: Vec<WorkflowGraphEdgeDto>,
}

fn to_graph_dto(graph: &WorkflowGraph) -> Result<WorkflowGraphDto, ApiError> {
    Ok(serde_json::from_value(serde_json::to_value(graph)?)?)
}

fn from_graph_dto(graph: &WorkflowGraphDto) -> Result<WorkflowGraph, ApiError> {
    serde_json::from_value(serde_json::to_value(graph)?)
        .map_err(|_| ApiError::BadRequest("The workflow graph is not valid.".to_owned()))
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub(crate) struct TemplateDto {
    pub id: String,
    /// The built-in recipe this template replaces, if any.
    pub recipe: Option<String>,
    pub name: String,
    pub description: String,
    pub graph: WorkflowGraphDto,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub(crate) struct WorkflowDto {
    pub id: Uuid,
    pub project_id: Uuid,
    pub name: String,
    pub description: String,
    /// `enabled` or `disabled`.
    pub status: String,
    /// The draft being edited on the canvas.
    pub graph: WorkflowGraphDto,
    /// Send back when saving to detect edits made elsewhere.
    pub draft_revision: i64,
    pub published_version: Option<i32>,
    /// Stable key of the trigger of the published version.
    pub trigger: Option<String>,
    pub has_unpublished_changes: bool,
    pub signature_required: bool,
    pub next_run_at: Option<DateTime<Utc>>,
    pub last_run_at: Option<DateTime<Utc>>,
    /// Publication alerts: when it last looked, what it found, last problem.
    pub alert_status: Value,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

fn workflow_dto(record: WorkflowRecord) -> Result<WorkflowDto, ApiError> {
    Ok(WorkflowDto {
        id: record.id,
        project_id: record.project_id,
        name: record.name,
        description: record.description,
        status: record.status.as_str().to_owned(),
        graph: to_graph_dto(&record.draft_graph)?,
        draft_revision: record.draft_revision,
        published_version: record.published_version,
        trigger: record.trigger_kind,
        has_unpublished_changes: record.has_unpublished_changes,
        signature_required: record.webhook_signature_required,
        next_run_at: record.next_fire_at,
        last_run_at: record.last_fired_at,
        alert_status: record.poll_state,
        created_at: record.created_at,
        updated_at: record.updated_at,
    })
}

#[derive(Debug, Deserialize, ToSchema)]
pub(crate) struct CreateWorkflowRequest {
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub graph: Option<WorkflowGraphDto>,
}

#[derive(Debug, Deserialize, ToSchema)]
pub(crate) struct UpdateWorkflowRequest {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub graph: Option<WorkflowGraphDto>,
    /// The `draft_revision` the editor started from.
    #[serde(default)]
    pub expected_revision: Option<i64>,
}

#[derive(Debug, Deserialize, ToSchema)]
pub(crate) struct FromTemplateRequest {
    pub template_id: String,
    #[serde(default)]
    pub name: Option<String>,
}

#[derive(Debug, Deserialize, ToSchema)]
pub(crate) struct PublishRequest {
    #[serde(default)]
    pub note: Option<String>,
}

#[derive(Debug, Deserialize, ToSchema)]
pub(crate) struct RunRequest {
    /// Information handed to the trigger block, as the trigger would.
    #[serde(default)]
    pub trigger_data: Option<Value>,
}

#[derive(Debug, Deserialize, ToSchema)]
pub(crate) struct TestRunRequest {
    /// Sample information for the trigger block.
    #[serde(default)]
    pub trigger_data: Option<Value>,
    /// Wait up to this long (at most 20000) for the test to finish before answering.
    #[serde(default)]
    pub wait_ms: Option<u64>,
}

#[derive(Debug, Deserialize, ToSchema)]
pub(crate) struct EndpointSettingsRequest {
    /// Turn off to accept unsigned calls. The canvas should warn about this.
    pub signature_required: bool,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub(crate) struct IssueDto {
    /// The block with the problem; absent for problems with the whole workflow.
    pub node_id: Option<String>,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub(crate) struct ValidationReportDto {
    pub ok: bool,
    pub issues: Vec<IssueDto>,
}

fn report_dto(report: &ValidationReport) -> ValidationReportDto {
    ValidationReportDto {
        ok: report.is_ok(),
        issues: report
            .issues
            .iter()
            .map(|issue| IssueDto {
                node_id: issue.node_id.clone(),
                message: issue.message.clone(),
            })
            .collect(),
    }
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub(crate) struct VersionDto {
    pub id: Uuid,
    pub version: i32,
    pub trigger: String,
    pub note: Option<String>,
    pub published_by: String,
    pub published_at: DateTime<Utc>,
    pub graph: WorkflowGraphDto,
}

fn version_dto(record: WorkflowVersionRecord) -> Result<VersionDto, ApiError> {
    Ok(VersionDto {
        id: record.id,
        version: record.version,
        trigger: record.trigger_kind,
        note: record.note,
        published_by: record.published_by_id,
        published_at: record.published_at,
        graph: to_graph_dto(&record.graph)?,
    })
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub(crate) struct EndpointsDto {
    /// Path of the webhook address, to be appended to the server address.
    pub webhook_path: String,
    /// Shared secret used to sign webhook calls.
    pub webhook_secret: String,
    pub signature_required: bool,
    /// Header carrying `sha256=<hex HMAC-SHA256 of the raw body>`.
    pub signature_header: String,
    /// Path of the inbound e-mail address.
    pub email_path: String,
}

fn endpoints_dto(config: WebhookConfig) -> EndpointsDto {
    EndpointsDto {
        webhook_path: format!("/hooks/workflows/{}", config.token),
        webhook_secret: config.secret,
        signature_required: config.signature_required,
        signature_header: "X-DeepRef-Signature".to_owned(),
        email_path: format!("/hooks/email/{}", config.email_token),
    }
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub(crate) struct NodeRunDto {
    pub id: Uuid,
    pub node_id: String,
    pub node_type: String,
    /// Which repeat of a loop this belongs to; empty outside loops.
    pub iteration: String,
    /// `queued`, `running`, `completed`, `failed`, `skipped` or `cancelled`.
    pub status: String,
    pub attempts: i32,
    pub input: Option<Value>,
    pub output: Option<Value>,
    /// Beginning of the data when it was too large to include.
    pub input_preview: Option<String>,
    pub output_preview: Option<String>,
    /// What the block did, in plain language.
    pub note: Option<String>,
    pub error: Option<String>,
    pub created_at: DateTime<Utc>,
    pub started_at: Option<DateTime<Utc>>,
    pub finished_at: Option<DateTime<Utc>>,
}

fn node_run_dto(node: NodeRunRecord) -> NodeRunDto {
    NodeRunDto {
        id: node.id,
        node_id: node.node_id,
        node_type: node.node_type,
        iteration: node.iteration,
        status: node.status,
        attempts: node.attempts,
        input: node.input,
        output: node.output,
        input_preview: node.input_preview,
        output_preview: node.output_preview,
        note: node.note,
        error: node.error,
        created_at: node.created_at,
        started_at: node.started_at,
        finished_at: node.finished_at,
    }
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub(crate) struct WorkflowRunDto {
    pub id: Uuid,
    pub workflow_id: Uuid,
    pub workflow_name: String,
    pub version: Option<i32>,
    pub trigger: String,
    /// `queued`, `running`, `completed`, `failed` or `cancelled`.
    pub status: String,
    pub test_mode: bool,
    pub actor: String,
    pub error: Option<String>,
    pub created_at: DateTime<Utc>,
    pub started_at: Option<DateTime<Utc>>,
    pub finished_at: Option<DateTime<Utc>>,
    pub trigger_data: Value,
    /// The graph this run executed.
    pub graph: WorkflowGraphDto,
    /// Per-block log. Empty in lists.
    pub nodes: Vec<NodeRunDto>,
    /// The AI verdicts of the run's "Run an AI review" screening, when it used one.
    pub review_counts: Option<ReviewCountsDto>,
}

/// AI verdict counts of a run, as the run history shows them.
#[derive(Debug, Clone, Copy, Serialize, ToSchema)]
pub(crate) struct ReviewCountsDto {
    /// Records the AI would include.
    pub included: i64,
    /// Records the AI would exclude.
    pub excluded: i64,
    /// Records the AI was not sure about.
    pub unsure: i64,
}

fn run_dto(
    run: WorkflowRunRecord,
    counts: Option<deepref_postgres::ReviewCounts>,
) -> Result<WorkflowRunDto, ApiError> {
    Ok(WorkflowRunDto {
        id: run.id,
        workflow_id: run.workflow_id,
        workflow_name: run.workflow_name,
        version: run.version,
        trigger: run.trigger_kind,
        status: run.status,
        test_mode: run.test_mode,
        actor: run.actor_id,
        error: run.error,
        created_at: run.created_at,
        started_at: run.started_at,
        finished_at: run.finished_at,
        trigger_data: run.trigger_data,
        graph: to_graph_dto(&run.graph)?,
        nodes: run.nodes.into_iter().map(node_run_dto).collect(),
        review_counts: counts.map(|counts| ReviewCountsDto {
            included: counts.included,
            excluded: counts.excluded,
            unsure: counts.unsure,
        }),
    })
}

/// Run list rows, each with the verdict counts of its AI review, if it had one.
async fn run_list_dtos(
    pool: &PgPool,
    runs: Vec<WorkflowRunRecord>,
) -> Result<Vec<WorkflowRunDto>, ApiError> {
    let ids: Vec<Uuid> = runs.iter().map(|run| run.id).collect();
    let counts = deepref_postgres::review_counts_for_runs(pool, &ids)
        .await
        .map_err(map_error)?;
    runs.into_iter()
        .map(|run| {
            let found = counts.get(&run.id).copied();
            run_dto(run, found)
        })
        .collect()
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub(crate) struct RunStartedDto {
    pub run_id: Uuid,
    /// False when the same trigger event had already started this workflow.
    pub created: bool,
}

#[derive(Debug, Deserialize, IntoParams)]
pub(crate) struct RunListQuery {
    pub limit: Option<i64>,
    /// Only runs of this workflow.
    pub workflow_id: Option<Uuid>,
}

// ---------------------------------------------------------------------------
// Helpers

fn project_id(value: Uuid) -> Result<ProjectId, ApiError> {
    if value.is_nil() {
        return Err(ApiError::BadRequest(
            "project_id must not be nil".to_owned(),
        ));
    }
    Ok(ProjectId::from(value))
}

async fn ensure_project(pool: &PgPool, id: ProjectId) -> Result<(), ApiError> {
    let exists = sqlx::query_scalar::<_, bool>("SELECT EXISTS(SELECT 1 FROM projects WHERE id=$1)")
        .bind(id.as_uuid())
        .fetch_one(pool)
        .await?;
    if exists {
        Ok(())
    } else {
        Err(ApiError::NotFound("project not found".to_owned()))
    }
}

fn map_error(error: WorkflowError) -> ApiError {
    match error {
        WorkflowError::NotFound => ApiError::NotFound("automation not found".to_owned()),
        WorkflowError::RunNotFound => ApiError::NotFound("run not found".to_owned()),
        WorkflowError::Conflict(message) => ApiError::Conflict {
            code: "WORKFLOW_CONFLICT".to_owned(),
            message,
            details: json!({}),
        },
        WorkflowError::Invalid(report) => ApiError::Conflict {
            code: "WORKFLOW_INVALID".to_owned(),
            message: "The automation has problems to fix before it can be published.".to_owned(),
            details: json!({ "issues": report_dto(&report).issues }),
        },
        WorkflowError::InvalidInput(message) => ApiError::BadRequest(message),
        WorkflowError::NotPublished => ApiError::Conflict {
            code: "WORKFLOW_NOT_PUBLISHED".to_owned(),
            message: "Publish the automation first.".to_owned(),
            details: json!({}),
        },
        WorkflowError::RunFinished => ApiError::Conflict {
            code: "WORKFLOW_RUN_FINISHED".to_owned(),
            message: "The run has already finished.".to_owned(),
            details: json!({}),
        },
        WorkflowError::Serialization(error) => ApiError::Json(error),
        WorkflowError::InvalidStoredValue(message) => ApiError::DataIntegrity(message),
        WorkflowError::Database(error) => ApiError::Database(error),
    }
}

fn idempotency_key(headers: &HeaderMap) -> String {
    headers
        .get("idempotency-key")
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .filter(|value| !value.is_empty() && value.len() <= 200)
        .map_or_else(
            || format!("manual:{}", Uuid::new_v4()),
            |value| format!("manual:{value}"),
        )
}

// ---------------------------------------------------------------------------
// Catalog and templates

#[utoipa::path(
    get,
    path = "/automations/catalog",
    operation_id = "getAutomationCatalog",
    tag = "automations",
    responses((status = 200, description = "Every block available on the canvas", body = Vec<NodeTypeDto>))
)]
pub(crate) async fn get_catalog() -> Json<Vec<NodeTypeDto>> {
    Json(wf::catalog().iter().map(node_type_dto).collect())
}

fn template_dto(template: &wf::WorkflowTemplate) -> Result<TemplateDto, ApiError> {
    Ok(TemplateDto {
        id: template.id.to_owned(),
        recipe: template.recipe.map(|recipe| recipe.id().to_owned()),
        name: template.name.to_owned(),
        description: template.description.to_owned(),
        graph: to_graph_dto(&template.graph)?,
    })
}

#[utoipa::path(
    get,
    path = "/automations/templates",
    operation_id = "listAutomationTemplates",
    tag = "automations",
    responses((status = 200, description = "Ready-made automations", body = Vec<TemplateDto>))
)]
pub(crate) async fn list_templates() -> Result<Json<Vec<TemplateDto>>, ApiError> {
    Ok(Json(
        wf::templates()
            .iter()
            .map(template_dto)
            .collect::<Result<_, _>>()?,
    ))
}

// ---------------------------------------------------------------------------
// Workflows

#[utoipa::path(
    get,
    path = "/projects/{project_id}/automations/workflows",
    operation_id = "listWorkflows",
    tag = "automations",
    params(("project_id" = Uuid, Path, description = "Project identifier")),
    responses(
        (status = 200, description = "The project's automations", body = Vec<WorkflowDto>),
        (status = 404, description = "Project not found", body = ErrorResponse)
    )
)]
pub(crate) async fn list_workflows(
    State(state): State<AppState>,
    Path(project): Path<Uuid>,
) -> Result<Json<Vec<WorkflowDto>>, ApiError> {
    let project = project_id(project)?;
    ensure_project(&state.pool, project).await?;
    let records = deepref_postgres::list_workflows(&state.pool, project)
        .await
        .map_err(map_error)?;
    Ok(Json(
        records
            .into_iter()
            .map(workflow_dto)
            .collect::<Result<_, _>>()?,
    ))
}

#[utoipa::path(
    post,
    path = "/projects/{project_id}/automations/workflows",
    operation_id = "createWorkflow",
    tag = "automations",
    params(("project_id" = Uuid, Path, description = "Project identifier")),
    request_body = CreateWorkflowRequest,
    responses(
        (status = 201, description = "Automation created as a draft", body = WorkflowDto),
        (status = 400, description = "Invalid request", body = ErrorResponse),
        (status = 409, description = "Name already used", body = ErrorResponse)
    )
)]
pub(crate) async fn create_workflow(
    State(state): State<AppState>,
    Path(project): Path<Uuid>,
    headers: HeaderMap,
    Json(input): Json<CreateWorkflowRequest>,
) -> Result<(StatusCode, Json<WorkflowDto>), ApiError> {
    let project = project_id(project)?;
    ensure_project(&state.pool, project).await?;
    let graph = input
        .graph
        .as_ref()
        .map(from_graph_dto)
        .transpose()?
        .unwrap_or_default();
    let record = deepref_postgres::create_workflow(
        &state.pool,
        project,
        &input.name,
        input.description.as_deref().unwrap_or_default(),
        graph,
        &extract_actor(&headers)?,
    )
    .await
    .map_err(map_error)?;
    Ok((StatusCode::CREATED, Json(workflow_dto(record)?)))
}

#[utoipa::path(
    post,
    path = "/projects/{project_id}/automations/workflows/from-template",
    operation_id = "createWorkflowFromTemplate",
    tag = "automations",
    params(("project_id" = Uuid, Path, description = "Project identifier")),
    request_body = FromTemplateRequest,
    responses(
        (status = 201, description = "Automation created from the template", body = WorkflowDto),
        (status = 400, description = "Unknown template", body = ErrorResponse),
        (status = 409, description = "Name already used", body = ErrorResponse)
    )
)]
pub(crate) async fn create_workflow_from_template(
    State(state): State<AppState>,
    Path(project): Path<Uuid>,
    headers: HeaderMap,
    Json(input): Json<FromTemplateRequest>,
) -> Result<(StatusCode, Json<WorkflowDto>), ApiError> {
    let project = project_id(project)?;
    ensure_project(&state.pool, project).await?;
    let template = wf::template(&input.template_id)
        .ok_or_else(|| ApiError::BadRequest("That template does not exist.".to_owned()))?;
    let name = input.name.unwrap_or_else(|| template.name.to_owned());
    let record = deepref_postgres::create_workflow(
        &state.pool,
        project,
        &name,
        template.description,
        template.graph.clone(),
        &extract_actor(&headers)?,
    )
    .await
    .map_err(map_error)?;
    Ok((StatusCode::CREATED, Json(workflow_dto(record)?)))
}

#[utoipa::path(
    get,
    path = "/projects/{project_id}/automations/workflows/{workflow_id}",
    operation_id = "getWorkflow",
    tag = "automations",
    params(("project_id" = Uuid, Path), ("workflow_id" = Uuid, Path)),
    responses((status = 200, body = WorkflowDto), (status = 404, body = ErrorResponse))
)]
pub(crate) async fn get_workflow(
    State(state): State<AppState>,
    Path((project, workflow)): Path<(Uuid, Uuid)>,
) -> Result<Json<WorkflowDto>, ApiError> {
    let record = deepref_postgres::get_workflow(&state.pool, project_id(project)?, workflow)
        .await
        .map_err(map_error)?;
    Ok(Json(workflow_dto(record)?))
}

#[utoipa::path(
    put,
    path = "/projects/{project_id}/automations/workflows/{workflow_id}",
    operation_id = "updateWorkflow",
    tag = "automations",
    params(("project_id" = Uuid, Path), ("workflow_id" = Uuid, Path)),
    request_body = UpdateWorkflowRequest,
    responses(
        (status = 200, description = "Draft saved", body = WorkflowDto),
        (status = 404, body = ErrorResponse),
        (status = 409, description = "Edited elsewhere or name already used", body = ErrorResponse)
    )
)]
pub(crate) async fn update_workflow(
    State(state): State<AppState>,
    Path((project, workflow)): Path<(Uuid, Uuid)>,
    Json(input): Json<UpdateWorkflowRequest>,
) -> Result<Json<WorkflowDto>, ApiError> {
    let graph = input.graph.as_ref().map(from_graph_dto).transpose()?;
    let record = deepref_postgres::update_workflow(
        &state.pool,
        project_id(project)?,
        workflow,
        UpdateWorkflow {
            name: input.name,
            description: input.description,
            graph,
            expected_revision: input.expected_revision,
        },
    )
    .await
    .map_err(map_error)?;
    Ok(Json(workflow_dto(record)?))
}

#[utoipa::path(
    delete,
    path = "/projects/{project_id}/automations/workflows/{workflow_id}",
    operation_id = "deleteWorkflow",
    tag = "automations",
    params(("project_id" = Uuid, Path), ("workflow_id" = Uuid, Path)),
    responses((status = 204, description = "Deleted with its history"), (status = 404, body = ErrorResponse))
)]
pub(crate) async fn delete_workflow(
    State(state): State<AppState>,
    Path((project, workflow)): Path<(Uuid, Uuid)>,
) -> Result<StatusCode, ApiError> {
    deepref_postgres::delete_workflow(&state.pool, project_id(project)?, workflow)
        .await
        .map_err(map_error)?;
    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(
    post,
    path = "/projects/{project_id}/automations/workflows/{workflow_id}/validate",
    operation_id = "validateWorkflow",
    tag = "automations",
    params(("project_id" = Uuid, Path), ("workflow_id" = Uuid, Path)),
    responses((status = 200, description = "Problems found in the draft, per block", body = ValidationReportDto), (status = 404, body = ErrorResponse))
)]
pub(crate) async fn validate_workflow(
    State(state): State<AppState>,
    Path((project, workflow)): Path<(Uuid, Uuid)>,
) -> Result<Json<ValidationReportDto>, ApiError> {
    let report =
        deepref_postgres::validate_workflow_draft(&state.pool, project_id(project)?, workflow)
            .await
            .map_err(map_error)?;
    Ok(Json(report_dto(&report)))
}

/// The draft as it is on screen, and the block whose fields are being filled in.
#[derive(Debug, Clone, Deserialize, ToSchema)]
pub(crate) struct BlockDetailsRequest {
    /// The draft as it is on screen; it need not be saved.
    pub graph: WorkflowGraphDto,
    /// The block whose fields are being filled in.
    pub node_id: String,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub(crate) struct DetailDto {
    /// The name to write between braces, for example `title` or `screening.final`.
    pub key: String,
    pub label: String,
    /// `text`, `number`, `yes_no`, `list` or `any`.
    pub kind: String,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub(crate) struct FieldDetailsDto {
    /// The block the value comes from, when one is connected.
    pub source: Option<String>,
    pub fields: Vec<DetailDto>,
    /// The value is a whole list: only its size (`count`) is available.
    pub list: bool,
    /// The value has keys nobody can know in advance; the name is typed by hand.
    pub open: bool,
    /// The field is filled in once for each record of a list.
    pub each_item: bool,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub(crate) struct BlockDetailsDto {
    /// Details for each text and rule field of the block, by field key.
    pub fields: HashMap<String, FieldDetailsDto>,
}

#[utoipa::path(
    post,
    path = "/projects/{project_id}/automations/block-details",
    operation_id = "getBlockDetails",
    tag = "automations",
    params(("project_id" = Uuid, Path)),
    request_body = BlockDetailsRequest,
    responses(
        (status = 200, description = "The names each text and rule field of the block can use", body = BlockDetailsDto),
        (status = 400, description = "The graph is not valid", body = ErrorResponse),
        (status = 404, description = "The block is not in the graph", body = ErrorResponse)
    )
)]
/// Works out what a block's fields can read from the steps before it, from the
/// draft sent in. Nothing is read from or written to the database.
pub(crate) async fn get_block_details(
    Path(project): Path<Uuid>,
    Json(input): Json<BlockDetailsRequest>,
) -> Result<Json<BlockDetailsDto>, ApiError> {
    project_id(project)?;
    let graph = from_graph_dto(&input.graph)?;
    let node = graph
        .node(&input.node_id)
        .ok_or_else(|| ApiError::NotFound("block not found".to_owned()))?;
    let Some(def) = wf::node_type(&node.node_type) else {
        return Ok(Json(BlockDetailsDto {
            fields: HashMap::new(),
        }));
    };
    let fields = def
        .config
        .iter()
        .filter_map(|field| {
            wf::field_details(&graph, &input.node_id, &field.key)
                .map(|details| (field.key.clone(), field_details_dto(details)))
        })
        .collect();
    Ok(Json(BlockDetailsDto { fields }))
}

fn field_details_dto(details: wf::FieldDetails) -> FieldDetailsDto {
    FieldDetailsDto {
        source: details.source,
        fields: details
            .fields
            .into_iter()
            .map(|detail| DetailDto {
                key: detail.key,
                label: detail.label,
                kind: detail_kind_name(detail.kind).to_owned(),
            })
            .collect(),
        list: details.list,
        open: details.open,
        each_item: details.each_item,
    }
}

fn detail_kind_name(kind: wf::DetailKind) -> &'static str {
    match kind {
        wf::DetailKind::Text => "text",
        wf::DetailKind::Number => "number",
        wf::DetailKind::YesNo => "yes_no",
        wf::DetailKind::List => "list",
        wf::DetailKind::Any => "any",
    }
}

#[utoipa::path(
    post,
    path = "/projects/{project_id}/automations/workflows/{workflow_id}/publish",
    operation_id = "publishWorkflow",
    tag = "automations",
    params(("project_id" = Uuid, Path), ("workflow_id" = Uuid, Path)),
    request_body = PublishRequest,
    responses(
        (status = 201, description = "New immutable version", body = VersionDto),
        (status = 404, body = ErrorResponse),
        (status = 409, description = "The draft has problems; details.issues lists them per block", body = ErrorResponse)
    )
)]
pub(crate) async fn publish_workflow(
    State(state): State<AppState>,
    Path((project, workflow)): Path<(Uuid, Uuid)>,
    headers: HeaderMap,
    Json(input): Json<PublishRequest>,
) -> Result<(StatusCode, Json<VersionDto>), ApiError> {
    let version = deepref_postgres::publish_workflow(
        &state.pool,
        project_id(project)?,
        workflow,
        input.note.as_deref(),
        &extract_actor(&headers)?,
    )
    .await
    .map_err(map_error)?;
    Ok((StatusCode::CREATED, Json(version_dto(version)?)))
}

#[utoipa::path(
    get,
    path = "/projects/{project_id}/automations/workflows/{workflow_id}/versions",
    operation_id = "listWorkflowVersions",
    tag = "automations",
    params(("project_id" = Uuid, Path), ("workflow_id" = Uuid, Path)),
    responses((status = 200, body = Vec<VersionDto>), (status = 404, body = ErrorResponse))
)]
pub(crate) async fn list_versions(
    State(state): State<AppState>,
    Path((project, workflow)): Path<(Uuid, Uuid)>,
) -> Result<Json<Vec<VersionDto>>, ApiError> {
    let versions =
        deepref_postgres::list_workflow_versions(&state.pool, project_id(project)?, workflow)
            .await
            .map_err(map_error)?;
    Ok(Json(
        versions
            .into_iter()
            .map(version_dto)
            .collect::<Result<_, _>>()?,
    ))
}

#[utoipa::path(
    get,
    path = "/projects/{project_id}/automations/workflows/{workflow_id}/versions/{version}",
    operation_id = "getWorkflowVersion",
    tag = "automations",
    params(("project_id" = Uuid, Path), ("workflow_id" = Uuid, Path), ("version" = i32, Path)),
    responses((status = 200, body = VersionDto), (status = 404, body = ErrorResponse))
)]
pub(crate) async fn get_version(
    State(state): State<AppState>,
    Path((project, workflow, version)): Path<(Uuid, Uuid, i32)>,
) -> Result<Json<VersionDto>, ApiError> {
    let version = deepref_postgres::get_workflow_version(
        &state.pool,
        project_id(project)?,
        workflow,
        version,
    )
    .await
    .map_err(map_error)?;
    Ok(Json(version_dto(version)?))
}

#[utoipa::path(
    post,
    path = "/projects/{project_id}/automations/workflows/{workflow_id}/enable",
    operation_id = "enableWorkflow",
    tag = "automations",
    params(("project_id" = Uuid, Path), ("workflow_id" = Uuid, Path)),
    responses((status = 200, body = WorkflowDto), (status = 409, description = "Not published yet", body = ErrorResponse))
)]
pub(crate) async fn enable_workflow(
    State(state): State<AppState>,
    Path((project, workflow)): Path<(Uuid, Uuid)>,
) -> Result<Json<WorkflowDto>, ApiError> {
    let record =
        deepref_postgres::set_workflow_enabled(&state.pool, project_id(project)?, workflow, true)
            .await
            .map_err(map_error)?;
    Ok(Json(workflow_dto(record)?))
}

#[utoipa::path(
    post,
    path = "/projects/{project_id}/automations/workflows/{workflow_id}/disable",
    operation_id = "disableWorkflow",
    tag = "automations",
    params(("project_id" = Uuid, Path), ("workflow_id" = Uuid, Path)),
    responses((status = 200, body = WorkflowDto), (status = 404, body = ErrorResponse))
)]
pub(crate) async fn disable_workflow(
    State(state): State<AppState>,
    Path((project, workflow)): Path<(Uuid, Uuid)>,
) -> Result<Json<WorkflowDto>, ApiError> {
    let record =
        deepref_postgres::set_workflow_enabled(&state.pool, project_id(project)?, workflow, false)
            .await
            .map_err(map_error)?;
    Ok(Json(workflow_dto(record)?))
}

// ---------------------------------------------------------------------------
// Runs

#[utoipa::path(
    get,
    path = "/projects/{project_id}/automations/workflows/{workflow_id}/runs",
    operation_id = "listWorkflowRuns",
    tag = "automations",
    params(("project_id" = Uuid, Path), ("workflow_id" = Uuid, Path), ("limit" = Option<i64>, Query, description = "1 to 100, default 25")),
    responses((status = 200, body = Vec<WorkflowRunDto>), (status = 404, body = ErrorResponse))
)]
pub(crate) async fn list_workflow_runs(
    State(state): State<AppState>,
    Path((project, workflow)): Path<(Uuid, Uuid)>,
    Query(query): Query<RunListQuery>,
) -> Result<Json<Vec<WorkflowRunDto>>, ApiError> {
    let project = project_id(project)?;
    deepref_postgres::get_workflow(&state.pool, project, workflow)
        .await
        .map_err(map_error)?;
    let runs = deepref_postgres::list_workflow_runs(
        &state.pool,
        project,
        Some(workflow),
        query.limit.unwrap_or(25),
    )
    .await
    .map_err(map_error)?;
    Ok(Json(run_list_dtos(&state.pool, runs).await?))
}

#[utoipa::path(
    get,
    path = "/projects/{project_id}/automations/workflow-runs",
    operation_id = "listProjectWorkflowRuns",
    tag = "automations",
    params(("project_id" = Uuid, Path), RunListQuery),
    responses((status = 200, body = Vec<WorkflowRunDto>), (status = 404, body = ErrorResponse))
)]
pub(crate) async fn list_project_workflow_runs(
    State(state): State<AppState>,
    Path(project): Path<Uuid>,
    Query(query): Query<RunListQuery>,
) -> Result<Json<Vec<WorkflowRunDto>>, ApiError> {
    let project = project_id(project)?;
    ensure_project(&state.pool, project).await?;
    let runs = deepref_postgres::list_workflow_runs(
        &state.pool,
        project,
        query.workflow_id,
        query.limit.unwrap_or(25),
    )
    .await
    .map_err(map_error)?;
    Ok(Json(run_list_dtos(&state.pool, runs).await?))
}

#[utoipa::path(
    get,
    path = "/projects/{project_id}/automations/workflow-runs/{run_id}",
    operation_id = "getWorkflowRun",
    tag = "automations",
    params(("project_id" = Uuid, Path), ("run_id" = Uuid, Path)),
    responses((status = 200, description = "The run with the log of every block", body = WorkflowRunDto), (status = 404, body = ErrorResponse))
)]
pub(crate) async fn get_workflow_run(
    State(state): State<AppState>,
    Path((project, run)): Path<(Uuid, Uuid)>,
) -> Result<Json<WorkflowRunDto>, ApiError> {
    let run = deepref_postgres::get_workflow_run(&state.pool, project_id(project)?, run)
        .await
        .map_err(map_error)?;
    let counts = deepref_postgres::review_counts_for_runs(&state.pool, &[run.id])
        .await
        .map_err(map_error)?;
    let found = counts.get(&run.id).copied();
    Ok(Json(run_dto(run, found)?))
}

#[utoipa::path(
    post,
    path = "/projects/{project_id}/automations/workflow-runs/{run_id}/cancel",
    operation_id = "cancelWorkflowRun",
    tag = "automations",
    params(("project_id" = Uuid, Path), ("run_id" = Uuid, Path)),
    responses((status = 204, description = "Cancelled"), (status = 404, body = ErrorResponse), (status = 409, description = "Already finished", body = ErrorResponse))
)]
pub(crate) async fn cancel_workflow_run(
    State(state): State<AppState>,
    Path((project, run)): Path<(Uuid, Uuid)>,
) -> Result<StatusCode, ApiError> {
    deepref_postgres::cancel_workflow_run(&state.pool, project_id(project)?, run)
        .await
        .map_err(map_error)?;
    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(
    post,
    path = "/projects/{project_id}/automations/workflows/{workflow_id}/runs",
    operation_id = "startWorkflowRun",
    tag = "automations",
    params(("project_id" = Uuid, Path), ("workflow_id" = Uuid, Path), ("Idempotency-Key" = Option<String>, Header, description = "Optional key making the call replay-safe")),
    request_body = RunRequest,
    responses(
        (status = 201, description = "Run started", body = RunStartedDto),
        (status = 200, description = "Same Idempotency-Key as an earlier run", body = RunStartedDto),
        (status = 409, description = "Not published yet", body = ErrorResponse)
    )
)]
pub(crate) async fn start_workflow_run(
    State(state): State<AppState>,
    Path((project, workflow)): Path<(Uuid, Uuid)>,
    headers: HeaderMap,
    Json(input): Json<RunRequest>,
) -> Result<(StatusCode, Json<RunStartedDto>), ApiError> {
    let project = project_id(project)?;
    deepref_postgres::get_workflow(&state.pool, project, workflow)
        .await
        .map_err(map_error)?;
    let (version_id, graph) = deepref_postgres::get_published_graph(&state.pool, workflow)
        .await
        .map_err(map_error)?;
    let actor = extract_actor(&headers)?;
    let trigger_data = input.trigger_data.unwrap_or_else(|| json!({}));
    let started = deepref_postgres::start_run(
        &state.pool,
        &StartRun {
            project_id: project.as_uuid(),
            workflow_id: workflow,
            version_id: Some(version_id),
            graph,
            trigger_kind: "manual".to_owned(),
            trigger_data: json!({ "data": trigger_data }),
            idempotency_key: idempotency_key(&headers),
            actor_kind: actor.kind().as_str().to_owned(),
            actor_id: actor.id().to_owned(),
            test_mode: false,
        },
    )
    .await
    .map_err(map_error)?;
    Ok((
        if started.created {
            StatusCode::CREATED
        } else {
            StatusCode::OK
        },
        Json(RunStartedDto {
            run_id: started.run_id,
            created: started.created,
        }),
    ))
}

#[utoipa::path(
    post,
    path = "/projects/{project_id}/automations/workflows/{workflow_id}/test-runs",
    operation_id = "startWorkflowTestRun",
    tag = "automations",
    params(("project_id" = Uuid, Path), ("workflow_id" = Uuid, Path)),
    request_body = TestRunRequest,
    responses(
        (status = 200, description = "The test run so far; poll the run until it is finished if it is still running", body = WorkflowRunDto),
        (status = 409, description = "The draft has problems; details.issues lists them per block", body = ErrorResponse)
    )
)]
/// Runs the current draft with sample information. Blocks that would change
/// something (project data, messages, web requests) report what they would
/// have done instead of doing it.
pub(crate) async fn start_test_run(
    State(state): State<AppState>,
    Path((project, workflow)): Path<(Uuid, Uuid)>,
    headers: HeaderMap,
    Json(input): Json<TestRunRequest>,
) -> Result<Json<WorkflowRunDto>, ApiError> {
    let project = project_id(project)?;
    let record = deepref_postgres::get_workflow(&state.pool, project, workflow)
        .await
        .map_err(map_error)?;
    let ctx = deepref_postgres::validation_context(&state.pool, workflow, &record.draft_graph)
        .await
        .map_err(map_error)?;
    let report = wf::validate_graph(&record.draft_graph, &ctx);
    if !report.is_ok() {
        return Err(map_error(WorkflowError::Invalid(report)));
    }
    let actor = extract_actor(&headers)?;
    let sample = input.trigger_data.unwrap_or_else(|| json!({}));
    let started = deepref_postgres::start_run(
        &state.pool,
        &StartRun {
            project_id: project.as_uuid(),
            workflow_id: workflow,
            version_id: None,
            graph: record.draft_graph,
            trigger_kind: "test".to_owned(),
            trigger_data: sample_trigger_data(sample),
            idempotency_key: format!("test:{}", Uuid::new_v4()),
            actor_kind: actor.kind().as_str().to_owned(),
            actor_id: actor.id().to_owned(),
            test_mode: true,
        },
    )
    .await
    .map_err(map_error)?;
    let deadline =
        Instant::now() + Duration::from_millis(input.wait_ms.unwrap_or(0).min(MAX_TEST_WAIT_MS));
    loop {
        let run = deepref_postgres::get_workflow_run(&state.pool, project, started.run_id)
            .await
            .map_err(map_error)?;
        if !matches!(run.status.as_str(), "queued" | "running") || Instant::now() >= deadline {
            let counts = deepref_postgres::review_counts_for_runs(&state.pool, &[run.id])
                .await
                .map_err(map_error)?;
            let found = counts.get(&run.id).copied();
            return Ok(Json(run_dto(run, found)?));
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
}

/// Sample data may be given as the full set of trigger outputs (an object
/// keyed by output name) or as a plain value; plain values go to `data`.
fn sample_trigger_data(sample: Value) -> Value {
    sample
}

// ---------------------------------------------------------------------------
// Endpoint credentials

#[utoipa::path(
    get,
    path = "/projects/{project_id}/automations/workflows/{workflow_id}/endpoints",
    operation_id = "getWorkflowEndpoints",
    tag = "automations",
    params(("project_id" = Uuid, Path), ("workflow_id" = Uuid, Path)),
    responses((status = 200, body = EndpointsDto), (status = 404, body = ErrorResponse))
)]
pub(crate) async fn get_endpoints(
    State(state): State<AppState>,
    Path((project, workflow)): Path<(Uuid, Uuid)>,
) -> Result<Json<EndpointsDto>, ApiError> {
    let config = deepref_postgres::get_webhook_config(&state.pool, project_id(project)?, workflow)
        .await
        .map_err(map_error)?;
    Ok(Json(endpoints_dto(config)))
}

#[utoipa::path(
    put,
    path = "/projects/{project_id}/automations/workflows/{workflow_id}/endpoints",
    operation_id = "updateWorkflowEndpoints",
    tag = "automations",
    params(("project_id" = Uuid, Path), ("workflow_id" = Uuid, Path)),
    request_body = EndpointSettingsRequest,
    responses((status = 200, body = EndpointsDto), (status = 404, body = ErrorResponse))
)]
pub(crate) async fn update_endpoints(
    State(state): State<AppState>,
    Path((project, workflow)): Path<(Uuid, Uuid)>,
    Json(input): Json<EndpointSettingsRequest>,
) -> Result<Json<EndpointsDto>, ApiError> {
    let config = deepref_postgres::set_webhook_signature_required(
        &state.pool,
        project_id(project)?,
        workflow,
        input.signature_required,
    )
    .await
    .map_err(map_error)?;
    Ok(Json(endpoints_dto(config)))
}

async fn rotate(
    state: &AppState,
    project: Uuid,
    workflow: Uuid,
    target: RotateTarget,
) -> Result<Json<EndpointsDto>, ApiError> {
    let config = deepref_postgres::rotate_endpoint_credentials(
        &state.pool,
        project_id(project)?,
        workflow,
        target,
    )
    .await
    .map_err(map_error)?;
    Ok(Json(endpoints_dto(config)))
}

#[utoipa::path(
    post,
    path = "/projects/{project_id}/automations/workflows/{workflow_id}/endpoints/webhook/rotate",
    operation_id = "rotateWorkflowWebhook",
    tag = "automations",
    params(("project_id" = Uuid, Path), ("workflow_id" = Uuid, Path)),
    responses((status = 200, description = "New webhook address and signing secret; the old ones stop working", body = EndpointsDto))
)]
pub(crate) async fn rotate_webhook(
    State(state): State<AppState>,
    Path((project, workflow)): Path<(Uuid, Uuid)>,
) -> Result<Json<EndpointsDto>, ApiError> {
    rotate(&state, project, workflow, RotateTarget::Webhook).await
}

#[utoipa::path(
    post,
    path = "/projects/{project_id}/automations/workflows/{workflow_id}/endpoints/webhook-secret/rotate",
    operation_id = "rotateWorkflowWebhookSecret",
    tag = "automations",
    params(("project_id" = Uuid, Path), ("workflow_id" = Uuid, Path)),
    responses((status = 200, description = "New signing secret; the address stays the same", body = EndpointsDto))
)]
pub(crate) async fn rotate_webhook_secret(
    State(state): State<AppState>,
    Path((project, workflow)): Path<(Uuid, Uuid)>,
) -> Result<Json<EndpointsDto>, ApiError> {
    rotate(&state, project, workflow, RotateTarget::WebhookSecret).await
}

#[utoipa::path(
    post,
    path = "/projects/{project_id}/automations/workflows/{workflow_id}/endpoints/email/rotate",
    operation_id = "rotateWorkflowEmailAddress",
    tag = "automations",
    params(("project_id" = Uuid, Path), ("workflow_id" = Uuid, Path)),
    responses((status = 200, description = "New inbound e-mail address token; the old one stops working", body = EndpointsDto))
)]
pub(crate) async fn rotate_email(
    State(state): State<AppState>,
    Path((project, workflow)): Path<(Uuid, Uuid)>,
) -> Result<Json<EndpointsDto>, ApiError> {
    rotate(&state, project, workflow, RotateTarget::Email).await
}

#[utoipa::path(
    get,
    path = "/projects/{project_id}/automations/workflow-files/{file_id}",
    operation_id = "downloadWorkflowFile",
    tag = "automations",
    params(("project_id" = Uuid, Path), ("file_id" = Uuid, Path)),
    responses((status = 200, description = "A file produced by an export block", content_type = "application/octet-stream", body = Vec<u8>), (status = 404, body = ErrorResponse))
)]
pub(crate) async fn download_workflow_file(
    State(state): State<AppState>,
    Path((project, file)): Path<(Uuid, Uuid)>,
) -> Result<Response, ApiError> {
    let file = deepref_postgres::get_workflow_file(&state.pool, project_id(project)?, file)
        .await
        .map_err(map_error)?;
    let safe_name: String = file
        .name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_') {
                c
            } else {
                '_'
            }
        })
        .collect();
    Ok((
        [
            (header::CONTENT_TYPE, file.content_type),
            (
                header::CONTENT_DISPOSITION,
                format!("attachment; filename=\"{safe_name}\""),
            ),
        ],
        Body::from(file.content),
    )
        .into_response())
}

// ---------------------------------------------------------------------------
// Public hooks

struct RateWindow {
    started: Instant,
    count: u32,
}

fn hook_allowed(token: &str) -> bool {
    static WINDOWS: OnceLock<Mutex<HashMap<String, RateWindow>>> = OnceLock::new();
    let windows = WINDOWS.get_or_init(|| Mutex::new(HashMap::new()));
    let Ok(mut windows) = windows.lock() else {
        return true;
    };
    let now = Instant::now();
    if windows.len() > 10_000 {
        windows.retain(|_, window| now.duration_since(window.started) < Duration::from_secs(60));
    }
    let window = windows.entry(token.to_owned()).or_insert(RateWindow {
        started: now,
        count: 0,
    });
    if now.duration_since(window.started) >= Duration::from_secs(60) {
        window.started = now;
        window.count = 0;
    }
    window.count += 1;
    window.count <= HOOK_RATE_LIMIT_PER_MINUTE
}

#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct HookAcceptedDto {
    pub run_id: Uuid,
    /// True when this delivery had already been received.
    pub duplicate: bool,
}

fn too_many_requests() -> Response {
    (
        StatusCode::TOO_MANY_REQUESTS,
        [(header::RETRY_AFTER, "60")],
        Json(
            json!({ "code": "RATE_LIMITED", "message": "Too many calls. Try again in a minute." }),
        ),
    )
        .into_response()
}

async fn start_hook_run(
    state: &AppState,
    target: &deepref_postgres::EndpointTarget,
    expected_trigger: &str,
    trigger_data: Value,
    key: String,
) -> Result<Response, ApiError> {
    if !target.enabled || target.trigger_kind.as_deref() != Some(expected_trigger) {
        return Err(ApiError::Conflict {
            code: "WORKFLOW_NOT_ACTIVE".to_owned(),
            message: "This automation is not switched on for this kind of trigger.".to_owned(),
            details: json!({}),
        });
    }
    let (version_id, graph) =
        deepref_postgres::get_published_graph(&state.pool, target.workflow_id)
            .await
            .map_err(map_error)?;
    let started = deepref_postgres::start_run(
        &state.pool,
        &StartRun {
            project_id: target.project_id,
            workflow_id: target.workflow_id,
            version_id: Some(version_id),
            graph,
            trigger_kind: expected_trigger.to_owned(),
            trigger_data,
            idempotency_key: key,
            actor_kind: "system".to_owned(),
            actor_id: format!("workflow-{expected_trigger}"),
            test_mode: false,
        },
    )
    .await
    .map_err(map_error)?;
    Ok((
        StatusCode::ACCEPTED,
        Json(HookAcceptedDto {
            run_id: started.run_id,
            duplicate: !started.created,
        }),
    )
        .into_response())
}

fn delivery_key(headers: &HeaderMap, prefix: &str, fallback: Option<String>) -> String {
    let supplied = ["idempotency-key", "x-delivery-id", "x-request-id"]
        .iter()
        .find_map(|name| headers.get(*name).and_then(|value| value.to_str().ok()))
        .map(str::trim)
        .filter(|value| !value.is_empty() && value.len() <= 200)
        .map(str::to_owned)
        .or(fallback);
    supplied.map_or_else(
        || format!("{prefix}:{}", Uuid::new_v4()),
        |value| format!("{prefix}:{value}"),
    )
}

#[utoipa::path(
    post,
    path = "/hooks/workflows/{token}",
    operation_id = "receiveWorkflowWebhook",
    tag = "hooks",
    params(
        ("token" = String, Path, description = "The workflow's private webhook token"),
        ("X-DeepRef-Signature" = Option<String>, Header, description = "sha256=<hex HMAC-SHA256 of the raw body, keyed with the signing secret>. Required unless the workflow accepts unsigned calls."),
        ("Idempotency-Key" = Option<String>, Header, description = "Optional delivery id; repeated deliveries start one run")
    ),
    request_body(content = serde_json::Value, content_type = "application/json"),
    responses(
        (status = 202, description = "Accepted", body = HookAcceptedDto),
        (status = 403, description = "Missing or wrong signature", body = ErrorResponse),
        (status = 404, description = "Unknown address", body = ErrorResponse),
        (status = 409, description = "Automation not switched on", body = ErrorResponse),
        (status = 413, description = "Body larger than 256 KB", body = ErrorResponse),
        (status = 429, description = "Too many calls (60 per minute per address)")
    )
)]
/// Public endpoint that starts a workflow with the request body as trigger data.
pub(crate) async fn receive_webhook(
    State(state): State<AppState>,
    Path(token): Path<String>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Result<Response, ApiError> {
    if !hook_allowed(&token) {
        return Ok(too_many_requests());
    }
    if body.len() > MAX_HOOK_BODY_BYTES {
        return Err(ApiError::PayloadTooLarge(
            "The body is larger than 256 KB.".to_owned(),
        ));
    }
    let target = deepref_postgres::find_workflow_by_webhook_token(&state.pool, &token)
        .await
        .map_err(map_error)?
        .ok_or_else(|| ApiError::NotFound("unknown address".to_owned()))?;
    if target.signature_required {
        let signature = headers
            .get(SIGNATURE_HEADER)
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default();
        if !wf::verify_signature(&target.secret, &body, signature) {
            return Err(ApiError::Forbidden(
                "The signature is missing or wrong.".to_owned(),
            ));
        }
    }
    let payload = serde_json::from_slice::<Value>(&body)
        .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&body).into_owned()));
    let data = json!({ "data": payload, "signed": target.signature_required });
    let key = delivery_key(&headers, "webhook", None);
    start_hook_run(&state, &target, "webhook", data, key).await
}

async fn email_from_request(request: Request) -> Result<(InboundEmail, Option<String>), ApiError> {
    let content_type = request
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_lowercase();
    fn bad<E>(_: E) -> ApiError {
        ApiError::BadRequest("The e-mail could not be read.".to_owned())
    }
    let adapt = |error: wf::InboundEmailError| ApiError::BadRequest(error.to_string());
    if content_type.starts_with("multipart/form-data") {
        let mut multipart = Multipart::from_request(request, &()).await.map_err(bad)?;
        let mut fields = serde_json::Map::new();
        while let Some(field) = multipart.next_field().await.map_err(bad)? {
            let Some(name) = field.name().map(str::to_owned) else {
                continue;
            };
            if field.file_name().is_some() {
                continue;
            }
            let text = field.text().await.map_err(bad)?;
            fields.insert(name, Value::String(text));
        }
        let fields = Value::Object(fields);
        let id = fields
            .get("Message-Id")
            .and_then(Value::as_str)
            .map(str::to_owned);
        return Ok((wf::from_mailgun(&fields).map_err(adapt)?, id));
    }
    let bytes = to_bytes(request.into_body(), MAX_HOOK_BODY_BYTES)
        .await
        .map_err(|_| ApiError::PayloadTooLarge("The body is larger than 256 KB.".to_owned()))?;
    if content_type.starts_with("application/x-www-form-urlencoded") {
        let map: HashMap<String, String> = serde_urlencoded::from_bytes(&bytes)
            .map_err(|_| ApiError::BadRequest("The e-mail could not be read.".to_owned()))?;
        let id = map.get("Message-Id").cloned();
        let fields = serde_json::to_value(map)?;
        return Ok((wf::from_mailgun(&fields).map_err(adapt)?, id));
    }
    let value: Value = serde_json::from_slice(&bytes).map_err(|_| {
        ApiError::BadRequest("The e-mail must be JSON, form fields or multipart.".to_owned())
    })?;
    if value.get("TextBody").is_some()
        || value.get("HtmlBody").is_some()
        || value.get("FromFull").is_some()
    {
        let id = value
            .get("MessageID")
            .and_then(Value::as_str)
            .map(str::to_owned);
        return Ok((wf::from_postmark(&value).map_err(adapt)?, id));
    }
    if value.get("body-plain").is_some() || value.get("body-html").is_some() {
        return Ok((wf::from_mailgun(&value).map_err(adapt)?, None));
    }
    let email: InboundEmail = serde_json::from_value(value)
        .map_err(|_| ApiError::BadRequest("The e-mail format is not recognised.".to_owned()))?;
    Ok((email.normalized().map_err(adapt)?, None))
}

#[utoipa::path(
    post,
    path = "/hooks/email/{token}",
    operation_id = "receiveInboundEmail",
    tag = "hooks",
    params(("token" = String, Path, description = "The workflow's private e-mail token")),
    request_body(content = serde_json::Value, content_type = "application/json", description = "Normalized {from, to, subject, text, html, attachments[]}, a Postmark inbound payload, or a Mailgun route (JSON, urlencoded or multipart)"),
    responses(
        (status = 202, description = "Accepted", body = HookAcceptedDto),
        (status = 400, description = "Unreadable message", body = ErrorResponse),
        (status = 404, description = "Unknown address", body = ErrorResponse),
        (status = 409, description = "Automation not switched on", body = ErrorResponse),
        (status = 413, description = "Body larger than 256 KB", body = ErrorResponse),
        (status = 429, description = "Too many calls (60 per minute per address)")
    )
)]
/// Public endpoint for inbound e-mail providers. DOIs and PubMed IDs found in
/// the message are passed to the workflow as trigger data.
pub(crate) async fn receive_inbound_email(
    State(state): State<AppState>,
    Path(token): Path<String>,
    request: Request,
) -> Result<Response, ApiError> {
    if !hook_allowed(&token) {
        return Ok(too_many_requests());
    }
    let target = deepref_postgres::find_workflow_by_email_token(&state.pool, &token)
        .await
        .map_err(map_error)?
        .ok_or_else(|| ApiError::NotFound("unknown address".to_owned()))?;
    let headers = request.headers().clone();
    let (email, message_id) = email_from_request(request).await?;
    let data = email.trigger_data();
    let trigger =
        json!({ "data": data.clone(), "text": data.get("text").cloned().unwrap_or(Value::Null) });
    let key = delivery_key(&headers, "email", message_id);
    start_hook_run(&state, &target, "email", trigger, key).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hook_rate_limit_applies_per_token() {
        let token = format!("test-{}", Uuid::new_v4());
        for _ in 0..HOOK_RATE_LIMIT_PER_MINUTE {
            assert!(hook_allowed(&token));
        }
        assert!(!hook_allowed(&token));
        assert!(hook_allowed("another-token"));
    }

    #[test]
    fn catalog_dto_covers_every_block() {
        assert_eq!(
            wf::catalog().len(),
            wf::catalog().iter().map(node_type_dto).count()
        );
        let kinds: Vec<String> = wf::catalog()
            .iter()
            .flat_map(|def| def.config.iter().map(|field| field_kind_name(field.kind)))
            .collect();
        assert!(kinds.iter().all(|kind| !kind.is_empty()));
    }

    #[test]
    fn delivery_keys_prefer_the_supplied_id() {
        let mut headers = HeaderMap::new();
        headers.insert("x-delivery-id", "abc".parse().expect("header"));
        assert_eq!(delivery_key(&headers, "webhook", None), "webhook:abc");
        assert_eq!(
            delivery_key(&HeaderMap::new(), "email", Some("m1".to_owned())),
            "email:m1"
        );
    }
}
