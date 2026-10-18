use chrono::Utc;
use deepref_domain::{FetchStatus, ProviderKind, Work, WorkWithReferences};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::crossref::parse_openalex;

/// Raw key for the OpenAlex IDs in `referenced_works`. `references` stays empty
/// until the reference resolver turns these IDs into DOIs, so this list is what
/// that resolver reads.
const REFERENCED_WORKS_KEY: &str = "deepref_openalex_referenced_works";
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
    #[serde(default)]
    referenced_works: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct OpenAlexLocation {
    source: Option<OpenAlexSource>,
}

#[derive(Debug, Deserialize)]
struct OpenAlexSource {
    display_name: Option<String>,
    host_organization_name: Option<String>,
}

impl OpenAlexWork {
    /// Maps the work to the domain record. The abstract is rebuilt from the inverted
    /// index through the Crossref cleanup. References stay empty until the resolver
    /// runs, and the raw payload carries the OpenAlex IDs it will resolve.
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
        let oa_urls = json!({
            "best_oa_location": raw.get("best_oa_location").cloned().unwrap_or(Value::Null),
            "locations": raw.get("locations").cloned().unwrap_or_else(|| json!([])),
        });
        if let Some(object) = raw.as_object_mut() {
            object.insert(OA_URLS_KEY.to_owned(), oa_urls);
            object.insert(
                REFERENCED_WORKS_KEY.to_owned(),
                json!(self.referenced_works),
            );
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

/// The year at the start of an ISO date such as `2005-08-02`.
fn year_of(date: &str) -> Option<i32> {
    date.get(..4)?.parse().ok()
}
