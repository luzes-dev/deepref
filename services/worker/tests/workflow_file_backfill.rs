#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]
//! The durable backfill job for `workflow_files`: scheduled by the
//! reconciler, run by the worker, resumable after the worker dies mid-job.

use std::{sync::Arc, time::Duration};

use deepref_app::{
    claim_job, complete_job, recover_expired_jobs, schedule_workflow_file_backfills,
};
use deepref_documents::{DocumentStore, ObjectKey};
use deepref_worker::{delivery::DeliveryAction, processor::handle_job_with_documents_owned};
use sqlx::PgPool;
use uuid::Uuid;

async fn database() -> Option<PgPool> {
    deepref_app::test_isolation::isolated_pool_from_env("DATABASE_URL", 8).await
}

async fn legacy_files(pool: &PgPool, count: u8) -> (Uuid, Uuid, Vec<Uuid>) {
    let project = Uuid::new_v4();
    let run = Uuid::new_v4();
    sqlx::query("INSERT INTO projects (id,name) VALUES ($1,'backfill job')")
        .bind(project)
        .execute(pool)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO durable_runs (id, project_id, kind, actor_kind, actor_id, idempotency_key, status)
         VALUES ($1,$2,'workflow','user','fixture',$1::text,'queued')",
    )
    .bind(run)
    .bind(project)
    .execute(pool)
    .await
    .unwrap();
    let mut files = Vec::new();
    for index in 0..count {
        files.push(
            sqlx::query_scalar(
                "INSERT INTO workflow_files (project_id, run_id, node_id, name, content_type, content)
                 VALUES ($1,$2,'export','legacy.csv','text/csv',$3) RETURNING id",
            )
            .bind(project)
            .bind(run)
            .bind(vec![index; 16])
            .fetch_one(pool)
            .await
            .unwrap(),
        );
    }
    (project, run, files)
}

fn config() -> deepref_worker::config::WorkerConfig {
    deepref_worker::config::WorkerConfig::from_source(&|name| {
        (name == "APP_ENV").then(|| "local".into())
    })
    .expect("local worker configuration")
}

#[tokio::test]
async fn the_worker_job_moves_scheduled_files_and_completes() {
    let Some(pool) = database().await else { return };
    let store = Arc::new(DocumentStore::memory());
    let (project, run, files) = legacy_files(&pool, 3).await;

    assert_eq!(
        schedule_workflow_file_backfills(&pool, 10).await.unwrap(),
        1
    );
    let job = claim_job(&pool, "backfill-worker", Duration::from_secs(30))
        .await
        .unwrap()
        .expect("scheduled job is claimable");
    assert_eq!(job.kind, deepref_domain::JobKind::BackfillWorkflowFiles);
    let lease = job.lease("backfill-worker");
    let action = handle_job_with_documents_owned(
        pool.clone(),
        &job,
        &lease,
        Duration::from_secs(30),
        Some(Arc::clone(&store)),
        None,
        &config(),
    )
    .await
    .expect("job succeeds");
    assert_eq!(action, DeliveryAction::Ack);
    assert!(complete_job(&pool, &lease).await.unwrap());

    for file in files {
        let key: Option<String> =
            sqlx::query_scalar("SELECT storage_key FROM workflow_files WHERE id=$1")
                .bind(file)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(
            key.unwrap(),
            ObjectKey::workflow_file(project, run, file).as_str()
        );
    }
    assert_eq!(
        schedule_workflow_file_backfills(&pool, 10).await.unwrap(),
        0
    );
}

#[tokio::test]
async fn a_job_whose_worker_died_is_requeued_and_finishes_the_remaining_files() {
    let Some(pool) = database().await else { return };
    let store = Arc::new(DocumentStore::memory());
    let (project, run, files) = legacy_files(&pool, 4).await;

    schedule_workflow_file_backfills(&pool, 10).await.unwrap();
    let job = claim_job(&pool, "doomed-worker", Duration::from_secs(30))
        .await
        .unwrap()
        .expect("job");

    // The worker moved two files, then died without settling the job.
    deepref_app::backfill_workflow_files(&pool, &store, job.project_id, 2)
        .await
        .unwrap();
    sqlx::query("UPDATE jobs SET leased_until = now() - interval '1 minute', available_at = now() WHERE id=$1")
        .bind(job.id)
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(recover_expired_jobs(&pool).await.unwrap(), 1);
    sqlx::query("UPDATE jobs SET available_at = now() WHERE id=$1")
        .bind(job.id)
        .execute(&pool)
        .await
        .unwrap();

    let retry = claim_job(&pool, "second-worker", Duration::from_secs(30))
        .await
        .unwrap()
        .expect("the same job is claimed again");
    assert_eq!(retry.id, job.id);
    let lease = retry.lease("second-worker");
    assert_eq!(
        handle_job_with_documents_owned(
            pool.clone(),
            &retry,
            &lease,
            Duration::from_secs(30),
            Some(Arc::clone(&store)),
            None,
            &config(),
        )
        .await
        .unwrap(),
        DeliveryAction::Ack
    );
    let pending: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM workflow_files WHERE project_id=$1 AND storage_key IS NULL",
    )
    .bind(project)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(pending, 0);
    for file in files {
        store
            .get_object(&ObjectKey::workflow_file(project, run, file), 1024)
            .await
            .expect("every file has its object");
    }
}

#[tokio::test]
async fn a_payload_naming_another_project_is_refused() {
    let Some(pool) = database().await else { return };
    let (project, _, _) = legacy_files(&pool, 1).await;
    let job = deepref_application::jobs::ClaimedJob {
        id: Uuid::new_v4(),
        project_id: project.into(),
        kind: deepref_domain::JobKind::BackfillWorkflowFiles,
        payload: serde_json::json!({ "project_id": Uuid::new_v4() }),
        attempts: 1,
        max_attempts: 5,
    };
    let result = handle_job_with_documents_owned(
        pool.clone(),
        &job,
        &job.lease("w"),
        Duration::from_secs(30),
        Some(Arc::new(DocumentStore::memory())),
        None,
        &config(),
    )
    .await;
    assert!(result.is_err());
}
