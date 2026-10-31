#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::panic,
    clippy::panic_in_result_fn
)]
//! Real broker + disposable PostgreSQL contracts; strict CI runner never skips.
use anyhow::{Context, Result};
use deepref_app::{
    PostgresDurableExecutionStore, enqueue_job_pool, scoped_job, work_publication as publication,
};
use deepref_application::{
    jobs::{ClaimedJob, IntakeControl, JobLease, JobScope, WorkRequest, WorkTransport},
    outbox::{NewOutboxEvent, OutboxMessage, OutboxRegistry, TypedOutboxHandler},
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

#[derive(Clone, Default)]
struct LogCapture(Arc<std::sync::Mutex<Vec<u8>>>);
impl std::io::Write for LogCapture {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for LogCapture {
    type Writer = Self;
    fn make_writer(&'a self) -> Self::Writer {
        self.clone()
    }
}

// Minimal core protocol client keeps SDK types inside the integrations crate.
async fn nats_request(
    config: &NatsConfig,
    subject: &str,
    body: serde_json::Value,
) -> Result<serde_json::Value> {
    use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
    let url = url::Url::parse(&config.url)?;
    let mut socket =
        tokio::net::TcpStream::connect((url.host_str().unwrap(), url.port().unwrap_or(4222)))
            .await?;
    let inbox = format!("_INBOX.{}", Uuid::new_v4().simple());
    let bytes = serde_json::to_vec(&body)?;
    socket
        .write_all(
            format!(
                "CONNECT {{}}\r\nSUB {inbox} 1\r\nPUB {subject} {inbox} {}\r\n",
                bytes.len()
            )
            .as_bytes(),
        )
        .await?;
    socket.write_all(&bytes).await?;
    socket.write_all(b"\r\n").await?;
    let mut socket = BufReader::new(socket);
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            let mut line = String::new();
            socket.read_line(&mut line).await?;
            if line.starts_with("MSG ") {
                let len: usize = line.split_whitespace().last().unwrap().parse()?;
                let mut data = vec![0; len + 2];
                socket.read_exact(&mut data).await?;
                return Ok::<_, anyhow::Error>(serde_json::from_slice(&data[..len])?);
            }
            if line.starts_with("PING") {
                socket.get_mut().write_all(b"PONG\r\n").await?;
            }
            anyhow::ensure!(!line.is_empty(), "NATS connection ended");
        }
    })
    .await?
}

#[tokio::test]
async fn deduplicated_old_writer_never_emits_phantom_wakeups() -> Result<()> {
    let Some((pool, _, _)) = fixture().await else {
        return Ok(());
    };
    let a = enqueue(&pool, JobKind::RetrieveDocument).await?;
    let key = a.to_string();
    let b = Uuid::new_v4();
    let c = Uuid::new_v4();
    let mut job = scoped_job(
        b,
        JobScope::Platform,
        JobKind::RetrieveDocument,
        serde_json::json!({}),
        key.clone(),
    );
    assert_eq!(enqueue_job_pool(&pool, &job).await?, a);
    sqlx::query("INSERT INTO jobs(id,scope,kind,payload,dedupe_key) VALUES($1,'platform','retrieve_document','{}',$2) ON CONFLICT DO NOTHING")
        .bind(c).bind(&key).execute(&pool).await?;
    job.id = Uuid::new_v4();
    let mut replica = job.clone();
    replica.id = Uuid::new_v4();
    let (left, right) = tokio::join!(
        enqueue_job_pool(&pool, &job),
        enqueue_job_pool(&pool, &replica)
    );
    assert_eq!(left?, a);
    assert_eq!(right?, a);
    let orphans:i64=sqlx::query_scalar("SELECT count(*) FROM outbox_events e WHERE message_type='deepref.work' AND NOT EXISTS(SELECT 1 FROM jobs j WHERE j.id=(e.payload->>'job_id')::uuid)").fetch_one(&pool).await?;
    assert_eq!(orphans, 0, "successful INSERT/UPDATE alone may publish");
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM outbox_events WHERE message_type='deepref.work'"
        )
        .fetch_one(&pool)
        .await?,
        1
    );
    let old = Uuid::new_v4();
    sqlx::query("INSERT INTO jobs(id,scope,kind,payload,dedupe_key) VALUES($1,'platform','retrieve_document','{}',$1::text)").bind(old).execute(&pool).await?;
    assert_eq!(envelope(&pool, old).await.work()?.job_id, old);
    Ok(())
}

#[derive(Clone, Serialize, Deserialize)]
struct LastAttempt;
impl OutboxMessage for LastAttempt {
    const TYPE: &'static str = "test.last-attempt";
    const VERSION: u32 = 1;
    const CONSUMERS: &'static [&'static str] = &["one"];
}
#[tokio::test]
async fn abandoned_final_business_attempt_is_not_invoked_again() -> Result<()> {
    let Some((pool, broker, _)) = fixture().await else {
        return Ok(());
    };
    let calls = Arc::new(AtomicUsize::new(0));
    let called = Arc::clone(&calls);
    let handler: TypedOutboxHandler<LastAttempt> = Arc::new(move |_, _| {
        called.fetch_add(1, Ordering::SeqCst);
        Box::pin(async { Ok(()) })
    });
    let mut registry = OutboxRegistry::new();
    registry.register::<LastAttempt>(&[("one", handler)])?;
    let mut event = NewOutboxEvent::for_message(Uuid::new_v4(), &LastAttempt)?;
    event.max_attempts = 1;
    let mut tx = pool.begin().await?;
    deepref_app::outbox::insert_outbox_event(&mut tx, &event).await?;
    tx.commit().await?;
    let e = publication::claim_publications(&pool, "relay", Duration::from_secs(2), 1)
        .await?
        .pop()
        .unwrap()
        .envelope;
    assert!(
        publication::claim_event_delivery(&pool, &registry, &e, "crashed", Duration::from_secs(1))
            .await?
            .is_some()
    );
    sqlx::query("UPDATE outbox_events SET lease_expires_at=clock_timestamp()-interval '1 second' WHERE event_id=$1").bind(e.event_id).execute(&pool).await?;
    broker.publish(&e).await?;
    let consumer = broker.consumer("last", None).await?;
    consume_event(
        &pool,
        &broker,
        &registry,
        "redelivered",
        Duration::from_secs(2),
        consumer.fetch(1).await?.pop().unwrap(),
    )
    .await?;
    assert_eq!(
        calls.load(Ordering::SeqCst),
        0,
        "crashed attempt consumed final business budget"
    );
    assert_eq!(
        publication::event_delivery_state(&pool, e.event_id)
            .await?
            .as_deref(),
        Some("quarantined")
    );
    assert_eq!(
        sqlx::query_scalar::<_, i32>("SELECT attempts FROM outbox_receipts WHERE event_id=$1")
            .bind(e.event_id)
            .fetch_one(&pool)
            .await?,
        1
    );
    Ok(())
}

