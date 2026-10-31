//! Bounded, read-only global operational sampling. Gauge consumers must select
//! fresh samples and aggregate replicas with max, never sum global counts.
use std::time::Duration;

use deepref_domain::WorkloadClass;
use deepref_telemetry::metrics::{Backlog, WorkerMetrics};
use sqlx::{PgPool, Row};
use tokio::sync::watch;

const INTERVAL: Duration = Duration::from_secs(30);
const SAMPLE_TIMEOUT: Duration = Duration::from_secs(5);

struct PoolSample {
    class: WorkloadClass,
    depth: u64,
    age: f64,
    running: u64,
    dead: u64,
    capacity: u64,
}

struct Snapshot {
    pools: Vec<PoolSample>,
    backlogs: Vec<(Backlog, u64, f64)>,
    live_workers: u64,
    timestamp: f64,
    export_conflicts: u64,
    export_unattributed: u64,
}

async fn snapshot(pool: &PgPool) -> anyhow::Result<Snapshot> {
    let mut tx = pool.begin().await?;
    sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ READ ONLY")
        .execute(&mut *tx)
        .await?;
    sqlx::raw_sql("SET LOCAL statement_timeout='1500ms'; SET LOCAL lock_timeout='250ms'; SET LOCAL idle_in_transaction_session_timeout='5s'")
        .execute(&mut *tx).await?;
    let classes = WorkloadClass::ALL.map(WorkloadClass::as_str).to_vec();
    let rows = sqlx::query(
        "WITH classes(class) AS (SELECT unnest($1::text[])), capacities AS (
           SELECT entry.key AS class, sum(entry.value::bigint)::bigint AS capacity
           FROM worker_heartbeats h
           CROSS JOIN LATERAL jsonb_each_text(h.pool_capacities) entry
           WHERE h.seen_at > now()-interval '60 seconds' GROUP BY entry.key
         )
         SELECT classes.class,
           count(j.id) FILTER (WHERE j.state IN ('queued','capability_wait') AND j.available_at<=now())::bigint AS depth,
           COALESCE(EXTRACT(EPOCH FROM now()-min(j.available_at) FILTER (WHERE j.state IN ('queued','capability_wait') AND j.available_at<=now())),0)::double precision AS age,
           count(j.id) FILTER (WHERE j.state='running')::bigint AS running,
           count(j.id) FILTER (WHERE j.state='dead')::bigint AS dead,
           COALESCE(capacities.capacity,0)::bigint AS capacity
         FROM classes LEFT JOIN jobs j ON COALESCE(j.workload_class,jobs_workload_class_for_kind(j.kind))=classes.class
           AND (j.state IN ('queued','capability_wait','running') OR j.state='dead')
         LEFT JOIN capacities ON capacities.class=classes.class
         GROUP BY classes.class,capacities.capacity ORDER BY classes.class"
    ).bind(classes).fetch_all(&mut *tx).await?;
    let pools = rows
        .into_iter()
        .map(|row| {
            Ok(PoolSample {
                class: row.try_get::<String, _>("class")?.parse()?,
                depth: u64::try_from(row.try_get::<i64, _>("depth")?)?,
                age: row.try_get::<f64, _>("age")?.max(0.0),
                running: u64::try_from(row.try_get::<i64, _>("running")?)?,
                dead: u64::try_from(row.try_get::<i64, _>("dead")?)?,
                capacity: u64::try_from(row.try_get::<i64, _>("capacity")?)?,
            })
        })
        .collect::<anyhow::Result<Vec<_>>>()?;
    let row = sqlx::query("SELECT count(*)::bigint AS live, EXTRACT(EPOCH FROM now())::double precision AS timestamp FROM worker_heartbeats WHERE seen_at>now()-interval '60 seconds'")
        .fetch_one(&mut *tx).await?;
    let live_workers = u64::try_from(row.try_get::<i64, _>("live")?)?;
    let timestamp = row.try_get("timestamp")?;
    let rows = sqlx::query(
        "SELECT 'outbox' AS family,count(*)::bigint AS depth,COALESCE(EXTRACT(EPOCH FROM now()-min(created_at)),0)::double precision AS age FROM outbox_events WHERE delivery_state<>'delivered'
         UNION ALL SELECT 'autonomy_outbox',count(*)::bigint,COALESCE(EXTRACT(EPOCH FROM now()-min(created_at)),0)::double precision FROM ai_autonomy_outbox
         UNION ALL SELECT 'inbox',count(*)::bigint,COALESCE(EXTRACT(EPOCH FROM now()-min(received_at)),0)::double precision FROM integration_inbox_events WHERE processing_state<>'processed'
         UNION ALL SELECT 'usage_settlement',count(*)::bigint,COALESCE(EXTRACT(EPOCH FROM now()-min(opened_at)),0)::double precision FROM ai_usage_reservations WHERE state IN ('open','observed','unresolved') AND opened_at<=now()-interval '5 minutes'"
    ).fetch_all(&mut *tx).await?;
    let mut backlogs = rows
        .into_iter()
        .map(|row| {
            let family = match row.try_get::<&str, _>("family")? {
                "outbox" => Backlog::Outbox,
                "autonomy_outbox" => Backlog::AutonomyOutbox,
                "inbox" => Backlog::Inbox,
                "usage_settlement" => Backlog::UsageSettlement,
                _ => anyhow::bail!("unknown operational metric family"),
            };
            Ok((
                family,
                u64::try_from(row.try_get::<i64, _>("depth")?)?,
                row.try_get::<f64, _>("age")?.max(0.0),
            ))
        })
        .collect::<anyhow::Result<Vec<_>>>()?;
    let export = deepref_app::usage_export_backlog(&mut *tx).await?;
    backlogs.push((
        Backlog::UsageExport,
        u64::try_from(export.pending_events)?,
        export.oldest_pending_age_seconds.unwrap_or(0).max(0) as f64,
    ));
    backlogs.push((
        Backlog::UsageUnemitted,
        u64::try_from(
            export
                .unemitted_facts
                .checked_add(export.unemitted_corrections)
                .ok_or_else(|| anyhow::anyhow!("usage metric overflow"))?,
        )?,
        export.oldest_unemitted_age_seconds.unwrap_or(0).max(0) as f64,
    ));
    let export_conflicts = u64::try_from(export.conflicting_facts)?;
    let export_unattributed = u64::try_from(export.unattributed_facts)?;
    // No state change, and no metrics are emitted until the complete snapshot
    // (including export identity diagnostics) has succeeded.
    tx.rollback().await?;
    Ok(Snapshot {
        pools,
        backlogs,
        live_workers,
        timestamp,
        export_conflicts,
        export_unattributed,
    })
}

