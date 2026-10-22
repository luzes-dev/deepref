#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
//! Workload pools and bounded batch claiming in the claim loop: pool
//! isolation, per-pool capacity, batch caps, mid-batch shutdown release and
//! scoped job execution. Skipped unless `DATABASE_URL` points at a reachable
//! PostgreSQL.

use std::{
    collections::HashMap,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

use deepref_app::{
    PostgresClaimTransport, PostgresDurableExecutionStore, enqueue_job_pool, job, scoped_job,
};
use deepref_application::jobs::{ClaimedJob, JobLease, JobScope, WorkRequest, WorkTransport};
use deepref_domain::{JobKind, ProjectId, WorkloadClass, WorkspaceId};
use deepref_worker::{ClaimLoopConfig, JobHandler, delivery::DeliveryAction, run_claim_loop};
use sqlx::{PgPool, Row};
use uuid::Uuid;

async fn database() -> Option<PgPool> {
    let pool = deepref_app::test_isolation::isolated_pool_from_env("DATABASE_URL", 8).await?;
    deepref_app::migrate(&pool).await.expect("migrations apply");
    Some(pool)
}

async fn project(pool: &PgPool) -> ProjectId {
    let project_id = ProjectId::new(Uuid::new_v4());
    sqlx::query("INSERT INTO projects (id,name) VALUES ($1,'workload pool fixture')")
        .bind(project_id.as_uuid())
        .execute(pool)
        .await
        .expect("project");
    project_id
}

async fn job_row(pool: &PgPool, id: Uuid) -> (String, i32) {
    let row = sqlx::query("SELECT state,attempts FROM jobs WHERE id=$1")
        .bind(id)
        .fetch_one(pool)
        .await
        .expect("job row");
    (row.get("state"), row.get("attempts"))
}

async fn wait_for(mut condition: impl FnMut() -> bool) {
    while !condition() {
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

fn loop_config(
    pools: &[WorkloadClass],
    concurrency: &[(WorkloadClass, usize)],
    max_batch: usize,
) -> ClaimLoopConfig {
    ClaimLoopConfig {
        pools: pools.to_vec(),
        pool_concurrency: concurrency.iter().copied().collect::<HashMap<_, _>>(),
        max_batch,
        claim_lease: Duration::from_secs(30),
        drain_deadline: Duration::from_secs(5),
    }
}

/// Wraps the PostgreSQL transport to record every batch request and result.
#[derive(Clone)]
struct RecordingTransport {
    inner: PostgresClaimTransport,
    calls: Arc<Mutex<Vec<(usize, usize)>>>,
}

impl RecordingTransport {
    fn new(pool: &PgPool) -> Self {
        Self {
            inner: PostgresClaimTransport {
                store: PostgresDurableExecutionStore { pool: pool.clone() },
            },
            calls: Arc::new(Mutex::new(Vec::new())),
        }
    }

    fn calls(&self) -> Vec<(usize, usize)> {
        self.calls.lock().unwrap().clone()
    }
}

impl WorkTransport for RecordingTransport {
    async fn next_work(&self, request: WorkRequest<'_>) -> anyhow::Result<Vec<ClaimedJob>> {
        let jobs = self.inner.next_work(request).await?;
        self.calls
            .lock()
            .unwrap()
            .push((request.max_jobs, jobs.len()));
        Ok(jobs)
    }
}

/// Claims like the PostgreSQL transport, then raises the shutdown signal as
/// soon as a batch comes back: the loop observes it before dispatching the
/// batch's first job.
struct ShutdownAfterBatchTransport {
    inner: PostgresClaimTransport,
    shutdown: Mutex<Option<tokio::sync::oneshot::Sender<()>>>,
}

/// Locks the claimed row before signaling shutdown so releasing the unstarted
/// claim blocks on PostgreSQL until the common shutdown deadline expires.
struct LockedReleaseTransport {
    inner: PostgresClaimTransport,
    pool: PgPool,
    job_id: Uuid,
    lock_ready: Mutex<Option<tokio::sync::oneshot::Sender<()>>>,
    unlock: Mutex<Option<tokio::sync::oneshot::Receiver<()>>>,
    shutdown: Mutex<Option<tokio::sync::oneshot::Sender<()>>>,
}

impl WorkTransport for LockedReleaseTransport {
    async fn next_work(&self, request: WorkRequest<'_>) -> anyhow::Result<Vec<ClaimedJob>> {
        let jobs = self.inner.next_work(request).await?;
        let lock_ready = if jobs.is_empty() {
            None
        } else {
            self.lock_ready.lock().unwrap().take()
        };
        if let Some(lock_ready) = lock_ready {
            let mut transaction = self.pool.begin().await?;
            sqlx::query("SELECT id FROM jobs WHERE id=$1 FOR UPDATE")
                .bind(self.job_id)
                .fetch_one(&mut *transaction)
                .await?;
            let unlock = self.unlock.lock().unwrap().take().unwrap();
            tokio::spawn(async move {
                let _ = unlock.await;
                let _ = transaction.rollback().await;
            });
            let _ = lock_ready.send(());
            if let Some(shutdown) = self.shutdown.lock().unwrap().take() {
                let _ = shutdown.send(());
            }
        }
        Ok(jobs)
    }
}

impl WorkTransport for ShutdownAfterBatchTransport {
    async fn next_work(&self, request: WorkRequest<'_>) -> anyhow::Result<Vec<ClaimedJob>> {
        let jobs = self.inner.next_work(request).await?;
        if !jobs.is_empty()
            && let Some(sender) = self.shutdown.lock().unwrap().take()
        {
            let _ = sender.send(());
        }
        Ok(jobs)
    }
}

/// A worker serving `documents` never claims `ai`-class work: pool isolation
/// holds even when the other class is the only queued work.
#[tokio::test]
async fn a_pool_worker_never_claims_another_workload_class() {
    let Some(pool) = database().await else {
        return;
    };
    let project_id = project(&pool).await;
    let documents_id = Uuid::new_v4();
    let ai_id = Uuid::new_v4();
    enqueue_job_pool(
        &pool,
        &job(
            documents_id,
            project_id,
            JobKind::ParseDocument,
            serde_json::json!({"document_id": Uuid::new_v4()}),
            format!("pools-documents:{documents_id}"),
        ),
    )
    .await
    .expect("enqueue documents");
    enqueue_job_pool(
        &pool,
        &job(
            ai_id,
            project_id,
            JobKind::AssistantAgentRun,
            serde_json::json!({}),
            format!("pools-ai:{ai_id}"),
        ),
    )
    .await
    .expect("enqueue ai");

    let foreign = Arc::new(AtomicUsize::new(0));
    let handler: JobHandler = {
        let foreign = Arc::clone(&foreign);
        Arc::new(move |job: ClaimedJob, _lease: JobLease| {
            let foreign = Arc::clone(&foreign);
            Box::pin(async move {
                if job.kind.workload_class() != WorkloadClass::Documents {
                    foreign.fetch_add(1, Ordering::SeqCst);
                }
                Ok(DeliveryAction::Ack)
            })
        })
    };
    let shutdown = {
        let pool = pool.clone();
        async move {
            loop {
                let (state, _) = job_row(&pool, documents_id).await;
                if state == "completed" {
                    return;
                }
                tokio::time::sleep(Duration::from_millis(25)).await;
            }
        }
    };
    let result = tokio::time::timeout(
        Duration::from_secs(20),
        run_claim_loop(
            PostgresClaimTransport {
                store: PostgresDurableExecutionStore { pool: pool.clone() },
            },
            PostgresDurableExecutionStore { pool: pool.clone() },
            "pools-isolation-test".to_owned(),
            loop_config(
                &[WorkloadClass::Documents],
                &[(WorkloadClass::Documents, 2)],
                2,
            ),
            handler,
            shutdown,
        ),
    )
    .await
    .expect("the documents pool drains its own work");
    result.expect("the claim loop runs clean");

    assert_eq!(
        foreign.load(Ordering::SeqCst),
        0,
        "no ai work was delivered"
    );
    let (state, attempts) = job_row(&pool, documents_id).await;
    assert_eq!(state, "completed");
    assert_eq!(attempts, 1);
    let (state, attempts) = job_row(&pool, ai_id).await;
    assert_eq!(state, "queued", "ai work stays queued for an ai worker");
    assert_eq!(attempts, 0, "ai work is never claimed, let alone attempted");
}

#[tokio::test]
async fn a_stalled_shutdown_release_uses_the_drain_deadline_and_leaves_the_lease_for_recovery() {
    let Some(pool) = database().await else {
        return;
    };
    let project_id = project(&pool).await;
    let job_id = Uuid::new_v4();
    enqueue_job_pool(
        &pool,
        &job(
            job_id,
            project_id,
            JobKind::ParseDocument,
            serde_json::json!({"document_id": Uuid::new_v4()}),
            format!("pools-locked-release:{job_id}"),
        ),
    )
    .await
    .expect("enqueue fixture");

    let (lock_ready, locked) = tokio::sync::oneshot::channel();
    let (unlock, unlock_signal) = tokio::sync::oneshot::channel();
    let (shutdown, shutdown_signal) = tokio::sync::oneshot::channel();
    let transport = LockedReleaseTransport {
        inner: PostgresClaimTransport {
            store: PostgresDurableExecutionStore { pool: pool.clone() },
        },
        pool: pool.clone(),
        job_id,
        lock_ready: Mutex::new(Some(lock_ready)),
        unlock: Mutex::new(Some(unlock_signal)),
        shutdown: Mutex::new(Some(shutdown)),
    };
    let handler_called = Arc::new(AtomicUsize::new(0));
    let handler: JobHandler = {
        let handler_called = Arc::clone(&handler_called);
        Arc::new(move |_job: ClaimedJob, _lease: JobLease| {
            let handler_called = Arc::clone(&handler_called);
            Box::pin(async move {
                handler_called.fetch_add(1, Ordering::SeqCst);
                Ok(DeliveryAction::Ack)
            })
        })
    };
    let loop_task = tokio::spawn(run_claim_loop(
        transport,
        PostgresDurableExecutionStore { pool: pool.clone() },
        "pools-locked-release-test".to_owned(),
        ClaimLoopConfig {
            pools: vec![WorkloadClass::Documents],
            pool_concurrency: HashMap::from([(WorkloadClass::Documents, 1)]),
            max_batch: 1,
            claim_lease: Duration::from_secs(30),
            drain_deadline: Duration::from_millis(200),
        },
        handler,
        async move {
            let _ = shutdown_signal.await;
        },
    ));

    tokio::time::timeout(Duration::from_secs(5), locked)
        .await
        .expect("transport locks the claimed row")
        .expect("row lock is held");
    tokio::time::timeout(Duration::from_secs(2), loop_task)
        .await
        .expect("shutdown returns within its one deadline")
        .expect("claim loop task joins")
        .expect("timed-out unstarted releases are left for recovery");

    let row = sqlx::query("SELECT state,attempts,claim_seq FROM jobs WHERE id=$1")
        .bind(job_id)
        .fetch_one(&pool)
        .await
        .expect("claimed row");
    assert_eq!(row.get::<String, _>("state"), "running");
    assert_eq!(row.get::<i32, _>("attempts"), 1);
    assert_eq!(row.get::<i64, _>("claim_seq"), 1);
    assert_eq!(handler_called.load(Ordering::SeqCst), 0);

    // Abandoning the release future does not cancel its statement on the
    // server: once the lock is gone PostgreSQL may still apply that fenced
    // release, refunding the unstarted attempt. Otherwise lease recovery
    // requeues the claim and keeps the spent attempt. Both are correct.
    let _ = unlock.send(());
    tokio::time::sleep(Duration::from_millis(100)).await;
    sqlx::query(
        "UPDATE jobs SET leased_until=now()-interval '1 second' WHERE id=$1 AND state='running'",
    )
    .bind(job_id)
    .execute(&pool)
    .await
    .expect("expire the abandoned lease");
    deepref_app::recover_expired_jobs(&pool)
        .await
        .expect("lease recovery requeues the abandoned claim");
    let recovered = sqlx::query("SELECT state,attempts FROM jobs WHERE id=$1")
        .bind(job_id)
        .fetch_one(&pool)
        .await
        .expect("recovered row");
    assert_eq!(recovered.get::<String, _>("state"), "queued");
    assert!(
        matches!(recovered.get::<i32, _>("attempts"), 0 | 1),
        "a late fenced release refunds the attempt; recovery keeps it"
    );
    assert_eq!(handler_called.load(Ordering::SeqCst), 0);
}

/// A tick claims at most `min(free pool capacity, max_batch)` jobs in one
/// statement. A saturated pool claims nothing until handlers finish; there is
/// no unbounded inner claim loop.
#[tokio::test]
async fn batch_claims_are_bounded_by_free_pool_capacity() {
    let Some(pool) = database().await else {
        return;
    };
    let project_id = project(&pool).await;
    let mut ids = Vec::new();
    for index in 0..6 {
        let id = Uuid::new_v4();
        ids.push(id);
        enqueue_job_pool(
            &pool,
            &job(
                id,
                project_id,
                JobKind::ParseDocument,
                serde_json::json!({"document_id": Uuid::new_v4(), "index": index}),
                format!("pools-capacity:{id}"),
            ),
        )
        .await
        .expect("enqueue fixture");
    }

    let (release, released) = tokio::sync::watch::channel(false);
    let started = Arc::new(AtomicUsize::new(0));
    let handler: JobHandler = {
        let started = Arc::clone(&started);
        Arc::new(move |_job: ClaimedJob, _lease: JobLease| {
            let started = Arc::clone(&started);
            let released = released.clone();
            Box::pin(async move {
                started.fetch_add(1, Ordering::SeqCst);
                let mut released = released;
                while !*released.borrow() {
                    if released.changed().await.is_err() {
                        break;
                    }
                }
                Ok(DeliveryAction::Ack)
            })
        })
    };
    let transport = RecordingTransport::new(&pool);
    let shutdown = {
        let pool = pool.clone();
        async move {
            loop {
                let done: i64 = sqlx::query_scalar(
                    "SELECT count(*) FROM jobs WHERE state='completed' AND dedupe_key LIKE 'pools-capacity:%'",
                )
                .fetch_one(&pool)
                .await
                .expect("count");
                if done == 6 {
                    return;
                }
                tokio::time::sleep(Duration::from_millis(25)).await;
            }
        }
    };
    let loop_task = tokio::spawn(run_claim_loop(
        transport.clone(),
        PostgresDurableExecutionStore { pool: pool.clone() },
        "pools-capacity-test".to_owned(),
        // Capacity 3 binds the batch below the configured cap of 5.
        loop_config(
            &[WorkloadClass::Documents],
            &[(WorkloadClass::Documents, 3)],
            5,
        ),
        handler,
        shutdown,
    ));

    // Free capacity 3: at most 3 handlers run, and a saturated pool claims
    // nothing at all until capacity returns.
    wait_for(|| started.load(Ordering::SeqCst) == 3).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(
        started.load(Ordering::SeqCst),
        3,
        "free capacity bounds the running handlers"
    );
    let blocked_calls = transport.calls();
    assert_eq!(blocked_calls.len(), 1, "a saturated pool claims nothing");
    assert!(
        blocked_calls.iter().all(|(requested, _)| *requested <= 3),
        "a tick never requests more than free capacity: {blocked_calls:?}"
    );
    assert!(
        blocked_calls.iter().all(|(_, claimed)| *claimed <= 3),
        "a tick never claims more than free capacity: {blocked_calls:?}"
    );

    release.send_replace(true);
    tokio::time::timeout(Duration::from_secs(20), loop_task)
        .await
        .expect("the loop drains the queue")
        .expect("the claim loop runs clean")
        .expect("the claim loop runs clean");
    let calls = transport.calls();
    assert!(
        calls
            .iter()
            .all(|(requested, claimed)| *claimed <= *requested && *requested <= 3),
        "every batch stayed within free capacity: {calls:?}"
    );
    for id in ids {
        let (state, attempts) = job_row(&pool, id).await;
        assert_eq!(state, "completed");
        assert_eq!(attempts, 1);
    }
}

#[tokio::test]
async fn a_poison_row_is_quarantined_while_valid_batch_siblings_run() {
    let Some(pool) = database().await else {
        return;
    };
    let project_id = project(&pool).await;
    let poison_id = Uuid::new_v4();
    // The poison row carries a scope kind from a newer release than this
    // binary decodes (the database allows it; the claim's row decoder does
    // not). The claim-decode quarantine exists for exactly that
    // forward-compatibility case. (Claims are capability-aware, so an unknown
    // job kind is never claimed at all.)
    sqlx::query("ALTER TABLE jobs DROP CONSTRAINT jobs_scope_check")
        .execute(&pool)
        .await
        .expect("drop the scope check in the isolated database");
    sqlx::query(
        "ALTER TABLE jobs ADD CONSTRAINT jobs_scope_check CHECK (
           (scope = 'project' AND workspace_id IS NULL
             AND (project_id IS NOT NULL OR state IN ('completed','failed','dead')))
         OR (scope = 'workspace' AND project_id IS NULL AND workspace_id IS NOT NULL)
         OR (scope = 'platform' AND project_id IS NULL AND workspace_id IS NULL)
         OR (scope = 'organization' AND project_id IS NULL AND workspace_id IS NULL)
        ) NOT VALID",
    )
    .execute(&pool)
    .await
    .expect("a newer release's scope kind joins the allowed set");
    sqlx::query(
        "INSERT INTO jobs (id,scope,kind,workload_class,priority,payload,dedupe_key)
         VALUES ($1,'organization','durable_run_step','workflow',2147483647,'{}',$2)",
    )
    .bind(poison_id)
    .bind(format!("pools-poison:{poison_id}"))
    .execute(&pool)
    .await
    .expect("poison row");
    let valid_ids = [Uuid::new_v4(), Uuid::new_v4()];
    for id in valid_ids {
        enqueue_job_pool(
            &pool,
            &job(
                id,
                project_id,
                JobKind::DurableRunStep,
                serde_json::json!({"fixture": id}),
                format!("pools-poison-valid:{id}"),
            ),
        )
        .await
        .expect("valid sibling");
    }

    let handled = Arc::new(Mutex::new(Vec::new()));
    let handler: JobHandler = {
        let handled = Arc::clone(&handled);
        Arc::new(move |job: ClaimedJob, _lease: JobLease| {
            let handled = Arc::clone(&handled);
            Box::pin(async move {
                handled.lock().unwrap().push(job.id);
                Ok(DeliveryAction::Ack)
            })
        })
    };
    let shutdown = {
        let pool = pool.clone();
        async move {
            loop {
                let (dead, complete): (i64, i64) = sqlx::query_as(
                    "SELECT count(*) FILTER (WHERE id=$1 AND state='dead'),
                            count(*) FILTER (WHERE dedupe_key LIKE 'pools-poison-valid:%' AND state='completed')
                     FROM jobs",
                )
                .bind(poison_id)
                .fetch_one(&pool)
                .await
                .expect("poison completion counts");
                if dead == 1 && complete == 2 {
                    return;
                }
                tokio::time::sleep(Duration::from_millis(25)).await;
            }
        }
    };
    tokio::time::timeout(
        Duration::from_secs(10),
        run_claim_loop(
            PostgresClaimTransport {
                store: PostgresDurableExecutionStore { pool: pool.clone() },
            },
            PostgresDurableExecutionStore { pool: pool.clone() },
            "pools-poison-test".to_owned(),
            loop_config(
                &[WorkloadClass::Workflow],
                &[(WorkloadClass::Workflow, 3)],
                3,
            ),
            handler,
            shutdown,
        ),
    )
    .await
    .expect("valid siblings drain after poison quarantine")
    .expect("claim loop succeeds");

    let handled = handled.lock().unwrap().clone();
    assert_eq!(handled.len(), 2);
    assert!(valid_ids.iter().all(|id| handled.contains(id)));
    let poison = sqlx::query("SELECT state,attempts,last_error FROM jobs WHERE id=$1")
        .bind(poison_id)
        .fetch_one(&pool)
        .await
        .expect("poison row");
    assert_eq!(poison.get::<String, _>("state"), "dead");
    assert_eq!(poison.get::<i32, _>("attempts"), 0);
    assert!(
        poison
            .get::<String, _>("last_error")
            .contains("claim decode"),
        "the quarantine names the decode failure"
    );
    for id in valid_ids {
        let (state, attempts) = job_row(&pool, id).await;
        assert_eq!(state, "completed");
        assert_eq!(attempts, 1);
    }
}

/// The configured max batch caps every claim statement even when more
/// capacity is free.
#[tokio::test]
async fn the_configured_batch_cap_bounds_every_claim_statement() {
    let Some(pool) = database().await else {
        return;
    };
    let project_id = project(&pool).await;
    for _ in 0..5 {
        let id = Uuid::new_v4();
        enqueue_job_pool(
            &pool,
            &job(
                id,
                project_id,
                JobKind::RecomputeMetrics,
                serde_json::json!({}),
                format!("pools-batch-cap:{id}"),
            ),
        )
        .await
        .expect("enqueue fixture");
    }
    let handler: JobHandler = Arc::new(|_job: ClaimedJob, _lease: JobLease| {
        Box::pin(async move { Ok(DeliveryAction::Ack) })
    });
    let transport = RecordingTransport::new(&pool);
    let shutdown = {
        let pool = pool.clone();
        async move {
            loop {
                let done: i64 = sqlx::query_scalar(
                    "SELECT count(*) FROM jobs WHERE state='completed' AND dedupe_key LIKE 'pools-batch-cap:%'",
                )
                .fetch_one(&pool)
                .await
                .expect("count");
                if done == 5 {
                    return;
                }
                tokio::time::sleep(Duration::from_millis(25)).await;
            }
        }
    };
    let result = tokio::time::timeout(
        Duration::from_secs(20),
        run_claim_loop(
            transport.clone(),
            PostgresDurableExecutionStore { pool: pool.clone() },
            "pools-batch-cap-test".to_owned(),
            // Capacity 8 is ample; the batch cap of 2 binds every statement.
            loop_config(
                &[WorkloadClass::Projection],
                &[(WorkloadClass::Projection, 8)],
                2,
            ),
            handler,
            shutdown,
        ),
    )
    .await
    .expect("the queue drains within the batch cap");
    result.expect("the claim loop runs clean");
    let calls = transport.calls();
    assert!(!calls.is_empty());
    assert!(
        calls
            .iter()
            .all(|(requested, claimed)| *claimed <= *requested && *requested <= 2),
        "no tick exceeds the configured max batch: {calls:?}"
    );
}

/// A saturated pool neither drains nor starves another: with `documents` at
/// capacity, `ai` work still runs, and the `documents` queue is not drained
/// beyond its capacity.
#[tokio::test]
async fn a_saturated_pool_neither_drains_nor_starves_another() {
    let Some(pool) = database().await else {
        return;
    };
    let project_id = project(&pool).await;
    let blocking_id = Uuid::new_v4();
    let waiting_id = Uuid::new_v4();
    let ai_id = Uuid::new_v4();
    for (id, kind, payload) in [
        (
            blocking_id,
            JobKind::ParseDocument,
            serde_json::json!({"document_id": Uuid::new_v4(), "blocker": true}),
        ),
        (
            waiting_id,
            JobKind::ParseDocument,
            serde_json::json!({"document_id": Uuid::new_v4()}),
        ),
        (ai_id, JobKind::AssistantAgentRun, serde_json::json!({})),
    ] {
        enqueue_job_pool(
            &pool,
            &job(
                id,
                project_id,
                kind,
                payload,
                format!("pools-saturation:{id}"),
            ),
        )
        .await
        .expect("enqueue fixture");
    }

    let (release, released) = tokio::sync::watch::channel(false);
    let documents_started = Arc::new(AtomicUsize::new(0));
    let handler: JobHandler = {
        let documents_started = Arc::clone(&documents_started);
        Arc::new(move |job: ClaimedJob, _lease: JobLease| {
            let documents_started = Arc::clone(&documents_started);
            let released = released.clone();
            let blocking = job.id == blocking_id;
            Box::pin(async move {
                if job.kind.workload_class() == WorkloadClass::Documents {
                    documents_started.fetch_add(1, Ordering::SeqCst);
                }
                if blocking {
                    let mut released = released;
                    while !*released.borrow() {
                        if released.changed().await.is_err() {
                            break;
                        }
                    }
                }
                Ok(DeliveryAction::Ack)
            })
        })
    };
    let shutdown = {
        let pool = pool.clone();
        async move {
            loop {
                let done: i64 = sqlx::query_scalar(
                    "SELECT count(*) FROM jobs WHERE state='completed' AND dedupe_key LIKE 'pools-saturation:%'",
                )
                .fetch_one(&pool)
                .await
                .expect("count");
                if done == 3 {
                    return;
                }
                tokio::time::sleep(Duration::from_millis(25)).await;
            }
        }
    };
    let loop_task = tokio::spawn(run_claim_loop(
        PostgresClaimTransport {
            store: PostgresDurableExecutionStore { pool: pool.clone() },
        },
        PostgresDurableExecutionStore { pool: pool.clone() },
        "pools-saturation-test".to_owned(),
        loop_config(
            &[WorkloadClass::Documents, WorkloadClass::Ai],
            &[(WorkloadClass::Documents, 1), (WorkloadClass::Ai, 1)],
            2,
        ),
        handler,
        shutdown,
    ));

    // While one documents handler holds the pool's only capacity:
    wait_for(|| documents_started.load(Ordering::SeqCst) == 1).await;
    tokio::time::timeout(
        Duration::from_secs(10),
        wait_for_async(|| async {
            let (state, _) = job_row(&pool, ai_id).await;
            state == "completed"
        }),
    )
    .await
    .expect("the ai pool is not starved by the saturated documents pool");
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(
        documents_started.load(Ordering::SeqCst),
        1,
        "the saturated documents pool is not drained beyond its capacity"
    );
    let (_, attempts) = job_row(&pool, waiting_id).await;
    assert_eq!(attempts, 0, "the second documents job is not even claimed");

    release.send_replace(true);
    tokio::time::timeout(Duration::from_secs(20), loop_task)
        .await
        .expect("the loop drains both pools")
        .expect("the claim loop runs clean")
        .expect("the claim loop runs clean");
    let (state, _) = job_row(&pool, waiting_id).await;
    assert_eq!(
        state, "completed",
        "waiting work still runs once capacity frees"
    );
}

/// SIGTERM mid-batch: the batch is abandoned at once, every unstarted claim is
/// released (queued, immediately claimable, attempt refunded) and no handler
/// runs. The shutdown never waits out a lease.
#[tokio::test]
async fn shutdown_mid_batch_releases_unstarted_claims() {
    let Some(pool) = database().await else {
        return;
    };
    let project_id = project(&pool).await;
    let mut ids = Vec::new();
    for _ in 0..4 {
        let id = Uuid::new_v4();
        ids.push(id);
        enqueue_job_pool(
            &pool,
            &job(
                id,
                project_id,
                JobKind::ParseDocument,
                serde_json::json!({"document_id": Uuid::new_v4()}),
                format!("pools-shutdown:{id}"),
            ),
        )
        .await
        .expect("enqueue fixture");
    }

    let started = Arc::new(AtomicUsize::new(0));
    let handler: JobHandler = {
        let started = Arc::clone(&started);
        Arc::new(move |_job: ClaimedJob, _lease: JobLease| {
            let started = Arc::clone(&started);
            Box::pin(async move {
                started.fetch_add(1, Ordering::SeqCst);
                Ok(DeliveryAction::Ack)
            })
        })
    };
    let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel::<()>();
    let transport = ShutdownAfterBatchTransport {
        inner: PostgresClaimTransport {
            store: PostgresDurableExecutionStore { pool: pool.clone() },
        },
        shutdown: Mutex::new(Some(shutdown_tx)),
    };
    let shutdown = async move {
        let _ = shutdown_rx.await;
    };
    let result = tokio::time::timeout(
        Duration::from_secs(10),
        run_claim_loop(
            transport,
            PostgresDurableExecutionStore { pool: pool.clone() },
            "pools-shutdown-test".to_owned(),
            loop_config(
                &[WorkloadClass::Documents],
                &[(WorkloadClass::Documents, 4)],
                4,
            ),
            handler,
            shutdown,
        ),
    )
    .await
    .expect("shutdown is observed mid-batch, not after the lease expires");
    result.expect("the claim loop runs clean");
    assert_eq!(started.load(Ordering::SeqCst), 0, "no handler ran");

    for id in &ids {
        let row = sqlx::query(
            "SELECT state,attempts,lease_owner,leased_until,available_at FROM jobs WHERE id=$1",
        )
        .bind(id)
        .fetch_one(&pool)
        .await
        .expect("job row");
        assert_eq!(
            row.get::<String, _>("state"),
            "queued",
            "released claims are queued"
        );
        assert_eq!(
            row.get::<i32, _>("attempts"),
            0,
            "released claims refund their attempt"
        );
        assert!(row.get::<Option<String>, _>("lease_owner").is_none());
        assert!(
            row.get::<Option<chrono::DateTime<chrono::Utc>>, _>("leased_until")
                .is_none()
        );
        assert!(
            row.get::<chrono::DateTime<chrono::Utc>, _>("available_at") <= chrono::Utc::now(),
            "released claims are immediately claimable"
        );
    }
    // The released work is claimable again right away: the shutdown left
    // nothing leased and nothing lost.
    let store = PostgresDurableExecutionStore { pool: pool.clone() };
    use deepref_application::jobs::DurableExecutionStore;
    let reclaimed = store
        .claim("pools-shutdown-reclaim", Duration::from_secs(30), &[], 10)
        .await
        .expect("reclaim");
    assert_eq!(
        reclaimed.len(),
        ids.len(),
        "every released claim is claimable"
    );
}

/// Workspace- and platform-scoped jobs execute through the claim loop with no
/// project row: the handler sees the typed scope, and the persisted rows keep
/// it.
#[tokio::test]
async fn scoped_jobs_execute_through_the_claim_loop() {
    let Some(pool) = database().await else {
        return;
    };
    let workspace_id = WorkspaceId::new(Uuid::new_v4());
    let workspace_job_id = Uuid::new_v4();
    let platform_job_id = Uuid::new_v4();
    enqueue_job_pool(
        &pool,
        &scoped_job(
            workspace_job_id,
            JobScope::Workspace(workspace_id),
            JobKind::RecomputeMetrics,
            serde_json::json!({"fixture": "workspace"}),
            format!("pools-scoped-workspace:{workspace_job_id}"),
        ),
    )
    .await
    .expect("enqueue workspace job");
    enqueue_job_pool(
        &pool,
        &scoped_job(
            platform_job_id,
            JobScope::Platform,
            JobKind::RecomputeMetrics,
            serde_json::json!({"fixture": "platform"}),
            format!("pools-scoped-platform:{platform_job_id}"),
        ),
    )
    .await
    .expect("enqueue platform job");

    let seen = Arc::new(Mutex::new(Vec::<JobScope>::new()));
    let handler: JobHandler = {
        let seen = Arc::clone(&seen);
        Arc::new(move |job: ClaimedJob, _lease: JobLease| {
            let seen = Arc::clone(&seen);
            Box::pin(async move {
                seen.lock().unwrap().push(job.scope);
                Ok(DeliveryAction::Ack)
            })
        })
    };
    let shutdown = {
        let pool = pool.clone();
        async move {
            loop {
                let done: i64 = sqlx::query_scalar(
                    "SELECT count(*) FROM jobs WHERE state='completed' AND dedupe_key LIKE 'pools-scoped-%'",
                )
                .fetch_one(&pool)
                .await
                .expect("count");
                if done == 2 {
                    return;
                }
                tokio::time::sleep(Duration::from_millis(25)).await;
            }
        }
    };
    let result = tokio::time::timeout(
        Duration::from_secs(20),
        run_claim_loop(
            PostgresClaimTransport {
                store: PostgresDurableExecutionStore { pool: pool.clone() },
            },
            PostgresDurableExecutionStore { pool: pool.clone() },
            "pools-scoped-test".to_owned(),
            loop_config(
                &[WorkloadClass::Projection],
                &[(WorkloadClass::Projection, 2)],
                2,
            ),
            handler,
            shutdown,
        ),
    )
    .await
    .expect("scoped work executes");
    result.expect("the claim loop runs clean");

    let seen = seen.lock().unwrap().clone();
    assert_eq!(seen.len(), 2, "both scoped jobs reached their handler");
    assert!(
        seen.contains(&JobScope::Platform),
        "the platform-scoped job executed, got {seen:?}"
    );
    assert!(
        seen.contains(&JobScope::Workspace(workspace_id)),
        "the workspace-scoped job executed, got {seen:?}"
    );
    for (id, scope, workspace) in [
        (workspace_job_id, "workspace", Some(workspace_id.as_uuid())),
        (platform_job_id, "platform", None),
    ] {
        let row = sqlx::query(
            "SELECT state,attempts,project_id,workspace_id,scope FROM jobs WHERE id=$1",
        )
        .bind(id)
        .fetch_one(&pool)
        .await
        .expect("job row");
        assert_eq!(row.get::<String, _>("state"), "completed");
        assert_eq!(row.get::<i32, _>("attempts"), 1);
        assert_eq!(row.get::<String, _>("scope"), scope);
        assert!(row.get::<Option<Uuid>, _>("project_id").is_none());
        assert_eq!(row.get::<Option<Uuid>, _>("workspace_id"), workspace);
    }
    let projects: i64 = sqlx::query_scalar("SELECT count(*) FROM projects")
        .fetch_one(&pool)
        .await
        .expect("projects");
    assert_eq!(projects, 0, "no dummy project is created for scoped work");
}

/// `wait_for` over async conditions (the blocking helper covers sync ones).
async fn wait_for_async<F, Fut>(mut condition: F)
where
    F: FnMut() -> Fut,
    Fut: Future<Output = bool>,
{
    while !condition().await {
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}
