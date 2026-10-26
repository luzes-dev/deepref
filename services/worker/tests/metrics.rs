#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::panic_in_result_fn
)]

use std::time::Duration;

use deepref_application::jobs::JobScope;
use deepref_domain::{JobKind, ProjectId, WorkspaceId};
use deepref_telemetry::metrics::WorkerMetrics;
use opentelemetry::metrics::MeterProvider;
use opentelemetry_sdk::metrics::{
    InMemoryMetricExporter, PeriodicReader, SdkMeterProvider,
    data::{AggregatedMetrics, MetricData},
};
use sqlx::Row;
use uuid::Uuid;

fn value(
    exporter: &InMemoryMetricExporter,
    name: &str,
    label: Option<(&str, &str)>,
) -> Option<f64> {
    for resource in exporter.get_finished_metrics().unwrap() {
        for scope in resource.scope_metrics() {
            for metric in scope.metrics().filter(|metric| metric.name() == name) {
                match metric.data() {
                    AggregatedMetrics::U64(MetricData::Gauge(gauge)) => {
                        for point in gauge.data_points() {
                            if label.is_none_or(|(key, value)| {
                                point.attributes().any(|attribute| {
                                    attribute.key.as_str() == key
                                        && attribute.value.as_str() == value
                                })
                            }) {
                                return Some(point.value() as f64);
                            }
                        }
                    }
                    AggregatedMetrics::F64(MetricData::Gauge(gauge)) => {
                        for point in gauge.data_points() {
                            if label.is_none_or(|(key, value)| {
                                point.attributes().any(|attribute| {
                                    attribute.key.as_str() == key
                                        && attribute.value.as_str() == value
                                })
                            }) {
                                return Some(point.value());
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
    }
    None
}

#[tokio::test]
async fn cleanup_and_live_slot_samples_keep_last_good_values_without_mutating_obligations()
-> anyhow::Result<()> {
    use deepref_ai::{AiConcurrencyAdmission, AiConcurrencyLimits, AiConcurrencyScope};
    let Some(pool) = deepref_app::test_isolation::isolated_pool_from_env("DATABASE_URL", 4).await
    else {
        return Ok(());
    };
    let admission = deepref_app::PostgresAiConcurrencyAdmission::new(&pool);
    let held = admission
        .acquire(AiConcurrencyScope::new(
            WorkspaceId::new(Uuid::new_v4()),
            "provider",
            "metric-route",
            AiConcurrencyLimits {
                workspace: 1,
                route: 1,
            },
        ))
        .await?;
    sqlx::query("INSERT INTO ai_concurrency_slots(scope_kind,scope_key,slot_index,holder_id,generation,acquired_at,lease_expires_at) VALUES('route','expired-fixture',0,$1,1,now()-interval '2 minutes',now()-interval '1 minute')")
        .bind(Uuid::new_v4()).execute(&pool).await?;
    sqlx::query("INSERT INTO object_lifecycle(storage_key,cleanup_after,retired_at,deleted_at) VALUES ('documents/00000000-0000-4000-8000-000000000001',now()-interval '10 minutes',NULL,NULL),('documents/00000000-0000-4000-8000-000000000002',now()+interval '5 minutes',now()-interval '1 hour',NULL),('documents/00000000-0000-4000-8000-000000000003',NULL,now()-interval '2 hours',now()),('documents/00000000-0000-4000-8000-000000000004',NULL,NULL,NULL)")
        .execute(&pool).await?;
    let valid:bool=sqlx::query_scalar("SELECT indisvalid AND indisready FROM pg_index WHERE indexrelid='object_lifecycle_unfinished_retirement_idx'::regclass").fetch_one(&pool).await?;
    assert!(valid);
    let before_slots: Vec<serde_json::Value> = sqlx::query_scalar(
        "SELECT to_jsonb(s) FROM ai_concurrency_slots s ORDER BY scope_kind,scope_key,slot_index",
    )
    .fetch_all(&pool)
    .await?;
    let before_objects: Vec<serde_json::Value> =
        sqlx::query_scalar("SELECT to_jsonb(o) FROM object_lifecycle o ORDER BY storage_key")
            .fetch_all(&pool)
            .await?;
    let exporter = InMemoryMetricExporter::default();
    let provider = SdkMeterProvider::builder()
        .with_reader(
            PeriodicReader::builder(exporter.clone())
                .with_interval(Duration::from_secs(3600))
                .build(),
        )
        .build();
    let metrics = WorkerMetrics::new(&provider.meter("cleanup-and-slot-test"));
    deepref_worker::metrics::sample_once(&pool, &metrics).await?;
    provider.force_flush()?;
    assert_eq!(
        value(
            &exporter,
            "deepref.ai.concurrency.live",
            Some(("scope", "workspace"))
        ),
        Some(1.0)
    );
    assert_eq!(
        value(
            &exporter,
            "deepref.ai.concurrency.live",
            Some(("scope", "route"))
        ),
        Some(1.0),
        "expired slot is not active"
    );
    assert_eq!(
        value(
            &exporter,
            "deepref.backlog.depth",
            Some(("family", "object_cleanup"))
        ),
        Some(1.0)
    );
    assert_eq!(
        value(
            &exporter,
            "deepref.backlog.depth",
            Some(("family", "retired_objects"))
        ),
        Some(1.0),
        "retired retry remains debt even before its next attempt"
    );
    assert!(
        value(
            &exporter,
            "deepref.backlog.oldest.age",
            Some(("family", "object_cleanup"))
        )
        .unwrap()
            >= 600.0
    );
    let success = value(&exporter, "deepref.worker.metrics.last_success", None);
    exporter.reset();
    let mut blocker = pool.begin().await?;
    sqlx::query("LOCK TABLE object_lifecycle IN ACCESS EXCLUSIVE MODE")
        .execute(&mut *blocker)
        .await?;
    assert!(
        deepref_worker::metrics::sample_once(&pool, &metrics)
            .await
            .is_err()
    );
    blocker.rollback().await?;
    provider.force_flush()?;
    assert_eq!(
        value(&exporter, "deepref.worker.metrics.sample.up", None),
        Some(0.0)
    );
    assert_eq!(
        value(&exporter, "deepref.worker.metrics.last_success", None),
        success
    );
    assert_eq!(
        value(
            &exporter,
            "deepref.backlog.depth",
            Some(("family", "retired_objects"))
        ),
        Some(1.0)
    );
    assert_eq!(before_slots,sqlx::query_scalar::<_,serde_json::Value>("SELECT to_jsonb(s) FROM ai_concurrency_slots s ORDER BY scope_kind,scope_key,slot_index").fetch_all(&pool).await?);
    assert_eq!(
        before_objects,
        sqlx::query_scalar::<_, serde_json::Value>(
            "SELECT to_jsonb(o) FROM object_lifecycle o ORDER BY storage_key"
        )
        .fetch_all(&pool)
        .await?
    );
    admission.release(held).await?;
    provider.shutdown()?;
    Ok(())
}

#[tokio::test]
async fn current_schema_sample_includes_capability_wait_and_failed_reads_never_report_empty_queue()
-> anyhow::Result<()> {
    let Some(pool) = deepref_app::test_isolation::isolated_pool_from_env("DATABASE_URL", 4).await
    else {
        return Ok(());
    };
    let project = ProjectId::new(Uuid::new_v4());
    sqlx::query("INSERT INTO projects(id,name) VALUES($1,'metric fixture')")
        .bind(project.as_uuid())
        .execute(&pool)
        .await?;
    let workspace = WorkspaceId::new(
        sqlx::query_scalar("SELECT workspace_id FROM projects WHERE id=$1")
            .bind(project.as_uuid())
            .fetch_one(&pool)
            .await?,
    );
    let queued = Uuid::new_v4();
    let protected = Uuid::new_v4();
    let future = Uuid::new_v4();
    let dead = Uuid::new_v4();
    for (id, scope, kind) in [
        (queued, JobScope::Project(project), JobKind::ParseDocument),
        (
            protected,
            JobScope::Workspace(workspace),
            JobKind::AiUsageReconcile,
        ),
        (future, JobScope::Project(project), JobKind::ParseDocument),
        (dead, JobScope::Project(project), JobKind::ParseDocument),
    ] {
        deepref_app::enqueue_job_pool(
            &pool,
            &deepref_app::scoped_job(
                id,
                scope,
                kind,
                serde_json::json!({}),
                format!("metrics:{id}"),
            ),
        )
        .await?;
    }
    sqlx::query("UPDATE jobs SET available_at=now()-interval '10 minutes' WHERE id IN ($1,$2)")
        .bind(queued)
        .bind(protected)
        .execute(&pool)
        .await?;
    sqlx::query("UPDATE jobs SET available_at=now()+interval '1 hour' WHERE id=$1")
        .bind(future)
        .execute(&pool)
        .await?;
    sqlx::query("UPDATE jobs SET state='dead',workload_class=NULL,created_at=now()-interval '1 year' WHERE id=$1")
        .bind(dead)
        .execute(&pool)
        .await?;
    let historical_class: Option<String> =
        sqlx::query_scalar("SELECT workload_class FROM jobs WHERE id=$1")
            .bind(dead)
            .fetch_one(&pool)
            .await?;
    assert_eq!(
        historical_class, None,
        "fixture represents supported pre-pool terminal history"
    );
    let state: String = sqlx::query_scalar("SELECT state FROM jobs WHERE id=$1")
        .bind(protected)
        .fetch_one(&pool)
        .await?;
    assert_eq!(state, "capability_wait");
    sqlx::query("INSERT INTO worker_heartbeats(owner,pool_capacities) VALUES('metrics-worker','{\"ai\":2,\"documents\":4}')").execute(&pool).await?;
    sqlx::query("INSERT INTO outbox_events(event_id,message_type,message_version,payload,expected_consumers,created_at) VALUES($1,'metrics.fixture',1,'{}',ARRAY['fixture'],now()-interval '15 minutes')").bind(Uuid::new_v4()).execute(&pool).await?;
    sqlx::query("INSERT INTO integration_inbox_events(provider,provider_event_id,event_type,payload,received_at) VALUES('fixture','metrics-event','fixture','{}',now()-interval '20 minutes')").execute(&pool).await?;
    let before = sqlx::query("SELECT id,state,claim_seq,attempts FROM jobs ORDER BY id")
        .fetch_all(&pool)
        .await?
        .into_iter()
        .map(|row| {
            (
                row.get::<Uuid, _>("id"),
                row.get::<String, _>("state"),
                row.get::<i64, _>("claim_seq"),
                row.get::<i32, _>("attempts"),
            )
        })
        .collect::<Vec<_>>();
    let exporter = InMemoryMetricExporter::default();
    let provider = SdkMeterProvider::builder()
        .with_reader(
            PeriodicReader::builder(exporter.clone())
                .with_interval(Duration::from_secs(3600))
                .build(),
        )
        .build();
    let metrics = WorkerMetrics::new(&provider.meter("database-metric-test"));
    deepref_worker::metrics::sample_once(&pool, &metrics).await?;
    provider.force_flush()?;
    assert_eq!(
        value(
            &exporter,
            "deepref.worker.queue.depth",
            Some(("pool", "documents"))
        ),
        Some(1.0)
    );
    assert_eq!(
        value(
            &exporter,
            "deepref.worker.queue.depth",
            Some(("pool", "ai"))
        ),
        Some(1.0)
    );
    assert!(
        value(
            &exporter,
            "deepref.worker.queue.oldest_due.age",
            Some(("pool", "ai"))
        )
        .unwrap()
            >= 600.0
    );
    assert_eq!(
        value(
            &exporter,
            "deepref.worker.capacity",
            Some(("pool", "documents"))
        ),
        Some(4.0)
    );
    assert_eq!(value(&exporter, "deepref.worker.live", None), Some(1.0));
    assert_eq!(
        value(
            &exporter,
            "deepref.backlog.depth",
            Some(("family", "outbox"))
        ),
        // Two durable job wakeups plus the explicit fixture event.
        Some(3.0)
    );
    assert_eq!(
        value(
            &exporter,
            "deepref.backlog.depth",
            Some(("family", "inbox"))
        ),
        Some(1.0)
    );
    assert_eq!(
        value(
            &exporter,
            "deepref.worker.dead.depth",
            Some(("pool", "documents"))
        ),
        Some(1.0),
        "dead depth includes old retained jobs without inventing a death timestamp"
    );
    let success_time = value(&exporter, "deepref.worker.metrics.last_success", None).unwrap();
    exporter.reset();
    let mut blocker = pool.begin().await?;
    sqlx::query("LOCK TABLE jobs IN ACCESS EXCLUSIVE MODE")
        .execute(&mut *blocker)
        .await?;
    assert!(
        deepref_worker::metrics::sample_once(&pool, &metrics)
            .await
            .is_err()
    );
    blocker.rollback().await?;
    provider.force_flush()?;
    assert_eq!(
        value(&exporter, "deepref.worker.metrics.sample.up", None),
        Some(0.0)
    );
    assert_eq!(
        value(&exporter, "deepref.worker.metrics.last_success", None),
        Some(success_time)
    );
    assert_eq!(
        value(
            &exporter,
            "deepref.worker.queue.depth",
            Some(("pool", "ai"))
        ),
        Some(1.0)
    );
    let after = sqlx::query("SELECT id,state,claim_seq,attempts FROM jobs ORDER BY id")
        .fetch_all(&pool)
        .await?
        .into_iter()
        .map(|row| {
            (
                row.get::<Uuid, _>("id"),
                row.get::<String, _>("state"),
                row.get::<i64, _>("claim_seq"),
                row.get::<i32, _>("attempts"),
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(before, after, "sampling must not claim or mutate work");
    provider.shutdown()?;
    Ok(())
}
