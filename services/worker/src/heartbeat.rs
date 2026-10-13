//! Worker liveness: a `worker_heartbeats` row written on a timer, reported by
//! the API's `/health/dependencies`.

use std::time::Duration;

use sqlx::PgPool;
use tokio::sync::watch;

/// How often a worker refreshes its heartbeat. `/health/dependencies` treats a
/// worker as live for a few of these intervals.
pub const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(15);

/// Refresh `owner`'s heartbeat until `shutdown` flips, then remove it so a
/// clean stop is not mistaken for a live worker. A failed write is logged and
/// retried on the next tick; it never stops the worker.
pub async fn run(
    pool: PgPool,
    owner: String,
    interval: Duration,
    mut shutdown: watch::Receiver<bool>,
) {
    let mut ticker = tokio::time::interval(interval);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        tokio::select! {
            _ = ticker.tick() => {
                if let Err(error) = deepref_postgres::record_worker_heartbeat(&pool, &owner).await {
                    tracing::warn!(%error, owner, "worker heartbeat write failed");
                }
            }
            changed = shutdown.changed() => {
                if changed.is_err() || *shutdown.borrow() {
                    break;
                }
            }
        }
    }
    if let Err(error) = deepref_postgres::remove_worker_heartbeat(&pool, &owner).await {
        tracing::warn!(%error, owner, "worker heartbeat removal failed");
    }
}
