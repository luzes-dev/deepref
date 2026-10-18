use deepref_application::{
    OutboundRequest, OutboundService, PermitError, ProviderFuture, ProviderPermit,
};
use deepref_domain::ProviderKind;
use deepref_integrations::{DATACITE_RATE_PER_SECOND, EUROPE_PMC_RATE_PER_SECOND, OpenAlexClient};
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
/// deployment's configured rate, and OpenAlex the rate of its client's key state.
pub struct ProviderLimiter<'a> {
    pool: &'a PgPool,
    crossref_rate_per_second: u32,
    openalex: Option<&'a OpenAlexClient>,
}

impl<'a> ProviderLimiter<'a> {
    pub fn new(
        pool: &'a PgPool,
        crossref_rate_per_second: u32,
        openalex: Option<&'a OpenAlexClient>,
    ) -> Self {
        Self {
            pool,
            crossref_rate_per_second,
            openalex,
        }
    }
}

impl ProviderPermit for ProviderLimiter<'_> {
    fn acquire<'a>(&'a self, request: OutboundRequest) -> ProviderFuture<'a, (), PermitError> {
        Box::pin(async move {
            let rate = service_rate(
                request.service,
                self.crossref_rate_per_second,
                self.openalex,
            )?;
            acquire(self.pool, request.service.as_str(), rate)
                .await
                .map_err(|error| PermitError(error.to_string()))
        })
    }
}

/// The per-second rate for `service`. OpenAlex reads its rate from the client on each
/// permit, so a downgrade to keyless lowers it for every later request. DataCite's and
/// Europe PMC's rates are fixed by their service limits.
fn service_rate(
    service: OutboundService,
    crossref_rate_per_second: u32,
    openalex: Option<&OpenAlexClient>,
) -> Result<u32, PermitError> {
    match service {
        OutboundService::Provider(ProviderKind::Crossref) => Ok(crossref_rate_per_second),
        OutboundService::Provider(ProviderKind::OpenAlex) => openalex
            .map(OpenAlexClient::rate_per_second)
            .ok_or_else(|| no_rate(service)),
        OutboundService::Provider(ProviderKind::DataCite) => Ok(DATACITE_RATE_PER_SECOND),
        OutboundService::EuropePmc => Ok(EUROPE_PMC_RATE_PER_SECOND),
    }
}

fn no_rate(service: OutboundService) -> PermitError {
    PermitError(format!("no rate is configured for {}", service.as_str()))
}

#[cfg(test)]
mod tests {
    use deepref_config::OpenAlexCorpus;
    use deepref_integrations::{KEYED_RATE_PER_SECOND, KEYLESS_RATE_PER_SECOND};

    use super::*;

    fn openalex(api_key: Option<&str>) -> OpenAlexClient {
        OpenAlexClient::with_base_url(
            "http://127.0.0.1:9",
            "ops@example.org",
            api_key.map(str::to_owned),
            OpenAlexCorpus::Core,
        )
        .expect("client should build")
    }

    #[test]
    fn openalex_takes_its_rate_from_the_key_state() {
        let keyed = openalex(Some("test-key"));
        let keyless = openalex(None);

        assert_eq!(
            service_rate(
                OutboundService::Provider(ProviderKind::OpenAlex),
                4,
                Some(&keyed)
            ),
            Ok(KEYED_RATE_PER_SECOND)
        );
        assert_eq!(
            service_rate(
                OutboundService::Provider(ProviderKind::OpenAlex),
                4,
                Some(&keyless)
            ),
            Ok(KEYLESS_RATE_PER_SECOND)
        );
    }

    #[test]
    fn crossref_keeps_the_configured_rate_datacite_its_fixed_one_and_openalex_needs_a_client() {
        let provider = OutboundService::Provider;
        assert_eq!(
            service_rate(provider(ProviderKind::Crossref), 4, None),
            Ok(4)
        );
        assert_eq!(
            service_rate(provider(ProviderKind::DataCite), 4, None),
            Ok(DATACITE_RATE_PER_SECOND)
        );
        assert_eq!(
            service_rate(OutboundService::EuropePmc, 4, None),
            Ok(EUROPE_PMC_RATE_PER_SECOND)
        );
        assert!(service_rate(provider(ProviderKind::OpenAlex), 4, None).is_err());
    }
}
