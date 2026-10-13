use axum::{
    Json,
    extract::{Query, State},
    http::StatusCode,
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::Row;
use utoipa::{IntoParams, ToSchema};
use uuid::Uuid;

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
    /// Jobs that failed within the recent window (worker only).
    pub recent_failed: Option<i64>,
    /// Worker processes with a recent heartbeat (worker only).
    pub live_workers: Option<i64>,
    /// The most recent worker heartbeat, live or not (worker only).
    pub last_heartbeat_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct DependencyStatus {
    pub postgresql: DependencyDetail,
    pub worker: DependencyDetail,
}

#[derive(Debug, Deserialize, IntoParams)]
pub struct DependencyQuery {
    /// Check only this project's background jobs. Omit for the whole workspace.
    pub project_id: Option<Uuid>,
}

/// Queued jobs older than this mean the worker is not keeping up.
const STALE_QUEUE_SECONDS: i64 = 300;
/// Jobs that exhausted their retries longer ago than this are history, not an
/// ongoing worker problem; they stay visible in job listings.
const RECENT_DEAD_WINDOW_SECONDS: i64 = 30 * 60;
/// A worker heartbeats every 15 s; one seen within this window is live.
const LIVE_WORKER_WINDOW_SECONDS: i64 = 60;

fn worker_state(recent_dead: i64, oldest_queued_age_seconds: Option<i64>) -> DependencyState {
    if recent_dead > 0 || oldest_queued_age_seconds.is_some_and(|age| age > STALE_QUEUE_SECONDS) {
        DependencyState::Degraded
    } else {
        DependencyState::Available
    }
}

fn available() -> DependencyDetail {
    DependencyDetail {
        state: DependencyState::Available,
        lag: None,
        backlog: None,
        oldest_age_seconds: None,
        last_success_at: None,
        recent_failed: None,
        live_workers: None,
        last_heartbeat_at: None,
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

fn required_schema_version() -> i64 {
    deepref_postgres::MIGRATOR.iter().map(|migration| migration.version).max().unwrap_or(0)
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
    let required = required_schema_version();
    if schema_version < required {
        return Err(ApiError::Configuration(format!(
            "database schema {schema_version} is older than required version {required}"
        )));
    }
    if schema_version > required {
        tracing::warn!(schema_version, required_schema_version = required, "database schema is ahead of this service");
    }
    Ok(Json(ReadinessResponse {
        status: "ready",
        schema_version,
        required_schema_version: required,
    }))
}

#[utoipa::path(get, path="/health/dependencies", operation_id="getDependencyStatus", tag="health",
    params(DependencyQuery),
    responses((status=200, body=DependencyStatus)))]
pub async fn dependencies(
    State(state): State<AppState>,
    Query(query): Query<DependencyQuery>,
) -> Json<DependencyStatus> {
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
                count(*) FILTER (
                    WHERE state = 'dead'
                      AND available_at > now() - make_interval(secs => $1::double precision)
                )::bigint AS recent_dead,
                EXTRACT(EPOCH FROM (now() - min(available_at) FILTER (WHERE state = 'queued')))::bigint AS oldest_age_seconds,
                max(completed_at) AS last_success_at,
                (SELECT count(*) FROM worker_heartbeats
                  WHERE seen_at > now() - make_interval(secs => $3::double precision))::bigint
                  AS live_workers,
                (SELECT max(seen_at) FROM worker_heartbeats) AS last_heartbeat_at
         FROM jobs
         WHERE ($2::uuid IS NULL OR project_id = $2)",
    )
    .bind(RECENT_DEAD_WINDOW_SECONDS as f64)
    .bind(query.project_id)
    .bind(LIVE_WORKER_WINDOW_SECONDS as f64)
    .fetch_one(&state.pool)
    .await
    {
        Ok(row) => {
            let backlog: i64 = row.get("backlog");
            let recent_dead: i64 = row.get("recent_dead");
            let oldest_age_seconds: Option<i64> = row.get("oldest_age_seconds");
            let live_workers: i64 = row.get("live_workers");
            // Work is waiting and no worker is alive to take it.
            let state = if backlog > 0 && live_workers == 0 {
                DependencyState::Degraded
            } else {
                worker_state(recent_dead, oldest_age_seconds)
            };
            DependencyDetail {
                state,
                backlog: Some(backlog),
                lag: None,
                oldest_age_seconds,
                last_success_at: row.get("last_success_at"),
                recent_failed: Some(recent_dead),
                live_workers: Some(live_workers),
                last_heartbeat_at: row.get("last_heartbeat_at"),
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

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn readiness_requires_latest_embedded_migration() {
        assert_eq!(required_schema_version(), deepref_postgres::MIGRATOR.iter().map(|migration| migration.version).max().expect("embedded migrations"));
    }


    #[test]
    fn historical_dead_jobs_do_not_degrade_the_worker() {
        assert!(matches!(worker_state(0, None), DependencyState::Available));
        assert!(matches!(
            worker_state(0, Some(12)),
            DependencyState::Available
        ));
    }

    #[test]
    fn recent_dead_jobs_or_a_stale_queue_degrade_the_worker() {
        assert!(matches!(worker_state(1, None), DependencyState::Degraded));
        assert!(matches!(
            worker_state(0, Some(301)),
            DependencyState::Degraded
        ));
    }
}
