use std::{future::Future, pin::Pin};

use deepref_domain::{AcquisitionRunId, IdentifierScheme, ImportFormat};
use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RawIdentifier {
    pub scheme: IdentifierScheme,
    pub value: String,
    pub normalized_value: String,
}

impl RawIdentifier {
    pub fn new(scheme: IdentifierScheme, value: impl Into<String>) -> Self {
        let value = value.into();
        let normalized_value = value.trim().to_lowercase();
        Self {
            scheme,
            value,
            normalized_value,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RawAuthor {
    pub given: Option<String>,
    pub family: Option<String>,
    pub literal: Option<String>,
}

impl RawAuthor {
    pub fn literal(value: impl Into<String>) -> Self {
        Self {
            given: None,
            family: None,
            literal: Some(value.into()),
        }
    }

    pub fn named(given: Option<String>, family: Option<String>) -> Self {
        Self {
            given,
            family,
            literal: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RawRecord {
    pub source_identifiers: Vec<RawIdentifier>,
    pub title: Option<String>,
    pub abstract_text: Option<String>,
    pub authors: Vec<RawAuthor>,
    pub publication_year: Option<i32>,
    pub journal: Option<String>,
    pub raw: serde_json::Value,
}

impl RawRecord {
    pub fn empty(raw: serde_json::Value) -> Self {
        Self {
            source_identifiers: Vec::new(),
            title: None,
            abstract_text: None,
            authors: Vec::new(),
            publication_year: None,
            journal: None,
            raw,
        }
    }
}

pub type ProviderFuture<'a, T> =
    Pin<Box<dyn Future<Output = Result<T, ProviderError>> + Send + 'a>>;

#[derive(Debug, Error)]
pub enum ProviderError {
    #[error("provider request failed: {0}")]
    Request(String),
    #[error("provider returned invalid data: {0}")]
    InvalidData(String),
    #[error("provider does not support identifier {0}")]
    UnsupportedIdentifier(String),
}

#[derive(Debug, Error)]
pub enum ImportError {
    #[error("invalid {format} import: {message}")]
    Invalid { format: String, message: String },
    #[error("CSV imports require an explicit column mapping")]
    MissingCsvMapping,
    #[error("CSV column is not present: {0}")]
    MissingCsvColumn(String),
}

pub trait MetadataProvider: Send + Sync {
    fn name(&self) -> &'static str;
    fn fetch_metadata<'a>(&'a self, identifier: &'a RawIdentifier)
    -> ProviderFuture<'a, RawRecord>;
}

pub trait CitationProvider: Send + Sync {
    fn name(&self) -> &'static str;
    fn fetch_citations<'a>(&'a self, record: &'a RawRecord) -> ProviderFuture<'a, Vec<RawRecord>>;
}

pub trait SearchProvider: Send + Sync {
    fn name(&self) -> &'static str;
    fn search<'a>(&'a self, query: &'a str) -> ProviderFuture<'a, Vec<RawRecord>>;
}

pub trait FullTextResolver: Send + Sync {
    fn name(&self) -> &'static str;
    fn resolve<'a>(&'a self, record: &'a RawRecord) -> ProviderFuture<'a, Option<String>>;
}

pub trait ImportParser: Send + Sync {
    fn format(&self) -> ImportFormat;
    fn parse(&self, bytes: &[u8]) -> Result<Vec<RawRecord>, ImportError>;
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CsvColumnMapping {
    pub doi: Option<String>,
    pub pmid: Option<String>,
    pub pmcid: Option<String>,
    pub title: Option<String>,
    pub abstract_text: Option<String>,
    pub authors: Option<String>,
    pub publication_year: Option<String>,
    pub journal: Option<String>,
}

impl CsvColumnMapping {
    pub fn is_empty(&self) -> bool {
        [
            &self.doi,
            &self.pmid,
            &self.pmcid,
            &self.title,
            &self.abstract_text,
            &self.authors,
            &self.publication_year,
            &self.journal,
        ]
        .into_iter()
        .all(Option::is_none)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AcquisitionRunSummary {
    pub id: AcquisitionRunId,
    pub project_id: uuid::Uuid,
    pub source: String,
    pub strategy: String,
    pub format: Option<ImportFormat>,
}
