#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
//! Real broker + disposable PostgreSQL contracts; strict CI runner never skips.
use anyhow::Result;
use deepref_app::{
    PostgresDurableExecutionStore, enqueue_job_pool, scoped_job, work_publication as publication,
};
use deepref_application::{
    jobs::{DurableExecutionStore, JobScope, WorkRequest, WorkTransport},
    outbox::{
        NewOutboxEvent, OutboxHandlerError, OutboxMessage, OutboxRegistry, TypedOutboxHandler,
    },
    work_delivery::EventEnvelope,
};
use deepref_config::{Environment, NatsConfig};
use deepref_domain::{JobKind, WorkloadClass};
use deepref_integrations::jetstream::JetStream;
use deepref_worker::{
    ClaimLoopConfig,
    delivery::DeliveryAction,
    jetstream_transport::{JetStreamWorkTransport, consume_event, publication_tick},
    run_claim_loop,
};
use serde::{Deserialize, Serialize};
use sqlx::{PgPool, Row};
use std::{
    collections::HashMap,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};
use tokio::sync::oneshot;
use uuid::Uuid;

async fn fixture() -> Option<(PgPool, JetStream, NatsConfig)> {
    let url = match std::env::var("NATS_TEST_URL") {
        Ok(url) => url,
        Err(_) => {
            assert_ne!(
                std::env::var("DEEPREF_REQUIRE_NATS").as_deref(),
                Ok("1"),
                "strict NATS test requires NATS_TEST_URL"
            );
            return None;
        }
    };
    let pool = deepref_app::test_isolation::isolated_pool_from_env("DATABASE_URL", 8).await?;
    let identity = Uuid::new_v4().simple().to_string();
    let config = NatsConfig::from_source(Environment::Development, &|name| match name {
        "NATS_URL" => Some(url.clone()),
        "NATS_MAX_BYTES" => Some("1048576".into()),
        "NATS_STREAM" => Some(format!("TEST_{identity}")),
        "NATS_SUBJECT_PREFIX" => Some(format!("test.{identity}")),
        "NATS_ACK_WAIT_SECS" => Some("3".into()),
        "NATS_REQUEST_TIMEOUT_SECS" => Some("1".into()),
        "NATS_REPAIR_AFTER_SECS" => Some("15".into()),
        "NATS_MAX_AGE_SECS" => Some("60".into()),
        _ => None,
    })
    .unwrap();
    let broker = JetStream::connect(config.clone())
        .await
        .expect("real NATS JetStream available");
    Some((pool, broker, config))
}
async fn enqueue(pool: &PgPool, kind: JobKind) -> Result<Uuid> {
    let id = Uuid::new_v4();
    enqueue_job_pool(
        pool,
        &scoped_job(
            id,
            JobScope::Platform,
            kind,
            serde_json::json!({}),
            id.to_string(),
        ),
    )
    .await?;
    Ok(id)
}
fn request(pool: WorkloadClass) -> WorkRequest<'static> {
    WorkRequest {
        owner: "contract-worker",
        lease: Duration::from_secs(2),
        pool,
        max_jobs: 4,
    }
}
async fn publish(pool: &PgPool, broker: &JetStream) -> Result<()> {
    publication_tick(pool, broker, "relay", Duration::from_secs(10)).await?;
    Ok(())
}
async fn attempts(pool: &PgPool, id: Uuid) -> i32 {
    sqlx::query_scalar("SELECT attempts FROM jobs WHERE id=$1")
        .bind(id)
        .fetch_one(pool)
        .await
        .unwrap()
}
async fn envelope(pool: &PgPool, id: Uuid) -> EventEnvelope {
    let row=sqlx::query("SELECT * FROM outbox_events WHERE event_id=job_wakeup_event_id($1,(SELECT wakeup_generation FROM jobs WHERE id=$1))").bind(id).fetch_one(pool).await.unwrap();
    EventEnvelope {
        envelope_version: 1,
        event_id: row.get("event_id"),
        publication_epoch: row.get("publication_epoch"),
        message_type: row.get("message_type"),
        message_version: row.get::<i32, _>("message_version") as u32,
        occurred_at: row.get("created_at"),
        trace_id: None,
        payload: row.get("payload"),
    }
}

