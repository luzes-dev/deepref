use super::actor::extract_actor;
use crate::{
    error::{ApiError, ErrorResponse},
    state::AppState,
};
use axum::{
    Json,
    extract::{Path, State},
    http::HeaderMap,
};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use uuid::Uuid;

#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct AuditResultDto {
    observed_relevant: u32,
    first_unsafe_total: u32,
    p_value: f64,
    passed: bool,
    reference_retention: Option<f64>,
}
#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct AiFirstForecastDto {
    human_records_remaining: u64,
    minimum_sample: u32,
    minimum_controls: u32,
    human_judgments_if_passed: u64,
    human_judgments_if_failed: u64,
    savings_vs_single: i64,
    recommended: bool,
}
#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct AiFirstCohortDto {
    id: Uuid,
    status: String,
    target_percent: i32,
    members: i64,
    evaluated: i64,
    quarantined: i64,
    sampled: i64,
    controls: i64,
    labels: i64,
    reference_relevant: Option<i32>,
    alpha_billionths: Option<i32>,
    result: Option<AuditResultDto>,
    forecast: Option<AiFirstForecastDto>,
    invalidation_reason: Option<String>,
}
#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct AiFirstOverviewDto {
    ceiling: String,
    suspended_reason: Option<String>,
    cohorts: Vec<AiFirstCohortDto>,
}
#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct StartAiFirstRequest {
    target_percent: u32,
    allow_finalization: bool,
}
#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct DrawAiFirstRequest {
    sample_size: Option<u32>,
}
#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct FinalizeAiFirstRequest {
    acknowledge_reference_limits: bool,
}
#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct AiFirstActionDto {
    cohort_id: Uuid,
    count: u32,
}

fn map_error(error: deepref_postgres::AiFirstError) -> ApiError {
    match error {
        deepref_postgres::AiFirstError::NotFound => {
            ApiError::NotFound("AI-first resource not found".into())
        }
        deepref_postgres::AiFirstError::Invalid(message) => ApiError::BadRequest(message),
        deepref_postgres::AiFirstError::Refused(message) => ApiError::Conflict {
            code: "ai_first_refused".into(),
            message,
            details: serde_json::Value::Null,
        },
        deepref_postgres::AiFirstError::Database(error) => ApiError::Database(error),
        other => ApiError::Internal(anyhow::anyhow!(other)),
    }
}

#[utoipa::path(get,path="/projects/{project_id}/ai/first",operation_id="getAiFirstOverview",tag="ai",
 params(("project_id"=Uuid,Path)),responses((status=200,body=AiFirstOverviewDto),(status=500,body=ErrorResponse)))]
pub(crate) async fn get_overview(
    State(state): State<AppState>,
    Path(project): Path<Uuid>,
) -> Result<Json<AiFirstOverviewDto>, ApiError> {
    let overview = deepref_postgres::get_ai_first_overview(&state.pool, project)
        .await
        .map_err(map_error)?;
    let cohorts = overview
        .cohorts
        .into_iter()
        .map(|c| {
            let result = c
                .result
                .map(serde_json::from_value::<deepref_application::ai_first::AuditResult>)
                .transpose()
                .map_err(|e| ApiError::Internal(e.into()))?
                .map(|r| AuditResultDto {
                    observed_relevant: r.observed_relevant,
                    first_unsafe_total: r.first_unsafe_total,
                    p_value: r.p_value,
                    passed: r.passed,
                    reference_retention: r.reference_retention,
                });
            Ok(AiFirstCohortDto {
                id: c.id,
                status: c.status,
                target_percent: c.target_percent,
                members: c.members,
                evaluated: c.evaluated,
                quarantined: c.quarantined,
                sampled: c.sampled,
                controls: c.controls,
                labels: c.labels,
                reference_relevant: c.reference_relevant,
                alpha_billionths: c.alpha_billionths,
                forecast: c.forecast.map(|f| AiFirstForecastDto {
                    human_records_remaining: f.human_records_remaining,
                    minimum_sample: f.minimum_sample,
                    minimum_controls: f.minimum_controls,
                    human_judgments_if_passed: f.human_judgments_if_passed,
                    human_judgments_if_failed: f.human_judgments_if_failed,
                    savings_vs_single: f.savings_vs_single,
                    recommended: f.recommended,
                }),
                result,
                invalidation_reason: c.invalidation_reason,
            })
        })
        .collect::<Result<Vec<_>, ApiError>>()?;
    Ok(Json(AiFirstOverviewDto {
        ceiling: overview.ceiling,
        suspended_reason: overview.suspended_reason,
        cohorts,
    }))
}

#[utoipa::path(post,path="/projects/{project_id}/ai/first",operation_id="startAiFirstCohort",tag="ai",
 params(("project_id"=Uuid,Path)),request_body=StartAiFirstRequest,
 responses((status=200,body=AiFirstActionDto),(status=400,body=ErrorResponse),(status=409,body=ErrorResponse)))]
pub(crate) async fn start(
    State(state): State<AppState>,
    Path(project): Path<Uuid>,
    headers: HeaderMap,
    Json(body): Json<StartAiFirstRequest>,
) -> Result<Json<AiFirstActionDto>, ApiError> {
    let actor = extract_actor(&headers)?;
    let cohort = deepref_postgres::start_ai_first_cohort(
        &state.pool,
        project,
        body.target_percent,
        body.allow_finalization,
        &actor,
    )
    .await
    .map_err(map_error)?;
    Ok(Json(AiFirstActionDto {
        cohort_id: cohort,
        count: 0,
    }))
}

