use sqlx::PgPool;

/// Reserves a provider permit through the PostgreSQL adapter. The returned wait
/// happens after commit, so concurrent pods share one global schedule without
/// holding database connections while sleeping.
pub async fn acquire(pool: &PgPool, provider: &str, rate_per_second: u32) -> anyhow::Result<()> {
    let wait = deepref_postgres::reserve_provider_permit(pool, provider, rate_per_second).await?;
    if !wait.is_zero() {
        tokio::time::sleep(wait).await;
    }
    Ok(())
}
