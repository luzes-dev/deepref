//! Worker liveness: a `worker_heartbeats` row written on a timer, reported by
//! the API's `/health/dependencies`.

use std::time::Duration;

use deepref_domain::WorkloadClass;
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
    pool_capacities: Vec<(WorkloadClass, usize)>,
) {
    let mut ticker = tokio::time::interval(interval);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        tokio::select! {
            _ = ticker.tick() => {
                if let Err(error) = deepref_app::record_worker_heartbeat_with_capacities(&pool, &owner, &pool_capacities).await {
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
    if let Err(error) = deepref_app::remove_worker_heartbeat(&pool, &owner).await {
        tracing::warn!(%error, owner, "worker heartbeat removal failed");
    }
}

/// Stop heartbeat ownership before closing its reserved pool.
pub async fn stop(
    mut task: tokio::task::JoinHandle<()>,
    signal: watch::Sender<bool>,
    deadline: Duration,
) {
    signal.send_replace(true);
    if tokio::time::timeout(deadline, &mut task).await.is_err() {
        task.abort();
        // Await cancellation: dropping the handle would detach the blocked
        // query and leave the reserved connection held during pool closure.
        let _ = task.await;
        tracing::warn!("worker heartbeat was aborted during shutdown; its row will age out");
    }
}
