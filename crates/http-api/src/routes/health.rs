use axum::{Json, extract::State, http::StatusCode};
use chrono::{DateTime, Utc};
use serde::Serialize;
use sqlx::Row;
use utoipa::ToSchema;

use crate::{
    error::{ApiError, ApiErrorBody},
    state::AppState,
};

#[derive(Debug, Serialize, ToSchema)]
pub struct LivenessResponse {
    pub status: &'static str,
    pub uptime_seconds: u64,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct ReadinessResponse {
    pub status: &'static str,
    pub schema_version: i64,
    pub required_schema_version: i64,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum DependencyState {
    Available,
    Degraded,
    Unavailable,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct DependencyDetail {
    pub state: DependencyState,
    pub lag: Option<i64>,
    pub backlog: Option<i64>,
    pub oldest_age_seconds: Option<i64>,
    pub last_success_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct DependencyStatus {
    pub postgresql: DependencyDetail,
    pub worker: DependencyDetail,
}

fn available() -> DependencyDetail {
    DependencyDetail {
        state: DependencyState::Available,
        lag: None,
        backlog: None,
        oldest_age_seconds: None,
        last_success_at: None,
    }
}

#[utoipa::path(get, path="/health/live", operation_id="getLiveness", tag="health",
    responses((status=200, body=LivenessResponse)))]
pub async fn live(State(state): State<AppState>) -> Json<LivenessResponse> {
    Json(LivenessResponse {
        status: "ok",
        uptime_seconds: state.started_at.elapsed().as_secs(),
    })
}

#[utoipa::path(get, path="/health/ready", operation_id="getReadiness", tag="health",
    responses((status=200, body=ReadinessResponse), (status=503, body=ApiErrorBody)))]
pub async fn ready(State(state): State<AppState>) -> Result<Json<ReadinessResponse>, ApiError> {
    let schema_version: i64 = sqlx::query_scalar(
        "SELECT COALESCE(max(version),0)::bigint FROM _sqlx_migrations WHERE success",
    )
    .fetch_one(&state.pool)
    .await
    .map_err(ApiError::Database)?;
    if schema_version < 8 {
        return Err(ApiError::Configuration(format!(
            "database schema {schema_version} is older than required version 8"
        )));
    }
    Ok(Json(ReadinessResponse {
        status: "ready",
        schema_version,
        required_schema_version: 8,
    }))
}

#[utoipa::path(get, path="/health/dependencies", operation_id="getDependencyStatus", tag="health",
    responses((status=200, body=DependencyStatus)))]
pub async fn dependencies(State(state): State<AppState>) -> Json<DependencyStatus> {
    let postgresql = match sqlx::query("SELECT 1").execute(&state.pool).await {
        Ok(_) => available(),
        Err(error) => {
            tracing::warn!(%error, "PostgreSQL dependency check failed");
            DependencyDetail {
                state: DependencyState::Unavailable,
                ..available()
            }
        }
    };
    let worker = match sqlx::query(
        "SELECT count(*) FILTER (WHERE state IN ('queued','running'))::bigint AS backlog,
                count(*) FILTER (WHERE state = 'dead')::bigint AS dead,
                EXTRACT(EPOCH FROM (now() - min(available_at) FILTER (WHERE state = 'queued')))::bigint AS oldest_age_seconds,
                max(completed_at) AS last_success_at
         FROM jobs",
    )
    .fetch_one(&state.pool)
    .await
    {
        Ok(row) => {
            let backlog: i64 = row.get("backlog");
            let dead: i64 = row.get("dead");
            let oldest_age_seconds: Option<i64> = row.get("oldest_age_seconds");
            DependencyDetail {
                state: if dead > 0 || oldest_age_seconds.is_some_and(|age| age > 300) {
                    DependencyState::Degraded
                } else {
                    DependencyState::Available
                },
                backlog: Some(backlog),
                lag: None,
                oldest_age_seconds,
                last_success_at: row.get("last_success_at"),
            }
        }
        Err(error) => {
            tracing::warn!(%error, "durable worker job check failed");
            DependencyDetail {
                state: DependencyState::Unavailable,
                ..available()
            }
        }
    };
    Json(DependencyStatus { postgresql, worker })
}

#[utoipa::path(get, path="/health", operation_id="getDeprecatedHealth", tag="health",
    responses((status=200, body=LivenessResponse)))]
pub async fn deprecated_health(
    State(state): State<AppState>,
) -> (StatusCode, Json<LivenessResponse>) {
    (StatusCode::OK, live(State(state)).await)
}
