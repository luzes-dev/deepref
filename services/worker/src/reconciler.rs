use std::time::Duration;

use sqlx::{PgPool, Row};
use tokio::sync::watch;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ReconcileReport {
    pub expired_event_claims: u64,
    pub expired_doi_leases: u64,
    pub repaired_work: u64,
    pub expired_jobs: u64,
}

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
    }
}

pub async fn reconcile_once(pool: &PgPool) -> anyhow::Result<ReconcileReport> {
    let expired_jobs = deepref_postgres::recover_expired_jobs(pool).await?;
    let expired_event_claims = sqlx::query(
        "UPDATE processed_events SET owner_token=NULL,last_error=COALESCE(last_error,'lease expired') WHERE completed_at IS NULL AND owner_token IS NOT NULL AND lease_expires_at < now()",
    )
    .execute(pool)
    .await?
    .rows_affected();
    let expired_doi_leases = sqlx::query(
        "UPDATE doi_fetch_state SET status='failed',owner_token=NULL,last_error=COALESCE(last_error,'lease expired'),updated_at=now() WHERE status='fetching' AND lease_expires_at < now()",
    )
    .execute(pool)
    .await?
    .rows_affected();
    sqlx::query(
        "UPDATE ingestion_items SET status='queued',last_error=COALESCE(last_error,'lease recovered') WHERE status='fetching' AND NOT EXISTS (SELECT 1 FROM doi_fetch_state d WHERE d.canonical_doi=ingestion_items.canonical_doi AND d.status='fetching' AND d.lease_expires_at>now())",
    )
    .execute(pool)
    .await?;
    let repaired_work = repair_missing_work(pool).await?;
    Ok(ReconcileReport {
        expired_event_claims,
        expired_doi_leases,
        repaired_work,
        expired_jobs,
    })
}

async fn repair_missing_work(pool: &PgPool) -> anyhow::Result<u64> {
    let rows = sqlx::query(
        "SELECT i.ingestion_id,i.project_id,i.canonical_doi,i.depth,i.parent_doi,g.max_depth FROM ingestion_items i JOIN ingestions g ON g.id=i.ingestion_id WHERE i.status='queued' AND NOT EXISTS (SELECT 1 FROM jobs j WHERE j.id=i.work_event_id) ORDER BY i.queued_at LIMIT 100",
    )
    .fetch_all(pool)
    .await?;
    let mut repaired = 0;
    for row in rows {
        let ingestion_id: uuid::Uuid = row.get("ingestion_id");
        let project_id: uuid::Uuid = row.get("project_id");
        let doi: String = row.get("canonical_doi");
        let event_id: uuid::Uuid = sqlx::query_scalar(
            "SELECT work_event_id FROM ingestion_items WHERE ingestion_id=$1 AND canonical_doi=$2",
        )
        .bind(ingestion_id)
        .bind(&doi)
        .fetch_one(pool)
        .await?;
        let payload = serde_json::json!({
            "schema_version": 1,
            "event_id": event_id,
            "event_type": deepref_events::SUBJECT_WORK_FETCH_REQUESTED,
            "occurred_at": chrono::Utc::now(),
            "producer": "deepref.worker.reconciler",
            "correlation_id": ingestion_id,
            "causation_id": null,
            "entity_type": "work",
            "entity_key": format!("{ingestion_id}|{doi}"),
            "revision": 0,
            "payload": {
                "doi": doi,
                "project_id": project_id,
                "ingestion_id": ingestion_id,
                "depth": row.get::<i32, _>("depth"),
                "max_depth": row.get::<i32, _>("max_depth"),
                "parent_doi": row.get::<Option<String>, _>("parent_doi")
            }
        });
        deepref_postgres::enqueue_job_pool(
            pool,
            &deepref_postgres::job(
                event_id,
                "work_fetch_requested",
                payload,
                format!("work_fetch:{event_id}"),
            ),
        )
        .await?;
        repaired += 1;
    }
    Ok(repaired)
}
