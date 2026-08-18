use axum::{
    Json,
    extract::{Path, State},
};
use chrono::{DateTime, Utc};
use deepref_core::{
    Actor, ExclusionReasonId, ProtocolVersionId, ReportId, ScreeningCommand, ScreeningDecision,
    ScreeningStage, ScreeningState,
};
use serde::{Deserialize, Serialize};
use sqlx::Row;
use utoipa::ToSchema;
use uuid::Uuid;

use crate::{
    error::{ApiError, ErrorResponse},
    state::AppState,
};

#[derive(Debug, Deserialize, ToSchema)]
pub(crate) struct ScreenReportRequest {
    stage: String,
    decision: String,
    exclusion_reason_id: Option<Uuid>,
    protocol_version_id: Uuid,
    expected_revision: i64,
    reviewer: String,
    notes: Option<String>,
}

#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct ScreeningStateDto {
    project_id: Uuid,
    report_id: Uuid,
    title_abstract_status: String,
    full_text_status: String,
    final_status: String,
    revision: i64,
    last_event_id: Uuid,
    updated_at: DateTime<Utc>,
}

#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct ScreeningEventDto {
    id: Uuid,
    stage: String,
    decision: String,
    exclusion_reason_id: Option<Uuid>,
    notes: Option<String>,
    protocol_version_id: Uuid,
    actor_kind: String,
    actor_id: Option<String>,
    supersedes_event_id: Option<Uuid>,
    created_at: DateTime<Utc>,
}

fn parse_stage(value: &str) -> Result<ScreeningStage, ApiError> {
    match value {
        "title_abstract" => Ok(ScreeningStage::TitleAbstract),
        "full_text" => Ok(ScreeningStage::FullText),
        _ => Err(ApiError::BadRequest(format!("unknown screening stage: {value}"))),
    }
}

fn parse_decision(value: &str) -> Result<ScreeningDecision, ApiError> {
    match value {
        "include" => Ok(ScreeningDecision::Include),
        "exclude" => Ok(ScreeningDecision::Exclude),
        "maybe" => Ok(ScreeningDecision::Maybe),
        _ => Err(ApiError::BadRequest(format!("unknown screening decision: {value}"))),
    }
}

fn state_value(stage: ScreeningStage, decision: ScreeningDecision) -> &'static str {
    match (stage, decision) {
        (ScreeningStage::TitleAbstract, ScreeningDecision::Include) => "awaiting_full_text",
        (_, ScreeningDecision::Include) => "included",
        (_, ScreeningDecision::Exclude) => "excluded",
        (_, ScreeningDecision::Maybe) => "maybe",
    }
}

