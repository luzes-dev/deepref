#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]
//! Only synthetic contracts exercise the 04a dispatcher; metrics stay on jobs.

#[path = "support/processors.rs"]
mod support;

use deepref_app::outbox::{
    claim_outbox_batch, dispatch_outbox_event, insert_outbox_event, record_outbox_receipt,
};
use deepref_application::outbox::{
    NewOutboxEvent, OutboxHandlerError, OutboxMessage, OutboxRegistry, TypedOutboxHandler,
};
use deepref_worker::{outbox_dispatch, shutdown::on_signal};
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use std::{
    sync::{Arc, atomic::Ordering},
    time::Duration,
};
use support::BlockedHandler;
use tokio::sync::watch;
use uuid::Uuid;

#[derive(Serialize, Deserialize)]
struct WriteProject {
    project_id: Uuid,
}
impl OutboxMessage for WriteProject {
    const TYPE: &'static str = "test.worker.write-project";
    const VERSION: u32 = 1;
    const CONSUMERS: &'static [&'static str] = &["fixture"];
}
fn registry(pool: &PgPool, blocked: Option<Arc<BlockedHandler>>) -> Arc<OutboxRegistry> {
    let pool = pool.clone();
    let handler: TypedOutboxHandler<WriteProject> = Arc::new(move |message, context| {
        let pool = pool.clone();
        let blocked = blocked.clone();
        Box::pin(async move {
            let effect = async {
                let mut tx = pool.begin().await?;
                sqlx::query("INSERT INTO projects(id,name) VALUES($1,'synthetic outbox effect')")
                    .bind(message.project_id)
                    .execute(&mut *tx)
                    .await?;
                support::block(blocked).await;
                record_outbox_receipt(&mut tx, &context).await?;
                tx.commit().await?;
                Ok::<(), anyhow::Error>(())
            }
            .await;
            effect.map_err(|e| OutboxHandlerError::Transient(e.to_string()))
        })
    });
    let mut registry = OutboxRegistry::new();
    registry
        .register::<WriteProject>(&[("fixture", handler)])
        .expect("fixture registers");
    Arc::new(registry)
}
fn config() -> outbox_dispatch::OutboxDispatchConfig {
    let runtime = deepref_config::RuntimeConfig::from_source("outbox-test", &|name| {
        (name == "APP_ENV").then(|| "development".into())
    })
    .expect("default local runtime configuration");
    outbox_dispatch::OutboxDispatchConfig {
        batch: 8,
        concurrency: 2,
        claim_lease: Duration::from_secs(10),
        poll_interval: Duration::from_millis(20),
        drain_deadline: runtime.telemetry.shutdown_deadline,
    }
}
async fn insert(pool: &PgPool, project_id: Uuid) -> Uuid {
    let id = Uuid::new_v4();
    let event = NewOutboxEvent::for_message(id, &WriteProject { project_id }).expect("serializes");
    let mut tx = pool.begin().await.expect("begin");
    insert_outbox_event(&mut tx, &event).await.expect("insert");
    tx.commit().await.expect("commit");
    id
}

async fn outbox_lease_snapshot(pool: &PgPool, event: Uuid) -> (i64, i64, i32, Option<String>) {
    sqlx::query_as(
        "SELECT (EXTRACT(EPOCH FROM lease_expires_at) * 1000000)::bigint, claim_generation, attempts, lease_owner FROM outbox_events WHERE event_id=$1",
    )
    .bind(event)
    .fetch_one(pool)
    .await
    .unwrap()
}

#[tokio::test]
async fn the_dispatcher_delivers_a_synthetic_fixture_and_drains() {
    let Some(pool) = support::database().await else {
        return;
    };
    let (stop, signal) = watch::channel(false);
    let task = tokio::spawn(outbox_dispatch::run(
        pool.clone(),
        registry(&pool, None),
        "fixture".into(),
        config(),
        on_signal(signal),
    ));
    let project = Uuid::new_v4();
    let event = insert(&pool, project).await;
    support::wait_for_project(&pool, project).await;
    let state: String =
        sqlx::query_scalar("SELECT delivery_state FROM outbox_events WHERE event_id=$1")
            .bind(event)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(state, "delivered");
    stop.send_replace(true);
    assert_eq!(task.await.unwrap().unwrap().claim_errors, 0);
}