#[tokio::test]
async fn malformed_private_content_never_enters_diagnostics() -> Result<()> {
    let Some((pool, broker, config)) = fixture().await else {
        return Ok(());
    };
    let marker = "private-marker-should-never-escape";
    let logs = LogCapture::default();
    let subscriber = tracing_subscriber::fmt()
        .with_writer(logs.clone())
        .without_time()
        .with_ansi(false)
        .finish();
    let _logging = tracing::subscriber::set_default(subscriber);
    let good = serde_json::json!({"envelope_version":1,"event_id":Uuid::new_v4(),"publication_epoch":0,"message_type":"test.invalid","message_version":1,"occurred_at":chrono::Utc::now(),"trace_id":null,"payload":{}});
    for unknown in [false, true] {
        let mut bad = good.clone();
        if unknown {
            bad[marker] = serde_json::json!(1)
        } else {
            bad["envelope_version"] = serde_json::json!(marker)
        }
        nats_request(&config, &format!("{}.events", config.subject_prefix), bad).await?;
    }
    let consumer = broker.consumer("private", None).await?;
    for delivery in consumer.fetch(4).await? {
        consume_event(
            &pool,
            &broker,
            &OutboxRegistry::new(),
            "private",
            Duration::from_secs(1),
            delivery,
        )
        .await?;
    }
    let reasons: Vec<String> = sqlx::query_scalar("SELECT reason FROM transport_dead_letters")
        .fetch_all(&pool)
        .await?;
    assert_eq!(reasons.len(), 2);
    assert!(
        reasons.iter().all(|r| !r.contains(marker)),
        "DB contains private parser data: {reasons:?}"
    );
    let last = nats_request(
        &config,
        &format!("$JS.API.STREAM.MSG.GET.{}_DLQ", config.stream),
        serde_json::json!({"last_by_subj":format!("{}.dead.outbox",config.subject_prefix)}),
    )
    .await?;
    // Base64 encodes the stored diagnostic; decode without adding a new SDK/dependency.
    let data = last["message"]["data"].as_str().unwrap();
    let decoded = std::process::Command::new("python3")
        .args([
            "-c",
            "import base64,sys;print(base64.b64decode(sys.argv[1]).decode())",
            data,
        ])
        .output()?;
    assert!(!String::from_utf8(decoded.stdout)?.contains(marker));
    assert!(
        !String::from_utf8(logs.0.lock().unwrap().clone())?.contains(marker),
        "private parser contents entered logs"
    );
    let mut invalid_work = good;
    invalid_work["message_type"] = serde_json::json!("deepref.work");
    invalid_work["payload"] =
        serde_json::json!({"job_id":marker,"generation":1,"workload_class":"documents"});
    let invalid_work: EventEnvelope = serde_json::from_value(invalid_work)?;
    assert!(
        !invalid_work
            .work()
            .unwrap_err()
            .to_string()
            .contains(marker)
    );
    Ok(())
}

#[tokio::test]
async fn consumer_body_replay_and_durability_drift_fails_closed() -> Result<()> {
    let Some((_, broker, config)) = fixture().await else {
        return Ok(());
    };
    for (name, field, value) in [
        ("headers", "headers_only", serde_json::json!(true)),
        (
            "expiry",
            "inactive_threshold",
            serde_json::json!(10_000_000_000u64),
        ),
        ("delivery", "deliver_policy", serde_json::json!("new")),
        ("replay", "replay_policy", serde_json::json!("original")),
    ] {
        broker.consumer(name, None).await?;
        let info = nats_request(
            &config,
            &format!("$JS.API.CONSUMER.INFO.{}.{}", config.stream, name),
            serde_json::json!({}),
        )
        .await?;
        let mut drift = info["config"].clone();
        drift[field] = value;
        // Recreate because some delivery fields are immutable on an existing durable.
        nats_request(
            &config,
            &format!("$JS.API.CONSUMER.DELETE.{}.{}", config.stream, name),
            serde_json::json!({}),
        )
        .await?;
        let created = nats_request(
            &config,
            &format!("$JS.API.CONSUMER.DURABLE.CREATE.{}.{}", config.stream, name),
            serde_json::json!({"stream_name":config.stream,"config":drift}),
        )
        .await?;
        assert!(created.get("error").is_none(), "{created}");
        assert!(
            broker.consumer(name, None).await.is_err(),
            "accepted {field} drift"
        );
    }
    Ok(())
}

// Start the short-lived blocked document intake only after the healthy AI
// sibling has run and the test is ready to observe it. Preserve both the real
// transport's intake budget and its PostgreSQL statement/lock deadlines.
struct GatedDocumentIntake {
    transport: JetStreamWorkTransport,
    ready: tokio::sync::watch::Receiver<bool>,
}

impl WorkTransport for GatedDocumentIntake {
    fn recover_before_poll(&self) -> bool {
        self.transport.recover_before_poll()
    }

    async fn next_work(&self, request: WorkRequest<'_>) -> Result<Vec<ClaimedJob>> {
        self.transport.next_work(request).await
    }

