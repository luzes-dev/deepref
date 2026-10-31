//! Clients for the external bibliographic services DeepRef talks to.
//!
//! DOI metadata runs through a provider chain that `deepref-application` routes.
//! `openalex` answers first, with free singleton lookups and reference lists
//! from list calls that share a daily budget. Crossref answers the DOIs OpenAlex
//! cannot, and its reference list is unioned into every OpenAlex answer, which
//! also covers the edges lost when the budget runs out. DataCite answers the DOIs neither
//! holds, mostly datasets, software and repository records. Crossref also serves
//! the publication alerts and the abstract enrichment, which tries Europe PMC and
//! then OpenAlex. PubMed, Unpaywall and GROBID keep their own clients, and the
//! parsers for imported reference files live here too.

mod crossref;
mod datacite;
mod grobid;
mod imports;
mod openalex;
mod pubmed;
#[cfg(test)]
mod test_permit;
mod unpaywall;
pub mod valkey;
#[cfg(feature = "valkey-test-support")]
pub mod valkey_fixture;
mod workos;

pub use crossref::{
    AbstractEnricher, AbstractSource, CROSSREF_BASE, CrossrefClient, CrossrefError,
    CrossrefSearchError, EUROPE_PMC_RATE_PER_SECOND, EnricherBuildError, OpenAlexLookup,
    StatusClassification, classify_status, clean_abstract, raw_record_from_crossref_work,
    search_crossref,
};
pub use datacite::{
    DATACITE_BASE, DATACITE_RATE_PER_SECOND, DataCiteClient, DataCiteError,
    raw_record_from_datacite_work,
};
pub use grobid::{
    GrobidClient, GrobidConfig, GrobidError, TeiDocument, TeiError, TeiReference, TeiSection,
    parse_tei,
};
pub use imports::{ImportParserAdapter, parse_import};
pub use openalex::{
    KEYED_RATE_PER_SECOND, KEYLESS_RATE_PER_SECOND, OpenAlexClient, OpenAlexError, OpenAlexKey,
    OpenAlexKeyState, pdf_urls_from_oa_urls, raw_record_from_openalex_work,
};
pub use pubmed::{
    MAX_IDS_PER_FETCH, PubmedArticle, PubmedClient, PubmedError, REQUESTS_PER_SECOND,
    normalize_pmid, parse_efetch,
};
pub use unpaywall::{UnpaywallClient, UnpaywallError, pdf_url_from_response};
pub use workos::WorkOsIdentityProvider;

pub mod dodo;
pub mod jetstream;

mod valkey_client;
pub mod valkey_concurrency;

pub mod valkey_project_counts;
