use chrono::{DateTime, Utc};
use sqlx::{PgPool, Row};

/// Reserves a provider permit under a PostgreSQL row lock. The returned wait
/// happens after commit, so concurrent pods share one global schedule without
/// holding database connections while sleeping.
pub async fn acquire(pool: &PgPool, provider: &str, rate_per_second: u32) -> anyhow::Result<()> {
    let spacing_ms = (1_000_u64 / u64::from(rate_per_second.max(1))).max(1) as i64;
    let mut tx = pool.begin().await?;
    sqlx::query(
        "INSERT INTO provider_rate_state (provider, next_permit_at) VALUES ($1, now()) ON CONFLICT DO NOTHING",
    ).bind(provider).execute(&mut *tx).await?;
    let row = sqlx::query(
        "SELECT GREATEST(next_permit_at, now()) AS permit_at FROM provider_rate_state WHERE provider = $1 FOR UPDATE",
    ).bind(provider).fetch_one(&mut *tx).await?;
    let permit_at: DateTime<Utc> = row.get("permit_at");
    sqlx::query(
        "UPDATE provider_rate_state SET next_permit_at = $2 + ($3 * interval '1 millisecond'), updated_at = now() WHERE provider = $1",
    ).bind(provider).bind(permit_at).bind(spacing_ms).execute(&mut *tx).await?;
    tx.commit().await?;
    let wait = (permit_at - Utc::now()).to_std().unwrap_or_default();
    if !wait.is_zero() {
        tokio::time::sleep(wait).await;
    }
    Ok(())
}