    async fn next_work_controlled(
        &self,
        request: WorkRequest<'_>,
        control: &IntakeControl,
        budget: Duration,
    ) -> Result<Vec<ClaimedJob>> {
        if request.pool == WorkloadClass::Documents {
            let mut ready = self.ready.clone();
            tokio::select! {
                _ = control.cancelled() => return Ok(Vec::new()),
                result = ready.wait_for(|ready| *ready) => {
                    result.context("document intake fixture gate closed")?;
                }
            }
        }
        self.transport
            .next_work_controlled(request, control, budget)
            .await
    }

    async fn settled(&self, lease: &JobLease) -> Result<()> {
        self.transport.settled(lease).await
    }
}

async fn blocked_work_intake(kind: &str) -> Result<()> {
    let Some((pool, broker, config)) = fixture().await else {
        return Ok(());
    };
    let first = enqueue(&pool, JobKind::RetrieveDocument).await?;
    publish(&pool, &broker).await?;
    let mut blocker = pool.begin().await?;
    if kind == "dlq" {
        sqlx::query("LOCK TABLE transport_dead_letters IN ACCESS EXCLUSIVE MODE")
            .execute(&mut *blocker)
            .await?;
        nats_request(
            &config,
            &format!("{}.jobs.documents", config.subject_prefix),
            serde_json::json!({"envelope_version":"private-malformed"}),
        )
        .await?;
    } else {
        let blocked = enqueue(&pool, JobKind::RetrieveDocument).await?;
        // The claim statement can block after SKIP LOCKED (e.g. a trigger or
        // downstream relation lock). Only this row waits; AI remains healthy.
        sqlx::query("SELECT pg_advisory_xact_lock(409705)")
            .execute(&mut *blocker)
            .await?;
        sqlx::query("UPDATE jobs SET payload='{\"block_claim\":true}' WHERE id=$1")
            .bind(blocked)
            .execute(&pool)
            .await?;
        sqlx::raw_sql("CREATE FUNCTION test_block_claim() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN IF NEW.payload->>'block_claim'='true' AND NEW.state='running' THEN PERFORM pg_advisory_xact_lock(409705); END IF; RETURN NEW; END $$; CREATE TRIGGER test_block_claim BEFORE UPDATE ON jobs FOR EACH ROW EXECUTE FUNCTION test_block_claim();").execute(&pool).await?;
        publish(&pool, &broker).await?;
    }
    let ai = enqueue(&pool, JobKind::AssistantAgentRun).await?;
    publish(&pool, &broker).await?;
    let transport = JetStreamWorkTransport::new(
        pool.clone(),
        broker,
        &[WorkloadClass::Documents, WorkloadClass::Ai],
    )
    .await?;
    let (healthy, seen) = oneshot::channel();
    let healthy = Arc::new(std::sync::Mutex::new(Some(healthy)));
    let handler = Arc::new(move |job: deepref_application::jobs::ClaimedJob, _| {
        let healthy = Arc::clone(&healthy);
        Box::pin(async move {
            if job.id == ai
                && let Some(tx) = healthy.lock().unwrap().take()
            {
                let _ = tx.send(());
            }
            Ok(DeliveryAction::Ack)
        })
            as std::pin::Pin<Box<dyn std::future::Future<Output = Result<DeliveryAction>> + Send>>
    });
    let (stop, stopped) = oneshot::channel();
    let (start_documents, ready) = tokio::sync::watch::channel(false);
    let worker = tokio::spawn(run_claim_loop(
        GatedDocumentIntake { transport, ready },
        PostgresDurableExecutionStore { pool: pool.clone() },
        "intake-review".into(),
        ClaimLoopConfig {
            pools: vec![WorkloadClass::Documents, WorkloadClass::Ai],
            pool_concurrency: HashMap::from([
                (WorkloadClass::Documents, 3),
                (WorkloadClass::Ai, 1),
            ]),
            max_batch: 3,
            claim_lease: Duration::from_secs(2),
            drain_deadline: Duration::from_millis(150),
        },
        handler,
        async {
            let _ = stopped.await;
        },
    ));
    tokio::time::timeout(Duration::from_secs(2), seen).await??;
    // A late observer must not miss a document intake window that started while
    // it was still waiting for the independent AI result.
    tokio::time::sleep(Duration::from_millis(300)).await;
    start_documents.send(true)?;
    // Prove the partial batch exists before requesting shutdown: the first
    // fence is committed and the next boundary is actually waiting. A short
    // unchecked poll can otherwise cancel during setup on a slower runner.
    let waiting = if kind == "dlq" {
        "SELECT EXISTS(SELECT 1 FROM jobs WHERE id=$1 AND state='running')
           AND EXISTS(SELECT 1 FROM pg_locks WHERE NOT granted
             AND database=(SELECT oid FROM pg_database WHERE datname=current_database())
             AND locktype='relation' AND relation='transport_dead_letters'::regclass)"
    } else {
        "SELECT EXISTS(SELECT 1 FROM jobs WHERE id=$1 AND state='running')
           AND EXISTS(SELECT 1 FROM pg_locks WHERE NOT granted
             AND database=(SELECT oid FROM pg_database WHERE datname=current_database())
             AND locktype='advisory' AND classid=0 AND objid=409705)"
    };
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            let ready: bool = sqlx::query_scalar(waiting)
                .bind(first)
                .fetch_one(&pool)
                .await?;
            if ready {
                return Ok::<(), sqlx::Error>(());
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .context("first committed claim and blocked next boundary must precede shutdown")??;
    let started = tokio::time::Instant::now();
    stop.send(()).unwrap();
    tokio::time::timeout(Duration::from_millis(220), worker).await???;
    assert!(started.elapsed() < Duration::from_millis(220));
    blocker.rollback().await?;
    tokio::time::sleep(Duration::from_millis(60)).await;
    let state: String = sqlx::query_scalar("SELECT state FROM jobs WHERE id=$1")
        .bind(first)
        .fetch_one(&pool)
        .await?;
    assert_eq!(
        state, "queued",
        "earlier partial-batch claim must return for exact unstarted release"
    );
    assert_eq!(attempts(&pool, first).await, 0);
    assert_eq!(attempts(&pool, ai).await, 1);
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM jobs WHERE state='running'")
            .fetch_one(&pool)
            .await?,
        0,
        "no detached late claim"
    );
    Ok(())
}
#[tokio::test]
async fn blocked_work_dlq_preserves_partial_claim_and_healthy_sibling_during_short_drain()
-> Result<()> {
    blocked_work_intake("dlq").await
}
#[tokio::test]
async fn blocked_work_claim_preserves_partial_claim_and_healthy_sibling_during_short_drain()
-> Result<()> {
    blocked_work_intake("claim").await
}

