use std::time::Duration;

use deepref_application::{inbox::ProviderReconcilerRegistry, jobs::JobScope};
use deepref_domain::{JobKind, WorkspaceId};
use sqlx::PgPool;
use std::sync::Arc;
use tokio::sync::watch;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ReconcileReport {
    pub expired_doi_leases: u64,
    pub repaired_work: u64,
    pub expired_jobs: u64,
    pub provider_records_checked: u64,
    pub provider_projections_repaired: u64,
    pub provider_reconcile_failures: usize,
}

pub async fn run(pool: PgPool, interval: Duration, shutdown: watch::Receiver<bool>) {
    run_with_provider_reconcilers(
        pool,
        interval,
        shutdown,
        Arc::new(ProviderReconcilerRegistry::new()),
    )
    .await;
}

pub async fn run_with_provider_reconcilers(
    pool: PgPool,
    interval: Duration,
    mut shutdown: watch::Receiver<bool>,
    provider_reconcilers: Arc<ProviderReconcilerRegistry>,
) {
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
        match reconcile_once_with_provider_reconcilers(&pool, &provider_reconcilers).await {
            Ok(report) => tracing::debug!(?report, "worker lease reconciliation completed"),
            Err(error) => tracing::error!(%error, "worker lease reconciliation failed"),
        }
        if let Err(error) = enqueue_ai_usage_reconciliation_jobs(&pool).await {
            tracing::error!(%error, "AI usage reconciliation jobs could not be enqueued");
        }
        crate::second_review::sweep(&pool).await;
    }
}

async fn enqueue_ai_usage_reconciliation_jobs(pool: &PgPool) -> anyhow::Result<u64> {
    let workspace_ids =
        sqlx::query_scalar::<_, uuid::Uuid>("SELECT id FROM workspaces ORDER BY id")
            .fetch_all(pool)
            .await?;
    let minute = chrono::Utc::now().format("%Y%m%d%H%M").to_string();
    let mut queued = 0;
    for workspace_id in workspace_ids {
        let workspace_id = WorkspaceId::new(workspace_id);
        let dedupe_key = format!("ai_usage_reconcile:{}:{minute}", workspace_id.as_uuid());
        deepref_app::enqueue_job_pool(
            pool,
            &deepref_app::scoped_job(
                uuid::Uuid::new_v4(),
                JobScope::Workspace(workspace_id),
                JobKind::AiUsageReconcile,
                serde_json::json!({}),
                dedupe_key,
            ),
        )
        .await?;
        queued += 1;
    }
    tracing::debug!(
        workspaces = queued,
        "workspace AI usage reconciliation scheduled"
    );
    Ok(queued)
}

pub async fn reconcile_once(pool: &PgPool) -> anyhow::Result<ReconcileReport> {
    reconcile_once_with_provider_reconcilers(pool, &ProviderReconcilerRegistry::new()).await
}

pub async fn reconcile_once_with_provider_reconcilers(
    pool: &PgPool,
    provider_reconcilers: &ProviderReconcilerRegistry,
) -> anyhow::Result<ReconcileReport> {
    let recovery = deepref_app::recover_expired_worker_state(pool).await?;
    let repaired_work = repair_missing_work(pool).await?;
    crate::assistant::reconcile_orphan_runs(pool).await?;
    let mut report = ReconcileReport {
        expired_doi_leases: recovery.expired_doi_leases,
        repaired_work,
        expired_jobs: recovery.expired_jobs,
        ..ReconcileReport::default()
    };
    for (provider, outcome) in provider_reconcilers.reconcile_all().await {
        match outcome {
            Ok(outcome) => {
                report.provider_records_checked = report
                    .provider_records_checked
                    .saturating_add(outcome.checked);
                report.provider_projections_repaired = report
                    .provider_projections_repaired
                    .saturating_add(outcome.repaired);
                tracing::info!(
                    provider,
                    checked = outcome.checked,
                    repaired = outcome.repaired,
                    "provider projection reconciliation completed"
                );
            }
            Err(error) => {
                report.provider_reconcile_failures += 1;
                tracing::warn!(provider, %error, "provider projection reconciliation failed");
            }
        }
    }
    Ok(report)
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
