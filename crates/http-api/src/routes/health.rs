mod transport;
pub use transport::TransportHealth;

use axum::{Json, extract::State, http::StatusCode};
use chrono::{DateTime, Utc};
use serde::Serialize;
use sqlx::Row;
use utoipa::ToSchema;
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
    /// The dependency answered, but its newest evidence is older than the
    /// freshness threshold, so it proves nothing about current health.
    Stale,
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
    /// Per-workload queue and capacity measurements (worker only).
    pub pools: Option<Vec<WorkloadPoolDetail>>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct WorkloadPoolDetail {
    /// Fixed queue class name; contains no tenant or user identity.
    pub class: String,
    /// Due queued jobs in this pool.
    pub queue_depth: i64,
    /// Age in seconds of the oldest due queued job.
    pub oldest_due_age_seconds: Option<i64>,
    /// Jobs currently in the running state.
    pub in_flight: i64,
    /// Capacity summed across workers with a live heartbeat.
    pub capacity: i64,
    /// In-flight divided by capacity; absent when no worker serves the pool.
    pub saturation: Option<f64>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct DependencyStatus {
    pub postgresql: DependencyDetail,
    pub worker: DependencyDetail,
    /// Actual bounded read probe; no write, deletion, or quota guarantee.
    pub object_storage: DependencyDetail,
    /// Required JetStream diagnostic evidence; absent in valid PostgreSQL mode.
    pub jetstream: Option<TransportHealth>,
}

/// Queued jobs older than this mean the worker is not keeping up.
const STALE_QUEUE_SECONDS: i64 = 300;
const PROBE_DEADLINE: std::time::Duration = std::time::Duration::from_secs(2);
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
        pools: None,
    }
}

async fn workload_pool_health(
    pool: &sqlx::PgPool,
    project_id: Option<Uuid>,
) -> Result<Vec<WorkloadPoolDetail>, sqlx::Error> {
    let rows = sqlx::query(
        "WITH classes(class) AS (
             VALUES ('ai'::text),('documents'),('workflow'),('integration'),('projection')
         ), capacities AS (
             SELECT entry.key AS class, sum(entry.value::bigint)::bigint AS capacity
             FROM worker_heartbeats heartbeat
             CROSS JOIN LATERAL jsonb_each_text(heartbeat.pool_capacities) entry
             WHERE heartbeat.seen_at > now() - make_interval(secs => $2::double precision)
               AND heartbeat.seen_at <= now()
             GROUP BY entry.key
         )
         SELECT classes.class,
                count(j.id) FILTER (
                    WHERE j.state IN ('queued','capability_wait') AND j.available_at <= now()
                )::bigint AS queue_depth,
                EXTRACT(EPOCH FROM (now() - min(j.available_at) FILTER (
                    WHERE j.state IN ('queued','capability_wait') AND j.available_at <= now()
                )))::bigint AS oldest_due_age_seconds,
                count(j.id) FILTER (WHERE j.state='running')::bigint AS in_flight,
                COALESCE(capacities.capacity,0)::bigint AS capacity
         FROM classes
         LEFT JOIN jobs j
           ON j.workload_class=classes.class
          AND ($1::uuid IS NULL OR j.project_id=$1)
         LEFT JOIN capacities ON capacities.class=classes.class
         GROUP BY classes.class,capacities.capacity
         ORDER BY classes.class",
    )
    .bind(project_id)
    .bind(LIVE_WORKER_WINDOW_SECONDS as f64)
    .fetch_all(pool)
    .await?;

    Ok(rows
        .iter()
        .map(|row| {
            let in_flight: i64 = row.get("in_flight");
            let capacity: i64 = row.get("capacity");
            WorkloadPoolDetail {
                class: row.get("class"),
                queue_depth: row.get("queue_depth"),
                oldest_due_age_seconds: row.get("oldest_due_age_seconds"),
                in_flight,
                capacity,
                saturation: (capacity > 0).then(|| in_flight as f64 / capacity as f64),
            }
        })
        .collect())
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
    deepref_app::MIGRATOR
        .iter()
        .map(|migration| migration.version)
        .max()
        .unwrap_or(0)
}

#[utoipa::path(get, path="/health/ready", operation_id="getReadiness", tag="health",
    responses((status=200, body=ReadinessResponse), (status=503, body=ApiErrorBody)))]
