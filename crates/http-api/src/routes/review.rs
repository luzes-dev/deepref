use axum::{
    Json,
    extract::{Path, Query, State},
    http::HeaderMap,
};
use chrono::{DateTime, Utc};
use deepref_application::ScreenReportCommand;
use deepref_domain::{
    CurrentScreeningState, ProjectId, ProtocolVersionId, ReportId, ScreeningDecision,
    ScreeningStage, ScreeningTransition, ScreeningValidationError,
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sqlx::Row;
use utoipa::ToSchema;
use uuid::Uuid;

use crate::{
    error::{ApiError, ErrorResponse},
    jobs::recompute_prisma_dedupe_key,
    state::AppState,
};

const DEFAULT_CRITERIA: &str = r#"[
  {"id":"population","label":"Population","description":"Matches the review population."},
  {"id":"intervention","label":"Intervention or exposure","description":"Matches the intervention or exposure of interest."},
  {"id":"outcome","label":"Outcome","description":"Reports a relevant outcome."}
]"#;

const ACTOR_KIND_HEADER: &str = "x-actor-kind";
const ACTOR_ID_HEADER: &str = "x-actor-id";

#[derive(Debug, Clone)]
struct Actor {
    kind: String,
    id: String,
}

/// Extracts the caller-provided actor context for review audit events.
///
/// Authentication and actor verification are intentionally outside this API's
/// scope. Until that boundary exists, missing headers use the documented local
/// fallback `user/local-user`; callers can provide the same fields explicitly.
fn extract_actor(headers: &HeaderMap) -> Result<Actor, ApiError> {
    let kind = headers
        .get(ACTOR_KIND_HEADER)
        .map(|value| {
            value
                .to_str()
                .map(str::to_owned)
                .map_err(|_| ApiError::BadRequest("x-actor-kind must be valid ASCII".to_owned()))
        })
        .transpose()?
        .unwrap_or_else(|| "user".to_owned());
    if !matches!(kind.as_str(), "user" | "automation" | "system") {
        return Err(ApiError::BadRequest(
            "x-actor-kind must be user, automation, or system".to_owned(),
        ));
    }
    let id = headers
        .get(ACTOR_ID_HEADER)
        .map(|value| {
            value
                .to_str()
                .map(str::trim)
                .map(str::to_owned)
                .map_err(|_| ApiError::BadRequest("x-actor-id must be valid ASCII".to_owned()))
        })
        .transpose()?
        .unwrap_or_else(|| "local-user".to_owned());
    if id.is_empty() {
        return Err(ApiError::BadRequest(
            "x-actor-id must not be blank".to_owned(),
        ));
    }
    Ok(Actor { kind, id })
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub(crate) struct ProtocolDto {
    pub id: Uuid,
    pub version: i32,
    pub name: String,
    pub status: String,
    pub criteria: serde_json::Value,
    pub published_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub(crate) struct ScreeningQueueItemDto {
    pub report_id: Uuid,
    pub title: Option<String>,
    pub abstract_text: Option<String>,
    pub doi: Option<String>,
    pub publication_year: Option<i32>,
    pub title_abstract_status: String,
    pub full_text_status: String,
    pub final_status: String,
    pub revision: i64,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub(crate) struct ScreeningQueueDto {
    pub items: Vec<ScreeningQueueItemDto>,
    pub status: String,
    pub total: i64,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub(crate) struct ScreeningStateDto {
    pub project_id: Uuid,
    pub report_id: Uuid,
    pub title_abstract_status: String,
    pub full_text_status: String,
    pub final_status: String,
    pub revision: i64,
    pub last_event_id: Option<Uuid>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ScreeningStageInput {
    TitleAbstract,
    FullText,
}

impl ScreeningStageInput {
    fn domain(&self) -> ScreeningStage {
        match self {
            Self::TitleAbstract => ScreeningStage::TitleAbstract,
            Self::FullText => ScreeningStage::FullText,
        }
    }
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ScreeningDecisionInput {
    Include,
    Exclude,
    Maybe,
}

impl ScreeningDecisionInput {
    fn domain(&self) -> ScreeningDecision {
        match self {
            Self::Include => ScreeningDecision::Include,
            Self::Exclude => ScreeningDecision::Exclude,
            Self::Maybe => ScreeningDecision::Maybe,
        }
    }
}

#[derive(Debug, Deserialize, ToSchema)]
pub(crate) struct ScreenReportRequest {
    pub stage: ScreeningStageInput,
    pub decision: ScreeningDecisionInput,
    pub protocol_version_id: Uuid,
    pub expected_revision: i64,
    pub exclusion_reason_id: Option<Uuid>,
    pub notes: Option<String>,
}

#[derive(Debug, Deserialize, ToSchema)]
pub(crate) struct ScreeningQueueParams {
    pub status: Option<String>,
    pub limit: Option<i64>,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub(crate) struct PrismaDto {
    pub project_id: Uuid,
    pub records_identified: i64,
    pub records_deduplicated: i64,
    pub title_abstract_pending: i64,
    pub title_abstract_included: i64,
    pub title_abstract_excluded: i64,
    pub full_text_pending: i64,
    pub full_text_included: i64,
    pub full_text_excluded: i64,
    pub revision: i64,
    pub updated_at: Option<DateTime<Utc>>,
}

#[utoipa::path(
    get,
    path = "/projects/{project_id}/protocol",
    operation_id = "getProjectProtocol",
    tag = "review",
    params(("project_id" = Uuid, Path, description = "Project identifier")),
    responses(
        (status = 200, description = "Published protocol", body = ProtocolDto),
        (status = 404, description = "Project not found", body = ErrorResponse),
        (status = 500, description = "Internal server error", body = ErrorResponse)
    )
)]
pub(crate) async fn get_protocol(
    State(state): State<AppState>,
    Path(project_id): Path<Uuid>,
) -> Result<Json<ProtocolDto>, ApiError> {
    let row = ensure_protocol(&state, project_id).await?;
    Ok(Json(protocol_from_row(row)))
}

#[utoipa::path(
    get,
    path = "/projects/{project_id}/screening/title-abstract",
    operation_id = "listTitleAbstractScreeningQueue",
    tag = "review",
    params(
        ("project_id" = Uuid, Path, description = "Project identifier"),
        ("status" = Option<String>, Query, description = "Queue status"),
        ("limit" = Option<i64>, Query, description = "Maximum rows, 1 through 100")
    ),
    responses(
        (status = 200, description = "Title/abstract screening queue", body = ScreeningQueueDto),
        (status = 400, description = "Invalid queue parameters", body = ErrorResponse),
        (status = 500, description = "Internal server error", body = ErrorResponse)
    )
)]
pub(crate) async fn list_title_abstract_queue(
    State(state): State<AppState>,
    Path(project_id): Path<Uuid>,
    Query(params): Query<ScreeningQueueParams>,
) -> Result<Json<ScreeningQueueDto>, ApiError> {
    let status = params.status.unwrap_or_else(|| "unscreened".to_owned());
    if !matches!(
        status.as_str(),
        "unscreened" | "include" | "exclude" | "maybe" | "all"
    ) {
        return Err(ApiError::BadRequest(
            "status must be unscreened, include, exclude, maybe, or all".to_owned(),
        ));
    }
    let limit = params.limit.unwrap_or(50);
    if !(1..=100).contains(&limit) {
        return Err(ApiError::BadRequest(
            "limit must be between 1 and 100".to_owned(),
        ));
    }
    let rows = sqlx::query(
        r#"
        SELECT
          r.id AS report_id, r.title, r.abstract_text, r.publication_year,
          doi.value AS doi,
          coalesce(ss.title_abstract_status, 'unscreened') AS title_abstract_status,
          coalesce(ss.full_text_status, 'not_required') AS full_text_status,
          coalesce(ss.final_status, 'unscreened') AS final_status,
          coalesce(ss.revision, 0)::bigint AS revision
        FROM project_reports pr
        JOIN reports r ON r.id = pr.report_id
        LEFT JOIN LATERAL (
          SELECT value FROM report_identifiers
          WHERE report_id = r.id AND scheme = 'doi'
          ORDER BY created_at, id
          LIMIT 1
        ) doi ON true
        LEFT JOIN screening_state ss ON ss.project_id = pr.project_id AND ss.report_id = pr.report_id
        WHERE pr.project_id = $1
          AND ($2 = 'all' OR coalesce(ss.title_abstract_status, 'unscreened') = $2)
        ORDER BY r.created_at, r.id
        LIMIT $3
        "#,
    )
    .bind(project_id)
    .bind(&status)
    .bind(limit)
    .fetch_all(&state.pool)
    .await?;
    let total: i64 = sqlx::query_scalar(
        "SELECT count(*)::bigint FROM project_reports pr LEFT JOIN screening_state ss ON ss.project_id=pr.project_id AND ss.report_id=pr.report_id WHERE pr.project_id=$1 AND ($2='all' OR coalesce(ss.title_abstract_status,'unscreened')=$2)",
    )
    .bind(project_id)
    .bind(&status)
    .fetch_one(&state.pool)
    .await?;
    Ok(Json(ScreeningQueueDto {
        items: rows.into_iter().map(queue_item_from_row).collect(),
        status,
        total,
    }))
}

#[utoipa::path(
    post,
    path = "/projects/{project_id}/reports/{report_id}/screening",
    operation_id = "screenReport",
    tag = "review",
    params(
        ("project_id" = Uuid, Path, description = "Project identifier"),
        ("report_id" = Uuid, Path, description = "Report identifier")
    ),
    request_body = ScreenReportRequest,
    responses(
        (status = 200, description = "Current screening state", body = ScreeningStateDto),
        (status = 400, description = "Invalid decision", body = ErrorResponse),
        (status = 404, description = "Report or protocol not found", body = ErrorResponse),
        (status = 409, description = "Revision conflict", body = ErrorResponse),
        (status = 500, description = "Internal server error", body = ErrorResponse)
    )
)]
pub(crate) async fn screen_report(
    State(state): State<AppState>,
    Path((project_id, report_id)): Path<(Uuid, Uuid)>,
    headers: HeaderMap,
    Json(input): Json<ScreenReportRequest>,
) -> Result<Json<ScreeningStateDto>, ApiError> {
    let actor = extract_actor(&headers)?;
    let command = ScreenReportCommand {
        project_id: ProjectId::from(project_id),
        report_id: ReportId::from(report_id),
        stage: input.stage.domain(),
        decision: input.decision.domain(),
        exclusion_reason_id: input.exclusion_reason_id.map(Into::into),
        protocol_version_id: ProtocolVersionId::from(input.protocol_version_id),
        expected_revision: input.expected_revision,
    };
    let stage = match &input.stage {
        ScreeningStageInput::TitleAbstract => "title_abstract",
        ScreeningStageInput::FullText => "full_text",
    };

    let mut tx = state.pool.begin().await?;
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 0))")
        .bind(format!("screening:{project_id}:{report_id}"))
        .execute(&mut *tx)
        .await?;
    let protocol_exists: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM protocol_versions WHERE id=$1 AND project_id=$2 AND status='published')",
    )
    .bind(input.protocol_version_id)
    .bind(project_id)
    .fetch_one(&mut *tx)
    .await?;
    if !protocol_exists {
        return Err(ApiError::NotFound(
            "published protocol not found".to_owned(),
        ));
    }
    let report_exists: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM project_reports WHERE project_id=$1 AND report_id=$2)",
    )
    .bind(project_id)
    .bind(report_id)
    .fetch_one(&mut *tx)
    .await?;
    if !report_exists {
        return Err(ApiError::NotFound(
            "report is not part of this project".to_owned(),
        ));
    }
    if let Some(reason_id) = input.exclusion_reason_id {
        let reason_stage: Option<String> =
            sqlx::query_scalar("SELECT stage FROM exclusion_reasons WHERE id=$1 AND project_id=$2")
                .bind(reason_id)
                .bind(project_id)
                .fetch_optional(&mut *tx)
                .await?;
        let Some(reason_stage) = reason_stage else {
            return Err(ApiError::BadRequest(
                "exclusion_reason_id does not belong to this project".to_owned(),
            ));
        };
        if reason_stage != stage {
            return Err(ApiError::BadRequest(format!(
                "exclusion_reason_id is for {reason_stage} screening, not {stage} screening"
            )));
        }
    }
    let current = sqlx::query(
        "SELECT project_id, report_id, title_abstract_status, full_text_status, full_text_exclusion_reason_id, final_status, revision, last_event_id, updated_at FROM screening_state WHERE project_id=$1 AND report_id=$2 FOR UPDATE",
    )
    .bind(project_id)
    .bind(report_id)
    .fetch_optional(&mut *tx)
    .await?;
    let title_status = current
        .as_ref()
        .map(|row| row.get::<String, _>("title_abstract_status"))
        .unwrap_or_else(|| "unscreened".to_owned());
    let full_status = current
        .as_ref()
        .map(|row| row.get::<String, _>("full_text_status"))
        .unwrap_or_else(|| "not_required".to_owned());
    let final_status = current
        .as_ref()
        .map(|row| row.get::<String, _>("final_status"))
        .unwrap_or_else(|| "unscreened".to_owned());
    let old_reason = current
        .as_ref()
        .and_then(|row| row.get::<Option<Uuid>, _>("full_text_exclusion_reason_id"));
    let title_decision = screening_decision_from_status(&title_status)?;
    let full_decision = screening_decision_from_status(&full_status)?;
    validate_final_status(&final_status)?;
    let old_revision = current
        .as_ref()
        .map(|row| row.get::<i64, _>("revision"))
        .unwrap_or(0);
    if old_revision != input.expected_revision {
        let current_state = current
            .as_ref()
            .map(screening_state_json_from_row)
            .unwrap_or_else(|| {
                json!({
                    "project_id": project_id,
                    "report_id": report_id,
                    "title_abstract_status": "unscreened",
                    "full_text_status": "not_required",
                    "final_status": "unscreened",
                    "revision": 0
                })
            });
        return Err(ApiError::Conflict {
            code: "screening_revision_conflict".to_owned(),
            message: "screening state changed; refresh before saving".to_owned(),
            details: json!({ "currentRevision": old_revision, "currentState": current_state }),
        });
    }
    let current_state = CurrentScreeningState {
        title_abstract: title_decision,
        full_text: full_decision,
        full_text_exclusion_reason_id: old_reason.map(Into::into),
    };
    let next_state = match command
        .validate(current_state)
        .map_err(map_screening_validation_error)?
    {
        ScreeningTransition::Applied(next_state) => next_state,
        ScreeningTransition::Repeated => {
            return Err(ApiError::Conflict {
                code: "screening_decision_repeated".to_owned(),
                message: "screening decision is already current".to_owned(),
                details: json!({ "currentRevision": old_revision }),
            });
        }
    };
    let decision = match input.decision {
        ScreeningDecisionInput::Include => "include",
        ScreeningDecisionInput::Exclude => "exclude",
        ScreeningDecisionInput::Maybe => "maybe",
    };
    let next_title_status = next_state
        .title_abstract
        .map(screening_decision_status)
        .unwrap_or("unscreened");
    let next_full_status = next_state
        .full_text
        .map(screening_decision_status)
        .unwrap_or("not_required");
    let final_status = match (next_state.title_abstract, next_state.full_text) {
        (Some(ScreeningDecision::Include), Some(full_text)) => screening_decision_status(full_text),
        (Some(ScreeningDecision::Include), None) => "pending_full_text",
        (Some(ScreeningDecision::Exclude), _) => "exclude",
        (Some(ScreeningDecision::Maybe), _) => "maybe",
        (None, _) => "unscreened",
    };
    let full_text_reason: Option<Uuid> = next_state.full_text_exclusion_reason_id.map(Into::into);
    let event_id = Uuid::new_v4();
    let previous_event_id = current
        .as_ref()
        .and_then(|row| row.get::<Option<Uuid>, _>("last_event_id"));
    sqlx::query(
        "INSERT INTO screening_events (id,project_id,report_id,stage,decision,exclusion_reason_id,notes,protocol_version_id,actor_kind,actor_id,supersedes_event_id) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11)",
    )
    .bind(event_id)
    .bind(project_id)
    .bind(report_id)
    .bind(stage)
    .bind(decision)
    .bind(input.exclusion_reason_id)
    .bind(input.notes)
    .bind(input.protocol_version_id)
    .bind(&actor.kind)
    .bind(&actor.id)
    .bind(previous_event_id)
    .execute(&mut *tx)
    .await?;
    let revision = old_revision + 1;
    let row = sqlx::query(
        r#"
        INSERT INTO screening_state (
          project_id, report_id, title_abstract_status, full_text_status,
          full_text_exclusion_reason_id, final_status, revision, last_event_id, updated_at
        ) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,now())
        ON CONFLICT (project_id, report_id) DO UPDATE SET
          title_abstract_status=EXCLUDED.title_abstract_status,
          full_text_status=EXCLUDED.full_text_status,
          full_text_exclusion_reason_id=EXCLUDED.full_text_exclusion_reason_id,
          final_status=EXCLUDED.final_status,
          revision=EXCLUDED.revision,
          last_event_id=EXCLUDED.last_event_id,
          updated_at=now()
        RETURNING project_id, report_id, title_abstract_status, full_text_status, final_status, revision, last_event_id, updated_at
        "#,
    )
    .bind(project_id)
    .bind(report_id)
    .bind(next_title_status)
    .bind(next_full_status)
    .bind(full_text_reason)
    .bind(final_status)
    .bind(revision)
    .bind(event_id)
    .fetch_one(&mut *tx)
    .await?;
    let lifecycle = match final_status {
        "include" => "included",
        "exclude" => "excluded",
        "maybe" => "maybe",
        _ => "screening",
    };
    sqlx::query(
        "UPDATE project_reports SET lifecycle_status=$3 WHERE project_id=$1 AND report_id=$2",
    )
    .bind(project_id)
    .bind(report_id)
    .bind(lifecycle)
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        "INSERT INTO review_events (id,project_id,event_type,aggregate_type,aggregate_id,payload,actor_kind,actor_id) VALUES ($1,$2,'report_screened','report',$3,$4,$5,$6)",
    )
    .bind(Uuid::new_v4())
    .bind(project_id)
    .bind(report_id)
    .bind(json!({ "stage": stage, "decision": decision, "revision": revision, "protocol_version_id": input.protocol_version_id }))
    .bind(&actor.kind)
    .bind(&actor.id)
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        "INSERT INTO jobs (id,kind,payload,priority,max_attempts,dedupe_key) VALUES ($1,'recompute_prisma',$2,10,5,$3) ON CONFLICT (dedupe_key) DO NOTHING",
    )
    .bind(Uuid::new_v4())
    .bind(json!({ "project_id": project_id }))
    .bind(recompute_prisma_dedupe_key(project_id, event_id))
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(Json(screening_state_dto_from_row(&row)))
}

