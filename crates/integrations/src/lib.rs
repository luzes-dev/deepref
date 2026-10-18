//! Clients for the external bibliographic services DeepRef talks to: Crossref
//! and OpenAlex for work metadata (with Europe PMC / OpenAlex abstract
//! enrichment), PubMed, Unpaywall and GROBID, plus the parsers for imported
//! reference files.

mod crossref;
mod grobid;
mod imports;
mod openalex;
mod pubmed;
mod unpaywall;

pub use crossref::{
    AbstractEnricher, AbstractSource, CrossrefClient, CrossrefError, CrossrefSearchError,
    StatusClassification, classify_status, clean_abstract, raw_record_from_crossref_work,
    search_crossref,
};
pub use grobid::{
    GrobidClient, GrobidConfig, GrobidError, TeiDocument, TeiError, TeiReference, TeiSection,
    parse_tei,
};
pub use imports::{ImportParserAdapter, parse_import};
pub use openalex::{
    KEYED_RATE_PER_SECOND, KEYLESS_RATE_PER_SECOND, OpenAlexClient, OpenAlexError, OpenAlexKey,
    OpenAlexKeyState, raw_record_from_openalex_work,
};
pub use pubmed::{
    MAX_IDS_PER_FETCH, PubmedArticle, PubmedClient, PubmedError, REQUESTS_PER_SECOND,
    normalize_pmid, parse_efetch,
};
pub use unpaywall::{UnpaywallClient, UnpaywallError, pdf_url_from_response};
