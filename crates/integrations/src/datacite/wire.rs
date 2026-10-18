use chrono::Utc;
use deepref_domain::{
    FetchStatus, ProviderKind, Reference, Work, WorkWithReferences, normalize_doi,
};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::crossref::clean_abstract;

/// Raw key for the DOIs of works that cite this record. The edge points the other way, so
/// these never become references.
pub(crate) const INCOMING_KEY: &str = "deepref_datacite_incoming";

/// The base64 XML DataCite returns beside the JSON attributes. It repeats the JSON, so it
/// stays out of the stored payload.
const XML_KEY: &str = "xml";

/// The `data.attributes` fields the mapping reads. The whole attribute object stays in raw.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DataCiteAttributes {
    pub(crate) doi: Option<String>,
    #[serde(default)]
    titles: Vec<Title>,
    #[serde(default)]
    descriptions: Vec<Description>,
    publication_year: Option<i32>,
    publisher: Option<Publisher>,
    container: Option<Container>,
    types: Option<Types>,
    url: Option<String>,
    citation_count: Option<i32>,
    reference_count: Option<i32>,
    #[serde(default)]
    related_identifiers: Vec<RelatedIdentifier>,
}

#[derive(Debug, Deserialize)]
struct Title {
    title: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Description {
    description_type: Option<String>,
    description: Option<String>,
}

/// DataCite has sent the publisher as a plain name and as an object with a `name`.
#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum Publisher {
    Name(String),
    Named { name: String },
}

impl Publisher {
    fn into_name(self) -> String {
        match self {
            Self::Name(name) | Self::Named { name } => name,
        }
    }
}