#[utoipa::path(
    get,
    path = "/projects/{project_id}/prisma",
    operation_id = "getProjectPrisma",
    tag = "review",
    params(("project_id" = Uuid, Path, description = "Project identifier")),
    responses(
        (status = 200, description = "PRISMA projection counts", body = PrismaDto),
        (status = 404, description = "Project not found", body = ErrorResponse),
        (status = 500, description = "Internal server error", body = ErrorResponse)
    )
)]
pub(crate) async fn get_prisma(
    State(state): State<AppState>,
    Path(project_id): Path<Uuid>,
) -> Result<Json<PrismaDto>, ApiError> {
    let row = sqlx::query(
        "SELECT records_identified,records_deduplicated,title_abstract_pending,title_abstract_included,title_abstract_excluded,full_text_pending,full_text_included,full_text_excluded,revision,updated_at FROM prisma_snapshots WHERE project_id=$1",
    )
    .bind(project_id)
    .fetch_optional(&state.pool)
    .await?;
    if row.is_none() {
        let project_exists: bool =
            sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM projects WHERE id=$1)")
                .bind(project_id)
                .fetch_one(&state.pool)
                .await?;
        if !project_exists {
            return Err(ApiError::NotFound("project not found".to_owned()));
        }
    }
    Ok(Json(PrismaDto {
        project_id,
        records_identified: row.as_ref().map_or(0, |r| r.get("records_identified")),
        records_deduplicated: row.as_ref().map_or(0, |r| r.get("records_deduplicated")),
        title_abstract_pending: row.as_ref().map_or(0, |r| r.get("title_abstract_pending")),
        title_abstract_included: row.as_ref().map_or(0, |r| r.get("title_abstract_included")),
        title_abstract_excluded: row.as_ref().map_or(0, |r| r.get("title_abstract_excluded")),
        full_text_pending: row.as_ref().map_or(0, |r| r.get("full_text_pending")),
        full_text_included: row.as_ref().map_or(0, |r| r.get("full_text_included")),
        full_text_excluded: row.as_ref().map_or(0, |r| r.get("full_text_excluded")),
        revision: row.as_ref().map_or(0, |r| r.get("revision")),
        updated_at: row.as_ref().and_then(|r| r.get("updated_at")),
    }))
}