#[derive(Clone, Serialize, Deserialize)]
struct Pilot {
    trace_id: String,
}
impl OutboxMessage for Pilot {
    const TYPE: &'static str = "test.pilot";
    const VERSION: u32 = 1;
    const CONSUMERS: &'static [&'static str] = &["alpha", "beta"];
}

#[tokio::test]
async fn pilot_atomicity_fanout_receipts_and_explicit_replay() -> Result<()> {
    let Some((pool, broker, _)) = fixture().await else {
        return Ok(());
    };
    sqlx::query(
        "CREATE TABLE pilot_effects(event_id uuid,consumer text,PRIMARY KEY(event_id,consumer))",
    )
    .execute(&pool)
    .await?;
    let event = NewOutboxEvent::for_message(
        Uuid::new_v4(),
        &Pilot {
            trace_id: "correlation-123".into(),
        },
    )?;
    let mut tx = pool.begin().await?;
    deepref_app::outbox::insert_outbox_event(&mut tx, &event).await?;
    tx.rollback().await?;
    assert!(
        publication::claim_publications(&pool, "rollback", Duration::from_secs(2), 4)
            .await?
            .is_empty()
    );
    let mut tx = pool.begin().await?;
    deepref_app::outbox::insert_outbox_event(&mut tx, &event).await?;
    tx.commit().await?;
    let calls = Arc::new(AtomicUsize::new(0));
    let beta = Arc::new(AtomicUsize::new(0));
    let mut handlers = Vec::new();
    for consumer in ["alpha", "beta"] {
        let pool = pool.clone();
        let calls = Arc::clone(&calls);
        let beta = Arc::clone(&beta);
        let handler: TypedOutboxHandler<Pilot> = Arc::new(move |message, context| {
            let pool = pool.clone();
            let calls = Arc::clone(&calls);
            let beta = Arc::clone(&beta);
            Box::pin(async move {
                assert_eq!(message.trace_id, "correlation-123");
                if consumer == "beta" && beta.fetch_add(1, Ordering::SeqCst) == 0 {
                    return Err(OutboxHandlerError::Transient("first beta delivery".into()));
                }
                calls.fetch_add(1, Ordering::SeqCst);
                let result: Result<()> = async {
                    let mut tx = pool.begin().await?;
                    sqlx::query("INSERT INTO pilot_effects VALUES($1,$2) ON CONFLICT DO NOTHING")
                        .bind(context.event_id)
                        .bind(&context.consumer)
                        .execute(&mut *tx)
                        .await?;
                    deepref_app::outbox::record_outbox_receipt(&mut tx, &context).await?;
                    tx.commit().await?;
                    Ok(())
                }
                .await;
                result.map_err(|error| OutboxHandlerError::Transient(error.to_string()))
            })
        });
        handlers.push((consumer, handler));
    }
    let mut registry = OutboxRegistry::new();
    registry.register::<Pilot>(&handlers)?;
    let consumer = broker.consumer("pilot", None).await?;
    publish(&pool, &broker).await?;
    assert_eq!(
        publication::event_delivery_state(&pool, event.event_id)
            .await?
            .as_deref(),
        Some("pending")
    );
    let messages = consumer.fetch(4).await?;
    assert_eq!(messages.len(), 1);
    assert_eq!(
        messages[0].envelope()?.trace_id.as_deref(),
        Some("correlation-123")
    );
    consume_event(
        &pool,
        &broker,
        &registry,
        "consumer",
        Duration::from_secs(2),
        messages.into_iter().next().unwrap(),
    )
    .await?;
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    // Ordinary publication excludes a published row with a missing recipient.
    assert!(
        publication::claim_publications(&pool, "ordinary", Duration::from_secs(2), 4)
            .await?
            .is_empty()
    );
    sqlx::query("UPDATE outbox_events SET next_attempt_at=now() WHERE event_id=$1")
        .bind(event.event_id)
        .execute(&pool)
        .await?;
    sqlx::query("UPDATE outbox_receipts SET next_attempt_at=now() WHERE event_id=$1")
        .bind(event.event_id)
        .execute(&pool)
        .await?;
    assert!(publication::replay_publication(&pool, event.event_id).await?);
    publish(&pool, &broker).await?;
    let messages = consumer.fetch(4).await?;
    assert_eq!(messages.len(), 1);
    consume_event(
        &pool,
        &broker,
        &registry,
        "consumer",
        Duration::from_secs(2),
        messages.into_iter().next().unwrap(),
    )
    .await?;
    assert_eq!(
        calls.load(Ordering::SeqCst),
        2,
        "alpha receipt prevents repeated effect"
    );
    assert_eq!(
        publication::event_delivery_state(&pool, event.event_id)
            .await?
            .as_deref(),
        Some("delivered")
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM pilot_effects")
            .fetch_one(&pool)
            .await?,
        2
    );
    Ok(())
}

