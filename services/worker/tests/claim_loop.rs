#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
//! Drives the worker claim loop against a database with substitute handlers.
//! Skipped unless `DATABASE_URL` points at a reachable PostgreSQL.

use std::{
    io::Write,
    sync::{Arc, Mutex},
    time::Duration,
};

use deepref_application::jobs::{ClaimedJob, JobLease};
use deepref_domain::ProjectId;
use deepref_postgres::{enqueue_job_pool, job, migrate};
use deepref_worker::{ClaimLoopConfig, JobHandler, delivery::DeliveryAction, run_claim_loop};
use sqlx::{PgPool, Row};
use uuid::Uuid;

async fn database() -> Option<PgPool> {
    let pool = deepref_postgres::test_isolation::isolated_pool_from_env("DATABASE_URL", 8).await?;
    migrate(&pool).await.expect("migrations apply");
    Some(pool)
}

async fn project(pool: &PgPool) -> ProjectId {
    let project_id = ProjectId::new(Uuid::new_v4());
    sqlx::query("INSERT INTO projects (id,name) VALUES ($1,'claim loop fixture')")
        .bind(project_id.as_uuid())
        .execute(pool)
        .await
        .expect("project");
    project_id
}

/// Collects formatted log output so a test can assert on what was logged.
#[derive(Clone, Default)]
struct CapturedLogs(Arc<Mutex<Vec<u8>>>);

impl CapturedLogs {
    fn contents(&self) -> String {
        String::from_utf8_lossy(&self.0.lock().unwrap()).into_owned()
    }
}

impl Write for CapturedLogs {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        // Mirror into the test output so the line is visible in failures.
        std::io::stderr().write_all(buf)?;
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn capture_logs() -> CapturedLogs {
    let logs = CapturedLogs::default();
    let writer = logs.clone();
    tracing_subscriber::fmt()
        .with_ansi(false)
        .with_max_level(tracing::Level::INFO)
        .with_writer(move || writer.clone())
        .init();
    logs
}

async fn job_row(pool: &PgPool, id: Uuid) -> (String, i32, Option<String>) {
    let row = sqlx::query("SELECT state,attempts,last_error FROM jobs WHERE id=$1")
        .bind(id)
        .fetch_one(pool)
        .await
        .expect("job row");
    (row.get("state"), row.get("attempts"), row.get("last_error"))
}

/// A handler panic (a poison payload tripping an `unwrap`, an index out of
/// bounds in a parser) must fail only that job, through the normal attempt
/// budget, and leave the worker claiming the next job.
#[tokio::test]
async fn handler_panic_fails_the_job_and_the_loop_keeps_claiming() {
    let Some(pool) = database().await else {
        return;
    };
    let logs = capture_logs();
    let project_id = project(&pool).await;
    let poison_id = Uuid::new_v4();
    let healthy_id = Uuid::new_v4();
    let mut poison = job(
        poison_id,
        project_id,
        "test_poison",
        serde_json::json!({}),
        format!("claim-loop-poison:{poison_id}"),
    );
    // Claimed first, so the healthy job only runs if the loop survives.
    poison.priority = 10;
    enqueue_job_pool(&pool, &poison)
        .await
        .expect("enqueue poison");
    enqueue_job_pool(
        &pool,
        &job(
            healthy_id,
            project_id,
            "test_healthy",
            serde_json::json!({}),
            format!("claim-loop-healthy:{healthy_id}"),
        ),
    )
    .await
    .expect("enqueue healthy");

    let handler: JobHandler = Arc::new(|job: ClaimedJob, _lease: JobLease| {
        Box::pin(async move {
            if job.kind == "test_poison" {
                panic!("poison payload tripped the handler");
            }
            Ok(DeliveryAction::Ack)
        })
    });
    let shutdown = {
        let pool = pool.clone();
        async move {
            loop {
                let (state, _, _) = job_row(&pool, healthy_id).await;
                if state == "completed" {
                    return;
                }
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        }
    };
    let result = tokio::time::timeout(
        Duration::from_secs(20),
        run_claim_loop(
            pool.clone(),
            "claim-loop-test".to_owned(),
            ClaimLoopConfig {
                concurrency: 1,
                claim_lease: Duration::from_secs(30),
                drain_deadline: Duration::from_secs(5),
            },
            handler,
            shutdown,
        ),
    )
    .await
    .expect("the loop must keep claiming and complete the healthy job");
    result.expect("a handler panic must not stop the worker");

    let (state, attempts, last_error) = job_row(&pool, poison_id).await;
    assert_eq!(
        state, "queued",
        "the panicked attempt is retried with backoff"
    );
    assert_eq!(attempts, 1);
    let last_error = last_error.expect("the panic is recorded");
    assert!(
        last_error.contains("poison payload tripped the handler"),
        "last_error should carry the panic message, got {last_error}"
    );
    let (state, _, _) = job_row(&pool, healthy_id).await;
    assert_eq!(state, "completed");

    let logs = logs.contents();
    let failure_line = logs
        .lines()
        .find(|line| line.contains("WARN") && line.contains(&poison_id.to_string()))
        .unwrap_or_else(|| panic!("no WARN line names the failed job; logs:\n{logs}"));
    assert!(failure_line.contains("kind=test_poison"), "{failure_line}");
    assert!(failure_line.contains("attempts=1"), "{failure_line}");
}
