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
    pub nats: DependencyDetail,
    pub outbox: DependencyDetail,
    pub worker: DependencyDetail,
    pub neo4j: DependencyDetail,
    pub projection: DependencyDetail,
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
    if schema_version < 5 {
        return Err(ApiError::Configuration(format!(
            "database schema {schema_version} is older than required version 5"
        )));
    }
    Ok(Json(ReadinessResponse {
        status: "ready",
        schema_version,
        required_schema_version: 5,
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
    let outbox_row = sqlx::query(
        "SELECT count(*)::bigint AS backlog, COALESCE(EXTRACT(EPOCH FROM now()-min(created_at))::bigint,0) AS age \
         FROM event_outbox WHERE published_at IS NULL AND exhausted_at IS NULL",
    ).fetch_optional(&state.pool).await.ok().flatten();
    let outbox = outbox_row
        .map(|row| {
            let backlog = row.get::<i64, _>("backlog");
            DependencyDetail {
                state: if backlog > 0 {
                    DependencyState::Degraded
                } else {
                    DependencyState::Available
                },
                backlog: Some(backlog),
                oldest_age_seconds: Some(row.get("age")),
                lag: None,
                last_success_at: None,
            }
        })
        .unwrap_or(DependencyDetail {
            state: DependencyState::Unavailable,
            ..available()
        });
    let worker_backlog = sqlx::query_scalar::<_, i64>(
        "SELECT count(*)::bigint FROM ingestion_items WHERE status IN ('queued','fetching')",
    )
    .fetch_one(&state.pool)
    .await
    .ok();
    let worker = DependencyDetail {
        state: worker_backlog.map_or(DependencyState::Unavailable, |count| {
            if count > 0 {
                DependencyState::Degraded
            } else {
                DependencyState::Available
            }
        }),
        backlog: worker_backlog,
        lag: worker_backlog,
        oldest_age_seconds: None,
        last_success_at: None,
    };
    let nats = DependencyDetail {
        state: if state.jetstream.is_some() {
            DependencyState::Available
        } else {
            DependencyState::Unavailable
        },
        ..available()
    };
    let neo4j = match &state.graph {
        Some(graph) if graph.ping().await.is_ok() => available(),
        _ => DependencyDetail {
            state: DependencyState::Unavailable,
            ..available()
        },
    };
    let projection_row = sqlx::query(
        "SELECT state,lag,last_success_at FROM projection_state WHERE projection_name='graph' AND project_id IS NULL",
    ).fetch_optional(&state.pool).await.ok().flatten();
    let projection = projection_row
        .map(|row| DependencyDetail {
            state: if row.get::<String, _>("state") == "ready" {
                DependencyState::Available
            } else {
                DependencyState::Degraded
            },
            lag: Some(row.get("lag")),
            backlog: None,
            oldest_age_seconds: None,
            last_success_at: row.get("last_success_at"),
        })
        .unwrap_or(DependencyDetail {
            state: DependencyState::Unavailable,
            ..available()
        });
    Json(DependencyStatus {
        postgresql,
        nats,
        outbox,
        worker,
        neo4j,
        projection,
    })
}

#[utoipa::path(get, path="/health", operation_id="getDeprecatedHealth", tag="health",
    responses((status=200, body=LivenessResponse)))]
pub async fn deprecated_health(
    State(state): State<AppState>,
) -> (StatusCode, Json<LivenessResponse>) {
    (StatusCode::OK, live(State(state)).await)
}