#[tokio::test]
async fn ambiguous_publish_two_relays_and_commit_before_ack_replay() -> Result<()> {
    let Some((pool, broker, _)) = fixture().await else {
        return Ok(());
    };
    let id = enqueue(&pool, JobKind::RetrieveDocument).await?;
    let consumer = broker
        .consumer("work-documents-v1", Some(WorkloadClass::Documents))
        .await?;
    let (a, b) = tokio::join!(
        publication::claim_publications(&pool, "a", Duration::from_secs(2), 1),
        publication::claim_publications(&pool, "b", Duration::from_secs(2), 1)
    );
    let mut events = a?;
    events.extend(b?);
    assert_eq!(events.len(), 1);
    let accepted = &events[0];
    broker.publish(&accepted.envelope).await?;
    // Acceptance with no DB confirmation models the ambiguous timeout/crash.
    sqlx::query("UPDATE outbox_events SET publication_until=now()-interval '1 second'")
        .execute(&pool)
        .await?;
    let retry =
        publication::claim_publications(&pool, "recovery", Duration::from_secs(2), 1).await?;
    assert_eq!(retry.len(), 1);
    broker.publish(&retry[0].envelope).await?;
    assert!(publication::settle_publication(&pool, &retry[0], None).await?);
    assert!(
        !publication::settle_publication(&pool, accepted, None).await?,
        "stale relay cannot settle"
    );
    assert_eq!(
        consumer.visibility().await?.0,
        1,
        "stable event/epoch deduplicates ambiguous publish"
    );
    let lost_before_claim = consumer.fetch(1).await?;
    assert_eq!(lost_before_claim.len(), 1);
    drop(lost_before_claim); // Process dies before a database claim.
    tokio::time::sleep(Duration::from_millis(3300)).await;
    let transport =
        JetStreamWorkTransport::new(pool.clone(), broker.clone(), &[WorkloadClass::Documents])
            .await?;
    let jobs = transport
        .next_work(request(WorkloadClass::Documents))
        .await?;
    assert_eq!(jobs.len(), 1);
    let lease = jobs[0].lease("contract-worker");
    let store = PostgresDurableExecutionStore { pool: pool.clone() };
    assert!(store.complete(&lease).await?);
    assert_eq!(
        publication::event_delivery_state(&pool, retry[0].envelope.event_id)
            .await?
            .as_deref(),
        Some("delivered")
    );
    drop(transport); // Crash after DB commit and before ACK.
    tokio::time::sleep(Duration::from_millis(3300)).await;
    let restarted =
        JetStreamWorkTransport::new(pool.clone(), broker.clone(), &[WorkloadClass::Documents])
            .await?;
    assert!(
        restarted
            .next_work(request(WorkloadClass::Documents))
            .await?
            .is_empty()
    );
    assert_eq!(
        attempts(&pool, id).await,
        1,
        "redelivery spends no business attempt"
    );
    Ok(())
}

