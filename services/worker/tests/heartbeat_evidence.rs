use chrono::{DateTime, Utc};
use deepref_application::operational_health::TransportReadiness;
use std::time::Duration;
use tokio::sync::watch;

#[tokio::test]
async fn real_heartbeat_ticks_never_refresh_old_adapter_evidence() -> anyhow::Result<()> {
    let Some(pool) = deepref_app::test_isolation::isolated_pool_from_env("DATABASE_URL", 3).await
    else {
        return Ok(());
    };
    let owner = format!("evidence-{}", uuid::Uuid::new_v4());
    let observed_at = DateTime::<Utc>::from_timestamp_micros(
        (Utc::now() - chrono::Duration::minutes(10)).timestamp_micros(),
    )
    .ok_or_else(|| anyhow::anyhow!("invalid fixture instant"))?;
    let (_observer, evidence) = watch::channel(Some(TransportReadiness {
        ready: true,
        observed_at,
    }));
    let (stop, signal) = watch::channel(false);
    let task = tokio::spawn(deepref_worker::heartbeat::run_with_transport(
        pool.clone(),
        owner.clone(),
        Duration::from_millis(20),
        signal,
        vec![],
        Some(evidence),
    ));
    let result = tokio::time::timeout(Duration::from_secs(3), async {
        let mut first_seen = None;
        loop {
            let sample: Option<(DateTime<Utc>, DateTime<Utc>)> = sqlx::query_as(
                "SELECT seen_at,transport_observed_at FROM worker_heartbeats WHERE owner=$1",
            )
            .bind(&owner)
            .fetch_optional(&pool)
            .await?;
            if let Some((seen, observed)) = sample {
                anyhow::ensure!(
                    observed == observed_at,
                    "heartbeat rejuvenated old adapter evidence"
                );
                match first_seen {
                    Some(first) if seen > first => return Ok::<(), anyhow::Error>(()),
                    None => first_seen = Some(seen),
                    _ => {}
                }
            }
            tokio::task::yield_now().await;
        }
    })
    .await;
    deepref_worker::heartbeat::stop(task, stop, Duration::from_secs(1)).await;
    result??;
    Ok(())
}
