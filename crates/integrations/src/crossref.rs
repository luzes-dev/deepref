//! Crossref: work metadata and reference lists by DOI, with best-effort abstract
//! enrichment from Europe PMC / OpenAlex when Crossref has no abstract.

mod abstracts;
mod error;
mod search;
mod status;
mod wire;

use std::time::Duration;

use deepref_application::{
    CitationProvider, MetadataProvider, ProviderError, ProviderFuture, RawAuthor, RawIdentifier,
    RawRecord,
};
use deepref_domain::WorkWithReferences;
use deepref_domain::{IdentifierScheme, normalize_doi};
use reqwest::header;
use serde_json::{Value, json};

pub use abstracts::{AbstractEnricher, AbstractSource, clean_abstract};
pub use error::CrossrefError;
pub use search::{CrossrefSearchError, search_crossref};
pub use status::{StatusClassification, classify_status};

use wire::CrossrefWorksResponse;

const CROSSREF_BASE: &str = "https://api.crossref.org";

/// The one Crossref client. `fetch_work` returns what Crossref holds;
/// `fetch_work_with_references` also fills a missing abstract from the enricher.
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
    /// Builds the client for `api.crossref.org` with the configured abstract enrichment.
    pub fn new(
        mailto: impl Into<String>,
        config: &deepref_config::CrossrefConfig,
    ) -> Result<Self, ProviderError> {
        let mailto = mailto.into();
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
            return Err(CrossrefError::MissingMailto);
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

    pub async fn fetch_work(&self, doi: &str) -> Result<WorkWithReferences, CrossrefError> {
        let doi = deepref_domain::normalize_doi(doi)?;
        let encoded = urlencoding::encode(&doi);
        let url = format!("{}/works/{}?mailto={}", self.base_url, encoded, self.mailto);

        let mut last_retryable = None;
        for attempt in 1..=self.max_attempts {
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

    pub async fn fetch_work_with_references(
        &self,
        doi: &str,
    ) -> Result<WorkWithReferences, CrossrefError> {
        let work = self.fetch_work(doi).await?;
        Ok(self.enrich_abstract(work).await)
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

async fn sleep_backoff(attempt: usize) {
    let millis = 250_u64.saturating_mul(2_u64.saturating_pow(attempt as u32 - 1));
    tokio::time::sleep(Duration::from_millis(millis.min(5_000))).await;
}

impl MetadataProvider for CrossrefClient {
    fn name(&self) -> &'static str {
        "crossref"
    }

    fn fetch_metadata<'a>(
        &'a self,
        identifier: &'a RawIdentifier,
    ) -> ProviderFuture<'a, RawRecord> {
        Box::pin(async move {
            if identifier.scheme != IdentifierScheme::Doi {
                return Err(ProviderError::UnsupportedIdentifier(
                    identifier.value.clone(),
                ));
            }
            let work = self
                .fetch_work_with_references(&identifier.normalized_value)
                .await
                .map_err(|error| ProviderError::Request(error.to_string()))?;
            Ok(raw_record_from_crossref_work(work))
        })
    }
}

impl CitationProvider for CrossrefClient {
    fn name(&self) -> &'static str {
        "crossref"
    }

    fn fetch_citations<'a>(&'a self, record: &'a RawRecord) -> ProviderFuture<'a, Vec<RawRecord>> {
        Box::pin(async move {
            let doi = record
                .source_identifiers
                .iter()
                .find(|identifier| identifier.scheme == IdentifierScheme::Doi)
                .ok_or_else(|| {
                    ProviderError::UnsupportedIdentifier("record has no DOI".to_owned())
                })?;
            let work = self
                .fetch_work(&doi.normalized_value)
                .await
                .map_err(|error| ProviderError::Request(error.to_string()))?;
            Ok(work
                .references
                .into_iter()
                .filter_map(|reference| reference.doi)
                .filter_map(|target| citation_record(doi, target))
                .collect())
        })
    }
}

fn citation_record(cited_by: &RawIdentifier, target: String) -> Option<RawRecord> {
    let normalized = normalize_doi(&target).ok()?;
    Some(RawRecord {
        source_identifiers: vec![RawIdentifier {
            scheme: IdentifierScheme::Doi,
            value: target.clone(),
            normalized_value: normalized,
        }],
        title: None,
        abstract_text: None,
        authors: Vec::new(),
        publication_year: None,
        journal: None,
        raw: json!({ "source": "crossref", "cited_by": cited_by.normalized_value, "doi": target }),
    })
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
    use deepref_domain::ImportFormat;
    use deepref_domain::{FetchStatus, Work, WorkWithReferences};

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

    #[test]
    fn citation_targets_use_domain_doi_normalization() {
        let source = RawIdentifier {
            scheme: IdentifierScheme::Doi,
            value: "10.1000/source".to_owned(),
            normalized_value: "10.1000/source".to_owned(),
        };
        let record = citation_record(&source, "https://doi.org/10.1000/TARGET.".to_owned())
            .expect("valid DOI reference should be emitted");
        assert_eq!(
            record.source_identifiers[0].normalized_value,
            "10.1000/target"
        );
        assert!(citation_record(&source, "not-a-doi".to_owned()).is_none());
    }
}
