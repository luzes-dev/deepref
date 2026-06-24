use chrono::Utc;
use deepref_core::{Reference, Work, WorkWithReferences, normalize_doi};
use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub(crate) struct CrossrefWorksResponse {
    message: CrossrefWorkMessage,
}

#[derive(Debug, Deserialize)]
struct CrossrefWorkMessage {
    #[serde(default)]
    title: Vec<String>,
    #[serde(rename = "abstract")]
    abstract_text: Option<String>,
    #[serde(rename = "type")]
    work_type: Option<String>,
    publisher: Option<String>,
    #[serde(rename = "container-title", default)]
    container_title: Vec<String>,
    #[serde(rename = "is-referenced-by-count", default)]
    is_referenced_by_count: i32,
    #[serde(rename = "references-count", default)]
    references_count: i32,
    url: Option<String>,
    issued: Option<CrossrefDateParts>,
    published: Option<CrossrefDateParts>,
    #[serde(default)]
    reference: Vec<CrossrefReference>,
}

#[derive(Debug, Deserialize)]
struct CrossrefDateParts {
    #[serde(rename = "date-parts")]
    date_parts: Vec<Vec<i32>>,
}

impl CrossrefDateParts {
    fn year(&self) -> Option<i32> {
        self.date_parts
            .first()
            .and_then(|parts| parts.first())
            .copied()
    }
}

#[derive(Debug, Deserialize)]
struct CrossrefReference {
    #[serde(rename = "DOI")]
    doi_upper: Option<String>,
    doi: Option<String>,
    key: Option<String>,
    #[serde(rename = "unstructured")]
    raw_unstructured: Option<String>,
    #[serde(rename = "article-title")]
    article_title: Option<String>,
    author: Option<String>,
    year: Option<String>,
    volume: Option<String>,
    #[serde(rename = "first-page")]
    first_page: Option<String>,
}

impl CrossrefWorksResponse {
    pub(crate) fn into_domain(self, doi: String, raw: serde_json::Value) -> WorkWithReferences {
        let message = self.message;
        let references = message
            .reference
            .into_iter()
            .map(|reference| Reference {
                doi: reference
                    .doi
                    .or(reference.doi_upper)
                    .and_then(|doi| normalize_doi(&doi).ok()),
                key: reference.key,
                raw_unstructured: reference.raw_unstructured,
                article_title: reference.article_title,
                author: reference.author,
                year: reference.year,
                volume: reference.volume,
                first_page: reference.first_page,
            })
            .collect();

        WorkWithReferences {
            work: Work {
                doi,
                title: message.title.into_iter().next(),
                abstract_text: message.abstract_text,
                work_type: message.work_type,
                publisher: message.publisher,
                container_title: message.container_title.into_iter().next(),
                issued_year: message.issued.as_ref().and_then(CrossrefDateParts::year),
                published_year: message.published.as_ref().and_then(CrossrefDateParts::year),
                url: message.url,
                total_citations: message.is_referenced_by_count,
                references_count: message.references_count,
                metadata_provider: "crossref".to_owned(),
                citation_provider: "crossref".to_owned(),
                fetched_at: Utc::now(),
                fetch_status: deepref_core::FetchStatus::Fetched.as_str().to_owned(),
            },
            references,
            raw,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn converts_crossref_message_to_domain_work() {
        let raw = serde_json::json!({
            "title": ["A Paper"],
            "type": "journal-article",
            "issued": { "date-parts": [[2024, 5, 1]] },
            "reference": [{ "DOI": "10.1000/ABC", "article-title": "Ref" }]
        });
        let response: CrossrefWorksResponse =
            serde_json::from_value(serde_json::json!({ "message": raw.clone() })).unwrap();

        let work = response.into_domain("10.1/main".to_owned(), raw.clone());

        assert_eq!(work.work.title.as_deref(), Some("A Paper"));
        assert_eq!(work.work.issued_year, Some(2024));
        assert_eq!(work.references[0].doi.as_deref(), Some("10.1000/abc"));
        assert_eq!(work.raw, raw);
    }
}