async fn final_delivery_failure(fail_dlq: bool) -> Result<()> {
    let Some((pool, broker, config)) = fixture().await else {
        return Ok(());
    };
    let response = nats_request(
        &config,
        &format!("{}.events", config.subject_prefix),
        serde_json::json!({"envelope_version":"private-terminal"}),
    )
    .await?;
    let sequence = response["seq"].as_u64().unwrap();
    let consumer = broker.consumer("terminal", None).await?;
    for expected in 1..5 {
        let delivery = consumer.fetch(1).await?.pop().unwrap();
        assert_eq!(delivery.deliveries()?, expected);
        delivery.retry(Duration::ZERO).await?;
        tokio::time::sleep(Duration::from_millis(30)).await;
    }
    let fifth = consumer.fetch(1).await?.pop().unwrap();
    assert_eq!(fifth.deliveries()?, 5);
    let mut blocked = None;
    let mut original_dlq = None;
    if fail_dlq {
        // A bounded full diagnostic stream: one retained message and a
        // DiscardNew message cap. Restore its exact config before recovery.
        let info = nats_request(
            &config,
            &format!("$JS.API.STREAM.INFO.{}_DLQ", config.stream),
            serde_json::json!({}),
        )
        .await?;
        let mut full = info["config"].clone();
        original_dlq = Some(full.clone());
        full["max_msgs"] = serde_json::json!(1);
        let updated = nats_request(
            &config,
            &format!("$JS.API.STREAM.UPDATE.{}_DLQ", config.stream),
            full,
        )
        .await?;
        assert!(updated.get("error").is_none(), "{updated}");
        nats_request(
            &config,
            &format!("{}.dead.fill", config.subject_prefix),
            serde_json::json!({"filler":true}),
        )
        .await?;
    } else {
        let mut tx = pool.begin().await?;
        sqlx::query("LOCK TABLE transport_dead_letters IN ACCESS EXCLUSIVE MODE")
            .execute(&mut *tx)
            .await?;
        blocked = Some(tx);
    }
    let registry = OutboxRegistry::new();
    let result = tokio::time::timeout(
        Duration::from_millis(180),
        consume_event(
            &pool,
            &broker,
            &registry,
            "terminal",
            Duration::from_secs(1),
            fifth,
        ),
    )
    .await;
    assert!(
        !matches!(result, Ok(Ok(()))),
        "final terminalization must fail with selected dependency"
    );
    if let Some(tx) = blocked {
        tx.rollback().await?;
    }
    if let Some(original) = original_dlq {
        nats_request(
            &config,
            &format!("$JS.API.STREAM.PURGE.{}_DLQ", config.stream),
            serde_json::json!({}),
        )
        .await?;
        let restored = nats_request(
            &config,
            &format!("$JS.API.STREAM.UPDATE.{}_DLQ", config.stream),
            original,
        )
        .await?;
        assert!(restored.get("error").is_none(), "{restored}");
    }
    tokio::time::sleep(Duration::from_millis(3300)).await;
    assert!(
        consumer.fetch(1).await?.is_empty(),
        "MaxDeliver never promises a sixth delivery"
    );
    for _ in 0..20 {
        if broker.terminal_evidence_count().await? > 0 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert_eq!(
        broker.terminal_evidence_count().await?,
        1,
        "retained advisory owns terminal sequence evidence"
    );
    let page = broker.terminal_advisories(0, 10).await?;
    anyhow::ensure!(page.complete_to_observed_end && page.entries.len() == 1);
    let advisory_sequence = page.entries[0].advisory_sequence;
    anyhow::ensure!(page.entries[0].advisory.stream_seq == sequence);
    anyhow::ensure!(broker.terminal_advisories(u64::MAX, 10).await.is_err());
    let untouched = enqueue(&pool, JobKind::RecomputeMetrics).await?;
    sqlx::query("UPDATE jobs SET attempts=2 WHERE id=$1")
        .bind(untouched)
        .execute(&pool)
        .await?;
    let before: serde_json::Value =
        sqlx::query_scalar("SELECT to_jsonb(j) FROM jobs j WHERE id=$1")
            .bind(untouched)
            .fetch_one(&pool)
            .await?;
    let result = deepref_worker::transport_operator::recover_terminal_advisory(
        &pool,
        &broker,
        advisory_sequence,
    )
    .await?;
    anyhow::ensure!(
        result["business_replayed"] == false && result["delivery_budget_reset"] == false
    );
    let after: serde_json::Value = sqlx::query_scalar("SELECT to_jsonb(j) FROM jobs j WHERE id=$1")
        .bind(untouched)
        .fetch_one(&pool)
        .await?;
    anyhow::ensure!(
        before == after,
        "diagnostic recovery must not reset real queued business state"
    );
    assert_eq!(broker.dead_letter_count().await?, 1);
    let reason: String =
        sqlx::query_scalar("SELECT reason FROM transport_dead_letters WHERE delivery_id=$1")
            .bind(broker.sequence_identity("terminal", sequence))
            .fetch_one(&pool)
            .await?;
    assert!(!reason.contains("private-terminal"));
    // Recovery is idempotent and never resets/advances the exhausted consumer.
    deepref_worker::transport_operator::recover_terminal_advisory(
        &pool,
        &broker,
        advisory_sequence,
    )
    .await?;
    assert_eq!(broker.dead_letter_count().await?, 1);
    assert!(consumer.fetch(1).await?.is_empty());
    Ok(())
}
#[tokio::test]
async fn fifth_delivery_db_failure_requires_explicit_terminal_sequence_recovery() -> Result<()> {
    final_delivery_failure(false).await
}
#[tokio::test]
async fn fifth_delivery_dlq_failure_requires_explicit_terminal_sequence_recovery() -> Result<()> {
    final_delivery_failure(true).await
}

#[tokio::test]
async fn real_delivery_exposes_metrics_and_readiness_observation_seam() -> Result<()> {
    use deepref_integrations::jetstream::{ClaimOutcome, TransportObservation, TransportObserver};
    #[derive(Default)]
    struct Captured(std::sync::Mutex<Vec<TransportObservation>>);
    impl TransportObserver for Captured {
        fn observe(&self, event: TransportObservation) {
            self.0.lock().unwrap().push(event);
        }
    }
    let Some((pool, broker, _)) = fixture().await else {
        return Ok(());
    };
    let observed = Arc::new(Captured::default());
    let broker = broker.with_observer(observed.clone());
    broker.readiness().await?;
    let id = enqueue(&pool, JobKind::RetrieveDocument).await?;
    publish(&pool, &broker).await?;
    let transport =
        JetStreamWorkTransport::new(pool.clone(), broker.clone(), &[WorkloadClass::Documents])
            .await?;
    let request = WorkRequest {
        owner: "metrics",
        lease: Duration::from_secs(2),
        pool: WorkloadClass::Documents,
        max_jobs: 1,
    };
    let jobs = transport.next_work(request).await?;
    assert_eq!(jobs.len(), 1);
    let mut duplicate = envelope(&pool, id).await;
    duplicate.publication_epoch += 1;
    broker.publish(&duplicate).await?;
    assert!(transport.next_work(request).await?.is_empty());
    let samples = observed.0.lock().unwrap();
    assert!(
        samples
            .iter()
            .any(|s| matches!(s, TransportObservation::Readiness { ready: true }))
    );
    assert!(
        samples
            .iter()
            .any(|s| matches!(s, TransportObservation::Visibility { .. }))
    );
    assert!(
        samples
            .iter()
            .any(|s| matches!(s, TransportObservation::ConsumeLatency { .. }))
    );
    assert!(samples.iter().any(|s| matches!(
        s,
        TransportObservation::Claim {
            outcome: ClaimOutcome::Claimed,
            ..
        }
    )));
    assert!(samples.iter().any(|s| matches!(
        s,
        TransportObservation::Claim {
            outcome: ClaimOutcome::Noop,
            ..
        }
    )));
    Ok(())
}

#[derive(Clone, Serialize, Deserialize)]
struct CrashFanout;
impl OutboxMessage for CrashFanout {
    const TYPE: &'static str = "test.crash-fanout";
    const VERSION: u32 = 1;
    const CONSUMERS: &'static [&'static str] = &["done", "waiting", "crashed"];
}
#[tokio::test]
async fn targeted_expiry_preserves_committed_recipients_and_independent_backoff() -> Result<()> {
    let Some((pool, _, _)) = fixture().await else {
        return Ok(());
    };
    let mut registry = OutboxRegistry::new();
    let handler: TypedOutboxHandler<CrashFanout> =
        Arc::new(|_, _| Box::pin(async { panic!("recovery must not invoke handlers") }));
    registry.register::<CrashFanout>(&[
        ("done", handler.clone()),
        ("waiting", handler.clone()),
        ("crashed", handler),
    ])?;
    let mut event = NewOutboxEvent::for_message(Uuid::new_v4(), &CrashFanout)?;
    event.max_attempts = 3;
    let mut tx = pool.begin().await?;
    deepref_app::outbox::insert_outbox_event(&mut tx, &event).await?;
    tx.commit().await?;
    let e = publication::claim_publications(&pool, "relay", Duration::from_secs(2), 1)
        .await?
        .pop()
        .unwrap()
        .envelope;
    let first =
        publication::claim_event_delivery(&pool, &registry, &e, "crashed", Duration::from_secs(1))
            .await?
            .unwrap();
    sqlx::query("UPDATE outbox_receipts SET state=CASE WHEN consumer='done' THEN 'delivered' ELSE 'retrying' END,delivered_at=CASE WHEN consumer='done' THEN clock_timestamp() ELSE NULL END,settled_generation=attempt_generation,next_attempt_at=clock_timestamp()+interval '2 minutes' WHERE event_id=$1 AND consumer IN ('done','waiting')").bind(e.event_id).execute(&pool).await?;
    sqlx::query("UPDATE outbox_events SET lease_expires_at=clock_timestamp()-interval '1 second' WHERE event_id=$1").bind(e.event_id).execute(&pool).await?;
    assert!(
        publication::claim_event_delivery(
            &pool,
            &registry,
            &e,
            "recovered",
            Duration::from_secs(2)
        )
        .await?
        .is_none()
    );
    let rows=sqlx::query("SELECT consumer,state,attempts,attempt_generation,settled_generation,next_attempt_at>clock_timestamp() AS backing_off FROM outbox_receipts WHERE event_id=$1 ORDER BY consumer").bind(e.event_id).fetch_all(&pool).await?;
    assert_eq!(rows.len(), 3);
    for row in rows {
        assert_eq!(row.get::<i32, _>("attempts"), 1);
        assert_eq!(
            row.get::<i64, _>("attempt_generation"),
            first.claim_fence.generation()
        );
        assert_eq!(
            row.get::<i64, _>("settled_generation"),
            first.claim_fence.generation()
        );
        let name: String = row.get("consumer");
        assert_eq!(
            row.get::<String, _>("state"),
            if name == "done" {
                "delivered"
            } else {
                "retrying"
            }
        );
        if name != "done" {
            assert!(row.get::<bool, _>("backing_off"));
        }
    }
    sqlx::query("UPDATE outbox_receipts SET next_attempt_at=clock_timestamp() WHERE event_id=$1 AND consumer='crashed'").bind(e.event_id).execute(&pool).await?;
    sqlx::query("UPDATE outbox_events SET next_attempt_at=clock_timestamp() WHERE event_id=$1")
        .bind(e.event_id)
        .execute(&pool)
        .await?;
    assert!(
        publication::claim_event_delivery(
            &pool,
            &registry,
            &e,
            "after-backoff",
            Duration::from_secs(2)
        )
        .await?
        .is_some()
    );
    let attempts: Vec<(String, i32)> = sqlx::query_as(
        "SELECT consumer,attempts FROM outbox_receipts WHERE event_id=$1 ORDER BY consumer",
    )
    .bind(e.event_id)
    .fetch_all(&pool)
    .await?;
    assert_eq!(
        attempts,
        vec![
            ("crashed".into(), 2),
            ("done".into(), 1),
            ("waiting".into(), 1)
        ]
    );
    Ok(())
}

