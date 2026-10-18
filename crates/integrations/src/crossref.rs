//! Crossref: work metadata and reference lists by DOI, with best-effort abstract
//! enrichment from Europe PMC / OpenAlex when Crossref has no abstract.

mod abstracts;
mod error;
mod search;
mod status;
mod wire;

use std::time::Duration;

use deepref_application::{
    AttemptPurpose, FetchedWork, OutboundRequest, ProviderError, ProviderFetchError,
    ProviderFuture, ProviderPermit, RawAuthor, RawIdentifier, RawRecord, ReferenceReport,
    WorkProvider,
};
use deepref_domain::WorkWithReferences;
use deepref_domain::{IdentifierScheme, ProviderKind, Reference, normalize_doi};
use reqwest::header;
use serde_json::{Value, json};

pub(crate) use abstracts::parse_openalex;
pub use abstracts::{
    AbstractEnricher, AbstractSource, EUROPE_PMC_RATE_PER_SECOND, EnricherBuildError,
    OpenAlexLookup, clean_abstract,
};
pub use error::CrossrefError;
pub use search::{CrossrefSearchError, search_crossref};
pub use status::{StatusClassification, classify_status};

use crate::openalex::OpenAlexClient;
use wire::CrossrefWorksResponse;

pub const CROSSREF_BASE: &str = "https://api.crossref.org";

/// The one Crossref client. Every lookup goes through the `WorkProvider` port, so
/// each request takes a permit. The enricher fills a missing abstract from Europe
/// PMC or OpenAlex.
#[derive(Debug, Clone)]
pub struct CrossrefClient {
    client: reqwest::Client,
    base_url: String,
    mailto: String,
    user_agent: String,
    max_attempts: usize,
    enricher: Option<AbstractEnricher>,
}

impl CrossrefClient {
    /// Builds the client for `api.crossref.org` with the deployment's contact
    /// address and the configured abstract enrichment.
    pub fn new(config: &deepref_config::IntegrationsConfig) -> Result<Self, ProviderError> {
        Self::for_base_url(CROSSREF_BASE, config, None)
    }

    /// The client for `base_url`, with the deployment's contact address and abstract
    /// enrichment. Tests point it at a mock server this way. With `openalex`, the enrichment's
    /// OpenAlex step goes through that shared client, so it keeps the metadata adapter's key
    /// state and rate instead of opening a second authentication path.
    pub fn for_base_url(
        base_url: impl Into<String>,
        config: &deepref_config::IntegrationsConfig,
        openalex: Option<&OpenAlexClient>,
    ) -> Result<Self, ProviderError> {
        let mailto = config.contact_email.clone();
        let enricher = if config.abstract_enrichment {
            match openalex {
                Some(openalex) => AbstractEnricher::with_openalex(
                    abstracts::EUROPE_PMC_BASE,
                    openalex.clone(),
                    mailto.clone(),
                ),
                None => AbstractEnricher::new(mailto.clone()),
            }
            .ok()
        } else {
            None
        };
        Self::with_base_url(base_url, mailto)
            .map(|client| Self { enricher, ..client })
            .map_err(|error| ProviderError::Request(error.to_string()))
    }

    /// Builds a client for another Crossref-compatible base URL, without enrichment.
    pub fn with_base_url(
        base_url: impl Into<String>,
        mailto: impl Into<String>,
    ) -> Result<Self, CrossrefError> {
        let mailto = mailto.into();
        if mailto.trim().is_empty() {
            return Err(CrossrefError::MissingContactEmail);
        }
        let user_agent = format!("deepref/0.1 (mailto:{mailto})");
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(15))
            .user_agent(user_agent.clone())
            .build()?;