#[tokio::test]
async fn slow_handler_progress_is_independent_of_database_lease_and_pools() -> Result<()> {
    let Some((pool, broker, _)) = fixture().await else {
        return Ok(());
    };
    let document = enqueue(&pool, JobKind::RetrieveDocument).await?;
    let ai = enqueue(&pool, JobKind::AssistantAgentRun).await?;
    publish(&pool, &broker).await?;
    let observer = broker
        .consumer("work-documents-v1", Some(WorkloadClass::Documents))
        .await?;
    let transport =
        JetStreamWorkTransport::new(pool.clone(), broker.clone(), &[WorkloadClass::Documents])
            .await?;
    let (done, wait) = oneshot::channel();
    let done = Arc::new(std::sync::Mutex::new(Some(done)));
    let handler = Arc::new(move |_: deepref_application::jobs::ClaimedJob, _| {
        let done = Arc::clone(&done);
        Box::pin(async move {
            tokio::time::sleep(Duration::from_millis(4300)).await;
            if let Some(done) = done.lock().unwrap().take() {
                let _ = done.send(());
            }
            Ok(DeliveryAction::Ack)
        })
            as std::pin::Pin<Box<dyn std::future::Future<Output = Result<DeliveryAction>> + Send>>
    });
    let (stop, stopped) = oneshot::channel();
    let worker = tokio::spawn(run_claim_loop(
        transport,
        PostgresDurableExecutionStore { pool: pool.clone() },
        "slow".into(),
        ClaimLoopConfig {
            pools: vec![WorkloadClass::Documents],
            pool_concurrency: HashMap::from([(WorkloadClass::Documents, 1)]),
            max_batch: 4,
            claim_lease: Duration::from_millis(600),
            drain_deadline: Duration::from_secs(3),
        },
        handler,
        async {
            let _ = stopped.await;
        },
    ));
    tokio::time::sleep(Duration::from_millis(3500)).await;
    let alive: bool =
        sqlx::query_scalar("SELECT state='running' AND leased_until>now() FROM jobs WHERE id=$1")
            .bind(document)
            .fetch_one(&pool)
            .await?;
    assert!(alive, "DB lease renews while handler exceeds ACK wait");
    assert!(
        observer.fetch(1).await?.is_empty(),
        "transport progress prevents redelivery after ACK wait"
    );
    assert_eq!(
        attempts(&pool, ai).await,
        0,
        "document saturation never claims AI work"
    );
    let _ = stop.send(()); // Signal while the slow handler is still in flight.
    wait.await?;
    worker.await??;
    assert_eq!(attempts(&pool, document).await, 1);
    assert_eq!(
        observer.visibility().await?.1,
        0,
        "ACK follows committed lifecycle/receipt"
    );
    // Transport progress alone cannot extend a consequential database fence.
    let second = enqueue(&pool, JobKind::RetrieveDocument).await?;
    publish(&pool, &broker).await?;
    let hints =
        JetStreamWorkTransport::new(pool.clone(), broker.clone(), &[WorkloadClass::Documents])
            .await?;
    let mut short = request(WorkloadClass::Documents);
    short.lease = Duration::from_millis(100);
    let jobs = hints.next_work(short).await?;
    assert_eq!(jobs.len(), 1);
    assert_eq!(jobs[0].id, second);
    tokio::time::sleep(Duration::from_millis(250)).await;
    let store = PostgresDurableExecutionStore { pool: pool.clone() };
    assert!(
        !store.complete(&jobs[0].lease("contract-worker")).await?,
        "broker progress grants no DB authority"
    );
    assert_eq!(
        observer.visibility().await?.1,
        1,
        "message still has transport progress ownership"
    );
    let mut expired = envelope(&pool, second).await;
    expired.publication_epoch = 99;
    broker.publish(&expired).await?;
    let recovery =
        JetStreamWorkTransport::new(pool.clone(), broker.clone(), &[WorkloadClass::Documents])
            .await?;
    assert!(
        recovery
            .next_work(request(WorkloadClass::Documents))
            .await?
            .is_empty()
    );
    let recovered: bool =
        sqlx::query_scalar("SELECT state='queued' AND available_at>now() FROM jobs WHERE id=$1")
            .bind(second)
            .fetch_one(&pool)
            .await?;
    assert!(
        recovered,
        "expired-lease notification ACK follows committed targeted recovery"
    );
    assert_eq!(attempts(&pool, second).await, 1);
    Ok(())
}