#[tokio::test]
async fn blocked_repair_branch_observes_short_supervisor_drain() -> Result<()> {
    use deepref_worker::jetstream_transport::{PublicationConfig, run_publication};
    let Some((pool, broker, config)) = fixture().await else {
        return Ok(());
    };
    let consumer = broker.consumer("outbox-v1", None).await?;
    let mut blocked = pool.begin().await?;
    sqlx::query("LOCK TABLE outbox_events IN ACCESS EXCLUSIVE MODE")
        .execute(&mut *blocked)
        .await?;
    let (stop, stopped) = tokio::sync::watch::channel(false);
    let active_pool = pool.clone();
    let supervisor = tokio::spawn(run_publication(
        active_pool,
        broker,
        Arc::new(OutboxRegistry::new()),
        "repair-review".into(),
        PublicationConfig {
            nats: config,
            claim_lease: Duration::from_secs(1),
            drain_deadline: Duration::from_millis(100),
        },
        stopped,
    ));
    // The first repair tick runs immediately. Confirm its SELECT reached the
    // relation lock rather than relying on compile/startup elapsed time.
    let mut reached = false;
    for _ in 0..100 {
        reached=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND query LIKE '%WITH stranded%')").fetch_one(&pool).await?;
        if reached {
            break;
        }
        tokio::time::sleep(Duration::from_millis(2)).await;
    }
    assert!(reached, "repair must reach the blocked boundary");
    stop.send(true)?;
    tokio::time::timeout(Duration::from_millis(150), supervisor).await???;
    blocked.rollback().await?;
    assert_eq!(consumer.visibility().await?.1, 0);
    Ok(())
}

