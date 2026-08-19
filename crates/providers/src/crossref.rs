use deepref_application::{
    CitationProvider, MetadataProvider, ProviderError, ProviderFuture, RawAuthor, RawIdentifier,
    RawRecord, SearchProvider,
};
use deepref_core::{WorkWithReferences, normalize_doi};
use deepref_crossref::{CrossrefClient, CrossrefError};
use deepref_domain::IdentifierScheme;
use serde_json::{Value, json};

#[derive(Debug, Clone)]
pub struct CrossrefProvider {
    client: CrossrefClient,
}

impl CrossrefProvider {
    pub fn new(mailto: impl Into<String>) -> Result<Self, ProviderError> {
        CrossrefClient::new(mailto.into())
            .map(|client| Self { client })
            .map_err(|error| ProviderError::Request(error.to_string()))
    }

    pub fn with_client(client: CrossrefClient) -> Self {
        Self { client }
    }

    #[must_use]
    pub fn with_max_attempts(mut self, max_attempts: usize) -> Self {
        self.client = self.client.with_max_attempts(max_attempts);
        self
    }

    pub async fn fetch_work_with_references(
        &self,
        doi: &str,
    ) -> Result<WorkWithReferences, CrossrefError> {
        self.client.fetch_work(doi).await
    }
}

impl MetadataProvider for CrossrefProvider {
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
                .client
                .fetch_work(&identifier.normalized_value)
                .await
                .map_err(|error| ProviderError::Request(error.to_string()))?;
            Ok(raw_record_from_crossref_work(work))
        })
    }
}

impl CitationProvider for CrossrefProvider {
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
                .client
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

#[derive(Debug, Clone, Default)]
pub struct StaticSearchProvider {
    records: Vec<RawRecord>,
}

impl StaticSearchProvider {
    pub fn new(records: Vec<RawRecord>) -> Self {
        Self { records }
    }
}

impl SearchProvider for StaticSearchProvider {
    fn name(&self) -> &'static str {
        "static"
    }

    fn search<'a>(&'a self, query: &'a str) -> ProviderFuture<'a, Vec<RawRecord>> {
        Box::pin(async move {
            let query = query.trim().to_lowercase();
            Ok(self
                .records
                .iter()
                .filter(|record| {
                    record
                        .title
                        .as_deref()
                        .is_some_and(|title| title.to_lowercase().contains(&query))
                })
                .cloned()
                .collect())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse_import;
    use chrono::Utc;
    use deepref_core::{FetchStatus, Work, WorkWithReferences};
    use deepref_domain::ImportFormat;

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
