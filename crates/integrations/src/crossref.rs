//! Crossref: work metadata and reference lists by DOI, with best-effort abstract
//! enrichment from Europe PMC / OpenAlex when Crossref has no abstract.

mod abstracts;
mod error;
mod search;
mod status;
mod wire;

use std::time::Duration;

use deepref_application::{
    FetchedWork, ProviderError, ProviderFetchError, ProviderFuture, ProviderPermit, RawAuthor,
    RawIdentifier, RawRecord, WorkProvider,
};
use deepref_domain::WorkWithReferences;
use deepref_domain::{IdentifierScheme, ProviderKind, Reference, normalize_doi};
use reqwest::header;
use serde_json::{Value, json};

pub(crate) use abstracts::parse_openalex;
pub use abstracts::{AbstractEnricher, AbstractSource, clean_abstract};
pub use error::CrossrefError;
pub use search::{CrossrefSearchError, search_crossref};
pub use status::{StatusClassification, classify_status};

use wire::CrossrefWorksResponse;

const CROSSREF_BASE: &str = "https://api.crossref.org";

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
        let mailto = config.contact_email.clone();
        let enricher = if config.abstract_enrichment {
            AbstractEnricher::new(mailto.clone()).ok()
        } else {
            None
        };
        Self::with_base_url(CROSSREF_BASE, mailto)
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

    /// One lookup with the client's retry policy. The caller's permit covers the
    /// first request, and each retry takes another.
    async fn fetch_metered(
        &self,
        doi: &str,
        permit: &dyn ProviderPermit,
    ) -> Result<WorkWithReferences, CrossrefError> {
        let doi = deepref_domain::normalize_doi(doi)?;
        let encoded = urlencoding::encode(&doi);
        let url = format!("{}/works/{}?mailto={}", self.base_url, encoded, self.mailto);

        let mut last_retryable = None;
        for attempt in 1..=self.max_attempts {
            if attempt > 1 {
                permit.acquire(ProviderKind::Crossref).await?;
            }
            let response = self
                .client
                .get(&url)
                .header(header::ACCEPT, "application/json")
                .header(header::USER_AGENT, &self.user_agent)
                .send()
                .await;

            match response {
                Ok(response) if response.status().is_success() => {
                    let raw: serde_json::Value = response.json().await?;
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
    /// is free, so OpenAlex uses it when its own reference resolution runs out of
    /// budget. The lookup takes its own permit.
    pub async fn fetch_reference_list(
        &self,
        doi: &str,
        permit: &dyn ProviderPermit,
    ) -> Result<Vec<Reference>, CrossrefError> {
        permit.acquire(ProviderKind::Crossref).await?;
        Ok(self.fetch_metered(doi, permit).await?.references)
    }

    /// Fills a missing abstract from Europe PMC / OpenAlex. Never fails.
    async fn enrich_abstract(&self, mut work: WorkWithReferences) -> WorkWithReferences {
        let source = if work.work.abstract_text.is_some() {
            AbstractSource::Crossref
        } else if let Some(enricher) = &self.enricher
            && let Some((text, source)) = enricher.lookup(&work.work.doi).await
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

/// The router's entry point. The caller's permit covers the first request, and
/// retries take their own.
impl WorkProvider for CrossrefClient {
    fn kind(&self) -> ProviderKind {
        ProviderKind::Crossref
    }

    fn fetch_work_with_references<'a>(
        &'a self,
        doi: &'a str,
        permit: &'a dyn ProviderPermit,
    ) -> ProviderFuture<'a, FetchedWork, ProviderFetchError> {
        Box::pin(async move {
            let work = self
                .fetch_metered(doi, permit)
                .await
                .map_err(CrossrefError::into_fetch_error)?;
            Ok(FetchedWork {
                work: self.enrich_abstract(work).await,
                provider: ProviderKind::Crossref,
                reference_gap: None,
            })
        })
    }
}

impl CrossrefError {
    /// Classifies a failed lookup for the provider chain. Status and transport
    /// failures retry, and everything else stops the chain. The worker's job-level
    /// retry splits the same errors the same way.
    fn into_fetch_error(self) -> ProviderFetchError {
        let provider = ProviderKind::Crossref;
        let message = self.to_string();
        match self {
            Self::NotFound(doi) => ProviderFetchError::NotFound { provider, doi },
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
    use deepref_application::PermitError;
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

    #[derive(Default)]
    struct RecordingPermit(std::sync::Mutex<Vec<ProviderKind>>);

    impl RecordingPermit {
        fn taken(&self) -> Vec<ProviderKind> {
            self.0.lock().expect("permit log").clone()
        }
    }

    impl ProviderPermit for RecordingPermit {
        fn acquire<'a>(&'a self, provider: ProviderKind) -> ProviderFuture<'a, (), PermitError> {
            Box::pin(async move {
                self.0.lock().expect("permit log").push(provider);
                Ok(())
            })
        }
    }

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
        assert!(
            permit.taken().is_empty(),
            "the first request runs on the permit the caller took"
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
            vec![ProviderKind::Crossref],
            "the retry takes exactly one permit"
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

    #[test]
    fn each_crossref_failure_maps_to_the_outcome_the_chain_acts_on() {
        let provider = ProviderKind::Crossref;
        let invalid = deepref_domain::normalize_doi("not a doi").expect_err("not a DOI");

        assert_eq!(
            CrossrefError::NotFound(DOI.to_owned()).into_fetch_error(),
            ProviderFetchError::NotFound {
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