struct CommitProxy {
    pool: PgPool,
    armed: Arc<std::sync::atomic::AtomicBool>,
    withheld: Arc<std::sync::atomic::AtomicBool>,
    release_held: Arc<tokio::sync::Notify>,
    task: tokio::task::JoinHandle<()>,
}

async fn spawn_commit_withholding_pg_proxy(upstream: &PgPool) -> Result<CommitProxy> {
    use std::sync::atomic::AtomicBool;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let upstream_opts = (*upstream.connect_options()).clone();
    let upstream_addr = format!("{}:{}", upstream_opts.get_host(), upstream_opts.get_port());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let proxy_port = listener.local_addr()?.port();
    let armed = Arc::new(AtomicBool::new(false));
    let withheld_once = Arc::new(AtomicBool::new(false));
    let release_held = Arc::new(tokio::sync::Notify::new());

    let armed_loop = Arc::clone(&armed);
    let withheld_loop = Arc::clone(&withheld_once);
    let release_loop = Arc::clone(&release_held);
    let proxy_task = tokio::spawn(async move {
        while let Ok((client, _)) = listener.accept().await {
            let Ok(server) = tokio::net::TcpStream::connect(&upstream_addr).await else {
                break;
            };
            let _ = client.set_nodelay(true);
            let _ = server.set_nodelay(true);
            let (mut client_read, mut client_write) = client.into_split();
            let (mut server_read, mut server_write) = server.into_split();
            let hold_replies = Arc::new(AtomicBool::new(false));
            let hold_replies_c2s = Arc::clone(&hold_replies);
            let armed_c2s = Arc::clone(&armed_loop);
            let withheld_c2s = Arc::clone(&withheld_loop);
            let release_s2c = Arc::clone(&release_loop);

            tokio::spawn(async move {
                let mut buf = [0u8; 4096];
                let mut pending = Vec::new();
                let mut startup = true;
                while let Ok(n) = client_read.read(&mut buf).await {
                    if n == 0 {
                        break;
                    }
                    pending.extend_from_slice(&buf[..n]);
                    loop {
                        let header = if startup { 4 } else { 5 };
                        if pending.len() < header {
                            break;
                        }
                        let offset = usize::from(!startup);
                        let length =
                            u32::from_be_bytes(pending[offset..offset + 4].try_into().unwrap())
                                as usize;
                        if !(4..=1_048_576).contains(&length) {
                            return;
                        }
                        let total = length + offset;
                        if pending.len() < total {
                            break;
                        }
                        let frame: Vec<u8> = pending.drain(..total).collect();
                        // Match the actual simple-query COMMIT frame, not a
                        // bind value such as the fixture owner 'commit-proxy'.
                        let commit = !startup
                            && frame[0] == b'Q'
                            && frame[5..].eq_ignore_ascii_case(b"COMMIT\0");
                        if commit
                            && armed_c2s.load(Ordering::SeqCst)
                            && withheld_c2s
                                .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
                                .is_ok()
                        {
                            hold_replies_c2s.store(true, Ordering::SeqCst);
                        }
                        if server_write.write_all(&frame).await.is_err() {
                            return;
                        }
                        startup = false;
                    }
                    if server_write.flush().await.is_err() {
                        return;
                    }
                }
            });

            tokio::spawn(async move {
                let mut buf = [0u8; 4096];
                while let Ok(n) = server_read.read(&mut buf).await {
                    if n == 0 {
                        break;
                    }
                    if hold_replies.load(Ordering::SeqCst) {
                        release_s2c.notified().await;
                        hold_replies.store(false, Ordering::SeqCst);
                    }
                    if client_write.write_all(&buf[..n]).await.is_err() {
                        break;
                    }
                    let _ = client_write.flush().await;
                }
            });
        }
    });

    let proxy_opts = (*upstream.connect_options())
        .clone()
        .host("127.0.0.1")
        .port(proxy_port)
        .ssl_mode(sqlx::postgres::PgSslMode::Disable);
    let proxied_pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(4)
        .connect_with(proxy_opts)
        .await?;
    Ok(CommitProxy {
        pool: proxied_pool,
        armed,
        withheld: withheld_once,
        release_held,
        task: proxy_task,
    })
}