#[utoipa::path(
    post,
    path = "/projects/{project_id}/reports/{report_id}/screening",
    operation_id = "screenReport",
    tag = "screening",
    params(
        ("project_id" = Uuid, Path, description = "Project identifier"),
        ("report_id" = Uuid, Path, description = "Report identifier")
    ),
    request_body = ScreenReportRequest,
    responses(
        (status = 200, description = "Current screening state", body = ScreeningStateDto),
        (status = 400, description = "Invalid screening decision", body = ErrorResponse),
        (status = 409, description = "Screening revision conflict", body = ErrorResponse),
        (status = 500, description = "Internal server error", body = ErrorResponse)
    )
)]
pub(crate) async fn screen_report(
    State(state): State<AppState>,
    Path((project_id, report_id)): Path<(Uuid, Uuid)>,
    Json(input): Json<ScreenReportRequest>,
) -> Result<Json<ScreeningStateDto>, ApiError> {
    let stage = parse_stage(&input.stage)?;
    let decision = parse_decision(&input.decision)?;
    let command = ScreeningCommand {
        project_id,
        report_id: ReportId::from(report_id),
        stage,
        decision,
        exclusion_reason_id: input.exclusion_reason_id.map(ExclusionReasonId::from),
        protocol_version_id: ProtocolVersionId::from(input.protocol_version_id),
        actor: Actor::User(input.reviewer.trim().to_owned()),
        expected_revision: input.expected_revision,
        notes: input.notes.clone(),
    };
    command
        .validate()
        .map_err(|error| ApiError::BadRequest(error.to_string()))?;
    if input.reviewer.trim().is_empty() {
        return Err(ApiError::BadRequest("reviewer must not be blank".to_owned()));
    }

    let mut transaction = state.pool.begin().await?;
    let membership = sqlx::query(
        "SELECT 1 FROM project_reports WHERE project_id = $1 AND report_id = $2",
    )
    .bind(project_id)
    .bind(report_id)
    .fetch_optional(&mut *transaction)
    .await?;
    if membership.is_none() {
        return Err(ApiError::BadRequest(
            "report is not part of this project".to_owned(),
        ));
    }

    let current = sqlx::query(
        r#"
        SELECT revision, last_event_id
        FROM screening_state
        WHERE project_id = $1 AND report_id = $2
        FOR UPDATE
        "#,
    )
    .bind(project_id)
    .bind(report_id)
    .fetch_optional(&mut *transaction)
    .await?;

    let (current_revision, supersedes_event_id) = current
        .as_ref()
        .map(|row| (row.get::<i64, _>("revision"), row.get::<Option<Uuid>, _>("last_event_id")))
        .unwrap_or((0, None));
    if current_revision != input.expected_revision {
        return Err(ApiError::Conflict(format!(
            "screening_revision_conflict: expected {}, current {}",
            input.expected_revision, current_revision
        )));
    }

    let event_id = Uuid::new_v4();
    sqlx::query(
        r#"
        INSERT INTO screening_events (
          id, project_id, report_id, stage, decision, exclusion_reason_id, notes,
          protocol_version_id, actor_kind, actor_id, supersedes_event_id
        )
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8, 'user', $9, $10)
        "#,
    )
    .bind(event_id)
    .bind(project_id)
    .bind(report_id)
    .bind(&input.stage)
    .bind(&input.decision)
    .bind(input.exclusion_reason_id)
    .bind(&input.notes)
    .bind(input.protocol_version_id)
    .bind(input.reviewer.trim())
    .bind(supersedes_event_id)
    .execute(&mut *transaction)
    .await?;

    let status = state_value(stage, decision);
    let next_revision = current_revision + 1;
    let row = match stage {
        ScreeningStage::TitleAbstract => {
            sqlx::query(
                r#"
                INSERT INTO screening_state (
                  project_id, report_id, title_abstract_status, full_text_status,
                  final_status, revision, last_event_id
                )
                VALUES ($1, $2, $3, 'unscreened', 'unscreened', $4, $5)
                ON CONFLICT (project_id, report_id) DO UPDATE SET
                  title_abstract_status = EXCLUDED.title_abstract_status,
                  revision = EXCLUDED.revision,
                  last_event_id = EXCLUDED.last_event_id,
                  updated_at = now()
                RETURNING project_id, report_id, title_abstract_status, full_text_status,
                          final_status, revision, last_event_id, updated_at
                "#,
            )
            .bind(project_id)
            .bind(report_id)
            .bind(status)
            .bind(next_revision)
            .bind(event_id)
            .fetch_one(&mut *transaction)
            .await?
        }
        ScreeningStage::FullText => {
            sqlx::query(
                r#"
                INSERT INTO screening_state (
                  project_id, report_id, title_abstract_status, full_text_status,
                  final_status, revision, last_event_id
                )
                VALUES ($1, $2, 'awaiting_full_text', $3, $3, $4, $5)
                ON CONFLICT (project_id, report_id) DO UPDATE SET
                  full_text_status = EXCLUDED.full_text_status,
                  final_status = EXCLUDED.final_status,
                  revision = EXCLUDED.revision,
                  last_event_id = EXCLUDED.last_event_id,
                  updated_at = now()
                RETURNING project_id, report_id, title_abstract_status, full_text_status,
                          final_status, revision, last_event_id, updated_at
                "#,
            )
            .bind(project_id)
            .bind(report_id)
            .bind(status)
            .bind(next_revision)
            .bind(event_id)
            .fetch_one(&mut *transaction)
            .await?
        }
    };

    sqlx::query(
        r#"
        INSERT INTO review_events (id, project_id, event_type, entity_type, entity_id, payload)
        VALUES ($1, $2, 'report_screened', 'report', $3, $4)
        "#,
    )
    .bind(Uuid::new_v4())
    .bind(project_id)
    .bind(report_id)
    .bind(serde_json::json!({
        "screening_event_id": event_id,
        "stage": input.stage,
        "decision": input.decision,
        "protocol_version_id": input.protocol_version_id,
        "revision": next_revision,
    }))
    .execute(&mut *transaction)
    .await?;

    transaction.commit().await?;
    Ok(Json(screening_state_from_row(row)))
}

#[utoipa::path(
    get,
    path = "/projects/{project_id}/reports/{report_id}/screening/history",
    operation_id = "getScreeningHistory",
    tag = "screening",
    params(
        ("project_id" = Uuid, Path, description = "Project identifier"),
        ("report_id" = Uuid, Path, description = "Report identifier")
    ),
    responses(
        (status = 200, description = "Append-only screening history", body = [ScreeningEventDto]),
        (status = 500, description = "Internal server error", body = ErrorResponse)
    )
)]
pub(crate) async fn screening_history(
    State(state): State<AppState>,
    Path((project_id, report_id)): Path<(Uuid, Uuid)>,
) -> Result<Json<Vec<ScreeningEventDto>>, ApiError> {
    let rows = sqlx::query(
        r#"
        SELECT id, stage, decision, exclusion_reason_id, notes, protocol_version_id,
               actor_kind, actor_id, supersedes_event_id, created_at
        FROM screening_events
        WHERE project_id = $1 AND report_id = $2
        ORDER BY created_at ASC, id ASC
        "#,
    )
    .bind(project_id)
    .bind(report_id)
    .fetch_all(&state.pool)
    .await?;

    Ok(Json(
        rows.into_iter()
            .map(|row| ScreeningEventDto {
                id: row.get("id"),
                stage: row.get("stage"),
                decision: row.get("decision"),
                exclusion_reason_id: row.get("exclusion_reason_id"),
                notes: row.get("notes"),
                protocol_version_id: row.get("protocol_version_id"),
                actor_kind: row.get("actor_kind"),
                actor_id: row.get("actor_id"),
                supersedes_event_id: row.get("supersedes_event_id"),
                created_at: row.get("created_at"),
            })
            .collect(),
    ))
}

fn screening_state_from_row(row: sqlx::postgres::PgRow) -> ScreeningStateDto {
    let domain_state = ScreeningState::initial(
        row.get("project_id"),
        ReportId::from(row.get::<Uuid, _>("report_id")),
        row.get("updated_at"),
    );
    ScreeningStateDto {
        project_id: domain_state.project_id,
        report_id: Uuid::from(domain_state.report_id),
        title_abstract_status: row.get("title_abstract_status"),
        full_text_status: row.get("full_text_status"),
        final_status: row.get("final_status"),
        revision: row.get("revision"),
        last_event_id: row.get("last_event_id"),
        updated_at: domain_state.updated_at,
    }
}