        Ok(Self {
            client,
            base_url: base_url.into().trim_end_matches('/').to_owned(),
            mailto,
            user_agent,
            max_attempts: 5,
            enricher: None,
        })
    }

    #[must_use]
    pub fn with_max_attempts(mut self, max_attempts: usize) -> Self {
        self.max_attempts = max_attempts.max(1);
        self
    }

    /// One lookup with the client's retry policy. Every request, the first included, takes a
    /// permit immediately before it is sent, named for `purpose`.
    async fn fetch_metered(
        &self,
        doi: &str,
        purpose: AttemptPurpose,
        permit: &dyn ProviderPermit,
    ) -> Result<WorkWithReferences, CrossrefError> {
        let doi = deepref_domain::normalize_doi(doi)?;
        let encoded = urlencoding::encode(&doi);
        let url = format!("{}/works/{}?mailto={}", self.base_url, encoded, self.mailto);

        let mut last_retryable = None;
        for attempt in 1..=self.max_attempts {
            permit
                .acquire(OutboundRequest::new(ProviderKind::Crossref, purpose))
                .await?;
            let response = self
                .client
                .get(&url)
                .header(header::ACCEPT, "application/json")
                .header(header::USER_AGENT, &self.user_agent)
                .send()
                .await;

            match response {
                Ok(response) if response.status().is_success() => {
                    let bytes = response.bytes().await?;
                    let raw: serde_json::Value = serde_json::from_slice(&bytes)?;
                    let message_raw = raw.get("message").cloned().unwrap_or_default();
                    let body: CrossrefWorksResponse = serde_json::from_value(raw)?;
                    return Ok(body.into_domain(doi, message_raw));
                }
                Ok(response) => match classify_status(response.status()) {
                    StatusClassification::NotFound => return Err(CrossrefError::NotFound(doi)),
                    StatusClassification::Blocked => {
                        return Err(CrossrefError::Blocked(response.status()));
                    }
                    StatusClassification::Retryable => {
                        last_retryable = Some(CrossrefError::RetryableStatus(response.status()));
                        sleep_backoff(attempt).await;
                    }
                    StatusClassification::NonRetryable => {
                        return Err(CrossrefError::NonRetryableStatus(response.status()));
                    }
                },
                Err(error) if error.is_timeout() || error.is_connect() => {
                    last_retryable = Some(CrossrefError::Request(error));
                    sleep_backoff(attempt).await;
                }
                Err(error) => return Err(CrossrefError::Request(error)),
            }
        }

        Err(last_retryable.unwrap_or(CrossrefError::RetryableStatus(
            reqwest::StatusCode::SERVICE_UNAVAILABLE,
        )))
    }

    /// The DOI's reference list from Crossref, unresolved entries included. The lookup
    /// is free, so the router unions it with OpenAlex's references. Every request takes
    /// a permit named for the union.
    pub async fn fetch_reference_list(
        &self,
        doi: &str,
        permit: &dyn ProviderPermit,
    ) -> Result<Vec<Reference>, CrossrefError> {
        Ok(self
            .fetch_metered(doi, AttemptPurpose::ReferenceUnion, permit)
            .await?
            .references)
    }

    /// The lookup for a DOI the chain already sent to OpenAlex. It is the same as
    /// `fetch_work_with_references` except that the abstract fallback skips OpenAlex and
    /// still tries Europe PMC.
    pub async fn fetch_after_openalex(
        &self,
        doi: &str,
        permit: &dyn ProviderPermit,
    ) -> Result<FetchedWork, ProviderFetchError> {
        self.fetch_work(doi, permit, OpenAlexLookup::Disabled).await
    }

    /// One lookup with the client's retry policy, then the abstract fallback.
    async fn fetch_work(
        &self,
        doi: &str,
        permit: &dyn ProviderPermit,
        openalex: OpenAlexLookup,
    ) -> Result<FetchedWork, ProviderFetchError> {
        let work = self
            .fetch_metered(doi, AttemptPurpose::Metadata, permit)
            .await
            .map_err(CrossrefError::into_fetch_error)?;
        Ok(FetchedWork {
            work: self.enrich_abstract(work, openalex, permit).await,
            provider: ProviderKind::Crossref,
            references: ReferenceReport::default(),
        })
    }

    /// Fills a missing abstract from Europe PMC, and from OpenAlex unless `openalex`
    /// disables it. Never fails.
    async fn enrich_abstract(
        &self,
        mut work: WorkWithReferences,
        openalex: OpenAlexLookup,
        permit: &dyn ProviderPermit,
    ) -> WorkWithReferences {
        let source = if work.work.abstract_text.is_some() {
            AbstractSource::Crossref
        } else if let Some(enricher) = &self.enricher
            && let Some((text, source)) = enricher.lookup(&work.work.doi, openalex, permit).await
        {
            work.work.abstract_text = Some(text);
            source
        } else {
            return work;
        };
        if let Some(raw) = work.raw.as_object_mut() {
            raw.insert("deepref_abstract_source".to_owned(), json!(source.as_str()));
        }
        work
    }
}

