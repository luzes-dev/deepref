#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::panic_in_result_fn
)]

use std::time::Duration;

use deepref_domain::WorkloadClass;
use tokio::sync::watch;

#[tokio::test]
async fn blocked_heartbeat_shutdown_joins_task_and_bounds_pool_close() -> anyhow::Result<()> {
    let Some(pool) = deepref_app::test_isolation::isolated_pool_from_env("DATABASE_URL", 3).await
    else {
        return Ok(());
    };
    let control = pool
        .options()
        .clone()
        .min_connections(0)
        .max_connections(1)
        .connect_lazy_with((*pool.connect_options()).clone());
    let mut blocker = pool.begin().await?;
    sqlx::query("LOCK TABLE worker_heartbeats IN ACCESS EXCLUSIVE MODE")
        .execute(&mut *blocker)
        .await?;
    let owner = format!("blocked-heartbeat-{}", uuid::Uuid::new_v4());
    let (stop, signal) = watch::channel(false);
    let task = tokio::spawn(deepref_worker::heartbeat::run(
        control.clone(),
        owner.clone(),
        Duration::from_secs(60),
        signal,
        vec![(WorkloadClass::Documents, 1)],
    ));
    let abort = task.abort_handle();
    // Prove the real INSERT is waiting for our lock before starting shutdown.
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let waiting: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND query LIKE '%INSERT INTO worker_heartbeats%')")
                .fetch_one(&pool).await?;
            if waiting { return Ok::<(), sqlx::Error>(()); }
            tokio::task::yield_now().await;
        }
    }).await??;
    let drained = tokio::time::timeout(Duration::from_secs(2), async {
        deepref_worker::heartbeat::stop(task, stop, Duration::from_millis(50)).await;
        deepref_worker::shutdown::close_control_pool(&control, Duration::from_millis(50)).await;
        assert!(
            abort.is_finished(),
            "heartbeat task must be joined before pool close returns"
        );
    })
    .await;
    // Cleanup even on the old detach bug, preserving a useful behavioral red.
    abort.abort();
    blocker.rollback().await?;
    tokio::time::timeout(Duration::from_secs(2), control.close()).await?;
    assert!(
        drained.is_ok(),
        "blocked heartbeat shutdown kept reserved pool close waiting"
    );
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM worker_heartbeats WHERE owner=$1")
        .bind(owner)
        .fetch_one(&pool)
        .await?;
    assert_eq!(
        count, 0,
        "aborted blocked INSERT must never commit after shutdown"
    );
    Ok(())
}

#[tokio::test]
async fn normal_heartbeat_shutdown_removes_its_live_row() -> anyhow::Result<()> {
    let Some(pool) = deepref_app::test_isolation::isolated_pool_from_env("DATABASE_URL", 3).await
    else {
        return Ok(());
    };
    let owner = format!("normal-heartbeat-{}", uuid::Uuid::new_v4());
    let (stop, signal) = watch::channel(false);
    let task = tokio::spawn(deepref_worker::heartbeat::run(
        pool.clone(),
        owner.clone(),
        Duration::from_secs(60),
        signal,
        vec![(WorkloadClass::Documents, 1)],
    ));
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let exists: bool =
                sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM worker_heartbeats WHERE owner=$1)")
                    .bind(&owner)
                    .fetch_one(&pool)
                    .await?;
            if exists {
                return Ok::<(), sqlx::Error>(());
            }
            tokio::task::yield_now().await;
        }
    })
    .await??;
    deepref_worker::heartbeat::stop(task, stop, Duration::from_secs(1)).await;
    let exists: bool =
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM worker_heartbeats WHERE owner=$1)")
            .bind(owner)
            .fetch_one(&pool)
            .await?;
    assert!(!exists);
    Ok(())
}

#[tokio::test]
async fn reserved_pool_close_is_bounded_with_an_outstanding_holder() -> anyhow::Result<()> {
    let Some(pool) = deepref_app::test_isolation::isolated_pool_from_env("DATABASE_URL", 2).await
    else {
        return Ok(());
    };
    let control = pool
        .options()
        .clone()
        .min_connections(0)
        .max_connections(1)
        .connect_lazy_with((*pool.connect_options()).clone());
    let held = control.acquire().await?;
    let closed = tokio::time::timeout(
        Duration::from_secs(1),
        deepref_worker::shutdown::close_control_pool(&control, Duration::from_millis(50)),
    )
    .await;
    assert!(control.is_closed(), "closing must prevent new acquisitions");
    drop(held);
    tokio::time::timeout(Duration::from_secs(2), control.close()).await?;
    assert!(
        closed.is_ok(),
        "pool close must not wait unboundedly for a holder"
    );
    Ok(())
}
