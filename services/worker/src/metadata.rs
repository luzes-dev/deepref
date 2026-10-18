//! The DOI metadata lane: the adapters a worker process shares, the router a fetch
//! runs through, and how a route that ends without an answer maps onto the
//! acquisition item.

use deepref_application::{LostEdgeFill, LostEdges, ProviderRouter, RouteError, WorkProvider};
use deepref_config::IntegrationsConfig;
use deepref_domain::IngestionItemStatus;
use deepref_integrations::{CROSSREF_BASE, CrossrefClient, OpenAlexClient, OpenAlexError};

/// The metadata adapters one worker process shares. The OpenAlex client is built
/// once at boot, because its key state and daily list budget belong to the process.
/// Each fetch clones the handle, and every clone sees the same state.
#[derive(Debug, Clone)]
pub struct MetadataClients {
    /// Where Crossref requests go. The public API unless a test points it at a mock.
    pub crossref_base_url: String,
    /// `None` without a contact e-mail, which leaves the OpenAlex lane unavailable,
    /// as it does for Crossref.
    pub openalex: Option<OpenAlexClient>,
}

impl Default for MetadataClients {
    /// Crossref at the public API and no OpenAlex, which is the process without a contact
    /// e-mail. Workflow steps that never reach the DOI lane take this.
    fn default() -> Self {
        Self {
            crossref_base_url: CROSSREF_BASE.to_owned(),
            openalex: None,
        }
    }
}

impl MetadataClients {
    pub fn from_config(integrations: &IntegrationsConfig) -> anyhow::Result<Self> {
        let openalex = match OpenAlexClient::new(integrations) {
            Ok(client) => Some(client),
            // A missing contact e-mail is reported per DOI job, not at boot.
            Err(OpenAlexError::MissingContactEmail) => None,
            Err(error) => return Err(error.into()),
        };
        Ok(Self {
            crossref_base_url: CROSSREF_BASE.to_owned(),
            openalex,
        })
    }
}

/// The router for one DOI fetch: the deployment chain with the adapters this
/// process shares. The adapters' retry budget comes from the caller, because it is
/// a runtime setting. A missing contact e-mail makes the lane unavailable with the
/// Crossref adapter's own message.
pub fn router(
    integrations: &IntegrationsConfig,
    clients: &MetadataClients,
    retry_attempts: usize,
) -> anyhow::Result<ProviderRouter> {
    let crossref = CrossrefClient::for_base_url(&clients.crossref_base_url, integrations)?
        .with_max_attempts(retry_attempts);
    let mut adapters: Vec<Box<dyn WorkProvider>> = vec![Box::new(crossref)];
    if let Some(openalex) = &clients.openalex {
        adapters.push(Box::new(openalex.clone().with_max_attempts(retry_attempts)));
    }
    Ok(ProviderRouter::new(
        integrations.metadata_providers.clone(),
        adapters,
    )?)
}