pub async fn ready(State(state): State<AppState>) -> Result<Json<ReadinessResponse>, ApiError> {
    let schema_version: i64 = tokio::time::timeout(
        PROBE_DEADLINE,
        sqlx::query_scalar(
            "SELECT COALESCE(max(version),0)::bigint FROM _sqlx_migrations WHERE success",
        )
        .fetch_one(&state.pool),
    )
    .await
    .map_err(|_| ApiError::Configuration("database readiness deadline exceeded".to_owned()))?
    .map_err(ApiError::Database)?;
    let required = required_schema_version();
    deepref_telemetry::metrics::HttpMetrics::global().schema_version(schema_version, required);
    if schema_version < required {
        return Err(ApiError::Configuration(format!(
            "database schema {schema_version} is older than required version {required}"
        )));
    }
    if schema_version > required {
        tracing::warn!(
            schema_version,
            required_schema_version = required,
            "database schema is ahead of this service"
        );
    }
    Ok(Json(ReadinessResponse {
        status: "ready",
        schema_version,
        required_schema_version: required,
    }))
}

/// Deployment-wide status. Public like the other probes, so it takes no
/// project filter and carries no per-project data.
#[utoipa::path(get, path="/health/dependencies", operation_id="getDependencyStatus", tag="health",
    responses((status=200, body=DependencyStatus)))]
pub async fn dependencies(State(state): State<AppState>) -> Json<DependencyStatus> {
    Json(dependency_status(&state, None).await)
}

/// The same probe narrowed to one project's jobs. The PEP requires
/// `project:view` on the path's project.
#[utoipa::path(get, path="/projects/{project_id}/health/dependencies",
    operation_id="getProjectDependencyStatus", tag="health",
    params(("project_id" = Uuid, Path, description = "Project identifier")),
    responses((status=200, body=DependencyStatus)))]
pub async fn project_dependencies(
    State(state): State<AppState>,
    axum::extract::Path(project_id): axum::extract::Path<Uuid>,
) -> Json<DependencyStatus> {
    Json(dependency_status(&state, Some(project_id)).await)
}

async fn dependency_status(state: &AppState, project_id: Option<Uuid>) -> DependencyStatus {
    let database = tokio::time::timeout(PROBE_DEADLINE, database_status(state, project_id));
    let storage = async {
        match &state.document_store {
            Some(store) => {
                match tokio::time::timeout(PROBE_DEADLINE, store.check_read_health()).await {
                    Ok(Ok(())) => DependencyDetail {
                        last_success_at: Some(Utc::now()),
                        ..available()
                    },
                    _ => unavailable(),
                }
            }
            None => unavailable(),
        }
    };
    let (database, object_storage, jetstream) =
        tokio::join!(database, storage, transport::health(state));
    let (postgresql, worker) = database.unwrap_or_else(|_| (unavailable(), unavailable()));
    DependencyStatus {
        postgresql,
        worker,
        object_storage,
        jetstream,
    }
}

fn unavailable() -> DependencyDetail {
    DependencyDetail {
        state: DependencyState::Unavailable,
        ..available()
    }
}

async fn database_status(
    state: &AppState,
    project_id: Option<Uuid>,
) -> (DependencyDetail, DependencyDetail) {
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
        "SELECT count(*) FILTER (WHERE state IN ('queued','capability_wait','running'))::bigint AS backlog,
                count(*) FILTER (
                    WHERE state = 'dead'
                      AND available_at > now() - make_interval(secs => $1::double precision)
                )::bigint AS recent_dead,
                EXTRACT(EPOCH FROM (now() - min(available_at) FILTER (WHERE state IN ('queued','capability_wait'))))::bigint AS oldest_age_seconds,
                max(completed_at) AS last_success_at,
                (SELECT count(*) FROM worker_heartbeats
                  WHERE seen_at > now() - make_interval(secs => $3::double precision) AND seen_at <= now())::bigint
                  AS live_workers,
                (SELECT max(seen_at) FROM worker_heartbeats) AS last_heartbeat_at
         FROM jobs
         WHERE ($2::uuid IS NULL OR project_id = $2)",
    )
    .bind(RECENT_DEAD_WINDOW_SECONDS as f64)
    .bind(project_id)
    .bind(LIVE_WORKER_WINDOW_SECONDS as f64)
    .fetch_one(&state.pool)
    .await
    {
        Ok(row) => {
            let backlog: i64 = row.get("backlog");
            let recent_dead: i64 = row.get("recent_dead");
            let oldest_age_seconds: Option<i64> = row.get("oldest_age_seconds");
            let live_workers: i64 = row.get("live_workers");
            let pools = match workload_pool_health(&state.pool, project_id).await {
                Ok(pools) => pools,
                Err(error) => {
                    tracing::warn!(%error, "worker workload pool check failed");
                    return (postgresql, unavailable());
                }
            };
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
                pools: Some(pools),
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
    (postgresql, worker)
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
        assert_eq!(
            required_schema_version(),
            deepref_app::MIGRATOR
                .iter()
                .map(|migration| migration.version)
                .max()
                .expect("embedded migrations")
        );
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
