#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]
//! Inbox loop recovery and ownership against isolated PostgreSQL fixtures.

#[path = "support/processors.rs"]
mod support;

use deepref_app::inbox::{
    ReceiveOutcome, ReceiveWebhookEvent, complete_inbox_event_in_transaction,
    fence_inbox_event_effect, receive_webhook_event,
};
use deepref_application::inbox::{
    HmacSha256Verifier, InboxHandlerError, InboxRegistry, TypedInboxHandler,
};
use deepref_application::workflows::security::sign_payload;
use deepref_worker::{inbox_processor, shutdown::on_signal};
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
fn registry(pool: &PgPool, blocked: Option<Arc<BlockedHandler>>) -> Arc<InboxRegistry> {
    let pool = pool.clone();
    let handler: TypedInboxHandler<WriteProject> = Arc::new(move |message, context| {
        let pool = pool.clone();
        let blocked = blocked.clone();
        Box::pin(async move {
            let effect = async {
                let mut tx = pool.begin().await?;
                fence_inbox_event_effect(&mut tx, &context).await?;
                sqlx::query("INSERT INTO projects(id,name) VALUES($1,'synthetic inbox effect')")
                    .bind(message.project_id)
                    .execute(&mut *tx)
                    .await?;
                support::block(blocked).await;
                complete_inbox_event_in_transaction(&mut tx, &context).await?;
                tx.commit().await?;
                Ok::<(), anyhow::Error>(())
            }
            .await;
            effect.map_err(|e| InboxHandlerError::Transient(e.to_string()))
        })
    });
    let mut registry = InboxRegistry::new();
    registry
        .register("fixture", "project.created", handler)
        .expect("fixture registers");
    Arc::new(registry)
}
fn config() -> inbox_processor::InboxProcessorConfig {
    inbox_processor::InboxProcessorConfig {
        batch: 8,
        concurrency: 2,
        claim_lease: Duration::from_secs(10),
        poll_interval: Duration::from_millis(20),
        drain_deadline: Duration::from_millis(100),
    }
}
async fn insert(pool: &PgPool, project_id: Uuid) -> i64 {
    let event_id = Uuid::new_v4().to_string();
    let body = serde_json::to_vec(&WriteProject { project_id }).unwrap();
    let signature = sign_payload("fixture-secret", &body);
    match receive_webhook_event(
        pool,
        &HmacSha256Verifier::new("fixture-secret"),
        &ReceiveWebhookEvent {
            provider: "fixture",
            provider_event_id: &event_id,
            event_type: Some("project.created"),
            body: &body,
            signature: Some(&signature),
        },
    )
    .await
    .unwrap()
    {
        ReceiveOutcome::Accepted(id) => id,
        ReceiveOutcome::Duplicate => panic!("new event"),
    }
}