async fn ensure_protocol(
    state: &AppState,
    project_id: Uuid,
) -> Result<sqlx::postgres::PgRow, ApiError> {
    let mut tx = state.pool.begin().await?;
    let project_exists: bool =
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM projects WHERE id=$1)")
            .bind(project_id)
            .fetch_one(&mut *tx)
            .await?;
    if !project_exists {
        return Err(ApiError::NotFound("project not found".to_owned()));
    }
    sqlx::query(
        "INSERT INTO protocol_versions (id,project_id,version,name,status,criteria,published_at) VALUES ($1,$2,1,'Default evidence screening protocol','published',$3,now()) ON CONFLICT (project_id,version) DO NOTHING",
    )
    .bind(Uuid::new_v4())
    .bind(project_id)
    .bind(serde_json::from_str::<serde_json::Value>(DEFAULT_CRITERIA).expect("default criteria is valid JSON"))
    .execute(&mut *tx)
    .await?;
    let row = sqlx::query(
        "SELECT id,version,name,status,criteria,published_at FROM protocol_versions WHERE project_id=$1 ORDER BY version DESC LIMIT 1",
    )
    .bind(project_id)
    .fetch_one(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(row)
}

fn protocol_from_row(row: sqlx::postgres::PgRow) -> ProtocolDto {
    ProtocolDto {
        id: row.get("id"),
        version: row.get("version"),
        name: row.get("name"),
        status: row.get("status"),
        criteria: row.get("criteria"),
        published_at: row.get("published_at"),
    }
}