/// Logs a budget stop that cost an answer some references. The router returns the
/// record, and the worker logs it, because the application layer does not log.
pub fn report_lost_edges(doi: &str, lost: &LostEdges) {
    let provider = lost.provider.as_str();
    let lost_edges = lost.count;
    let reset_secs = lost.reset_secs;
    match &lost.fill {
        LostEdgeFill::Crossref { added } => tracing::warn!(
            doi,
            provider,
            lost_edges,
            ?reset_secs,
            crossref_added = added,
            "OpenAlex budget spent before every reference resolved; Crossref filled the lost edges"
        ),
        LostEdgeFill::CrossrefNotEnabled => tracing::warn!(
            doi,
            provider,
            lost_edges,
            ?reset_secs,
            "OpenAlex budget spent before every reference resolved; Crossref is not enabled, so the references stay partial"
        ),
        LostEdgeFill::CrossrefFailed { message } => tracing::warn!(
            doi,
            provider,
            lost_edges,
            ?reset_secs,
            %message,
            "OpenAlex budget spent before every reference resolved; the Crossref reference list failed, so the references stay partial"
        ),
    }
}
/// The item status and last error for a route that will not be retried. A
/// rejected answer is not found with its reason, and a fatal failure is a failed
/// item.
pub fn terminal_outcome(error: &RouteError, doi: &str) -> (IngestionItemStatus, String) {
    match error {
        RouteError::NotFound {
            reason: Some(reason),
        } => (IngestionItemStatus::NotFound, reason.clone()),
        RouteError::NotFound { reason: None } => (
            IngestionItemStatus::NotFound,
            format!("No metadata provider has a record for {doi}."),
        ),
        RouteError::Retryable { message } | RouteError::Fatal { message } => {
            (IngestionItemStatus::Failed, message.clone())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use deepref_domain::ProviderKind;

    const DOI: &str = "10.1000/example";

    #[test]
    fn a_chain_where_nobody_holds_the_doi_is_not_found() {
        assert_eq!(
            terminal_outcome(&RouteError::NotFound { reason: None }, DOI),
            (
                IngestionItemStatus::NotFound,
                "No metadata provider has a record for 10.1000/example.".to_owned(),
            )
        );
    }

    #[test]
    fn a_rejected_answer_is_not_found_with_its_reason() {
        assert_eq!(
            terminal_outcome(
                &RouteError::NotFound {
                    reason: Some("paratext".to_owned()),
                },
                DOI,
            ),
            (IngestionItemStatus::NotFound, "paratext".to_owned())
        );
    }

    #[test]
    fn a_fatal_or_exhausted_retryable_failure_is_a_failed_item() {
        assert_eq!(
            terminal_outcome(
                &RouteError::Fatal {
                    message: "Crossref blocked or rejected the request: 403 Forbidden".to_owned(),
                },
                DOI,
            ),
            (
                IngestionItemStatus::Failed,
                "Crossref blocked or rejected the request: 403 Forbidden".to_owned(),
            )
        );
        assert_eq!(
            terminal_outcome(
                &RouteError::Retryable {
                    message: "Crossref returned retryable status: 503 Service Unavailable"
                        .to_owned(),
                },
                DOI,
            ),
            (
                IngestionItemStatus::Failed,
                "Crossref returned retryable status: 503 Service Unavailable".to_owned(),
            )
        );
    }

    fn clients(integrations: &IntegrationsConfig) -> MetadataClients {
        MetadataClients::from_config(integrations).expect("clients should build")
    }

    #[test]
    fn a_missing_contact_email_leaves_the_lane_unavailable_with_the_adapter_message() {
        let integrations = IntegrationsConfig::default();
        let error = router(&integrations, &clients(&integrations), 5)
            .err()
            .expect("no contact e-mail, no lane");
        assert_eq!(
            error.to_string(),
            "provider request failed: a contact e-mail address is required; the server must set DEEPREF_CONTACT_EMAIL"
        );
    }

    #[test]
    fn a_configured_contact_email_registers_both_adapters_in_deployment_order() {
        let integrations = IntegrationsConfig {
            contact_email: "ops@example.org".to_owned(),
            ..IntegrationsConfig::default()
        };
        let router =
            router(&integrations, &clients(&integrations), 5).expect("the lane should build");
        assert_eq!(
            router.enabled(),
            [ProviderKind::OpenAlex, ProviderKind::Crossref]
        );
    }

    #[test]
    fn a_crossref_only_chain_leaves_openalex_out_of_the_router() {
        let integrations = IntegrationsConfig {
            contact_email: "ops@example.org".to_owned(),
            metadata_providers: vec![ProviderKind::Crossref],
            ..IntegrationsConfig::default()
        };
        let router =
            router(&integrations, &clients(&integrations), 5).expect("the lane should build");
        assert_eq!(router.enabled(), [ProviderKind::Crossref]);
    }
}
