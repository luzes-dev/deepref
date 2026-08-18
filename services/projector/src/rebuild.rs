use std::hash::{Hash, Hasher};

use deepref_events::{DomainPayload, EventEnvelope};
use deepref_graph::{GraphRepository, mutation_from_event};
use sqlx::{PgPool, Row};
use uuid::Uuid;

pub const REBUILD_STAGES: [&str; 8] = [
    "advisory_lock",
    "snapshot_watermark",
    "pause",
    "clear_migrate",
    "bounded_bulk_load",
    "replay",
    "verify",
    "ready_resume",
];

pub async fn run(
    pool: &PgPool,
    graph: &GraphRepository,
    run_id: Uuid,
    batch_size: i64,
    advisory_lock_key: i64,
) -> anyhow::Result<()> {
    tracing::info!(%run_id, stages=?REBUILD_STAGES, "projection rebuild started");
    let mut lock = pool.acquire().await?;
    let acquired: bool = sqlx::query_scalar("SELECT pg_try_advisory_lock($1)")
        .bind(advisory_lock_key)
        .fetch_one(&mut *lock)
        .await?;
    if !acquired {
        anyhow::bail!("another projection rebuild holds the advisory lock");
    }
    let result = rebuild_locked(pool, graph, batch_size).await;
    let _ = sqlx::query("SELECT pg_advisory_unlock($1)")
        .bind(advisory_lock_key)
        .execute(&mut *lock)
        .await;
    result
}

async fn rebuild_locked(
    pool: &PgPool,
    graph: &GraphRepository,
    batch_size: i64,
) -> anyhow::Result<()> {
    let mut snapshot = pool.begin().await?;
    sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ, READ ONLY")
        .execute(&mut *snapshot)
        .await?;
    let watermark: i64 = sqlx::query_scalar("SELECT COALESCE(max(revision),0) FROM domain_events")
        .fetch_one(&mut *snapshot)
        .await?;
    crate::status::set_rebuild_state(pool, "rebuilding", watermark, None).await?;
    graph.clear_projection().await?;
    let mut snapshot_hasher = std::collections::hash_map::DefaultHasher::new();

    let mut last_doi = String::new();
    loop {
        let rows = sqlx::query(
            "SELECT canonical_doi,title,issued_year,total_citations FROM works \
             WHERE canonical_doi>$1 ORDER BY canonical_doi LIMIT $2",
        )
        .bind(&last_doi)
        .bind(batch_size)
        .fetch_all(&mut *snapshot)
        .await?;
        if rows.is_empty() {
            break;
        }
        for row in &rows {
            last_doi = row.get("canonical_doi");
            "work".hash(&mut snapshot_hasher);
            last_doi.hash(&mut snapshot_hasher);
            graph
                .load_work_snapshot(
                    &last_doi,
                    row.get::<Option<String>, _>("title").as_deref(),
                    row.get("issued_year"),
                    row.get("total_citations"),
                )
                .await?;
        }
    }
    let mut membership_offset = 0_i64;
    loop {
        let rows = sqlx::query(
            "SELECT project_id,canonical_doi,seed,min_depth FROM project_works \
             ORDER BY project_id,canonical_doi OFFSET $1 LIMIT $2",
        )
        .bind(membership_offset)
        .bind(batch_size)
        .fetch_all(&mut *snapshot)
        .await?;
        if rows.is_empty() {
            break;
        }
        membership_offset += rows.len() as i64;
        for row in rows {
            graph
                .load_membership_snapshot(
                    row.get("project_id"),
                    &row.get::<String, _>("canonical_doi"),
                    row.get("seed"),
                    row.get("min_depth"),
                )
                .await?;
        }
    }
    let mut citation_offset = 0_i64;
    loop {
        let rows = sqlx::query(
            "SELECT DISTINCT source_doi,target_doi FROM citations ORDER BY source_doi,target_doi OFFSET $1 LIMIT $2",
        ).bind(citation_offset).bind(batch_size).fetch_all(&mut *snapshot).await?;
        if rows.is_empty() {
            break;
        }
        citation_offset += rows.len() as i64;
        for row in rows {
            let source = row.get::<String, _>("source_doi");
            let target = row.get::<String, _>("target_doi");
            "citation".hash(&mut snapshot_hasher);
            source.hash(&mut snapshot_hasher);
            target.hash(&mut snapshot_hasher);
            graph.load_citation_snapshot(&source, &target).await?;
        }
    }
    let pg_counts: (i64, i64) = (
        sqlx::query_scalar("SELECT count(*)::bigint FROM works")
            .fetch_one(&mut *snapshot)
            .await?,
        sqlx::query_scalar("SELECT count(DISTINCT (source_doi,target_doi))::bigint FROM citations")
            .fetch_one(&mut *snapshot)
            .await?,
    );
    let snapshot_hash = snapshot_hasher.finish();
    let graph_counts = graph.counts().await?;
    let graph_hash = graph.projection_hash().await?;
    if graph_counts.work_count != pg_counts.0
        || graph_counts.edge_count != pg_counts.1
        || graph_hash != snapshot_hash
    {
        crate::status::set_rebuild_state(
            pool,
            "failed",
            watermark,
            Some("projection count/hash verification failed"),
        )
        .await?;
        anyhow::bail!(
            "projection count/hash verification failed: postgres={pg_counts:?}/{snapshot_hash}, graph={graph_counts:?}/{graph_hash}"
        );
    }
    snapshot.commit().await?;

    let mut revision = watermark;
    loop {
        let rows = sqlx::query(
            "SELECT event_id,schema_version,event_type,entity_type,entity_key,revision,payload,correlation_id,causation_id,created_at \
             FROM domain_events WHERE revision>$1 ORDER BY revision LIMIT $2",
        ).bind(revision).bind(batch_size).fetch_all(pool).await?;
        if rows.is_empty() {
            break;
        }
        for row in rows {
            revision = row.get("revision");
            let payload: DomainPayload = serde_json::from_value(row.get("payload"))?;
            let event = EventEnvelope {
                schema_version: row.get::<i16, _>("schema_version") as u16,
                event_id: row.get("event_id"),
                event_type: row.get("event_type"),
                occurred_at: row.get("created_at"),
                producer: "domain-log".into(),
                correlation_id: row.get("correlation_id"),
                causation_id: row.get("causation_id"),
                entity_type: parse_entity(row.get::<String, _>("entity_type").as_str())?,
                entity_key: row.get("entity_key"),
                revision,
                payload,
            };
            if let Some(mutation) = mutation_from_event(&event) {
                graph.apply_mutation(&mutation).await?;
            }
        }
    }
    sqlx::query(
        "UPDATE projection_state SET state='ready',revision=$1,watermark=$1,lag=0,last_success_at=now(), \
         last_error=NULL,rebuild_state='ready',updated_at=now() WHERE projection_name='graph' AND project_id IS NULL",
    ).bind(revision).execute(pool).await?;
    Ok(())
}

fn parse_entity(value: &str) -> anyhow::Result<deepref_events::EntityType> {
    use deepref_events::EntityType::*;
    Ok(match value {
        "project" => Project,
        "project_membership" => ProjectMembership,
        "citation" => Citation,
        "unresolved_reference" => UnresolvedReference,
        "work" => Work,
        "metric" => Metric,
        "projection" => Projection,
        "dead_letter" => DeadLetter,
        _ => anyhow::bail!("unknown entity type {value}"),
    })
}