pub(crate) async fn sleep_backoff(attempt: usize) {
    let millis = 250_u64.saturating_mul(2_u64.saturating_pow(attempt as u32 - 1));
    tokio::time::sleep(Duration::from_millis(millis.min(5_000))).await;
}

/// The router's entry point. Every request takes its own permit.
impl WorkProvider for CrossrefClient {
    fn kind(&self) -> ProviderKind {
        ProviderKind::Crossref
    }

    fn fetch_work_with_references<'a>(
        &'a self,
        doi: &'a str,
        permit: &'a dyn ProviderPermit,
    ) -> ProviderFuture<'a, FetchedWork, ProviderFetchError> {
        Box::pin(self.fetch_work(doi, permit, OpenAlexLookup::Enabled))
    }

    fn fetch_work_after<'a>(
        &'a self,
        doi: &'a str,
        tried: &'a [ProviderKind],
        permit: &'a dyn ProviderPermit,
    ) -> ProviderFuture<'a, FetchedWork, ProviderFetchError> {
        if tried.contains(&ProviderKind::OpenAlex) {
            Box::pin(self.fetch_after_openalex(doi, permit))
        } else {
            self.fetch_work_with_references(doi, permit)
        }
    }

    fn reference_list<'a>(
        &'a self,
        doi: &'a str,
        permit: &'a dyn ProviderPermit,
    ) -> ProviderFuture<'a, Vec<Reference>, ProviderFetchError> {
        Box::pin(async move {
            self.fetch_reference_list(doi, permit)
                .await
                .map_err(CrossrefError::into_fetch_error)
        })
    }
}

impl CrossrefError {
    /// Classifies a failed lookup for the provider chain. Status and transport
    /// failures retry, and everything else is fatal for this provider: the chain records
    /// it and moves on. The worker's job-level retry splits the same errors the same way.
    fn into_fetch_error(self) -> ProviderFetchError {
        let provider = ProviderKind::Crossref;
        let message = self.to_string();
        match self {
            Self::NotFound(doi) => ProviderFetchError::NotFound {
                provider,
                doi,
                reason: None,
            },
            Self::InvalidDoi(_) => ProviderFetchError::Rejected {
                provider,
                reason: message,
            },
            Self::RetryableStatus(_) | Self::Request(_) | Self::Permit(_) => {
                ProviderFetchError::Retryable { provider, message }
            }
            Self::MissingContactEmail
            | Self::Blocked(_)
            | Self::NonRetryableStatus(_)
            | Self::Json(_) => ProviderFetchError::Fatal { provider, message },
        }
    }
}

