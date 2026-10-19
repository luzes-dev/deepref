use std::time::Duration;

use sqlx::PgPool;
use tokio::sync::watch;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ReconcileReport {
    pub expired_doi_leases: u64,
    pub repaired_work: u64,
    pub expired_jobs: u64,
    pub workflow_file_backfills: u64,
}

/// Projects whose pending workflow files are scheduled per reconciliation pass.
const WORKFLOW_FILE_BACKFILL_PROJECTS: i64 = 100;

pub async fn run(pool: PgPool, interval: Duration, mut shutdown: watch::Receiver<bool>) {
    let mut ticker = tokio::time::interval(interval);
    loop {
        tokio::select! {
            _ = ticker.tick() => {}
            changed = shutdown.changed() => {
                if changed.is_err() || *shutdown.borrow() {
                    return;
                }
                continue;
            }
        }
        match reconcile_once(&pool).await {
            Ok(report) => tracing::debug!(?report, "worker lease reconciliation completed"),
            Err(error) => tracing::error!(%error, "worker lease reconciliation failed"),
        }
        crate::second_review::sweep(&pool).await;
    }
}

pub async fn reconcile_once(pool: &PgPool) -> anyhow::Result<ReconcileReport> {
    let recovery = deepref_app::recover_expired_worker_state(pool).await?;
    let repaired_work = repair_missing_work(pool).await?;
    crate::assistant::reconcile_orphan_runs(pool).await?;
    let workflow_file_backfills =
        deepref_app::schedule_workflow_file_backfills(pool, WORKFLOW_FILE_BACKFILL_PROJECTS).await?
            as u64;
    Ok(ReconcileReport {
        expired_doi_leases: recovery.expired_doi_leases,
        repaired_work,
        expired_jobs: recovery.expired_jobs,
        workflow_file_backfills,
    })
}

async fn repair_missing_work(pool: &PgPool) -> anyhow::Result<u64> {
    let items = deepref_app::find_missing_acquisition_work(pool, 100).await?;
    let mut repaired = 0;
    for item in items {
        let acquisition_id = item.acquisition_id;
        let project_id = item.project_id;
        let event_id = item.work_event_id;
        let doi = item.canonical_doi;
        let entity_key = format!("{acquisition_id}|{doi}");
        let payload = serde_json::json!({
            "schema_version": 1,
            "event_id": event_id,
            "event_type": deepref_domain::events::SUBJECT_WORK_FETCH_REQUESTED,
            "occurred_at": chrono::Utc::now(),
            "producer": "deepref.worker.reconciler",
            "correlation_id": acquisition_id,
            "causation_id": null,
            "entity_type": "work",
            "entity_key": entity_key,
            "revision": 0,
            "payload": {
                "doi": doi,
                "project_id": project_id,
                "acquisition_id": acquisition_id,
                "depth": item.depth,
                "max_depth": item.max_depth,
                "parent_doi": item.parent_doi
            }
        });
        deepref_app::enqueue_job_pool(
            pool,
            &deepref_app::job(
                event_id,
                deepref_domain::ProjectId::new(project_id),
                deepref_domain::JobKind::WorkFetchRequested,
                payload,
                format!("work_fetch:{event_id}"),
            ),
        )
        .await?;
        repaired += 1;
    }
    Ok(repaired)
}
