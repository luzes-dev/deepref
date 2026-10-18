//! The DOI metadata lane: the adapters a worker process shares, the router a fetch
//! runs through, and how a route that ends without an answer maps onto the
//! acquisition item.

use deepref_application::{
    AttemptOutcome, LostEdgeCause, LostEdgeFill, LostEdges, ProviderAttempt, ProviderRouter,
    RouteError, RouteOutput, WorkProvider,
};
use deepref_config::IntegrationsConfig;
use deepref_domain::{IngestionItemStatus, ProviderKind};
use deepref_integrations::{
    CROSSREF_BASE, CrossrefClient, DATACITE_BASE, DataCiteClient, OpenAlexClient, OpenAlexError,
};

/// The metadata adapters one worker process shares. The OpenAlex client is built
/// once at boot, because its key state and daily list budget belong to the process.
/// Each fetch clones the handle, and every clone sees the same state.
#[derive(Debug, Clone)]
pub struct MetadataClients {
    /// Where Crossref requests go. The public API unless a test points it at a mock.
    pub crossref_base_url: String,
    /// Where DataCite requests go. The public API unless a test points it at a mock.
    pub datacite_base_url: String,
    /// `None` without a contact e-mail, which leaves the OpenAlex lane unavailable,
    /// as it does for Crossref.
    pub openalex: Option<OpenAlexClient>,
}

impl Default for MetadataClients {
    /// Crossref and DataCite at the public APIs and no OpenAlex, which is the process
    /// without a contact e-mail. Workflow steps that never reach the DOI lane take this.
    fn default() -> Self {
        Self {
            crossref_base_url: CROSSREF_BASE.to_owned(),
            datacite_base_url: DATACITE_BASE.to_owned(),
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
            datacite_base_url: DATACITE_BASE.to_owned(),
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
    let datacite =
        DataCiteClient::with_base_url(&clients.datacite_base_url, &integrations.contact_email)?
            .with_max_attempts(retry_attempts);
    let mut adapters: Vec<Box<dyn WorkProvider>> = vec![Box::new(crossref), Box::new(datacite)];
    if let Some(openalex) = &clients.openalex {
        adapters.push(Box::new(openalex.clone().with_max_attempts(retry_attempts)));
    }
    Ok(
        ProviderRouter::new(integrations.metadata_providers.clone(), adapters)?
            .with_reference_union(integrations.reference_union),
    )
}

/// Who an attempt asked. A metadata attempt looks up the DOI's record. A references attempt
/// reads a reference list to fill edges the metadata answer lacked.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Purpose {
    Metadata,
    References,
}

impl Purpose {
    fn as_str(self) -> &'static str {
        match self {
            Self::Metadata => "metadata",
            Self::References => "references",
        }
    }
}

/// One tracing event a routed DOI produces. The mapping from the router output is pure,
/// so it is tested without a subscriber. `report_route` logs what it returns.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RouteEvent {
    /// One call into an adapter and what the call produced.
    Attempt {
        provider: ProviderKind,
        purpose: Purpose,
        outcome: &'static str,
    },
    /// The chain moved past `from`, which did not answer, on to `to`.
    Fallback {
        from: ProviderKind,
        to: ProviderKind,
    },
}

/// The events for one routed DOI in the order they happened: each metadata attempt, then
/// the fallback when that attempt moved the chain on, then the reference attempt Crossref
/// made to fill lost edges. A reference fill is an attempt, not a fallback, because the
/// metadata answer already came back.
fn route_events(output: &RouteOutput) -> Vec<RouteEvent> {
    let mut events = Vec::with_capacity(output.attempts.len() + 1);
    for (index, attempt) in output.attempts.iter().enumerate() {
        events.push(RouteEvent::Attempt {
            provider: attempt.provider,
            purpose: Purpose::Metadata,
            outcome: attempt.outcome.name(),
        });
        let next = output.attempts.get(index + 1);
        if let Some(next) = next.filter(|_| moves_on(attempt)) {
            events.push(RouteEvent::Fallback {
                from: attempt.provider,
                to: next.provider,
            });
        }
    }
    if let Some(lost) = &output.lost_edges {
        let outcome = match &lost.fill {
            LostEdgeFill::Crossref { .. } => Some(AttemptOutcome::Answered),
            LostEdgeFill::CrossrefFailed { error } => Some(AttemptOutcome::from_error(error)),
            LostEdgeFill::CrossrefNotEnabled => None,
        };
        if let Some(outcome) = outcome {
            events.push(RouteEvent::Attempt {
                provider: ProviderKind::Crossref,
                purpose: Purpose::References,
                outcome: outcome.name(),
            });
        }
    }
    events
}