fn queue_item_from_row(row: sqlx::postgres::PgRow) -> ScreeningQueueItemDto {
    ScreeningQueueItemDto {
        report_id: row.get("report_id"),
        title: row.get("title"),
        abstract_text: row.get("abstract_text"),
        doi: row.get("doi"),
        publication_year: row.get("publication_year"),
        title_abstract_status: row.get("title_abstract_status"),
        full_text_status: row.get("full_text_status"),
        final_status: row.get("final_status"),
        revision: row.get("revision"),
    }
}

fn screening_state_dto_from_row(row: &sqlx::postgres::PgRow) -> ScreeningStateDto {
    ScreeningStateDto {
        project_id: row.get("project_id"),
        report_id: row.get("report_id"),
        title_abstract_status: row.get("title_abstract_status"),
        full_text_status: row.get("full_text_status"),
        final_status: row.get("final_status"),
        revision: row.get("revision"),
        last_event_id: row.get("last_event_id"),
        updated_at: row.get("updated_at"),
    }
}

fn screening_state_json_from_row(row: &sqlx::postgres::PgRow) -> serde_json::Value {
    json!(screening_state_dto_from_row(row))
}

fn map_screening_validation_error(error: ScreeningValidationError) -> ApiError {
    ApiError::BadRequest(error.to_string())
}

fn screening_decision_from_status(status: &str) -> Result<Option<ScreeningDecision>, ApiError> {
    match status {
        "include" => Ok(Some(ScreeningDecision::Include)),
        "exclude" => Ok(Some(ScreeningDecision::Exclude)),
        "maybe" => Ok(Some(ScreeningDecision::Maybe)),
        "unscreened" | "not_required" => Ok(None),
        other => Err(ApiError::DataIntegrity(format!(
            "unknown screening status {other:?}"
        ))),
    }
}

fn validate_final_status(status: &str) -> Result<(), ApiError> {
    if matches!(
        status,
        "unscreened" | "pending_full_text" | "include" | "exclude" | "maybe"
    ) {
        Ok(())
    } else {
        Err(ApiError::DataIntegrity(format!(
            "unknown screening final status {status:?}"
        )))
    }
}

fn screening_decision_status(decision: ScreeningDecision) -> &'static str {
    match decision {
        ScreeningDecision::Include => "include",
        ScreeningDecision::Exclude => "exclude",
        ScreeningDecision::Maybe => "maybe",
    }
}