#[tokio::test]
async fn the_inbox_processor_delivers_a_synthetic_fixture_and_drains() {
    let Some(pool) = support::database().await else {
        return;
    };
    let (stop, signal) = watch::channel(false);
    let task = tokio::spawn(inbox_processor::run(
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
        sqlx::query_scalar("SELECT processing_state FROM integration_inbox_events WHERE id=$1")
            .bind(event)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(state, "processed");
    stop.send_replace(true);
    assert_eq!(task.await.unwrap().unwrap().claim_errors, 0);
}

#[tokio::test]
async fn a_transient_claim_error_does_not_stop_the_inbox_processor() {
    let Some(pool) = support::database().await else {
        return;
    };
    sqlx::query("ALTER TABLE integration_inbox_events RENAME TO paused_inbox_events")
        .execute(&pool)
        .await
        .unwrap();
    let (stop, signal) = watch::channel(false);
    let task = tokio::spawn(inbox_processor::run(
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
    sqlx::query("ALTER TABLE paused_inbox_events RENAME TO integration_inbox_events")
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
async fn drain_deadline_aborts_and_awaits_a_blocked_inbox_transaction() {
    let Some(pool) = support::database().await else {
        return;
    };
    let blocked = Arc::new(BlockedHandler::default());
    let (stop, signal) = watch::channel(false);
    let task = tokio::spawn(inbox_processor::run(
        pool.clone(),
        registry(&pool, Some(Arc::clone(&blocked))),
        "blocked-fixture".into(),
        config(),
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
async fn heartbeat_renews_then_cancels_a_superseded_inbox_handler() {
    let Some(pool) = support::database().await else {
        return;
    };
    let blocked = Arc::new(BlockedHandler::default());
    let mut config = config();
    config.claim_lease = Duration::from_secs(1);
    let (stop, signal) = watch::channel(false);
    let task = tokio::spawn(inbox_processor::run(
        pool.clone(),
        registry(&pool, Some(Arc::clone(&blocked))),
        "heartbeat-fixture".into(),
        config,
        on_signal(signal),
    ));
    let project = Uuid::new_v4();
    let event = insert(&pool, project).await;
    tokio::time::timeout(Duration::from_secs(30), blocked.entered.notified())
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(2200)).await;
    let row: (i32, bool) = sqlx::query_as(
        "SELECT attempts,lease_expires_at>clock_timestamp() FROM integration_inbox_events WHERE id=$1",
    )
    .bind(event)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(row, (1, true), "a long handler retains the same live claim");
    sqlx::query("UPDATE integration_inbox_events SET lease_owner='replacement',claim_generation=claim_generation+1,lease_expires_at=clock_timestamp()+interval '1 minute' WHERE id=$1").bind(event).execute(&pool).await.unwrap();
    support::wait_for_drop(&blocked).await;
    stop.send_replace(true);
    task.await.unwrap().unwrap();
    blocked.resume.notify_one();
    support::assert_no_project(&pool, project).await;
}

#[tokio::test]
async fn a_long_healthy_inbox_transaction_does_not_block_its_renewal() {
    let Some(pool) = support::database().await else {
        return;
    };
    let blocked = Arc::new(BlockedHandler::default());
    let mut config = config();
    config.claim_lease = Duration::from_secs(1);
    let (stop, signal) = watch::channel(false);
    let task = tokio::spawn(inbox_processor::run(
        pool.clone(),
        registry(&pool, Some(Arc::clone(&blocked))),
        "healthy-long-transaction".into(),
        config,
        on_signal(signal),
    ));
    let project = Uuid::new_v4();
    let event = insert(&pool, project).await;
    tokio::time::timeout(Duration::from_secs(30), blocked.entered.notified())
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(2200)).await;
    assert!(
        !blocked.dropped.load(Ordering::SeqCst),
        "the preflight check must not block lease renewal"
    );
    blocked.resume.notify_one();
    support::wait_for_project(&pool, project).await;
    let row: (String, i32) = sqlx::query_as(
        "SELECT processing_state,attempts FROM integration_inbox_events WHERE id=$1",
    )
    .bind(event)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(row, ("processed".into(), 1));
    stop.send_replace(true);
    task.await.unwrap().unwrap();
}

#[tokio::test]
async fn takeover_while_a_handler_becomes_ready_rejects_its_fenced_database_commit() {
    use std::sync::atomic::AtomicBool;
    let Some(pool) = support::database().await else {
        return;
    };
    let blocked = Arc::new(BlockedHandler::default());
    let rejected = Arc::new(AtomicBool::new(false));
    let handler: TypedInboxHandler<WriteProject> = {
        let pool = pool.clone();
        let blocked = Arc::clone(&blocked);
        let rejected = Arc::clone(&rejected);
        Arc::new(move |message, context| {
            let pool = pool.clone();
            let blocked = Arc::clone(&blocked);
            let rejected = Arc::clone(&rejected);
            Box::pin(async move {
                let mut tx = pool.begin().await.unwrap();
                fence_inbox_event_effect(&mut tx, &context).await.unwrap();
                sqlx::query("INSERT INTO projects(id,name) VALUES($1,'ready stale effect')")
                    .bind(message.project_id)
                    .execute(&mut *tx)
                    .await
                    .unwrap();
                support::block(Some(blocked)).await;
                let completion = complete_inbox_event_in_transaction(&mut tx, &context).await;
                rejected.store(completion.is_err(), Ordering::SeqCst);
                if let Err(error) = completion {
                    tx.rollback().await.unwrap();
                    return Err(InboxHandlerError::Transient(error.to_string()));
                }
                tx.commit().await.unwrap();
                Ok(())
            })
        })
    };
    let mut registry = InboxRegistry::new();
    registry
        .register("fixture", "project.created", handler)
        .unwrap();
    let project = Uuid::new_v4();
    let event = insert(&pool, project).await;
    let claimed =
        deepref_app::inbox::claim_inbox_batch(&pool, &registry, "old", Duration::from_secs(60), 1)
            .await
            .unwrap();
    let task = tokio::spawn({
        let pool = pool.clone();
        async move {
            deepref_app::inbox::process_inbox_event(
                &pool,
                &registry,
                Duration::from_secs(60),
                &claimed[0],
            )
            .await
        }
    });
    tokio::time::timeout(Duration::from_secs(30), blocked.entered.notified())
        .await
        .unwrap();
    // Ownership changes as the handler is made ready to complete. The heartbeat
    // is not yet due, so the database generation check is the final authority.
    sqlx::query("UPDATE integration_inbox_events SET lease_owner='replacement',claim_generation=claim_generation+1 WHERE id=$1")
        .bind(event).execute(&pool).await.unwrap();
    blocked.resume.notify_one();
    tokio::time::timeout(Duration::from_secs(10), task)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert!(
        rejected.load(Ordering::SeqCst),
        "the final fenced write must reject stale effects"
    );
    support::assert_no_project(&pool, project).await;
}
