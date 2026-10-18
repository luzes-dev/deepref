use std::collections::HashMap;

use chrono::Utc;
use deepref_domain::{FetchStatus, ProviderKind, Work, WorkWithReferences, normalize_doi};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::crossref::parse_openalex;

/// Raw key for the open-access locations (`best_oa_location` and `locations`),
/// kept for PDF and licence lookups.
const OA_URLS_KEY: &str = "deepref_oa_urls";

/// The OpenAlex fields the adapter maps. The request's `select=` returns these plus
/// the fields kept only in the raw payload.
#[derive(Debug, Deserialize)]
pub(crate) struct OpenAlexWork {
    pub(crate) doi: Option<String>,
    display_name: Option<String>,
    publication_year: Option<i32>,
    publication_date: Option<String>,
    #[serde(rename = "type")]
    work_type: Option<String>,
    primary_location: Option<OpenAlexLocation>,
    #[serde(default)]
    cited_by_count: i32,
    #[serde(default)]
    referenced_works_count: i32,
    best_oa_location: Option<OpenAlexLocation>,
    #[serde(default)]
    locations: Vec<OpenAlexLocation>,
    /// Front matter such as editorials and indexes. It ends the lookup as rejected.
    #[serde(default)]
    pub(crate) is_paratext: bool,
}

/// One OpenAlex location (`primary_location`, `best_oa_location` or an entry of
/// `locations`). Only the fields the mapping reads are declared.
#[derive(Debug, Deserialize)]
struct OpenAlexLocation {
    source: Option<OpenAlexSource>,
    landing_page_url: Option<String>,
    pdf_url: Option<String>,
    license: Option<String>,
    version: Option<String>,
    #[serde(default)]
    is_oa: bool,
}

/// The open-access locations stored under `raw.deepref_oa_urls`, built by
/// `OpenAlexWork::into_domain`. The open-access path in `documents.rs` reads this from
/// `works.raw` ahead of Unpaywall, so the shape is a contract:
///
/// ```json
/// { "best_oa_location": OaLocation | null, "locations": [OaLocation] }
/// ```
///
/// Each `OaLocation` is `{ "url", "pdf_url", "license", "version", "is_oa", "source" }`.
/// `url` is OpenAlex's `landing_page_url`, and `source` is the host's display name. A field
/// OpenAlex omits is `null`, and `is_oa` is `false` when it is omitted.
#[derive(Debug, Serialize)]
pub(crate) struct OaUrls {
    best_oa_location: Option<OaLocation>,
    locations: Vec<OaLocation>,
}

/// One open-access location in `deepref_oa_urls`. See `OaUrls` for the shape.
#[derive(Debug, Serialize)]
pub(crate) struct OaLocation {
    url: Option<String>,
    pdf_url: Option<String>,
    license: Option<String>,
    version: Option<String>,
    is_oa: bool,
    source: Option<String>,
}

impl From<OpenAlexLocation> for OaLocation {
    fn from(location: OpenAlexLocation) -> Self {
        Self {
            url: location.landing_page_url,
            pdf_url: location.pdf_url,
            license: location.license,
            version: location.version,
            is_oa: location.is_oa,
            source: location.source.and_then(|source| source.display_name),
        }
    }
}

#[derive(Debug, Deserialize)]
struct OpenAlexSource {
    display_name: Option<String>,
    host_organization_name: Option<String>,
}

impl OpenAlexWork {
    /// Maps the work to the domain record. The abstract is rebuilt from the inverted
    /// index through the Crossref cleanup. `references` stays empty here; the adapter
    /// fills it from the reference list calls.
    pub(crate) fn into_domain(self, doi: String, mut raw: Value) -> WorkWithReferences {
        let abstract_text = parse_openalex(&raw);
        let year = self
            .publication_year
            .or_else(|| self.publication_date.as_deref().and_then(year_of));
        let source = self.primary_location.and_then(|location| location.source);
        let publisher = source
            .as_ref()
            .and_then(|source| source.host_organization_name.clone());
        let container_title = source.and_then(|source| source.display_name);
        let oa_urls = OaUrls {
            best_oa_location: self.best_oa_location.map(OaLocation::from),
            locations: self.locations.into_iter().map(OaLocation::from).collect(),
        };
        if let Some(object) = raw.as_object_mut() {
            object.insert(OA_URLS_KEY.to_owned(), json!(oa_urls));
        }

        WorkWithReferences {
            work: Work {
                doi,
                title: self.display_name,
                abstract_text,
                work_type: self.work_type,
                publisher,
                container_title,
                issued_year: year,
                published_year: year,
                url: None,
                total_citations: self.cited_by_count,
                references_count: self.referenced_works_count,
                metadata_provider: ProviderKind::OpenAlex.as_str().to_owned(),
                citation_provider: ProviderKind::OpenAlex.as_str().to_owned(),
                fetched_at: Utc::now(),
                fetch_status: FetchStatus::Fetched.as_str().to_owned(),
            },
            references: Vec::new(),
            raw,
        }
    }
}

/// The short OpenAlex ID (`W` and digits) of an entity URL or bare ID, or `None` for
/// anything else. The IDs go into list filters, so only this shape passes.
pub(crate) fn openalex_id(value: &str) -> Option<String> {
    let short = value.strip_prefix("https://openalex.org/").unwrap_or(value);
    let digits = short.strip_prefix('W')?;
    let well_formed = !digits.is_empty() && digits.bytes().all(|byte| byte.is_ascii_digit());
    well_formed.then(|| short.to_owned())
}

/// The OpenAlex IDs a work lists in `referenced_works`, in order, malformed entries
/// dropped.
pub(crate) fn referenced_ids(raw: &Value) -> Vec<String> {
    raw.get("referenced_works")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .filter_map(openalex_id)
        .collect()
}

/// The DOI of each OpenAlex ID in a list response. A result without a DOI is left
/// out, so a target without one never becomes a reference.
pub(crate) fn list_dois(body: &Value) -> HashMap<String, String> {
    body.get("results")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|result| {
            let id = openalex_id(result.get("id")?.as_str()?)?;
            let doi = normalize_doi(result.get("doi")?.as_str()?).ok()?;
            Some((id, doi))
        })
        .collect()
}

/// The year at the start of an ISO date such as `2005-08-02`.
fn year_of(date: &str) -> Option<i32> {
    date.get(..4)?.parse().ok()
}
