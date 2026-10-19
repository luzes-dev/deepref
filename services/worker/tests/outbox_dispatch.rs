#![allow(
    clippy::expect_used,
    clippy::panic,
    clippy::unwrap_used,
    clippy::string_slice
)]
//! The in-process outbox dispatcher running in the worker process, driven
//! end-to-end with the handoff 04a pilot message: business transaction ->
//! outbox row -> claim -> typed handler -> committed durable job enqueue ->
//! receipt -> delivered, then a responsive drain on shutdown. Skipped unless
//! `DATABASE_URL` points at a reachable PostgreSQL.

use std::{sync::Arc, time::Duration};

use deepref_app::outbox::{insert_outbox_event, pilot_outbox_registry};
use deepref_application::outbox::{MetricsRecomputeMessage, NewOutboxEvent};
use deepref_domain::events::{
    EntityType, EventEnvelope, MetricsRecomputeRequested, SUBJECT_METRICS_RECOMPUTE_REQUESTED,
};
use deepref_worker::{outbox_dispatch, shutdown::on_signal};
use sqlx::PgPool;
use tokio::sync::watch;
use uuid::Uuid;

async fn database() -> Option<PgPool> {
    deepref_app::test_isolation::isolated_pool_from_env("DATABASE_URL", 8).await
}

#[tokio::test]
async fn the_dispatcher_loop_delivers_the_pilot_and_drains_on_shutdown() {
    let Some(pool) = database().await else {
        return;
    };
    let project_id = Uuid::new_v4();
    sqlx::query("INSERT INTO projects (id,name) VALUES ($1,'outbox dispatch fixture')")
        .bind(project_id)
        .execute(&pool)
        .await
        .expect("project fixture inserts");

    let registry = Arc::new(pilot_outbox_registry(&pool).expect("pilot contract registers"));
    let (stop, signal) = watch::channel(false);
    let loop_task = tokio::spawn(outbox_dispatch::run(
        pool.clone(),
        registry,
        "test-outbox-dispatcher".to_owned(),
        outbox_dispatch::OutboxDispatchConfig {
            batch: 8,
            concurrency: 2,
            claim_lease: Duration::from_secs(30),
            poll_interval: Duration::from_millis(50),
            drain_deadline: Duration::from_secs(5),
        },
        on_signal(signal),
    ));

    let envelope: MetricsRecomputeMessage = EventEnvelope::v1(
        SUBJECT_METRICS_RECOMPUTE_REQUESTED,
        "outbox-dispatch-test",
        EntityType::Project,
        format!("project:{project_id}"),
        1,
        Uuid::new_v4(),
        None,
        MetricsRecomputeRequested {
            project_id,
            ingestion_id: None,
        },
    );
    let event = NewOutboxEvent::for_message(envelope.event_id, &envelope).expect("serializes");
    let mut tx = pool.begin().await.expect("begin");
    insert_outbox_event(&mut tx, &event).await.expect("insert");
    tx.commit().await.expect("commit");

    let dedupe_key = format!("outbox-metrics-recompute:{}", event.event_id);
    let mut jobs = 0;
    for _ in 0..200 {
        jobs = sqlx::query_scalar::<_, i64>("SELECT count(*) FROM jobs WHERE dedupe_key=$1")
            .bind(&dedupe_key)
            .fetch_one(&pool)
            .await
            .expect("job count");
        if jobs > 0 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    assert_eq!(
        jobs, 1,
        "the pilot consumer committed its durable job enqueue"
    );

    let (state, delivered_at): (String, Option<chrono::DateTime<chrono::Utc>>) =
        sqlx::query_as("SELECT delivery_state, delivered_at FROM outbox_events WHERE event_id=$1")
            .bind(event.event_id)
            .fetch_one(&pool)
            .await
            .expect("event row");
    assert_eq!(state, "delivered");
    assert!(
        delivered_at.is_some(),
        "delivered means every receipt committed"
    );

    // Shutdown drains in-flight work and returns without waiting for the poll
    // interval's worst case.
    stop.send_replace(true);
    let drained = tokio::time::timeout(Duration::from_secs(10), loop_task)
        .await
        .expect("the dispatcher loop drains promptly")
        .expect("the dispatcher loop does not panic");
    assert!(drained.is_ok(), "clean drain: {drained:?}");
}
