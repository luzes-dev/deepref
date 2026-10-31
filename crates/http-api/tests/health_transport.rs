use axum::{
    body::{Body, to_bytes},
    http::Request,
};
use chrono::Utc;
use deepref_application::operational_health::{
    TransportReadiness, WorkTransportMode, WorkerTransportEvidence,
};
use deepref_http_api::{config::ApiConfig, routes::router, state::AppState};
use serde_json::Value;
use sqlx::PgPool;
use std::collections::HashMap;
use tower::ServiceExt;

fn config() -> anyhow::Result<ApiConfig> {
    Ok(ApiConfig {
        runtime: deepref_config::RuntimeConfig::from_map(
            "health-transport-fixture",
            &HashMap::from([("APP_ENV".to_owned(), "development".to_owned())]),
        )?,
        documents: deepref_config::DocumentConfig::from_source(&|key| {
            (key == "APP_ENV").then(|| "development".into())
        })?,
        bind_addr: "127.0.0.1:0".parse()?,
        cors_allow_any: false,
        cors_origins: vec![],
    })
}

async fn probe(pool: &PgPool, mode: WorkTransportMode, path: &str) -> anyhow::Result<(u16, Value)> {
    let mut state =
        AppState::new(pool.clone()).with_document_store(deepref_documents::DocumentStore::memory());
    state.work_transport = mode;
    let response = router(state, &config()?)
        .oneshot(Request::builder().uri(path).body(Body::empty())?)
        .await?;
    let status = response.status().as_u16();
    Ok((
        status,
        serde_json::from_slice(&to_bytes(response.into_body(), 1024 * 1024).await?)?,
    ))
}

#[tokio::test]
async fn transport_health_requires_actual_fresh_evidence_and_never_changes_readiness_or_rows()
-> anyhow::Result<()> {
    let Some(pool) = deepref_app::test_isolation::isolated_pool_from_env("DATABASE_URL", 4).await
    else {
        return Ok(());
    };
    use WorkTransportMode::{JetStream, Postgres};
    let (status, body) = probe(&pool, JetStream, "/health/dependencies").await?;
    anyhow::ensure!(status == 200 && body["jetstream"]["state"] == "degraded");
    anyhow::ensure!(body["object_storage"]["state"] == "available");
    // An N-1 raw heartbeat INSERT still works and never proves broker health.
    sqlx::query("INSERT INTO worker_heartbeats(owner) VALUES('fixture')")
        .execute(&pool)
        .await?;
    let (_, body) = probe(&pool, JetStream, "/health/dependencies").await?;
    anyhow::ensure!(body["jetstream"]["unknown_workers"] == 1);
    deepref_app::record_worker_heartbeat_with_transport(
        &pool,
        "fixture",
        &[],
        WorkerTransportEvidence::Postgres,
    )
    .await?;
    let (_, body) = probe(&pool, JetStream, "/health/dependencies").await?;
    anyhow::ensure!(body["jetstream"]["mismatched_workers"] == 1);
    let observed = Utc::now();
    for (evidence, field, state) in [
        (
            Some(TransportReadiness {
                ready: true,
                observed_at: observed,
            }),
            "fresh_ready_workers",
            "available",
        ),
        (
            Some(TransportReadiness {
                ready: false,
                observed_at: observed,
            }),
            "not_ready_workers",
            "degraded",
        ),
        (
            Some(TransportReadiness {
                ready: true,
                observed_at: observed - chrono::Duration::minutes(5),
            }),
            "stale_workers",
            "stale",
        ),
        (
            Some(TransportReadiness {
                ready: true,
                observed_at: observed + chrono::Duration::minutes(5),
            }),
            "future_workers",
            "degraded",
        ),
        (None, "unknown_workers", "degraded"),
    ] {
        deepref_app::record_worker_heartbeat_with_transport(
            &pool,
            "fixture",
            &[],
            WorkerTransportEvidence::JetStream(evidence),
        )
        .await?;
        let before: Value =
            sqlx::query_scalar("SELECT to_jsonb(h) FROM worker_heartbeats h WHERE owner='fixture'")
                .fetch_one(&pool)
                .await?;
        let (_, body) = probe(&pool, JetStream, "/health/dependencies").await?;
        anyhow::ensure!(
            body["jetstream"][field] == 1 && body["jetstream"]["state"] == state,
            "{body}"
        );
        let after: Value =
            sqlx::query_scalar("SELECT to_jsonb(h) FROM worker_heartbeats h WHERE owner='fixture'")
                .fetch_one(&pool)
                .await?;
        anyhow::ensure!(before == after, "health read mutated evidence");
        let (status, _) = probe(&pool, JetStream, "/health/ready").await?;
        anyhow::ensure!(
            status == 200,
            "broker health must not gate durable API acceptance"
        );
    }
    deepref_app::record_worker_heartbeat_with_capacities(&pool, "fixture", &[]).await?;
    let (_, body) = probe(&pool, JetStream, "/health/dependencies").await?;
    anyhow::ensure!(
        body["jetstream"]["unknown_workers"] == 1,
        "old writer must clear old health evidence"
    );
    sqlx::query("UPDATE worker_heartbeats SET seen_at=now()+interval '1 hour'")
        .execute(&pool)
        .await?;
    let (_, body) = probe(&pool, JetStream, "/health/dependencies").await?;
    anyhow::ensure!(
        body["jetstream"]["future_workers"] == 1 && body["jetstream"]["state"] == "degraded"
    );
    let (_, body) = probe(&pool, Postgres, "/health/dependencies").await?;
    anyhow::ensure!(
        body["jetstream"].is_null(),
        "Postgres rollback is a valid configured mode"
    );
    let (status, _) = probe(&pool, Postgres, "/health/ready").await?;
    anyhow::ensure!(status == 200);
    pool.close().await;
    let (_, body) = probe(&pool, JetStream, "/health/dependencies").await?;
    anyhow::ensure!(
        body["jetstream"]["state"] == "unavailable" && body["postgresql"]["state"] == "unavailable",
        "database errors must not reuse fresh health evidence: {body}"
    );
    let (status, _) = probe(&pool, JetStream, "/health/ready").await?;
    anyhow::ensure!(status == 503, "database failure must refuse readiness");
    Ok(())
}