#[tokio::test]
async fn early_due_schedule_crash_recovery_and_stale_generation() -> Result<()> {
    let Some((pool, broker, _)) = fixture().await else {
        return Ok(());
    };
    let id = enqueue(&pool, JobKind::RetrieveDocument).await?;
    let old = envelope(&pool, id).await;
    sqlx::query("UPDATE jobs SET available_at=now()+interval '2 seconds' WHERE id=$1")
        .bind(id)
        .execute(&pool)
        .await?;
    let generation: i64 = sqlx::query_scalar("SELECT wakeup_generation FROM jobs WHERE id=$1")
        .bind(id)
        .fetch_one(&pool)
        .await?;
    // Simulate an early current-generation notification. It must persist a
    // due obligation before ACK, then normal due publication repairs its hint.
    sqlx::query("INSERT INTO outbox_events(event_id,message_type,message_version,payload,expected_consumers) VALUES(job_wakeup_event_id($1,$2),'deepref.work',1,jsonb_build_object('job_id',$1::uuid,'generation',$2::bigint,'workload_class','documents'),ARRAY['durable-work'])")
        .bind(id).bind(generation).execute(&pool).await?;
    let early = envelope(&pool, id).await;
    broker.publish(&early).await?;
    let transport =
        JetStreamWorkTransport::new(pool.clone(), broker.clone(), &[WorkloadClass::Documents])
            .await?;
    assert!(
        transport
            .next_work(request(WorkloadClass::Documents))
            .await?
            .is_empty()
    );
    assert_eq!(attempts(&pool, id).await, 0);
    assert_eq!(publication::publish_due_work(&pool, 4).await?, 0);
    tokio::time::sleep(Duration::from_millis(2100)).await;
    let (left, right) = tokio::join!(
        publication::publish_due_work(&pool, 4),
        publication::publish_due_work(&pool, 4)
    );
    assert_eq!(
        left? + right?,
        1,
        "concurrent due publishers emit one persisted generation"
    );
    publish(&pool, &broker).await?;
    let jobs = transport
        .next_work(request(WorkloadClass::Documents))
        .await?;
    assert_eq!(jobs.len(), 1);
    let lease = jobs[0].lease("contract-worker");
    drop(transport); // death after accepted claim, before side effect
    sqlx::query("UPDATE jobs SET leased_until=now()-interval '1 second' WHERE id=$1")
        .bind(id)
        .execute(&pool)
        .await?;
    let store = PostgresDurableExecutionStore { pool: pool.clone() };
    assert_eq!(store.recover_expired().await?, 1);
    assert!(
        !store.complete(&lease).await?,
        "lost fence cannot settle result"
    );
    assert_eq!(
        publication::publish_due_work(&pool, 4).await?,
        0,
        "normal retry waits for persisted due time"
    );
    sqlx::query("UPDATE jobs SET available_at=now() WHERE id=$1")
        .bind(id)
        .execute(&pool)
        .await?;
    let mut stale = old;
    stale.publication_epoch = 99;
    broker.publish(&stale).await?;
    publish(&pool, &broker).await?;
    let restarted =
        JetStreamWorkTransport::new(pool.clone(), broker.clone(), &[WorkloadClass::Documents])
            .await?;
    let jobs = restarted
        .next_work(request(WorkloadClass::Documents))
        .await?;
    assert_eq!(jobs.len(), 1);
    assert_eq!(
        jobs[0].attempts, 2,
        "stale delivery cannot multiply attempts"
    );
    assert!(store.complete(&jobs[0].lease("contract-worker")).await?);
    restarted.settled(&jobs[0].lease("contract-worker")).await?;
    Ok(())
}

