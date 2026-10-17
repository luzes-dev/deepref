use deepref_application::{PermitError, ProviderFuture, ProviderPermit};
use deepref_domain::ProviderKind;
use sqlx::PgPool;

/// Reserves a provider permit through the PostgreSQL adapter. The returned wait
/// happens after commit, so concurrent pods share one global schedule without
/// holding database connections while sleeping.
pub async fn acquire(pool: &PgPool, provider: &str, rate_per_second: u32) -> anyhow::Result<()> {
    let wait = deepref_app::reserve_provider_permit(pool, provider, rate_per_second).await?;
    if !wait.is_zero() {
        tokio::time::sleep(wait).await;
    }
    Ok(())
}

/// The permit one job takes before each provider request. Crossref uses the
/// deployment's configured rate. No other provider has an adapter in this build.
pub struct ProviderLimiter<'a> {
    pool: &'a PgPool,
    crossref_rate_per_second: u32,
}

impl<'a> ProviderLimiter<'a> {
    pub fn new(pool: &'a PgPool, crossref_rate_per_second: u32) -> Self {
        Self {
            pool,
            crossref_rate_per_second,
        }
    }
}

impl ProviderPermit for ProviderLimiter<'_> {
    fn acquire<'a>(&'a self, provider: ProviderKind) -> ProviderFuture<'a, (), PermitError> {
        Box::pin(async move {
            let rate = match provider {
                ProviderKind::Crossref => self.crossref_rate_per_second,
                ProviderKind::OpenAlex | ProviderKind::DataCite => {
                    return Err(PermitError(format!(
                        "no rate is configured for {}",
                        provider.as_str()
                    )));
                }
            };
            acquire(self.pool, provider.as_str(), rate)
                .await
                .map_err(|error| PermitError(error.to_string()))
        })
    }
}