#[must_use]
pub fn raw_record_from_crossref_work(work: WorkWithReferences) -> RawRecord {
    let doi = work.work.doi.clone();
    let authors = work
        .raw
        .get("author")
        .and_then(Value::as_array)
        .map(|authors| {
            authors
                .iter()
                .map(|author| {
                    RawAuthor::named(
                        author
                            .get("given")
                            .and_then(Value::as_str)
                            .map(ToOwned::to_owned),
                        author
                            .get("family")
                            .and_then(Value::as_str)
                            .map(ToOwned::to_owned),
                    )
                })
                .collect()
        })
        .unwrap_or_default();
    RawRecord {
        source_identifiers: vec![RawIdentifier {
            scheme: IdentifierScheme::Doi,
            value: doi.clone(),
            normalized_value: normalize_doi(&doi).unwrap_or(doi),
        }],
        title: work.work.title,
        abstract_text: work.work.abstract_text,
        authors,
        publication_year: work.work.published_year.or(work.work.issued_year),
        journal: work.work.container_title,
        raw: work.raw,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse_import;
    use chrono::Utc;
    use deepref_application::{OutboundService, PermitError};
    use deepref_domain::ImportFormat;
    use deepref_domain::{FetchStatus, Work, WorkWithReferences};
    use reqwest::StatusCode;

    #[test]
    fn crossref_adapter_emits_the_application_raw_record() {
        let work = WorkWithReferences {
            work: Work {
                doi: "10.1000/example".to_owned(),
                title: Some("A title".to_owned()),
                abstract_text: Some("An abstract".to_owned()),
                work_type: None,
                publisher: None,
                container_title: Some("A journal".to_owned()),
                issued_year: Some(2024),
                published_year: None,
                url: None,
                total_citations: 0,
                references_count: 0,
                metadata_provider: "crossref".to_owned(),
                citation_provider: "crossref".to_owned(),
                fetched_at: Utc::now(),
                fetch_status: FetchStatus::Fetched.as_str().to_owned(),
            },
            references: Vec::new(),
            raw: serde_json::json!({
                "title": ["A title"],
                "abstract": "An abstract",
                "container-title": ["A journal"],
                "issued": {"date-parts": [[2024]]},
                "author": [{"given": "Ana", "family": "Müller"}]
            }),
        };
        let raw = raw_record_from_crossref_work(work);
        assert_eq!(raw.title.as_deref(), Some("A title"));
        assert_eq!(raw.publication_year, Some(2024));
        assert_eq!(
            raw.source_identifiers[0].normalized_value,
            "10.1000/example"
        );
    }

    #[test]
    fn crossref_and_ris_share_the_same_raw_record_semantic_boundary() {
        let imported = parse_import(
            include_bytes!("../tests/fixtures/sample.ris"),
            ImportFormat::Ris,
            None,
        )
        .unwrap()
        .remove(0);
        let work = WorkWithReferences {
            work: Work {
                doi: "10.5555/example-unicode".to_owned(),
                title: Some("Über die Wirkung von Kaffee".to_owned()),
                abstract_text: Some("Ein kurzer Überblick über Evidenz.".to_owned()),
                work_type: None,
                publisher: None,
                container_title: Some("Journal of Café Studies".to_owned()),
                issued_year: Some(2024),
                published_year: None,
                url: None,
                total_citations: 0,
                references_count: 0,
                metadata_provider: "crossref".to_owned(),
                citation_provider: "crossref".to_owned(),
                fetched_at: Utc::now(),
                fetch_status: FetchStatus::Fetched.as_str().to_owned(),
            },
            references: Vec::new(),
            raw: serde_json::json!({
                "author": [{"given": "Ana", "family": "Müller"}]
            }),
        };
        let provider_record = raw_record_from_crossref_work(work);
        assert_eq!(
            provider_record.source_identifiers,
            imported.source_identifiers
        );
        assert_eq!(provider_record.title, imported.title);
        assert_eq!(provider_record.abstract_text, imported.abstract_text);
        assert_eq!(provider_record.authors, imported.authors);
        assert_eq!(provider_record.publication_year, imported.publication_year);
        assert_eq!(provider_record.journal, imported.journal);
    }

    const DOI: &str = "10.1000/example";
    const WORK_BODY: &str = r#"{"message":{"title":["Example"],"reference":[{"DOI":"10.1000/CITED"},{"key":"ref1","unstructured":"A reference without a DOI"}],"is-referenced-by-count":3,"references-count":2}}"#;

    /// Serves the scripted responses in order, one connection each, then stops.
    async fn scripted_server(responses: Vec<(&'static str, &'static str)>) -> String {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(async move {
            for (status_line, body) in responses {
                let Ok((mut socket, _)) = listener.accept().await else {
                    return;
                };
                let mut request = [0_u8; 4096];
                let _ = socket.read(&mut request).await;
                let response = format!(
                    "HTTP/1.1 {status_line}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = socket.write_all(response.as_bytes()).await;
            }
        });
        format!("http://{address}")
    }

    use crate::test_permit::RecordingPermit;

    fn test_client(base_url: String, max_attempts: usize) -> CrossrefClient {
        CrossrefClient::with_base_url(base_url, "ops@example.org")
            .expect("client should build")
            .with_max_attempts(max_attempts)
    }

    #[tokio::test]
    async fn a_found_work_is_answered_as_crossref_on_the_callers_permit() {
        let base = scripted_server(vec![("200 OK", WORK_BODY)]).await;
        let client = test_client(base, 1);
        let permit = RecordingPermit::default();

        let fetched = WorkProvider::fetch_work_with_references(&client, DOI, &permit)
            .await
            .expect("crossref should answer");

        assert_eq!(client.kind(), ProviderKind::Crossref);
        assert_eq!(fetched.provider, ProviderKind::Crossref);
        assert_eq!(fetched.work.work.title.as_deref(), Some("Example"));
        assert_eq!(fetched.work.references.len(), 2);
        assert_eq!(
            permit.requests(),
            vec![OutboundRequest::new(
                ProviderKind::Crossref,
                AttemptPurpose::Metadata
            )],
            "the first request takes its own permit"
        );
    }

    #[tokio::test]
    async fn a_missing_work_is_not_found() {
        let base = scripted_server(vec![("404 Not Found", "{}")]).await;
        let client = test_client(base, 1);

        let error =
            WorkProvider::fetch_work_with_references(&client, DOI, &RecordingPermit::default())
                .await
                .expect_err("a 404 should not answer");

        assert_eq!(
            error,
            ProviderFetchError::NotFound {
                reason: None,
                provider: ProviderKind::Crossref,
                doi: DOI.to_owned(),
            }
        );
    }

    #[tokio::test]
    async fn a_rate_limited_lookup_retries_on_a_new_permit_and_then_answers() {
        let base =
            scripted_server(vec![("429 Too Many Requests", "{}"), ("200 OK", WORK_BODY)]).await;
        let client = test_client(base, 2);
        let permit = RecordingPermit::default();

        let fetched = WorkProvider::fetch_work_with_references(&client, DOI, &permit)
            .await
            .expect("the retry should answer");

        assert_eq!(fetched.provider, ProviderKind::Crossref);
        assert_eq!(
            permit.taken(),
            vec![ProviderKind::Crossref; 2],
            "the first request and its retry each take a permit"
        );
    }

    #[tokio::test]
    async fn a_server_error_that_persists_is_retryable() {
        let base = scripted_server(vec![("500 Internal Server Error", "{}")]).await;
        let client = test_client(base, 1);

        let error =
            WorkProvider::fetch_work_with_references(&client, DOI, &RecordingPermit::default())
                .await
                .expect_err("a 500 should not answer");

        match error {
            ProviderFetchError::Retryable { provider, message } => {
                assert_eq!(provider, ProviderKind::Crossref);
                assert!(
                    message.contains("500"),
                    "the message names the status: {message}"
                );
            }
            other => panic!("expected a retryable failure, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn the_reference_list_helper_keeps_entries_without_a_doi_and_takes_a_permit() {
        let base = scripted_server(vec![("200 OK", WORK_BODY)]).await;
        let client = test_client(base, 1);
        let permit = RecordingPermit::default();

        let references = client
            .fetch_reference_list(DOI, &permit)
            .await
            .expect("the lookup should succeed");

        assert_eq!(references.len(), 2);
        assert_eq!(references[0].doi.as_deref(), Some("10.1000/cited"));
        assert_eq!(references[1].doi, None);
        assert_eq!(
            references[1].raw_unstructured.as_deref(),
            Some("A reference without a DOI")
        );
        assert_eq!(permit.taken(), vec![ProviderKind::Crossref]);
    }

    const NO_EUROPE_PMC_MATCH: &str = r#"{"resultList":{"result":[]}}"#;
    const OPENALEX_ABSTRACT: &str = r#"{"abstract_inverted_index":{"Hello":[0],"world":[1]}}"#;

    /// A Crossref client whose abstract fallback reads from the given Europe PMC and
    /// OpenAlex servers.
    fn client_with_enricher(
        crossref: String,
        europe_pmc: String,
        openalex: String,
    ) -> CrossrefClient {
        let mut client = test_client(crossref, 1);
        client.enricher = Some(
            AbstractEnricher::with_base_urls(europe_pmc, openalex, "ops@example.org")
                .expect("enricher should build"),
        );
        client
    }

    #[tokio::test]
    async fn a_fallback_after_openalex_skips_the_openalex_abstract_step() {
        let client = client_with_enricher(
            scripted_server(vec![("200 OK", WORK_BODY)]).await,
            scripted_server(vec![("200 OK", NO_EUROPE_PMC_MATCH)]).await,
            scripted_server(vec![("200 OK", OPENALEX_ABSTRACT)]).await,
        );

        let fetched = client
            .fetch_after_openalex(DOI, &RecordingPermit::default())
            .await
            .expect("crossref should answer");

        assert_eq!(
            fetched.work.work.abstract_text, None,
            "OpenAlex was already asked, so the abstract step does not ask it again"
        );
        assert!(fetched.work.raw.get("deepref_abstract_source").is_none());
    }

    #[tokio::test]
    async fn the_default_lookup_still_takes_the_abstract_from_openalex() {
        let client = client_with_enricher(
            scripted_server(vec![("200 OK", WORK_BODY)]).await,
            scripted_server(vec![("200 OK", NO_EUROPE_PMC_MATCH)]).await,
            scripted_server(vec![("200 OK", OPENALEX_ABSTRACT)]).await,
        );

        let fetched =
            WorkProvider::fetch_work_with_references(&client, DOI, &RecordingPermit::default())
                .await
                .expect("crossref should answer");

        assert_eq!(
            fetched.work.work.abstract_text.as_deref(),
            Some("Hello world")
        );
        assert_eq!(
            fetched.work.raw["deepref_abstract_source"],
            json!("openalex")
        );
    }

    #[tokio::test]
    async fn the_port_skips_the_openalex_abstract_step_only_when_openalex_tried_the_doi() {
        let skipped = client_with_enricher(
            scripted_server(vec![("200 OK", WORK_BODY)]).await,
            scripted_server(vec![("200 OK", NO_EUROPE_PMC_MATCH)]).await,
            scripted_server(vec![("200 OK", OPENALEX_ABSTRACT)]).await,
        );
        let fetched = WorkProvider::fetch_work_after(
            &skipped,
            DOI,
            &[ProviderKind::OpenAlex],
            &RecordingPermit::default(),
        )
        .await
        .expect("crossref should answer");
        assert_eq!(fetched.work.work.abstract_text, None);

        let asked = client_with_enricher(
            scripted_server(vec![("200 OK", WORK_BODY)]).await,
            scripted_server(vec![("200 OK", NO_EUROPE_PMC_MATCH)]).await,
            scripted_server(vec![("200 OK", OPENALEX_ABSTRACT)]).await,
        );
        let fetched = WorkProvider::fetch_work_after(&asked, DOI, &[], &RecordingPermit::default())
            .await
            .expect("crossref should answer");
        assert_eq!(
            fetched.work.work.abstract_text.as_deref(),
            Some("Hello world")
        );
    }

    #[tokio::test]
    async fn abstract_enrichment_takes_a_permit_before_every_request_retries_included() {
        let client = client_with_enricher(
            scripted_server(vec![("200 OK", WORK_BODY)]).await,
            scripted_server(vec![
                ("503 Service Unavailable", "{}"),
                ("200 OK", NO_EUROPE_PMC_MATCH),
            ])
            .await,
            scripted_server(vec![("200 OK", OPENALEX_ABSTRACT)]).await,
        );
        let permit = RecordingPermit::default();

        let fetched = WorkProvider::fetch_work_with_references(&client, DOI, &permit)
            .await
            .expect("crossref should answer");

        assert_eq!(
            fetched.work.work.abstract_text.as_deref(),
            Some("Hello world")
        );
        assert_eq!(permit.count(OutboundService::EuropePmc), 2);
        assert_eq!(
            permit.count(OutboundService::Provider(ProviderKind::OpenAlex)),
            1
        );
        assert_eq!(
            permit.count(OutboundService::Provider(ProviderKind::Crossref)),
            1
        );
        assert!(
            permit
                .requests()
                .iter()
                .filter(|request| request.service != ProviderKind::Crossref)
                .all(|request| request.purpose == AttemptPurpose::AbstractEnrichment)
        );
    }

    #[tokio::test]
    async fn the_enrichment_shares_the_metadata_adapters_openalex_key_state() {
        let openalex_server = scripted_server(vec![
            ("401 Unauthorized", r#"{"error":"invalid key"}"#),
            ("200 OK", OPENALEX_ABSTRACT),
        ])
        .await;
        let shared = crate::OpenAlexClient::with_base_url(
            openalex_server,
            "ops@example.org",
            Some("test-key".to_owned()),
            deepref_config::OpenAlexCorpus::Core,
        )
        .expect("client should build");
        let mut client = test_client(scripted_server(vec![("200 OK", WORK_BODY)]).await, 1);
        client.enricher = Some(
            AbstractEnricher::with_openalex(
                scripted_server(vec![("200 OK", NO_EUROPE_PMC_MATCH)]).await,
                shared.clone(),
                "ops@example.org",
            )
            .expect("enricher should build"),
        );

        let fetched =
            WorkProvider::fetch_work_with_references(&client, DOI, &RecordingPermit::default())
                .await
                .expect("crossref should answer");

        assert_eq!(
            fetched.work.work.abstract_text.as_deref(),
            Some("Hello world")
        );
        assert_eq!(
            shared.key_state(),
            crate::OpenAlexKeyState::Keyless,
            "the rejected key downgrades the one shared state"
        );
    }

    #[tokio::test]
    async fn the_port_reads_the_reference_list_through_the_helper() {
        let client = test_client(scripted_server(vec![("200 OK", WORK_BODY)]).await, 1);
        let permit = RecordingPermit::default();

        let references = WorkProvider::reference_list(&client, DOI, &permit)
            .await
            .expect("the reference list should load");

        assert_eq!(references.len(), 2);
        assert_eq!(references[0].doi.as_deref(), Some("10.1000/cited"));
        assert_eq!(permit.taken(), vec![ProviderKind::Crossref]);
    }

    #[tokio::test]
    async fn a_body_that_is_not_json_is_a_fatal_response_not_a_transport_failure() {
        let client = test_client(
            scripted_server(vec![("200 OK", "not json at all")]).await,
            1,
        );

        let error =
            WorkProvider::fetch_work_with_references(&client, DOI, &RecordingPermit::default())
                .await
                .expect_err("a body that is not JSON must not answer");

        assert!(
            matches!(error, ProviderFetchError::Fatal { .. }),
            "{error:?}"
        );
    }

    #[test]
    fn each_crossref_failure_maps_to_the_outcome_the_chain_acts_on() {
        let provider = ProviderKind::Crossref;
        let invalid = deepref_domain::normalize_doi("not a doi").expect_err("not a DOI");

        assert_eq!(
            CrossrefError::NotFound(DOI.to_owned()).into_fetch_error(),
            ProviderFetchError::NotFound {
                reason: None,
                provider,
                doi: DOI.to_owned(),
            }
        );
        assert!(matches!(
            CrossrefError::InvalidDoi(invalid).into_fetch_error(),
            ProviderFetchError::Rejected { .. }
        ));
        assert!(matches!(
            CrossrefError::RetryableStatus(StatusCode::BAD_GATEWAY).into_fetch_error(),
            ProviderFetchError::Retryable { .. }
        ));
        assert!(matches!(
            CrossrefError::Permit(PermitError("limiter is down".to_owned())).into_fetch_error(),
            ProviderFetchError::Retryable { .. }
        ));
        assert!(matches!(
            CrossrefError::Blocked(StatusCode::FORBIDDEN).into_fetch_error(),
            ProviderFetchError::Fatal { .. }
        ));
        assert!(matches!(
            CrossrefError::NonRetryableStatus(StatusCode::BAD_REQUEST).into_fetch_error(),
            ProviderFetchError::Fatal { .. }
        ));
    }
}
