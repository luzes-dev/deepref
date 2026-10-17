//! The DOI metadata lane: the router a worker fetch runs through, and how a route
//! that ends without an answer maps onto the acquisition item.

use deepref_application::{ProviderRouter, RouteError};
use deepref_config::IntegrationsConfig;
use deepref_domain::IngestionItemStatus;
use deepref_integrations::CrossrefClient;

/// The router for one DOI fetch: the deployment chain with the Crossref adapter.
/// The adapter's retry budget comes from the caller, because it is a runtime
/// setting. A missing contact e-mail makes the lane unavailable with the
/// adapter's own message.
pub fn router(
    integrations: &IntegrationsConfig,
    retry_attempts: usize,
) -> anyhow::Result<ProviderRouter> {
    let crossref = CrossrefClient::new(integrations)?.with_max_attempts(retry_attempts);
    Ok(ProviderRouter::new(
        integrations.metadata_providers.clone(),
        vec![Box::new(crossref)],
    )?)
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

    #[test]
    fn a_missing_contact_email_leaves_the_lane_unavailable_with_the_adapter_message() {
        let integrations = IntegrationsConfig::default();
        let error = router(&integrations, 5)
            .err()
            .expect("no contact e-mail, no lane");
        assert_eq!(
            error.to_string(),
            "provider request failed: a contact e-mail address is required; the server must set DEEPREF_CONTACT_EMAIL"
        );
    }

    #[test]
    fn a_configured_contact_email_builds_the_deployment_chain() {
        let integrations = IntegrationsConfig {
            contact_email: "ops@example.org".to_owned(),
            ..IntegrationsConfig::default()
        };
        assert!(router(&integrations, 5).is_ok());
    }
}