#[utoipa::path(post,path="/projects/{project_id}/ai/first/{cohort_id}/close",operation_id="closeAiFirstCohort",tag="ai",
 params(("project_id"=Uuid,Path),("cohort_id"=Uuid,Path)),responses((status=200,body=AiFirstActionDto),(status=409,body=ErrorResponse)))]
pub(crate) async fn close(
    State(state): State<AppState>,
    Path((project, cohort)): Path<(Uuid, Uuid)>,
    headers: HeaderMap,
) -> Result<Json<AiFirstActionDto>, ApiError> {
    deepref_postgres::close_ai_first_cohort(
        &state.pool,
        project,
        cohort,
        &extract_actor(&headers)?,
    )
    .await
    .map_err(map_error)?;
    Ok(Json(AiFirstActionDto {
        cohort_id: cohort,
        count: 0,
    }))
}
#[utoipa::path(post,path="/projects/{project_id}/ai/first/{cohort_id}/draw",operation_id="drawAiFirstAudit",tag="ai",
 params(("project_id"=Uuid,Path),("cohort_id"=Uuid,Path)),request_body=DrawAiFirstRequest,
 responses((status=200,body=AiFirstActionDto),(status=400,body=ErrorResponse),(status=409,body=ErrorResponse)))]
pub(crate) async fn draw(
    State(state): State<AppState>,
    Path((project, cohort)): Path<(Uuid, Uuid)>,
    headers: HeaderMap,
    Json(body): Json<DrawAiFirstRequest>,
) -> Result<Json<AiFirstActionDto>, ApiError> {
    let count = deepref_postgres::draw_ai_first_audit(
        &state.pool,
        project,
        cohort,
        body.sample_size,
        &extract_actor(&headers)?,
    )
    .await
    .map_err(map_error)?;
    Ok(Json(AiFirstActionDto {
        cohort_id: cohort,
        count,
    }))
}
#[utoipa::path(post,path="/projects/{project_id}/ai/first/{cohort_id}/evaluate",operation_id="evaluateAiFirstAudit",tag="ai",
 params(("project_id"=Uuid,Path),("cohort_id"=Uuid,Path)),responses((status=200,body=AiFirstActionDto),(status=409,body=ErrorResponse)))]
pub(crate) async fn evaluate(
    State(state): State<AppState>,
    Path((project, cohort)): Path<(Uuid, Uuid)>,
    headers: HeaderMap,
) -> Result<Json<AiFirstActionDto>, ApiError> {
    let result = deepref_postgres::evaluate_ai_first_audit(
        &state.pool,
        project,
        cohort,
        &extract_actor(&headers)?,
    )
    .await
    .map_err(map_error)?;
    Ok(Json(AiFirstActionDto {
        cohort_id: cohort,
        count: result.observed_relevant,
    }))
}
#[utoipa::path(post,path="/projects/{project_id}/ai/first/{cohort_id}/finalize",operation_id="finalizeAiFirstCohort",tag="ai",
 params(("project_id"=Uuid,Path),("cohort_id"=Uuid,Path)),request_body=FinalizeAiFirstRequest,
 responses((status=200,body=AiFirstActionDto),(status=400,body=ErrorResponse),(status=409,body=ErrorResponse)))]
pub(crate) async fn finalize(
    State(state): State<AppState>,
    Path((project, cohort)): Path<(Uuid, Uuid)>,
    headers: HeaderMap,
    Json(body): Json<FinalizeAiFirstRequest>,
) -> Result<Json<AiFirstActionDto>, ApiError> {
    let count = deepref_postgres::finalize_ai_first_cohort(
        &state.pool,
        project,
        cohort,
        body.acknowledge_reference_limits,
        &extract_actor(&headers)?,
    )
    .await
    .map_err(map_error)?;
    Ok(Json(AiFirstActionDto {
        cohort_id: cohort,
        count,
    }))
}
#[utoipa::path(post,path="/projects/{project_id}/ai/first/{cohort_id}/recover",operation_id="recoverAiFirstCohort",tag="ai",
 params(("project_id"=Uuid,Path),("cohort_id"=Uuid,Path)),responses((status=200,body=AiFirstActionDto),(status=409,body=ErrorResponse)))]
pub(crate) async fn recover(
    State(state): State<AppState>,
    Path((project, cohort)): Path<(Uuid, Uuid)>,
    headers: HeaderMap,
) -> Result<Json<AiFirstActionDto>, ApiError> {
    deepref_postgres::recover_ai_first_cohort(
        &state.pool,
        project,
        cohort,
        &extract_actor(&headers)?,
    )
    .await
    .map_err(map_error)?;
    Ok(Json(AiFirstActionDto {
        cohort_id: cohort,
        count: 0,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn requests_reject_unsupported_stage_and_authority_fields() {
        assert!(
            serde_json::from_value::<StartAiFirstRequest>(serde_json::json!({
                "target_percent":95,"allow_finalization":false,"stage":"full_text"
            }))
            .is_err()
        );
        assert!(
            serde_json::from_value::<StartAiFirstRequest>(serde_json::json!({
                "target_percent":95,"allow_finalization":false,"authority":"human_replacement"
            }))
            .is_err()
        );
        assert!(
            serde_json::from_value::<DrawAiFirstRequest>(serde_json::json!({
                "sample_size":30,"seed":"chosen-after-labels"
            }))
            .is_err()
        );
        assert!(
            serde_json::from_value::<FinalizeAiFirstRequest>(serde_json::json!({
                "acknowledge_reference_limits":true,"auto_include":true
            }))
            .is_err()
        );
    }
}
