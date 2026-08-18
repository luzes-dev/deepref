use std::time::Duration;

use deepref_events::{EntityType, EventEnvelope, SUBJECT_WORK_FETCH_REQUESTED, WorkFetchRequested};
use sqlx::{PgPool, Row};

pub async fn run(pool: PgPool, interval: Duration) {
    let mut ticker = tokio::time::interval(interval);
    loop {
        ticker.tick().await;
        match reconcile_once(&pool).await {
            Ok(report) => tracing::info!(
                expired_event_claims = report.expired_event_claims,
                expired_doi_leases = report.expired_doi_leases,
                repaired_work = report.repaired_work,
                exhausted_outbox = report.exhausted_outbox,
                "worker reconciliation completed"
            ),
            Err(error) => tracing::error!(%error, "worker reconciliation failed"),
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ReconcileReport {
    pub expired_event_claims: u64,
    pub expired_doi_leases: u64,
    pub repaired_work: u64,
    pub exhausted_outbox: i64,
}

pub async fn reconcile_once(pool: &PgPool) -> anyhow::Result<ReconcileReport> {
    let expired_event_claims = sqlx::query(
        "UPDATE processed_events SET owner_token=NULL,last_error=COALESCE(last_error,'lease expired') \
         WHERE completed_at IS NULL AND owner_token IS NOT NULL AND lease_expires_at < now()",
    ).execute(pool).await?.rows_affected();
    let expired_doi_leases = sqlx::query(
        "UPDATE doi_fetch_state SET status='failed',owner_token=NULL,last_error=COALESCE(last_error,'lease expired'),updated_at=now() \
         WHERE status='fetching' AND lease_expires_at < now()",
    ).execute(pool).await?.rows_affected();
    sqlx::query(
        "UPDATE ingestion_items SET status='queued',last_error=COALESCE(last_error,'lease recovered') \
         WHERE status='fetching' AND NOT EXISTS (SELECT 1 FROM doi_fetch_state d \
         WHERE d.canonical_doi=ingestion_items.canonical_doi AND d.status='fetching' AND d.lease_expires_at>now())",
    ).execute(pool).await?;
    let repaired_work = repair_missing_work(pool).await?;
    sqlx::query(
        "UPDATE event_outbox SET locked_at=NULL,next_attempt_at=now() \
         WHERE published_at IS NULL AND exhausted_at IS NULL AND locked_at < now()-interval '30 seconds'",
    ).execute(pool).await?;
    let exhausted_outbox = sqlx::query_scalar(
        "SELECT count(*)::bigint FROM event_outbox WHERE exhausted_at IS NOT NULL",
    )
    .fetch_one(pool)
    .await?;
    Ok(ReconcileReport {
        expired_event_claims,
        expired_doi_leases,
        repaired_work,
        exhausted_outbox,
    })
}

async fn repair_missing_work(pool: &PgPool) -> anyhow::Result<u64> {
    let rows = sqlx::query(
        "SELECT i.ingestion_id,i.project_id,i.canonical_doi,i.depth,i.parent_doi,g.max_depth \
         FROM ingestion_items i JOIN ingestions g ON g.id=i.ingestion_id \
         LEFT JOIN event_outbox o ON o.id=i.work_event_id \
         WHERE i.status='queued' AND o.id IS NULL ORDER BY i.queued_at LIMIT 100",
    )
    .fetch_all(pool)
    .await?;
    let mut repaired = 0;
    for row in rows {
        let ingestion_id = row.get("ingestion_id");
        let project_id = row.get("project_id");
        let doi: String = row.get("canonical_doi");
        let mut tx = pool.begin().await?;
        let revision: i64 = sqlx::query_scalar("SELECT nextval('graph_domain_revision_seq')")
            .fetch_one(&mut *tx)
            .await?;
        let event = EventEnvelope::v1(
            SUBJECT_WORK_FETCH_REQUESTED,
            "deepref.worker.reconciler",
            EntityType::Work,
            format!("{ingestion_id}|{doi}"),
            revision,
            ingestion_id,
            None,
            WorkFetchRequested {
                project_id,
                ingestion_id,
                doi: doi.clone(),
                depth: row.get("depth"),
                max_depth: row.get("max_depth"),
                parent_doi: row.get("parent_doi"),
            },
        );
        let updated = sqlx::query(
            "UPDATE ingestion_items SET work_event_id=$3 WHERE ingestion_id=$1 AND canonical_doi=$2 AND status='queued'",
        ).bind(ingestion_id).bind(&doi).bind(event.event_id).execute(&mut *tx).await?;
        if updated.rows_affected() == 1 {
            crate::store::enqueue(
                &mut tx,
                event.event_id,
                SUBJECT_WORK_FETCH_REQUESTED,
                &event,
            )
            .await?;
            repaired += 1;
        }
        tx.commit().await?;
    }
    Ok(repaired)
}