pub async fn sample_once(pool: &PgPool, metrics: &WorkerMetrics) -> anyhow::Result<()> {
    let snapshot = match tokio::time::timeout(SAMPLE_TIMEOUT, snapshot(pool)).await {
        Ok(Ok(snapshot)) => snapshot,
        Ok(Err(error)) => {
            metrics.sample_failed();
            return Err(error);
        }
        Err(_) => {
            metrics.sample_failed();
            anyhow::bail!("operational metric sample deadline exceeded");
        }
    };
    for sample in snapshot.pools {
        metrics.pool(
            sample.class,
            sample.depth,
            sample.age,
            sample.running,
            sample.dead,
            sample.capacity,
        );
    }
    for (family, depth, age) in snapshot.backlogs {
        metrics.backlog(family, depth, age);
    }
    metrics.live_workers(snapshot.live_workers);
    metrics.export_attention(snapshot.export_conflicts, snapshot.export_unattributed);
    metrics.sample_succeeded(snapshot.timestamp);
    Ok(())
}

pub async fn run(pool: PgPool, mut shutdown: watch::Receiver<bool>) {
    let metrics = WorkerMetrics::default();
    metrics.sample_failed();
    let mut interval = tokio::time::interval(INTERVAL);
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        if *shutdown.borrow() {
            break;
        }
        tokio::select! {
            _ = shutdown.changed() => break,
            _ = interval.tick() => {
                tokio::select! {
                    _ = shutdown.changed() => break,
                    result = sample_once(&pool, &metrics) => {
                        if result.is_err() { tracing::warn!("operational metric sampling failed; last-known aggregates are retained"); }
                    }
                }
            }
        }
    }
}
