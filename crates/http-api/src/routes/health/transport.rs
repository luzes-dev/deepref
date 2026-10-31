use chrono::{DateTime, Utc};
use serde::Serialize;
use sqlx::Row;
use utoipa::ToSchema;

use super::{DependencyState, PROBE_DEADLINE};
use crate::state::AppState;
use deepref_application::operational_health::WorkTransportMode;

/// Counts from diagnostic heartbeats, never a fleet inventory or drain fence.
#[derive(Debug, Serialize, ToSchema)]
pub struct TransportHealth {
    pub state: DependencyState,
    pub expected_mode: &'static str,
    pub live_workers: i64,
    pub fresh_ready_workers: i64,
    pub not_ready_workers: i64,
    pub unknown_workers: i64,
    pub stale_workers: i64,
    pub future_workers: i64,
    pub mismatched_workers: i64,
    pub last_observation_at: Option<DateTime<Utc>>,
}

impl TransportHealth {
    fn unavailable() -> Self {
        Self {
            state: DependencyState::Unavailable,
            expected_mode: "jetstream",
            live_workers: 0,
            fresh_ready_workers: 0,
            not_ready_workers: 0,
            unknown_workers: 0,
            stale_workers: 0,
            future_workers: 0,
            mismatched_workers: 0,
            last_observation_at: None,
        }
    }
}

pub(super) async fn health(state: &AppState) -> Option<TransportHealth> {
    if state.work_transport == WorkTransportMode::Postgres {
        // PostgreSQL is a valid selected execution/rollback mode. Old broker
        // observations cannot refuse it or turn a heartbeat into authority.
        return None;
    }
    let query = sqlx::query(
        "WITH samples AS (
            SELECT *, CASE
                WHEN seen_at>now() OR transport_observed_at>now() THEN 'future'
                WHEN transport_mode IS NULL THEN 'unknown'
                WHEN transport_mode<>'jetstream' THEN 'mismatch'
                WHEN transport_ready IS NULL OR transport_observed_at IS NULL THEN 'unknown'
                WHEN transport_observed_at<now()-interval '90 seconds' THEN 'stale'
                WHEN transport_ready THEN 'ready' ELSE 'not_ready' END AS evidence
            FROM worker_heartbeats
            WHERE seen_at>now()-interval '60 seconds'
         ) SELECT count(*)::bigint AS live,
            count(*) FILTER(WHERE evidence='ready')::bigint AS ready,
            count(*) FILTER(WHERE evidence='not_ready')::bigint AS not_ready,
            count(*) FILTER(WHERE evidence='unknown')::bigint AS unknown,
            count(*) FILTER(WHERE evidence='stale')::bigint AS stale,
            count(*) FILTER(WHERE evidence='future')::bigint AS future,
            count(*) FILTER(WHERE evidence='mismatch')::bigint AS mismatch,
            max(transport_observed_at) FILTER(WHERE evidence<>'future') AS last_observation
         FROM samples",
    )
    .fetch_one(&state.pool);
    match tokio::time::timeout(PROBE_DEADLINE, query).await {
        Ok(Ok(row)) => {
            let live_workers: i64 = row.get("live");
            let fresh_ready_workers: i64 = row.get("ready");
            Some(TransportHealth {
                state: if live_workers > 0 && fresh_ready_workers == live_workers {
                    DependencyState::Available
                } else {
                    DependencyState::Degraded
                },
                expected_mode: "jetstream",
                live_workers,
                fresh_ready_workers,
                not_ready_workers: row.get("not_ready"),
                unknown_workers: row.get("unknown"),
                stale_workers: row.get("stale"),
                future_workers: row.get("future"),
                mismatched_workers: row.get("mismatch"),
                last_observation_at: row.get("last_observation"),
            })
        }
        _ => Some(TransportHealth::unavailable()),
    }
}