#[tokio::test]
async fn n_minus_one_writers_and_unknown_kinds_retain_work_and_dlq_is_db_first() -> Result<()> {
    let Some((pool, broker, _)) = fixture().await else {
        return Ok(());
    };
    let id = Uuid::new_v4();
    // Shape of an old writer: no wakeup columns, no explicit workload class.
    sqlx::query("INSERT INTO jobs(id,scope,kind,payload,dedupe_key) VALUES($1,'platform','retrieve_document','{}',$1::text)")
        .bind(id).execute(&pool).await?;
    let unknown = Uuid::new_v4();
    sqlx::query("INSERT INTO jobs(id,scope,kind,workload_class,payload,dedupe_key,state) VALUES($1,'platform','future_document_kind','documents','{}',$1::text,'capability_wait')")
        .bind(unknown).execute(&pool).await?;
    publish(&pool, &broker).await?;
    let transport =
        JetStreamWorkTransport::new(pool.clone(), broker.clone(), &[WorkloadClass::Documents])
            .await?;
    let jobs = transport
        .next_work(request(WorkloadClass::Documents))
        .await?;
    assert_eq!(jobs.len(), 1);
    assert_eq!(jobs[0].id, id);
    assert_eq!(attempts(&pool, unknown).await, 0);
    let unknown_event = envelope(&pool, unknown).await;
    // An independent durable consumer actually exhausts transport delivery.
    let budget = broker
        .consumer("budget", Some(WorkloadClass::Documents))
        .await?;
    for count in 1..=5 {
        let mut messages = budget.fetch(8).await?;
        let selected = messages
            .iter()
            .position(|delivery| delivery.envelope().unwrap().event_id == unknown_event.event_id)
            .expect("unknown wakeup present");
        let message = messages.swap_remove(selected);
        for other in messages {
            other.ack().await?;
        }
        assert_eq!(message.deliveries()?, count);
        if count < 5 {
            message.retry(Duration::ZERO).await?;
            tokio::time::sleep(Duration::from_millis(30)).await;
        } else {
            consume_event(
                &pool,
                &broker,
                &OutboxRegistry::new(),
                "budget",
                Duration::from_secs(2),
                message,
            )
            .await?;
        }
    }
    assert_eq!(attempts(&pool, unknown).await, 0);
    sqlx::query("UPDATE outbox_events SET published_at=now()-interval '1 hour' WHERE event_id=$1")
        .bind(unknown_event.event_id)
        .execute(&pool)
        .await?;
    assert_eq!(
        publication::repair_stranded_work(&pool, Duration::from_secs(15), 8).await?,
        1
    );
    assert!(
        publication::claim_publications(&pool, "repair", Duration::from_secs(2), 8)
            .await?
            .iter()
            .any(|p| p.envelope.event_id == unknown_event.event_id
                && p.envelope.publication_epoch == 1)
    );
    // Foreign/invalid wire versions are quarantined in DB then copied to DLQ.
    let mut invalid = unknown_event;
    invalid.envelope_version = 42;
    invalid.message_type = "test.invalid".into();
    invalid.publication_epoch = 100;
    // Adapter refuses invalid work; generic route can carry future contract versions.
    broker.publish(&invalid).await?;
    let consumer = broker.consumer("invalid", None).await?;
    let messages = consumer.fetch(4).await?;
    assert_eq!(messages.len(), 1);
    consume_event(
        &pool,
        &broker,
        &OutboxRegistry::new(),
        "invalid",
        Duration::from_secs(2),
        messages.into_iter().next().unwrap(),
    )
    .await?;
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM transport_dead_letters")
            .fetch_one(&pool)
            .await?,
        2
    );
    Ok(())
}

#[tokio::test]
async fn two_workers_race_duplicate_and_broker_outage_retains_postgres_authority() -> Result<()> {
    let Some((pool, broker, config)) = fixture().await else {
        return Ok(());
    };
    let id = enqueue(&pool, JobKind::RetrieveDocument).await?;
    let events =
        publication::claim_publications(&pool, "outage", Duration::from_secs(2), 4).await?;
    assert_eq!(events.len(), 1);
    // Failed broker publication is durable retry metadata, never delivery.
    assert!(publication::settle_publication(&pool, &events[0], Some("broker unavailable")).await?);
    assert_eq!(attempts(&pool, id).await, 0);
    assert_eq!(
        publication::event_delivery_state(&pool, events[0].envelope.event_id)
            .await?
            .as_deref(),
        Some("pending")
    );
    sqlx::query("UPDATE outbox_events SET publication_next_at=now()")
        .execute(&pool)
        .await?;
    let reconnected = JetStream::connect(config.clone()).await?;
    publish(&pool, &reconnected).await?;
    let mut duplicate = envelope(&pool, id).await;
    duplicate.publication_epoch = 77;
    broker.publish(&duplicate).await?;
    let first =
        JetStreamWorkTransport::new(pool.clone(), broker.clone(), &[WorkloadClass::Documents])
            .await?;
    let second =
        JetStreamWorkTransport::new(pool.clone(), broker.clone(), &[WorkloadClass::Documents])
            .await?;
    let (a, b) = tokio::join!(
        first.next_work(request(WorkloadClass::Documents)),
        second.next_work(request(WorkloadClass::Documents))
    );
    let mut jobs = a?;
    jobs.extend(b?);
    assert_eq!(jobs.len(), 1);
    assert_eq!(attempts(&pool, id).await, 1);
    // Configuration drift fails closed instead of altering live retention.
    let mut drift = config;
    drift.max_bytes *= 2;
    assert!(JetStream::connect(drift).await.is_err());
    Ok(())
}