/// Whether an attempt hands the DOI to the next provider. A not-found or a retryable failure
/// does. An answer, a rejection or a fatal failure ends the chain.
fn moves_on(attempt: &ProviderAttempt) -> bool {
    matches!(
        attempt.outcome,
        AttemptOutcome::NotFound | AttemptOutcome::Retryable { .. }
    )
}

/// Logs what one routed DOI did: an event per attempt, one per fallback, then the lost-edge
/// report. The router returns its record and the worker logs it, because the application
/// layer does not log. Call it wherever the router output is consumed.
pub fn report_route(doi: &str, output: &RouteOutput) {
    for event in route_events(output) {
        match event {
            RouteEvent::Attempt {
                provider,
                purpose,
                outcome,
            } => tracing::info!(
                doi,
                provider = provider.as_str(),
                purpose = purpose.as_str(),
                outcome,
                "provider attempt"
            ),
            RouteEvent::Fallback { from, to } => tracing::info!(
                doi,
                from = from.as_str(),
                to = to.as_str(),
                "provider fallback"
            ),
        }
    }
    if let Some(lost) = &output.lost_edges {
        report_lost_edges(doi, lost);
    }
}

/// Logs an answer the router tried to fill. A budget stop warns, since the budget is spent.
/// A shortfall or an unverified list is routine, since most works have a target or two
/// without a DOI, so it logs at debug.
fn report_lost_edges(doi: &str, lost: &LostEdges) {
    let provider = lost.provider.as_str();
    let lost_edges = lost.missing();
    let LostEdgeCause::BudgetExhausted { reset_secs } = lost.cause else {
        tracing::debug!(
            doi,
            provider,
            lost_edges,
            fill = ?lost.fill,
            "the reference union read Crossref's list after the OpenAlex answer"
        );
        return;
    };
    match &lost.fill {
        LostEdgeFill::Crossref { added } => tracing::warn!(
            doi,
            provider,
            lost_edges,
            reset_secs,
            crossref_added = added,
            "OpenAlex budget spent before every reference resolved; Crossref filled the lost edges"
        ),
        LostEdgeFill::CrossrefNotEnabled => tracing::warn!(
            doi,
            provider,
            lost_edges,
            reset_secs,
            "OpenAlex budget spent before every reference resolved; Crossref is not enabled, so the references stay partial"
        ),
        LostEdgeFill::CrossrefFailed { error } => tracing::warn!(
            doi,
            provider,
            lost_edges,
            reset_secs,
            %error,
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
    use std::io::Write;
    use std::sync::{Arc, Mutex};

    use deepref_application::ProviderFetchError;

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
    fn a_configured_contact_email_registers_every_adapter_in_deployment_order() {
        let integrations = IntegrationsConfig {
            contact_email: "ops@example.org".to_owned(),
            ..IntegrationsConfig::default()
        };
        let router =
            router(&integrations, &clients(&integrations), 5).expect("the lane should build");
        assert_eq!(
            router.enabled(),
            [
                ProviderKind::OpenAlex,
                ProviderKind::Crossref,
                ProviderKind::DataCite
            ]
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

    fn attempt(provider: ProviderKind, outcome: AttemptOutcome) -> ProviderAttempt {
        ProviderAttempt { provider, outcome }
    }

    fn routed(attempts: Vec<ProviderAttempt>, lost_edges: Option<LostEdges>) -> RouteOutput {
        RouteOutput {
            attempts,
            result: Err(RouteError::NotFound { reason: None }),
            lost_edges,
        }
    }

    fn lost(fill: LostEdgeFill) -> LostEdges {
        LostEdges {
            provider: ProviderKind::OpenAlex,
            expected: 2,
            resolved: 1,
            cause: LostEdgeCause::Shortfall,
            fill,
        }
    }

    fn metadata_attempt(provider: ProviderKind, outcome: &'static str) -> RouteEvent {
        RouteEvent::Attempt {
            provider,
            purpose: Purpose::Metadata,
            outcome,
        }
    }

    fn references_attempt(outcome: &'static str) -> RouteEvent {
        RouteEvent::Attempt {
            provider: ProviderKind::Crossref,
            purpose: Purpose::References,
            outcome,
        }
    }

    #[test]
    fn an_answer_from_the_first_provider_is_one_attempt_and_no_fallback() {
        let output = routed(
            vec![attempt(ProviderKind::OpenAlex, AttemptOutcome::Answered)],
            None,
        );
        assert_eq!(
            route_events(&output),
            vec![metadata_attempt(ProviderKind::OpenAlex, "answered")]
        );
    }

    #[test]
    fn a_not_found_moves_on_and_records_the_fallback() {
        let output = routed(
            vec![
                attempt(ProviderKind::OpenAlex, AttemptOutcome::NotFound),
                attempt(ProviderKind::Crossref, AttemptOutcome::Answered),
            ],
            None,
        );
        assert_eq!(
            route_events(&output),
            vec![
                metadata_attempt(ProviderKind::OpenAlex, "not_found"),
                RouteEvent::Fallback {
                    from: ProviderKind::OpenAlex,
                    to: ProviderKind::Crossref,
                },
                metadata_attempt(ProviderKind::Crossref, "answered"),
            ]
        );
    }

    #[test]
    fn a_retryable_failure_moves_on_and_records_the_fallback() {
        let output = routed(
            vec![
                attempt(
                    ProviderKind::OpenAlex,
                    AttemptOutcome::Retryable {
                        message: "503".to_owned(),
                    },
                ),
                attempt(ProviderKind::Crossref, AttemptOutcome::Answered),
            ],
            None,
        );
        assert_eq!(
            route_events(&output),
            vec![
                metadata_attempt(ProviderKind::OpenAlex, "retryable"),
                RouteEvent::Fallback {
                    from: ProviderKind::OpenAlex,
                    to: ProviderKind::Crossref,
                },
                metadata_attempt(ProviderKind::Crossref, "answered"),
            ]
        );
    }

    #[test]
    fn a_fatal_or_rejected_attempt_ends_the_chain_without_a_fallback() {
        let fatal = routed(
            vec![attempt(
                ProviderKind::OpenAlex,
                AttemptOutcome::Fatal {
                    message: "blocked".to_owned(),
                },
            )],
            None,
        );
        let rejected = routed(
            vec![attempt(
                ProviderKind::Crossref,
                AttemptOutcome::Rejected {
                    reason: "paratext".to_owned(),
                },
            )],
            None,
        );
        assert_eq!(
            route_events(&fatal),
            vec![metadata_attempt(ProviderKind::OpenAlex, "fatal")]
        );
        assert_eq!(
            route_events(&rejected),
            vec![metadata_attempt(ProviderKind::Crossref, "rejected")]
        );
    }

    #[test]
    fn a_retryable_last_provider_has_no_fallback_after_it() {
        let output = routed(
            vec![
                attempt(ProviderKind::OpenAlex, AttemptOutcome::NotFound),
                attempt(
                    ProviderKind::Crossref,
                    AttemptOutcome::Retryable {
                        message: "503".to_owned(),
                    },
                ),
            ],
            None,
        );
        assert_eq!(
            route_events(&output),
            vec![
                metadata_attempt(ProviderKind::OpenAlex, "not_found"),
                RouteEvent::Fallback {
                    from: ProviderKind::OpenAlex,
                    to: ProviderKind::Crossref,
                },
                metadata_attempt(ProviderKind::Crossref, "retryable"),
            ]
        );
    }

    #[test]
    fn a_reference_fill_from_crossref_is_a_references_attempt_after_the_metadata() {
        let output = routed(
            vec![attempt(
                ProviderKind::OpenAlex,
                AttemptOutcome::BudgetExhausted,
            )],
            Some(lost(LostEdgeFill::Crossref { added: 2 })),
        );
        assert_eq!(
            route_events(&output),
            vec![
                metadata_attempt(ProviderKind::OpenAlex, "budget_exhausted"),
                references_attempt("answered"),
            ]
        );
    }

    #[test]
    fn a_failed_reference_fill_records_the_outcome_of_its_error() {
        let output = routed(
            vec![attempt(ProviderKind::OpenAlex, AttemptOutcome::Answered)],
            Some(lost(LostEdgeFill::CrossrefFailed {
                error: ProviderFetchError::Retryable {
                    provider: ProviderKind::Crossref,
                    message: "upstream timed out".to_owned(),
                },
            })),
        );
        assert_eq!(
            route_events(&output),
            vec![
                metadata_attempt(ProviderKind::OpenAlex, "answered"),
                references_attempt("retryable"),
            ]
        );
    }

    #[test]
    fn a_reference_fill_without_crossref_makes_no_attempt() {
        let output = routed(
            vec![attempt(ProviderKind::OpenAlex, AttemptOutcome::Answered)],
            Some(lost(LostEdgeFill::CrossrefNotEnabled)),
        );
        assert_eq!(
            route_events(&output),
            vec![metadata_attempt(ProviderKind::OpenAlex, "answered")]
        );
    }

    /// Collects the formatted log lines of one test and mirrors them to stderr, as the
    /// claim loop's log tests do.
    #[derive(Clone, Default)]
    struct CapturedLogs(Arc<Mutex<Vec<u8>>>);

    impl CapturedLogs {
        fn contents(&self) -> String {
            String::from_utf8_lossy(&self.0.lock().unwrap()).into_owned()
        }
    }

    impl Write for CapturedLogs {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(buf);
            std::io::stderr().write_all(buf)?;
            Ok(buf.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    /// The log lines `report_route` writes for `output`.
    fn logged(output: &RouteOutput) -> String {
        let logs = CapturedLogs::default();
        let writer = logs.clone();
        let subscriber = tracing_subscriber::fmt()
            .with_ansi(false)
            .without_time()
            .with_max_level(tracing::Level::INFO)
            .with_writer(move || writer.clone())
            .finish();
        tracing::subscriber::with_default(subscriber, || report_route(DOI, output));
        logs.contents()
    }

    #[test]
    fn report_route_logs_each_attempt_and_fallback_with_structured_fields() {
        let text = logged(&routed(
            vec![
                attempt(ProviderKind::OpenAlex, AttemptOutcome::NotFound),
                attempt(ProviderKind::Crossref, AttemptOutcome::Answered),
            ],
            None,
        ));

        assert!(
            text.contains(r#"provider="openalex" purpose="metadata" outcome="not_found""#),
            "{text}"
        );
        assert!(
            text.contains(r#"provider="crossref" purpose="metadata" outcome="answered""#),
            "{text}"
        );
        assert!(text.contains(r#"from="openalex" to="crossref""#), "{text}");
    }

    #[test]
    fn report_route_logs_the_budget_reset_as_a_typed_field() {
        let text = logged(&RouteOutput {
            attempts: vec![attempt(
                ProviderKind::OpenAlex,
                AttemptOutcome::BudgetExhausted,
            )],
            result: Err(RouteError::NotFound { reason: None }),
            lost_edges: Some(LostEdges {
                provider: ProviderKind::OpenAlex,
                expected: 4,
                resolved: 2,
                cause: LostEdgeCause::BudgetExhausted {
                    reset_secs: Some(3_600),
                },
                fill: LostEdgeFill::Crossref { added: 2 },
            }),
        });

        assert!(text.contains("reset_secs=3600"), "{text}");
        assert!(text.contains(r#"crossref_added=2"#), "{text}");
    }
}
