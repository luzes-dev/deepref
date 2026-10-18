use std::{future::Future, pin::Pin};

use deepref_domain::{
    AcquisitionRunId, IdentifierScheme, ImportFormat, ProviderKind, Reference, WorkWithReferences,
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

/// Why an outbound request is sent. Permits and attempt records carry it, so the history
/// of a fetch says what each request was for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AttemptPurpose {
    /// The DOI's metadata record.
    Metadata,
    /// Resolving the referenced works the metadata named, such as OpenAlex's list calls.
    ReferenceResolution,
    /// Reading Crossref's reference list to union with another provider's references.
    ReferenceUnion,
    /// The best-effort abstract lookup after a Crossref answer.
    AbstractEnrichment,
}

impl AttemptPurpose {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Metadata => "metadata",
            Self::ReferenceResolution => "reference_resolution",
            Self::ReferenceUnion => "reference_union",
            Self::AbstractEnrichment => "abstract_enrichment",
        }
    }
}

/// The service an outbound request goes to. It names the rate limiter, which is not the
/// same set as the metadata providers: Europe PMC answers abstract lookups but is never
/// a link in the metadata chain.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum OutboundService {
    Provider(ProviderKind),
    EuropePmc,
}

impl OutboundService {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Provider(provider) => provider.as_str(),
            Self::EuropePmc => "europepmc",
        }
    }

    /// The metadata provider behind the service, when there is one.
    pub const fn provider(self) -> Option<ProviderKind> {
        match self {
            Self::Provider(provider) => Some(provider),
            Self::EuropePmc => None,
        }
    }
}

impl From<ProviderKind> for OutboundService {
    fn from(provider: ProviderKind) -> Self {
        Self::Provider(provider)
    }
}

impl PartialEq<ProviderKind> for OutboundService {
    fn eq(&self, other: &ProviderKind) -> bool {
        self.provider() == Some(*other)
    }
}

impl Serialize for OutboundService {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
    }
}

/// One outbound HTTP request, named before it is sent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OutboundRequest {
    pub service: OutboundService,
    pub purpose: AttemptPurpose,
}

impl OutboundRequest {
    pub fn new(provider: ProviderKind, purpose: AttemptPurpose) -> Self {
        Self {
            service: OutboundService::Provider(provider),
            purpose,
        }
    }

    pub fn europe_pmc() -> Self {
        Self {
            service: OutboundService::EuropePmc,
            purpose: AttemptPurpose::AbstractEnrichment,
        }
    }
}

/// Takes the outbound permit for one request, immediately before it is sent, including
/// each retry and each redirect hop. The worker backs it with the shared PostgreSQL
/// limiter, so all pods share one schedule.
pub trait ProviderPermit: Send + Sync {
    fn acquire<'a>(&'a self, request: OutboundRequest) -> ProviderFuture<'a, (), PermitError>;
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("provider permit unavailable: {0}")]
pub struct PermitError(pub String);

/// What a provider's reference resolution left unresolved. Providers that return the
/// reference list with the metadata, such as Crossref, report nothing and use the default.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ReferenceReport {
    /// Referenced works the provider named and tried to resolve to DOIs.
    pub targets: usize,
    /// Targets that resolved to a DOI and became references.
    pub retained: usize,
    /// Targets a successful response left out.
    pub omitted: usize,
    /// Targets the provider returned without a usable DOI.
    pub doi_less: usize,
    /// Targets never asked about because the daily budget stopped the calls.
    pub unattempted: usize,
    /// Set when the daily budget stopped paid calls. `reset_secs` is the reset the
    /// provider sent, if any.
    pub budget_stop: Option<BudgetStop>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BudgetStop {
    pub reset_secs: Option<u64>,
}

impl ReferenceReport {
    /// Whether the report shows a loss that is not a genuinely empty bibliography.
    pub fn is_degraded(&self) -> bool {
        self.omitted > 0 || self.doi_less > 0 || self.unattempted > 0 || self.budget_stop.is_some()
    }
}

#[derive(Debug, Clone)]
pub struct FetchedWork {
    pub work: WorkWithReferences,
    pub provider: ProviderKind,
    /// What reference resolution lost. The default reports nothing lost.
    pub references: ReferenceReport,
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ProviderFetchError {
    #[error("{} has no record for {doi}", .provider.as_str())]
    NotFound {
        provider: ProviderKind,
        doi: String,
        /// Why the provider holds no usable record, when it is more than a plain miss, such as
        /// `corpus_excluded`.
        reason: Option<String>,
    },
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
    /// A failure that retrying cannot fix. The chain records it and moves on.
    #[error("{} failed: {message}", .provider.as_str())]
    Fatal {
        provider: ProviderKind,
        message: String,
    },
    /// The metadata answered and a later reference request ran out of retries. The chain
    /// does not move on, because another provider's metadata would hide the failure.
    #[error("{} is unavailable for now while resolving references: {message}", .provider.as_str())]
    ReferenceRetryable {
        provider: ProviderKind,
        message: String,
    },
    /// The metadata answered and a reference request failed in a way retrying cannot fix,
    /// such as a response that breaks the list contract.
    #[error("{} failed while resolving references: {message}", .provider.as_str())]
    ReferenceFatal {
        provider: ProviderKind,
        message: String,
    },
}

/// One DOI lookup against one metadata service. The adapter takes a permit immediately
/// before every request it sends, the first one included, and names the request's purpose.
pub trait WorkProvider: Send + Sync {
    fn kind(&self) -> ProviderKind;
    fn fetch_work_with_references<'a>(
        &'a self,
        doi: &'a str,
        permit: &'a dyn ProviderPermit,
    ) -> ProviderFuture<'a, FetchedWork, ProviderFetchError>;

    /// The lookup for a DOI that the providers in `tried` already asked this run. An
    /// adapter overrides it to skip work those providers already did.
    fn fetch_work_after<'a>(
        &'a self,
        doi: &'a str,
        _tried: &'a [ProviderKind],
        permit: &'a dyn ProviderPermit,
    ) -> ProviderFuture<'a, FetchedWork, ProviderFetchError> {
        self.fetch_work_with_references(doi, permit)
    }

    /// The DOI's raw reference list, unresolved entries included, for the union with
    /// another provider's references. Only providers that publish reference lists implement it.
    fn reference_list<'a>(
        &'a self,
        _doi: &'a str,
        _permit: &'a dyn ProviderPermit,
    ) -> ProviderFuture<'a, Vec<Reference>, ProviderFetchError> {
        let provider = self.kind();
        Box::pin(async move {
            Err(ProviderFetchError::Fatal {
                provider,
                message: "has no reference list to read".to_owned(),
            })
        })
    }

    /// Whether an accepted answer from this provider is unioned with Crossref's reference
    /// list. The bibliography this provider resolves can omit references it never matched,
    /// and nothing in its answer shows which.
    fn unions_crossref_references(&self) -> bool {
        false
    }
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
