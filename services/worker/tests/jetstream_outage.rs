#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
//! Run after jetstream_contract: pause only the runner-owned broker, never a
//! shared developer service. Container name AND ownership label are checked.
use anyhow::{Context, Result};
use deepref_app::{enqueue_job_pool, scoped_job, work_publication};
use deepref_application::jobs::{JobScope, WorkRequest, WorkTransport};
use deepref_config::{Environment, NatsConfig};
use deepref_domain::{JobKind, WorkloadClass};
use deepref_integrations::jetstream::JetStream;
use deepref_worker::jetstream_transport::{JetStreamWorkTransport, publication_tick};
use std::{process::Command, time::Duration};
use uuid::Uuid;

struct PausedBroker(String);
impl Drop for PausedBroker {
    fn drop(&mut self) {
        let _ = Command::new("docker").args(["unpause", &self.0]).output();
    }
}

#[tokio::test]
async fn actual_broker_outage_queues_and_recovers_without_lost_work() -> Result<()> {
    let Ok(container) = std::env::var("NATS_TEST_CONTAINER") else {
        assert_ne!(
            std::env::var("DEEPREF_REQUIRE_NATS").as_deref(),
            Ok("1"),
            "strict outage test requires owned fixture container"
        );
        return Ok(());
    };
    anyhow::ensure!(
        container.starts_with("deepref-js-test-nats-"),
        "refusing non-fixture broker"
    );
    let ownership = Command::new("docker")
        .args([
            "inspect",
            "--format",
            "{{index .Config.Labels \"deepref.test\"}}",
            &container,
        ])
        .output()?;
    anyhow::ensure!(
        ownership.status.success() && ownership.stdout == b"jetstream\n",
        "refusing broker without fixture label"
    );
    let pool = deepref_app::test_isolation::isolated_pool_from_env("DATABASE_URL", 8)
        .await
        .context("disposable DB required")?;
    let url = std::env::var("NATS_TEST_URL")?;
    let identity = Uuid::new_v4().simple().to_string();
    let config = NatsConfig::from_source(Environment::Development, &|name| match name {
        "NATS_URL" => Some(url.clone()),
        "NATS_MAX_BYTES" => Some("1048576".into()),
        "NATS_STREAM" => Some(format!("OUTAGE_{identity}")),
        "NATS_SUBJECT_PREFIX" => Some(format!("outage.{identity}")),
        "NATS_REQUEST_TIMEOUT_SECS" => Some("1".into()),
        _ => None,
    })?;
    let broker = JetStream::connect(config).await?;
    assert!(
        Command::new("docker")
            .args(["pause", &container])
            .output()?
            .status
            .success()
    );
    let paused = PausedBroker(container);
    let id = Uuid::new_v4();
    enqueue_job_pool(
        &pool,
        &scoped_job(
            id,
            JobScope::Platform,
            JobKind::RetrieveDocument,
            serde_json::json!({}),
            id.to_string(),
        ),
    )
    .await?;
    // The actual SDK publish request times out against a paused server.
    tokio::time::timeout(
        Duration::from_secs(8),
        publication_tick(&pool, &broker, "outage", Duration::from_secs(20)),
    )
    .await??;
    let retained:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM outbox_events WHERE published_at IS NULL AND publication_error IS NOT NULL AND delivery_state='pending')").fetch_one(&pool).await?;
    assert!(
        retained,
        "unavailable broker leaves a diagnosed durable publication obligation"
    );
    assert_eq!(
        sqlx::query_scalar::<_, i32>("SELECT attempts FROM jobs WHERE id=$1")
            .bind(id)
            .fetch_one(&pool)
            .await?,
        0
    );
    drop(paused);
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            if broker.readiness().await.is_ok() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    })
    .await?;
    sqlx::query("UPDATE outbox_events SET publication_next_at=now()")
        .execute(&pool)
        .await?;
    publication_tick(&pool, &broker, "returned", Duration::from_secs(20)).await?;
    let transport =
        JetStreamWorkTransport::new(pool.clone(), broker, &[WorkloadClass::Documents]).await?;
    let jobs = transport
        .next_work(WorkRequest {
            owner: "returned",
            lease: Duration::from_secs(5),
            pool: WorkloadClass::Documents,
            max_jobs: 1,
        })
        .await?;
    assert_eq!(jobs.len(), 1);
    assert_eq!(jobs[0].id, id);
    assert!(deepref_app::complete_job(&pool, &jobs[0].lease("returned")).await?);
    transport.settled(&jobs[0].lease("returned")).await?;
    assert_eq!(
        work_publication::claim_publications(&pool, "drained", Duration::from_secs(2), 4)
            .await?
            .len(),
        0
    );
    Ok(())
}