#[derive(Debug, Deserialize)]
struct Container {
    title: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Types {
    resource_type_general: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RelatedIdentifier {
    related_identifier: String,
    related_identifier_type: String,
    relation_type: String,
}

/// Which way a DataCite relation points, seen from this record. `Outbound` names a work this
/// record cites. `Incoming` names a work that cites this record.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Direction {
    Outbound,
    Incoming,
}

/// The one place that decides which relation types are citation relations and which way they
/// point. A type not listed here is not a citation, and its entry stays in the raw payload.
const CITATION_RELATIONS: [(&str, Direction); 4] = [
    ("References", Direction::Outbound),
    ("Cites", Direction::Outbound),
    ("IsReferencedBy", Direction::Incoming),
    ("IsCitedBy", Direction::Incoming),
];

fn direction(relation_type: &str) -> Option<Direction> {
    CITATION_RELATIONS
        .iter()
        .find(|(name, _)| *name == relation_type)
        .map(|(_, direction)| *direction)
}

/// The DOI relations of a record, split by direction. Only DOI targets count, because a URL
/// or arXiv identifier cannot become a child job. Each outbound target appears once.
struct CitationRelations {
    references: Vec<Reference>,
    incoming: Vec<Value>,
}

fn citation_relations(related: &[RelatedIdentifier]) -> CitationRelations {
    let mut references: Vec<Reference> = Vec::new();
    let mut incoming = Vec::new();
    for relation in related {
        if !relation.related_identifier_type.eq_ignore_ascii_case("DOI") {
            continue;
        }
        let Ok(doi) = normalize_doi(&relation.related_identifier) else {
            continue;
        };
        match direction(&relation.relation_type) {
            Some(Direction::Outbound) => {
                let known = references
                    .iter()
                    .any(|reference| reference.doi.as_deref() == Some(doi.as_str()));
                if !known {
                    references.push(doi_reference(doi));
                }
            }
            Some(Direction::Incoming) => {
                incoming.push(json!({ "doi": doi, "relationType": relation.relation_type }));
            }
            None => {}
        }
    }
    CitationRelations {
        references,
        incoming,
    }
}

/// A reference known only by its DOI.
fn doi_reference(doi: String) -> Reference {
    Reference {
        doi: Some(doi),
        key: None,
        raw_unstructured: None,
        article_title: None,
        author: None,
        year: None,
        volume: None,
        first_page: None,
    }
}

impl DataCiteAttributes {
    /// Maps the record to the domain work. The abstract is the description typed `Abstract`,
    /// cleaned like the Crossref and OpenAlex abstracts. The raw payload keeps the attributes,
    /// with the incoming DOIs added under `INCOMING_KEY`.
    pub(crate) fn into_domain(self, doi: String, mut raw: Value) -> WorkWithReferences {
        let relations = citation_relations(&self.related_identifiers);
        let abstract_text = self
            .descriptions
            .iter()
            .find(|description| description.description_type.as_deref() == Some("Abstract"))
            .and_then(|description| description.description.as_deref())
            .and_then(clean_abstract);
        let title = self.titles.into_iter().next().and_then(|title| title.title);
        let work_type = self.types.and_then(|types| types.resource_type_general);
        let container_title = self.container.and_then(|container| container.title);
        let publisher = self.publisher.map(Publisher::into_name);

        if let Some(object) = raw.as_object_mut() {
            object.remove(XML_KEY);
            object.insert(INCOMING_KEY.to_owned(), Value::Array(relations.incoming));
        }

        WorkWithReferences {
            work: Work {
                doi,
                title,
                abstract_text,
                work_type,
                publisher,
                container_title,
                issued_year: self.publication_year,
                published_year: self.publication_year,
                url: self.url,
                total_citations: self.citation_count.unwrap_or(0),
                references_count: self.reference_count.unwrap_or(0),
                metadata_provider: ProviderKind::DataCite.as_str().to_owned(),
                citation_provider: ProviderKind::DataCite.as_str().to_owned(),
                fetched_at: Utc::now(),
                fetch_status: FetchStatus::Fetched.as_str().to_owned(),
            },
            references: relations.references,
            raw,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn relation(related: &str, kind: &str, relation_type: &str) -> RelatedIdentifier {
        serde_json::from_value(json!({
            "relatedIdentifier": related,
            "relatedIdentifierType": kind,
            "relationType": relation_type,
        }))
        .expect("relation should parse")
    }

    #[test]
    fn the_relation_table_points_references_and_cites_outbound_only() {
        assert_eq!(direction("References"), Some(Direction::Outbound));
        assert_eq!(direction("Cites"), Some(Direction::Outbound));
        assert_eq!(direction("IsReferencedBy"), Some(Direction::Incoming));
        assert_eq!(direction("IsCitedBy"), Some(Direction::Incoming));
        for other in ["IsSupplementTo", "IsVersionOf", "HasVersion", "cites"] {
            assert_eq!(direction(other), None, "{other} is not a citation relation");
        }
    }

    #[test]
    fn outbound_targets_appear_once_and_only_for_dois() {
        let related = [
            relation("10.1000/a", "DOI", "References"),
            relation("10.1000/a", "DOI", "Cites"),
            relation("arXiv:2407.01989v2", "arXiv", "Cites"),
            relation("https://example.org/paper", "URL", "References"),
            relation("10.1000/b", "DOI", "Cites"),
        ];

        let relations = citation_relations(&related);

        let dois: Vec<_> = relations
            .references
            .iter()
            .filter_map(|reference| reference.doi.as_deref())
            .collect();
        assert_eq!(dois, ["10.1000/a", "10.1000/b"]);
        assert!(relations.incoming.is_empty());
    }

    #[test]
    fn incoming_dois_are_kept_as_a_small_object_and_never_become_references() {
        let related = [
            relation("10.1000/citing", "DOI", "IsReferencedBy"),
            relation("https://example.org/citing", "URL", "IsCitedBy"),
        ];

        let relations = citation_relations(&related);

        assert!(relations.references.is_empty());
        assert_eq!(
            relations.incoming,
            [json!({ "doi": "10.1000/citing", "relationType": "IsReferencedBy" })]
        );
    }

    #[test]
    fn the_publisher_is_read_as_a_name_or_as_an_object() {
        let plain: Publisher = serde_json::from_value(json!("Zenodo")).expect("plain name");
        let object: Publisher =
            serde_json::from_value(json!({ "name": "Zenodo" })).expect("named publisher");
        assert_eq!(plain.into_name(), "Zenodo");
        assert_eq!(object.into_name(), "Zenodo");
    }
}