#[tokio::test]
async fn withheld_commit_response_after_accepted_claim_refunds_single_attempt_job_before_drain()
-> Result<()> {
    let Some((pool, broker, _)) = fixture().await else {
        return Ok(());
    };
    let job_id = enqueue(&pool, JobKind::RetrieveDocument).await?;
    sqlx::query("UPDATE jobs SET max_attempts = 1 WHERE id = $1")
        .bind(job_id)
        .execute(&pool)
        .await?;
    publish(&pool, &broker).await?;

    let CommitProxy {
        pool: proxied_pool,
        armed,
        withheld,
        release_held,
        task: proxy_task,
    } = spawn_commit_withholding_pg_proxy(&pool).await?;
    let transport = JetStreamWorkTransport::new(
        proxied_pool.clone(),
        broker.clone(),
        &[WorkloadClass::Documents],
    )
    .await?;
    let handled = Arc::new(AtomicUsize::new(0));
    let handled_clone = Arc::clone(&handled);
    let (stop, stopped) = oneshot::channel::<()>();

    armed.store(true, Ordering::SeqCst);
    let worker = tokio::spawn(run_claim_loop(
        transport,
        PostgresDurableExecutionStore {
            pool: proxied_pool.clone(),
        },
        "commit-proxy".into(),
        ClaimLoopConfig {
            pools: vec![WorkloadClass::Documents],
            pool_concurrency: HashMap::from([(WorkloadClass::Documents, 1)]),
            max_batch: 1,
            claim_lease: Duration::from_secs(2),
            drain_deadline: Duration::from_millis(350),
        },
        Arc::new(move |_, _| {
            let handled_clone = Arc::clone(&handled_clone);
            Box::pin(async move {
                handled_clone.fetch_add(1, Ordering::SeqCst);
                Ok(DeliveryAction::Ack)
            })
        }),
        async move {
            let _ = stopped.await;
        },
    ));

    let mut committed_on_server = false;
    for _ in 0..200 {
        committed_on_server = sqlx::query_scalar(
            "SELECT state = 'running' AND attempts = 1 AND max_attempts = 1 AND lease_owner = 'commit-proxy' FROM jobs WHERE id = $1",
        )
        .bind(job_id)
        .fetch_one(&pool)
        .await?;
        if committed_on_server {
            break;
        }
        tokio::time::sleep(Duration::from_millis(2)).await;
    }
    if !committed_on_server {
        let snapshot: (String, i32, Option<String>, i64) =
            sqlx::query_as("SELECT state,attempts,lease_owner,claim_seq FROM jobs WHERE id=$1")
                .bind(job_id)
                .fetch_one(&pool)
                .await?;
        panic!(
            "expected committed claim; actual={snapshot:?}, commit_withheld={}, handler_calls={}",
            withheld.load(Ordering::SeqCst),
            handled.load(Ordering::SeqCst)
        );
    }

    stop.send(()).unwrap();
    tokio::time::timeout(Duration::from_millis(500), worker).await???;
    assert_eq!(
        handled.load(Ordering::SeqCst),
        0,
        "withheld COMMIT claim must never start a handler"
    );

    let (state, attempt_count, lease_owner): (String, i32, Option<String>) =
        sqlx::query_as("SELECT state, attempts, lease_owner FROM jobs WHERE id = $1")
            .bind(job_id)
            .fetch_one(&pool)
            .await?;
    assert_eq!(
        (state.as_str(), attempt_count, lease_owner),
        ("queued", 0, None),
        "ambiguous COMMIT cleanup must refund the unstarted max_attempts=1 claim back to queued with attempts=0"
    );

    release_held.notify_one();
    tokio::time::timeout(Duration::from_secs(2), proxied_pool.close())
        .await
        .expect("released COMMIT proxy connections close within the fixture budget");
    proxy_task.abort();
    let _ = proxy_task.await;
    Ok(())
}

#[tokio::test]
async fn stream_subject_transform_and_routing_drift_fails_closed_before_startup() -> Result<()> {
    let Some((_pool, _broker, config)) = fixture().await else {
        return Ok(());
    };
    let info = nats_request(
        &config,
        &format!("$JS.API.STREAM.INFO.{}", config.stream),
        serde_json::json!({}),
    )
    .await?;
    let base_config = info["config"].clone();

    let mut transform_drift = base_config.clone();
    transform_drift["subject_transform"] = serde_json::json!({
        "src": format!("{}.events", config.subject_prefix),
        "dest": format!("{}.jobs.documents", config.subject_prefix),
    });

    let mut republish_drift = base_config.clone();
    republish_drift["republish"] = serde_json::json!({
        "src": format!("{}.events", config.subject_prefix),
        "dest": format!("{}.republished.events", config.subject_prefix),
    });

    let mut deny_delete_drift = base_config.clone();
    deny_delete_drift["deny_delete"] = serde_json::json!(true);

    let mut deny_purge_drift = base_config.clone();
    deny_purge_drift["deny_purge"] = serde_json::json!(true);

    let mut rollup_drift = base_config.clone();
    rollup_drift["allow_rollup_hdrs"] = serde_json::json!(true);

    let mut max_msgs_drift = base_config.clone();
    max_msgs_drift["max_msgs"] = serde_json::json!(10);

    let mut max_msgs_per_sub_drift = base_config.clone();
    max_msgs_per_sub_drift["max_msgs_per_subject"] = serde_json::json!(1);

    for (label, drift) in [
        ("subject_transform", transform_drift),
        ("republish", republish_drift),
        ("deny_delete", deny_delete_drift),
        ("deny_purge", deny_purge_drift),
        ("allow_rollup_hdrs", rollup_drift),
        ("max_msgs", max_msgs_drift),
        ("max_msgs_per_subject", max_msgs_per_sub_drift),
    ] {
        let deleted = nats_request(
            &config,
            &format!("$JS.API.STREAM.DELETE.{}", config.stream),
            serde_json::json!({}),
        )
        .await?;
        assert_eq!(deleted["success"], serde_json::json!(true), "{label}");
        let created = nats_request(
            &config,
            &format!("$JS.API.STREAM.CREATE.{}", config.stream),
            drift,
        )
        .await?;
        assert!(
            created.get("error").is_none(),
            "NATS server must accept precreated stream drift case {label}: {created}"
        );
        assert!(
            JetStream::connect(config.clone()).await.is_err(),
            "JetStream::connect must fail closed on stream drift case {label}"
        );
    }

    let _ = nats_request(
        &config,
        &format!("$JS.API.STREAM.DELETE.{}", config.stream),
        serde_json::json!({}),
    )
    .await?;
    let restored = nats_request(
        &config,
        &format!("$JS.API.STREAM.CREATE.{}", config.stream),
        base_config,
    )
    .await?;
    assert!(restored.get("error").is_none());
    assert!(JetStream::connect(config).await.is_ok());
    Ok(())
}