#[tokio::test]
async fn dlq_waits_for_database_commit_and_does_not_ack_on_db_failure() -> Result<()> {
    let Some((pool, broker, _)) = fixture().await else {
        return Ok(());
    };
    let event = NewOutboxEvent::for_message(
        Uuid::new_v4(),
        &Pilot {
            trace_id: "poison".into(),
        },
    )?;
    let mut tx = pool.begin().await?;
    deepref_app::outbox::insert_outbox_event(&mut tx, &event).await?;
    tx.commit().await?;
    publish(&pool, &broker).await?;
    // Deliver a future envelope version through a separate retained publication.
    let invalid = EventEnvelope {
        envelope_version: 99,
        event_id: event.event_id,
        publication_epoch: 99,
        message_type: Pilot::TYPE.into(),
        message_version: 1,
        occurred_at: chrono::Utc::now(),
        trace_id: None,
        payload: event.payload,
    };
    broker.publish(&invalid).await?;
    let consumer = broker.consumer("db-failure", None).await?;
    let mut messages = consumer.fetch(4).await?;
    assert_eq!(messages.len(), 2);
    let bad_index = messages
        .iter()
        .position(|delivery| delivery.envelope().is_err())
        .unwrap();
    let bad = messages.swap_remove(bad_index);
    for good in messages {
        good.ack().await?;
    }
    let mut blocked = pool.begin().await?;
    sqlx::query("LOCK TABLE transport_dead_letters IN ACCESS EXCLUSIVE MODE")
        .execute(&mut *blocked)
        .await?;
    let registry = OutboxRegistry::new();
    assert!(
        tokio::time::timeout(
            Duration::from_millis(150),
            consume_event(
                &pool,
                &broker,
                &registry,
                "blocked",
                Duration::from_secs(2),
                bad
            )
        )
        .await
        .is_err()
    );
    assert_eq!(
        broker.dead_letter_count().await?,
        0,
        "DLQ cannot precede durable DB evidence"
    );
    assert_eq!(
        consumer.visibility().await?.1,
        1,
        "blocked DB write never terminally acknowledges"
    );
    blocked.rollback().await?;
    tokio::time::sleep(Duration::from_millis(3300)).await;
    let messages = consumer.fetch(4).await?;
    assert_eq!(messages.len(), 1);
    consume_event(
        &pool,
        &broker,
        &registry,
        "recovered",
        Duration::from_secs(2),
        messages.into_iter().next().unwrap(),
    )
    .await?;
    assert_eq!(broker.dead_letter_count().await?, 1);
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM transport_dead_letters")
            .fetch_one(&pool)
            .await?,
        1
    );
    assert_eq!(consumer.visibility().await?.1, 0);
    Ok(())
}

