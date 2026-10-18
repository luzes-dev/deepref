use chrono::Utc;
use deepref_domain::{
    FetchStatus, ProviderKind, Reference, Work, WorkWithReferences, normalize_doi,
};
use serde::{Deserialize, Deserializer};
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
    #[serde(default, deserialize_with = "skip_malformed")]
    related_identifiers: Vec<RelatedIdentifier>,
}

/// Keeps the entries that parse and drops the rest, so one malformed relation does not fail
/// the whole record. A missing or null list is empty.
fn skip_malformed<'de, D, T>(deserializer: D) -> Result<Vec<T>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    let entries = Option::<Vec<Value>>::deserialize(deserializer)?.unwrap_or_default();
    Ok(entries
        .into_iter()
        .filter_map(|entry| T::deserialize(entry).ok())
        .collect())
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

/// DataCite has sent the publisher as a plain name and as an object with a `name`. An object
/// without a name gives no publisher.
#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum Publisher {
    Name(String),
    Named { name: Option<String> },
}

impl Publisher {
    fn into_name(self) -> Option<String> {
        match self {
            Self::Name(name) => Some(name),
            Self::Named { name } => name,
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

/// One DataCite relation. Each field may be absent, and an entry without the fields it needs
/// is skipped where the relations are read.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RelatedIdentifier {
    related_identifier: Option<String>,
    related_identifier_type: Option<String>,
    relation_type: Option<String>,
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
        let (Some(identifier), Some(kind), Some(relation_type)) = (
            relation.related_identifier.as_deref(),
            relation.related_identifier_type.as_deref(),
            relation.relation_type.as_deref(),
        ) else {
            continue;
        };
        if !kind.eq_ignore_ascii_case("DOI") {
            continue;
        }
        let Ok(doi) = normalize_doi(identifier) else {
            continue;
        };
        match direction(relation_type) {
            Some(Direction::Outbound) => {
                let known = references
                    .iter()
                    .any(|reference| reference.doi.as_deref() == Some(doi.as_str()));
                if !known {
                    references.push(doi_reference(doi));
                }
            }
            Some(Direction::Incoming) => {
                incoming.push(json!({ "doi": doi, "relationType": relation_type }));
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
        let publisher = self.publisher.and_then(Publisher::into_name);

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
        assert_eq!(plain.into_name().as_deref(), Some("Zenodo"));
        assert_eq!(object.into_name().as_deref(), Some("Zenodo"));
    }

    #[test]
    fn one_malformed_relation_is_skipped_and_the_good_one_is_kept() {
        let body = json!({
            "doi": "10.5281/zenodo.1",
            "titles": [{ "title": "A dataset" }],
            "relatedIdentifiers": [
                { "relatedIdentifier": "10.1000/no-type", "relatedIdentifierType": "DOI" },
                "not an object",
                {
                    "relatedIdentifier": "10.1000/kept",
                    "relatedIdentifierType": "DOI",
                    "relationType": "References"
                }
            ]
        });
        let attributes: DataCiteAttributes = serde_json::from_value(body.clone())
            .expect("one malformed relation should not fail the record");

        let work = attributes.into_domain("10.5281/zenodo.1".to_owned(), body);

        let dois: Vec<_> = work
            .references
            .iter()
            .filter_map(|reference| reference.doi.as_deref())
            .collect();
        assert_eq!(dois, ["10.1000/kept"]);
        assert_eq!(work.work.title.as_deref(), Some("A dataset"));
    }

    #[test]
    fn a_publisher_object_without_a_name_gives_no_publisher() {
        let body = json!({ "doi": "10.5281/zenodo.1", "publisher": {} });
        let attributes: DataCiteAttributes = serde_json::from_value(body.clone())
            .expect("a nameless publisher should not fail the record");

        let work = attributes.into_domain("10.5281/zenodo.1".to_owned(), body);

        assert_eq!(work.work.publisher, None);
    }
}
