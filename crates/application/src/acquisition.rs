use std::{future::Future, pin::Pin};

use deepref_domain::{
    AcquisitionRunId, IdentifierScheme, ImportFormat, ProviderKind, WorkWithReferences,
};
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

pub type ProviderFuture<'a, T, E> = Pin<Box<dyn Future<Output = Result<T, E>> + Send + 'a>>;

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

/// Takes the outbound permit for one provider before a request is sent. The worker
/// backs it with the shared PostgreSQL limiter, so all pods share one schedule.
pub trait ProviderPermit: Send + Sync {
    fn acquire<'a>(&'a self, provider: ProviderKind) -> ProviderFuture<'a, (), PermitError>;
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("provider permit unavailable: {0}")]
pub struct PermitError(pub String);

#[derive(Debug, Clone)]
pub struct FetchedWork {
    pub work: WorkWithReferences,
    pub provider: ProviderKind,
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ProviderFetchError {
    #[error("{} has no record for {doi}", .provider.as_str())]
    NotFound { provider: ProviderKind, doi: String },
    #[error("{} rejected the request: {reason}", .provider.as_str())]
    Rejected {
        provider: ProviderKind,
        reason: String,
    },
    #[error("{} is unavailable for now: {message}", .provider.as_str())]
    Retryable {
        provider: ProviderKind,
        message: String,
    },
    #[error("{} failed: {message}", .provider.as_str())]
    Fatal {
        provider: ProviderKind,
        message: String,
    },
}

/// One DOI lookup against one metadata service. The caller takes one permit before
/// the call. The adapter takes another before each further request it sends inside
/// the call, such as a retry or a reference lookup.
pub trait WorkProvider: Send + Sync {
    fn kind(&self) -> ProviderKind;
    fn fetch_work_with_references<'a>(
        &'a self,
        doi: &'a str,
        permit: &'a dyn ProviderPermit,
    ) -> ProviderFuture<'a, FetchedWork, ProviderFetchError>;
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