#[tokio::test]
async fn cancelled_durable_run_delayed_notification_never_spends_an_attempt() -> Result<()> {
    let Some((pool, broker, _)) = fixture().await else {
        return Ok(());
    };
    let project = Uuid::new_v4();
    let run = Uuid::new_v4();
    let step = Uuid::new_v4();
    let id = Uuid::new_v4();
    sqlx::query("INSERT INTO projects(id,name) VALUES($1,'cancelled wakeup')")
        .bind(project)
        .execute(&pool)
        .await?;
    sqlx::query("INSERT INTO durable_runs(id,project_id,kind,actor_kind,actor_id,idempotency_key,status) VALUES($1,$2,'workflow','system','test','test','cancelled')").bind(run).bind(project).execute(&pool).await?;
    sqlx::query("INSERT INTO durable_steps(id,project_id,run_id,handler_kind,step_key,status) VALUES($1,$2,$3,'workflow','cancelled','cancelled')").bind(step).bind(project).bind(run).execute(&pool).await?;
    enqueue_job_pool(
        &pool,
        &scoped_job(
            id,
            JobScope::Project(deepref_domain::ProjectId::new(project)),
            JobKind::DurableRunStep,
            serde_json::json!({"run_id":run,"step_id":step}),
            id.to_string(),
        ),
    )
    .await?;
    publish(&pool, &broker).await?;
    let transport =
        JetStreamWorkTransport::new(pool.clone(), broker.clone(), &[WorkloadClass::Workflow])
            .await?;
    assert!(
        transport
            .next_work(request(WorkloadClass::Workflow))
            .await?
            .is_empty()
    );
    assert_eq!(attempts(&pool, id).await, 0);
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM durable_attempts WHERE run_id=$1")
            .bind(run)
            .fetch_one(&pool)
            .await?,
        0
    );
    assert_eq!(
        publication::event_delivery_state(&pool, envelope(&pool, id).await.event_id)
            .await?
            .as_deref(),
        Some("delivered")
    );
    Ok(())
}

#[tokio::test]
async fn slow_generic_consumer_does_not_block_relay_and_drains_on_shutdown() -> Result<()> {
    let Some((pool, broker, config)) = fixture().await else {
        return Ok(());
    };
    let pilot = NewOutboxEvent::for_message(
        Uuid::new_v4(),
        &Pilot {
            trace_id: "slow-pilot".into(),
        },
    )?;
    let mut tx = pool.begin().await?;
    deepref_app::outbox::insert_outbox_event(&mut tx, &pilot).await?;
    tx.commit().await?;
    let started = Arc::new(tokio::sync::Notify::new());
    let (release, released) = tokio::sync::watch::channel(false);
    let handler: TypedOutboxHandler<Pilot> = {
        let pool = pool.clone();
        let started = Arc::clone(&started);
        Arc::new(move |_, context| {
            let pool = pool.clone();
            let started = Arc::clone(&started);
            let mut released = released.clone();
            Box::pin(async move {
                started.notify_one();
                while !*released.borrow_and_update() {
                    released
                        .changed()
                        .await
                        .map_err(|error| OutboxHandlerError::Transient(error.to_string()))?;
                }
                let result: Result<()> = async {
                    let mut tx = pool.begin().await?;
                    deepref_app::outbox::record_outbox_receipt(&mut tx, &context).await?;
                    tx.commit().await?;
                    Ok(())
                }
                .await;
                result.map_err(|error| OutboxHandlerError::Transient(error.to_string()))
            })
        })
    };
    let mut registry = OutboxRegistry::new();
    registry.register::<Pilot>(&[("alpha", Arc::clone(&handler)), ("beta", handler)])?;
    let (stop, stopped) = tokio::sync::watch::channel(false);
    let run = tokio::spawn(deepref_worker::jetstream_transport::run_publication(
        pool.clone(),
        broker,
        Arc::new(registry),
        "parallel-relay".into(),
        config,
        Duration::from_millis(600),
        Duration::from_secs(3),
        stopped,
    ));
    tokio::time::timeout(Duration::from_secs(3), started.notified()).await?;
    let document = enqueue(&pool, JobKind::RetrieveDocument).await?;
    tokio::time::timeout(Duration::from_secs(3),async {
        loop {
            let published:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM outbox_events WHERE event_id=job_wakeup_event_id($1,1) AND published_at IS NOT NULL)").bind(document).fetch_one(&pool).await?;
            if published {break;}
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        Ok::<(),anyhow::Error>(())
    }).await??;
    stop.send_replace(true);
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(
        !run.is_finished(),
        "shutdown drains the in-flight generic handler"
    );
    release.send_replace(true);
    run.await??;
    assert_eq!(
        publication::event_delivery_state(&pool, pilot.event_id)
            .await?
            .as_deref(),
        Some("delivered")
    );
    Ok(())
}