#[tokio::test]
async fn terminal_recovery_refuses_routing_drift_before_metadata_and_never_creates_consumers()
-> Result<()> {
    use deepref_integrations::jetstream::ConsumerScope;
    use deepref_worker::transport_operator::{
        recover_terminal_advisory, recover_terminal_advisory_with_scope,
    };
    let Some((pool, broker, config)) = fixture().await else {
        return Ok(());
    };
    let work = nats_request(
        &config,
        &format!("{}.jobs.documents", config.subject_prefix),
        serde_json::json!({"malformed":"private-routing-value"}),
    )
    .await?;
    let work_sequence = work["seq"]
        .as_u64()
        .ok_or_else(|| anyhow::anyhow!("missing work ACK"))?;
    let _consumer = broker.consumer("scope-events", None).await?;
    let before = nats_request(
        &config,
        &format!("$JS.API.STREAM.INFO.{}", config.stream),
        serde_json::json!({}),
    )
    .await?;
    let metadata = serde_json::json!({"type":"io.nats.jetstream.advisory.v1.max_deliver","timestamp":chrono::Utc::now(),"stream":config.stream,"consumer":"scope-events","stream_seq":work_sequence,"deliveries":5});
    // Same valid metadata on another retained consumer subject must refuse.
    let mismatch = nats_request(
        &config,
        &format!(
            "$JS.EVENT.ADVISORY.CONSUMER.MAX_DELIVERIES.{}.different-consumer",
            config.stream
        ),
        metadata.clone(),
    )
    .await?;
    let mismatch_sequence = mismatch["seq"]
        .as_u64()
        .ok_or_else(|| anyhow::anyhow!("missing advisory ACK"))?;
    anyhow::ensure!(
        recover_terminal_advisory(&pool, &broker, mismatch_sequence)
            .await
            .is_err()
    );
    // Exact advisory subject still cannot bind an events consumer to work.
    let advisory = nats_request(
        &config,
        &format!(
            "$JS.EVENT.ADVISORY.CONSUMER.MAX_DELIVERIES.{}.scope-events",
            config.stream
        ),
        metadata,
    )
    .await?;
    let advisory_sequence = advisory["seq"]
        .as_u64()
        .ok_or_else(|| anyhow::anyhow!("missing advisory ACK"))?;
    anyhow::ensure!(
        recover_terminal_advisory(&pool, &broker, advisory_sequence)
            .await
            .is_err()
    );
    let refused_count: i64 = sqlx::query_scalar("SELECT count(*) FROM transport_dead_letters")
        .fetch_one(&pool)
        .await?;
    anyhow::ensure!(
        refused_count == 0 && broker.dead_letter_count().await? == 0,
        "mismatched routing became durable evidence"
    );
    let after = nats_request(
        &config,
        &format!("$JS.API.STREAM.INFO.{}", config.stream),
        serde_json::json!({}),
    )
    .await?;
    anyhow::ensure!(
        before["state"]["consumer_count"].is_u64()
            && before["state"]["consumer_count"] == after["state"]["consumer_count"],
        "read-only recovery changed consumer inventory"
    );
    // A deleted consumer is metadata-only history until explicitly scoped.
    let deleted = nats_request(
        &config,
        &format!("$JS.API.CONSUMER.DELETE.{}.scope-events", config.stream),
        serde_json::json!({}),
    )
    .await?;
    anyhow::ensure!(deleted["success"] == true);
    broker.terminal_advisories(mismatch_sequence, 10).await?;
    anyhow::ensure!(
        recover_terminal_advisory(&pool, &broker, advisory_sequence)
            .await
            .is_err()
    );
    anyhow::ensure!(
        recover_terminal_advisory_with_scope(
            &pool,
            &broker,
            advisory_sequence,
            Some(ConsumerScope::Events)
        )
        .await
        .is_err()
    );
    let missing = nats_request(
        &config,
        &format!("$JS.API.CONSUMER.INFO.{}.scope-events", config.stream),
        serde_json::json!({}),
    )
    .await?;
    anyhow::ensure!(
        missing["error"]["err_code"] == 10014,
        "missing consumer was provisioned during refusal"
    );
    let recovered = recover_terminal_advisory_with_scope(
        &pool,
        &broker,
        advisory_sequence,
        Some(ConsumerScope::Work(WorkloadClass::Documents)),
    )
    .await?;
    anyhow::ensure!(
        recovered["consumer_contract_evidence"] == "operator_declared_historical"
            && recovered["business_replayed"] == false
            && recovered["delivery_budget_reset"] == false
    );
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM transport_dead_letters")
        .fetch_one(&pool)
        .await?;
    anyhow::ensure!(count == 1 && broker.dead_letter_count().await? == 1);
    let still_missing = nats_request(
        &config,
        &format!("$JS.API.CONSUMER.INFO.{}.scope-events", config.stream),
        serde_json::json!({}),
    )
    .await?;
    anyhow::ensure!(
        still_missing["error"]["err_code"] == 10014,
        "historical recovery provisioned a consumer"
    );
    Ok(())
}