#[tokio::test]
async fn a_transient_claim_error_does_not_stop_the_dispatcher() {
    let Some(pool) = support::database().await else {
        return;
    };
    sqlx::query("ALTER TABLE outbox_events RENAME TO paused_outbox_events")
        .execute(&pool)
        .await
        .unwrap();
    let (stop, signal) = watch::channel(false);
    let task = tokio::spawn(outbox_dispatch::run(
        pool.clone(),
        registry(&pool, None),
        "retry-fixture".into(),
        config(),
        on_signal(signal),
    ));
    tokio::time::sleep(Duration::from_secs(1)).await;
    assert!(
        !task.is_finished(),
        "a claim error must not stop processing"
    );
    sqlx::query("ALTER TABLE paused_outbox_events RENAME TO outbox_events")
        .execute(&pool)
        .await
        .unwrap();
    let project = Uuid::new_v4();
    insert(&pool, project).await;
    support::wait_for_project(&pool, project).await;
    stop.send_replace(true);
    assert!(
        task.await.unwrap().unwrap().claim_errors > 0,
        "the retry path was exercised"
    );
}

#[tokio::test]
async fn drain_deadline_aborts_and_awaits_a_blocked_outbox_transaction() {
    let Some(pool) = support::database().await else {
        return;
    };
    let blocked = Arc::new(BlockedHandler::default());
    let mut config = config();
    config.drain_deadline = Duration::from_millis(100);
    let (stop, signal) = watch::channel(false);
    let task = tokio::spawn(outbox_dispatch::run(
        pool.clone(),
        registry(&pool, Some(Arc::clone(&blocked))),
        "blocked-fixture".into(),
        config,
        on_signal(signal),
    ));
    let project = Uuid::new_v4();
    insert(&pool, project).await;
    tokio::time::timeout(Duration::from_secs(30), blocked.entered.notified())
        .await
        .unwrap();
    stop.send_replace(true);
    let result = tokio::time::timeout(Duration::from_secs(5), task)
        .await
        .unwrap()
        .unwrap();
    assert!(result.is_err());
    assert!(
        blocked.dropped.load(Ordering::SeqCst),
        "handler was dropped before shutdown returned"
    );
    blocked.resume.notify_one();
    support::assert_no_project(&pool, project).await;
}

#[tokio::test]
async fn heartbeat_renews_then_cancels_a_superseded_outbox_handler() {
    let Some(pool) = support::database().await else {
        return;
    };
    let blocked = Arc::new(BlockedHandler::default());
    let registry = registry(&pool, Some(Arc::clone(&blocked)));
    let lease = Duration::from_secs(5);
    let project = Uuid::new_v4();
    let event = insert(&pool, project).await;
    let mut claims = claim_outbox_batch(&pool, &registry, "heartbeat-fixture", lease, 1)
        .await
        .unwrap();
    assert_eq!(claims.len(), 1);
    let claim = claims.pop().unwrap();
    let claim_generation = claim.claim_fence.generation();
    let (claim_expires_at, generation, attempts, owner) = outbox_lease_snapshot(&pool, event).await;
    assert_eq!(generation, claim_generation);
    assert_eq!(attempts, 1);
    assert_eq!(owner.as_deref(), Some("heartbeat-fixture"));

    let dispatch_pool = pool.clone();
    let dispatch_registry = Arc::clone(&registry);
    let task = tokio::spawn(async move {
        dispatch_outbox_event(&dispatch_pool, &dispatch_registry, lease, &claim).await
    });
    tokio::time::timeout(Duration::from_secs(30), blocked.entered.notified())
        .await
        .unwrap();
    let (handler_expires_at, generation, attempts, owner) =
        outbox_lease_snapshot(&pool, event).await;
    assert_eq!(generation, claim_generation);
    assert_eq!(attempts, 1);
    assert_eq!(owner.as_deref(), Some("heartbeat-fixture"));
    assert!(handler_expires_at >= claim_expires_at);

    let renewed_expires_at = tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            let (expires_at, generation, attempts, owner) =
                outbox_lease_snapshot(&pool, event).await;
            assert_eq!(
                generation, claim_generation,
                "renewal keeps the claim generation"
            );
            assert_eq!(attempts, 1, "renewal does not spend another attempt");
            assert_eq!(owner.as_deref(), Some("heartbeat-fixture"));
            assert!(
                !blocked.dropped.load(Ordering::SeqCst),
                "the healthy handler stays active while its claim is renewed"
            );
            if expires_at > handler_expires_at {
                break expires_at;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await
    .expect("the heartbeat advances the lease while the handler is blocked");
    assert!(renewed_expires_at > claim_expires_at);

    sqlx::query(
        "UPDATE outbox_events SET lease_owner='replacement',claim_generation=claim_generation+1,lease_expires_at=clock_timestamp()+interval '1 minute' WHERE event_id=$1",
    )
    .bind(event)
    .execute(&pool)
    .await
    .unwrap();
    support::wait_for_drop(&blocked).await;
    let result = tokio::time::timeout(Duration::from_secs(10), task)
        .await
        .expect("the superseded dispatcher task finishes")
        .unwrap();
    assert!(result.is_err(), "the generation bump cancels the old claim");
    support::assert_no_project(&pool, project).await;
}
