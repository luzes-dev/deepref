use deepref_core::WorkWithReferences;

use crate::unresolved_reference_id;

#[derive(Debug, Clone)]
pub struct GraphUpsert {
    pub work_doi: String,
    pub cited_dois: Vec<String>,
    pub unresolved_reference_ids: Vec<String>,
}

pub fn build_graph_upsert(work: &WorkWithReferences) -> GraphUpsert {
    let cited_dois = work
        .references
        .iter()
        .filter_map(|reference| reference.doi.clone())
        .collect();
    let unresolved_reference_ids = work
        .references
        .iter()
        .filter(|reference| reference.doi.is_none())
        .map(|reference| unresolved_reference_id(&work.work.doi, reference))
        .collect();

    GraphUpsert {
        work_doi: work.work.doi.clone(),
        cited_dois,
        unresolved_reference_ids,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use deepref_core::{Reference, Work};

    #[test]
    fn separates_cited_and_unresolved_references() {
        let work = WorkWithReferences {
            work: Work {
                doi: "10.1/a".to_owned(),
                title: None,
                abstract_text: None,
                work_type: None,
                publisher: None,
                container_title: None,
                issued_year: None,
                published_year: None,
                url: None,
                total_citations: 0,
                references_count: 0,
                metadata_provider: "crossref".to_owned(),
                citation_provider: "crossref".to_owned(),
                fetched_at: Utc::now(),
                fetch_status: "fetched".to_owned(),
            },
            references: vec![
                Reference {
                    doi: Some("10.1/b".to_owned()),
                    key: None,
                    raw_unstructured: None,
                    article_title: None,
                    author: None,
                    year: None,
                    volume: None,
                    first_page: None,
                },
                Reference {
                    doi: None,
                    key: None,
                    raw_unstructured: Some("Unknown".to_owned()),
                    article_title: None,
                    author: None,
                    year: None,
                    volume: None,
                    first_page: None,
                },
            ],
            raw: serde_json::json!({}),
        };
        let upsert = build_graph_upsert(&work);
        assert_eq!(upsert.cited_dois, vec!["10.1/b"]);
        assert_eq!(upsert.unresolved_reference_ids.len(), 1);
    }
}
