use chrono::Utc;
use deepref_graph::GraphRepository;
use sqlx::PgPool;
use uuid::Uuid;

pub async fn recompute(
    pool: &PgPool,
    graph: &GraphRepository,
    project_id: Uuid,
    revision: i64,
) -> anyhow::Result<()> {
    let metrics = graph.compute_metrics(project_id).await?;
    let mut tx = pool.begin().await?;
    sqlx::query(
        "INSERT INTO metric_snapshots (project_id,revision,metrics_as_of,work_count,edge_count,payload) \
         VALUES ($1,$2,$3,$4,$5,$6) ON CONFLICT (project_id,revision) DO NOTHING",
    ).bind(project_id).bind(revision).bind(Utc::now()).bind(metrics.work_count).bind(metrics.edge_count)
     .bind(serde_json::json!({"work_count":metrics.work_count,"edge_count":metrics.edge_count}))
     .execute(&mut *tx).await?;
    sqlx::query("UPDATE project_works SET metrics_computed_at=now() WHERE project_id=$1")
        .bind(project_id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(())
}
