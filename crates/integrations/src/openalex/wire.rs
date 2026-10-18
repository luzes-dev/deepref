use std::collections::HashMap;

use chrono::Utc;
use deepref_domain::{FetchStatus, ProviderKind, Work, WorkWithReferences, normalize_doi};
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::{Value, json};

use crate::crossref::parse_openalex;

/// Raw key for the open-access locations (`best_oa_location` and `locations`),
/// kept for PDF and licence lookups.
const OA_URLS_KEY: &str = "deepref_oa_urls";

/// Reads a JSON `null` as the field's default. `#[serde(default)]` covers only an absent
/// field, and OpenAlex sends `null` for some counts, flags and lists.
fn null_as_default<'de, D, T>(deserializer: D) -> Result<T, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de> + Default,
{
    Ok(Option::<T>::deserialize(deserializer)?.unwrap_or_default())
}

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
    #[serde(default, deserialize_with = "null_as_default")]
    cited_by_count: i32,
    #[serde(default, deserialize_with = "null_as_default")]
    referenced_works_count: i32,
    best_oa_location: Option<OpenAlexLocation>,
    #[serde(default, deserialize_with = "null_as_default")]
    locations: Vec<OpenAlexLocation>,
    /// Front matter such as editorials and indexes. It ends the lookup as rejected.
    #[serde(default, deserialize_with = "null_as_default")]
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
    #[serde(default, deserialize_with = "null_as_default")]
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
/// OpenAlex omits is `null`, and `is_oa` is `false` when it is omitted. The same type reads the
/// stored value back in `pdf_urls_from_oa_urls`.
#[derive(Debug, Default, Deserialize, Serialize)]
#[serde(default)]
pub(crate) struct OaUrls {
    best_oa_location: Option<OaLocation>,
    locations: Vec<OaLocation>,
}

/// One open-access location in `deepref_oa_urls`. See `OaUrls` for the shape.
#[derive(Debug, Default, Deserialize, Serialize)]
#[serde(default)]
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

/// The PDF URLs to try before Unpaywall, read from a stored `deepref_oa_urls` value, in
/// preference order: the best location's PDF, then each listed location's PDF. Landing pages
/// are never returned, and a missing or malformed value gives no candidates. The URLs are not
/// validated here; the caller takes the first one that passes.
pub fn pdf_urls_from_oa_urls(stored: &Value) -> Vec<String> {
    // A JSON array would otherwise fill the struct's fields by position.
    if !stored.is_object() {
        return Vec::new();
    }
    let Ok(oa_urls) = OaUrls::deserialize(stored) else {
        return Vec::new();
    };
    oa_urls
        .best_oa_location
        .into_iter()
        .chain(oa_urls.locations)
        .filter_map(|location| location.pdf_url.filter(|url| !url.trim().is_empty()))
        .collect()
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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// A real OpenAlex answer whose open-access locations are all landing pages.
    const WORK: &str = include_str!("../../tests/fixtures/openalex/work_with_abstract.json");

    #[test]
    fn prefers_the_best_location_pdf() {
        let stored = json!({
            "best_oa_location": {
                "url": "https://repo.example.org/best",
                "pdf_url": "https://repo.example.org/best.pdf"
            },
            "locations": [
                { "url": "https://other.example.org/copy", "pdf_url": "https://other.example.org/copy.pdf" }
            ]
        });
        assert_eq!(
            pdf_urls_from_oa_urls(&stored),
            [
                "https://repo.example.org/best.pdf",
                "https://other.example.org/copy.pdf",
            ]
        );
    }

    #[test]
    fn takes_the_first_listed_pdf_when_the_best_location_has_none() {
        let stored = json!({
            "best_oa_location": { "url": "https://landing.example.org/a", "pdf_url": null },
            "locations": [
                { "url": "https://landing.example.org/b", "pdf_url": null },
                { "url": "https://other.example.org/c", "pdf_url": "https://other.example.org/first.pdf" },
                { "url": "https://repo.example.org/d", "pdf_url": "https://repo.example.org/second.pdf" }
            ]
        });
        assert_eq!(
            pdf_urls_from_oa_urls(&stored),
            [
                "https://other.example.org/first.pdf",
                "https://repo.example.org/second.pdf",
            ]
        );
    }

    #[test]
    fn never_returns_a_landing_page() {
        let stored = json!({
            "best_oa_location": { "url": "https://landing.example.org/a", "pdf_url": null },
            "locations": [{ "url": "https://landing.example.org/b", "pdf_url": null }]
        });
        assert!(pdf_urls_from_oa_urls(&stored).is_empty());
    }

    #[test]
    fn ignores_a_blank_pdf_url() {
        let stored = json!({
            "best_oa_location": { "pdf_url": "  " },
            "locations": [{ "pdf_url": "https://repo.example.org/only.pdf" }]
        });
        assert_eq!(
            pdf_urls_from_oa_urls(&stored),
            ["https://repo.example.org/only.pdf"]
        );
    }

    #[test]
    fn a_missing_value_gives_no_candidate() {
        assert!(pdf_urls_from_oa_urls(&Value::Null).is_empty());
        assert!(pdf_urls_from_oa_urls(&json!({})).is_empty());
        assert!(
            pdf_urls_from_oa_urls(&json!({ "best_oa_location": null, "locations": [] })).is_empty()
        );
    }

    #[test]
    fn a_malformed_value_gives_no_candidate() {
        for stored in [
            json!("https://repo.example.org/a.pdf"),
            json!([{ "pdf_url": "https://repo.example.org/a.pdf" }]),
            json!({ "locations": "https://repo.example.org/a.pdf" }),
            json!({ "best_oa_location": 5, "locations": [] }),
            json!({ "locations": [{ "pdf_url": 7 }] }),
            json!({
                "best_oa_location": { "pdf_url": 7 },
                "locations": [{ "pdf_url": "https://repo.example.org/a.pdf" }]
            }),
        ] {
            assert!(pdf_urls_from_oa_urls(&stored).is_empty(), "{stored}");
        }
    }

    #[test]
    fn reads_back_the_pdf_the_adapter_stored() {
        let body = json!({
            "display_name": "A paper",
            "best_oa_location": { "landing_page_url": "https://landing.example.org/a", "is_oa": true },
            "locations": [{
                "landing_page_url": "https://repo.example.org/b",
                "pdf_url": "https://repo.example.org/b.pdf",
                "is_oa": true
            }]
        });
        let parsed: OpenAlexWork = serde_json::from_value(body.clone()).unwrap();
        let work = parsed.into_domain("10.1000/stored".to_owned(), body);
        assert_eq!(
            pdf_urls_from_oa_urls(&work.raw["deepref_oa_urls"]),
            ["https://repo.example.org/b.pdf"]
        );
    }

    #[test]
    fn the_landing_page_only_fixture_has_no_candidate() {
        let body: Value = serde_json::from_str(WORK).unwrap();
        let parsed: OpenAlexWork = serde_json::from_value(body.clone()).unwrap();
        let work = parsed.into_domain("10.1371/journal.pmed.0020124".to_owned(), body);
        assert!(pdf_urls_from_oa_urls(&work.raw["deepref_oa_urls"]).is_empty());
    }
}
