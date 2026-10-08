use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Maximum nodes in a single workflow. Keeps the planner and the canvas fast.
pub const MAX_GRAPH_NODES: usize = 200;
/// Maximum edges in a single workflow.
pub const MAX_GRAPH_EDGES: usize = 600;

/// The kind of data a port carries. Edges may only join compatible ports.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PortType {
    /// A control signal with the details of what started the workflow.
    Trigger,
    /// A list of records (papers).
    Records,
    /// One record.
    Record,
    /// The full details of one report.
    Report,
    /// One study.
    Study,
    /// One full-text document.
    Document,
    /// Plain text.
    Text,
    /// Anything else.
    Json,
}

impl PortType {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Trigger => "trigger",
            Self::Records => "records",
            Self::Record => "record",
            Self::Report => "report",
            Self::Study => "study",
            Self::Document => "document",
            Self::Text => "text",
            Self::Json => "json",
        }
    }

    /// Whether a value of type `self` may be wired into a port of type `to`.
    ///
    /// Same types always connect; a single record can feed a list; reports
    /// and records are interchangeable (both identify one paper); everything
    /// can feed a generic `json` port; a trigger carries structured data so it
    /// can feed `json`.
    pub const fn connects_to(self, to: Self) -> bool {
        use PortType::{Json, Record, Records, Report, Text};
        if self as u8 == to as u8 {
            return true;
        }
        matches!(
            (self, to),
            (Record, Records) | (Report, Records) | (Record, Report) | (Report, Record)
        ) || matches!(to, Json)
            || matches!(
                (self, to),
                (Records, Text) | (Record, Text) | (Report, Text)
            )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, Default)]
pub struct Position {
    pub x: f64,
    pub y: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GraphNode {
    pub id: String,
    #[serde(rename = "type")]
    pub node_type: String,
    #[serde(default)]
    pub position: Position,
    /// Optional name the user gave this step on the canvas.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(default = "empty_object")]
    pub config: Value,
}

fn empty_object() -> Value {
    Value::Object(serde_json::Map::new())
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Endpoint {
    pub node: String,
    pub port: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GraphEdge {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    pub from: Endpoint,
    pub to: Endpoint,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct WorkflowGraph {
    #[serde(default)]
    pub nodes: Vec<GraphNode>,
    #[serde(default)]
    pub edges: Vec<GraphEdge>,
}

impl WorkflowGraph {
    pub fn node(&self, id: &str) -> Option<&GraphNode> {
        self.nodes.iter().find(|node| node.id == id)
    }

    pub fn incoming<'a>(&'a self, node_id: &'a str) -> impl Iterator<Item = &'a GraphEdge> + 'a {
        self.edges
            .iter()
            .filter(move |edge| edge.to.node == node_id)
    }

    pub fn outgoing<'a>(&'a self, node_id: &'a str) -> impl Iterator<Item = &'a GraphEdge> + 'a {
        self.edges
            .iter()
            .filter(move |edge| edge.from.node == node_id)
    }

    /// The single trigger node, when exactly one exists.
    pub fn trigger_node(&self) -> Option<&GraphNode> {
        let mut triggers = self
            .nodes
            .iter()
            .filter(|node| node.node_type.starts_with("trigger."));
        let first = triggers.next()?;
        if triggers.next().is_some() {
            None
        } else {
            Some(first)
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkflowStatus {
    /// Saved but never published, or turned off.
    Disabled,
    /// A published version reacts to its trigger.
    Enabled,
}

impl WorkflowStatus {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Disabled => "disabled",
            Self::Enabled => "enabled",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "disabled" => Some(Self::Disabled),
            "enabled" => Some(Self::Enabled),
            _ => None,
        }
    }
}

/// What starts a workflow, derived from its trigger node.
#[derive(Debug, Clone, PartialEq)]
pub enum WorkflowTrigger {
    /// A platform event such as "a record was added". The string is the
    /// stable event key (for example `report_added`).
    Event(String),
    Manual,
    Schedule(crate::workflows::schedule::Schedule),
    Webhook,
    PublicationAlert {
        /// Poll interval; publication alerts reuse the friendly schedule.
        schedule: crate::workflows::schedule::Schedule,
    },
    Email,
}

impl WorkflowTrigger {
    /// Stable storage key used for the `trigger_kind` column.
    pub fn kind_key(&self) -> String {
        match self {
            Self::Event(key) => key.clone(),
            Self::Manual => "manual".to_owned(),
            Self::Schedule(_) => "schedule".to_owned(),
            Self::Webhook => "webhook".to_owned(),
            Self::PublicationAlert { .. } => "publication_alert".to_owned(),
            Self::Email => "email".to_owned(),
        }
    }
}

/// The search of a publication alert, as the builder stores it: either the
/// simple fields (keywords, authors, journal) or, for older automations, only
/// the combined `terms`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub struct PublicationQuery {
    #[serde(default)]
    pub terms: String,
    #[serde(default)]
    pub keywords: Vec<String>,
    #[serde(default)]
    pub authors: Vec<String>,
    #[serde(default)]
    pub journal: Option<String>,
}

impl PublicationQuery {
    pub fn from_config(value: Option<&Value>) -> Self {
        value
            .and_then(|value| serde_json::from_value(value.clone()).ok())
            .unwrap_or_default()
    }

    fn parts(&self) -> (Vec<&str>, Vec<&str>, Option<&str>) {
        let journal = self
            .journal
            .as_deref()
            .map(str::trim)
            .filter(|journal| !journal.is_empty());
        (non_blank(&self.keywords), non_blank(&self.authors), journal)
    }

    /// PubMed query; mirrors `composeQueryTerms` in the web builder so the
    /// simple fields win over stale combined terms.
    pub fn pubmed_terms(&self) -> String {
        let (keywords, authors, journal) = self.parts();
        if keywords.is_empty() && authors.is_empty() && journal.is_none() {
            return self.terms.trim().to_owned();
        }
        let quoted = |term: &str| {
            if term.contains(char::is_whitespace) {
                format!("\"{term}\"")
            } else {
                term.to_owned()
            }
        };
        let mut parts = Vec::new();
        if !keywords.is_empty() {
            let any = keywords.iter().map(|word| quoted(word)).collect::<Vec<_>>();
            parts.push(format!("({})", any.join(" OR ")));
        }
        if !authors.is_empty() {
            let any = authors
                .iter()
                .map(|name| format!("{}[Author]", quoted(name)))
                .collect::<Vec<_>>();
            parts.push(format!("({})", any.join(" OR ")));
        }
        if let Some(journal) = journal {
            parts.push(format!("{}[Journal]", quoted(journal)));
        }
        parts.join(" AND ")
    }

    /// Plain words for Crossref, which does not understand PubMed field tags
    /// or boolean operators.
    pub fn crossref_terms(&self) -> String {
        let (keywords, authors, journal) = self.parts();
        if keywords.is_empty() && authors.is_empty() && journal.is_none() {
            return self.terms.trim().to_owned();
        }
        keywords
            .into_iter()
            .chain(authors)
            .chain(journal)
            .collect::<Vec<_>>()
            .join(" ")
    }

    pub fn is_empty(&self) -> bool {
        self.pubmed_terms().is_empty()
    }
}

fn non_blank(items: &[String]) -> Vec<&str> {
    items
        .iter()
        .map(|item| item.trim())
        .filter(|item| !item.is_empty())
        .collect()
}

/// Platform event keys that can start a workflow.
pub const EVENT_TRIGGER_KEYS: [&str; 10] = [
    "report_added",
    "acquisition_completed",
    "full_text_attached",
    "report_included",
    "study_created",
    "appraisal_completed",
    "screening_decision_recorded",
    "protocol_published",
    "document_parsed",
    "ai_proposal_created",
];

/// Resolve the trigger of a graph from its trigger node configuration.
pub fn trigger_of_graph(graph: &WorkflowGraph) -> Option<WorkflowTrigger> {
    let node = graph.trigger_node()?;
    let key = node.node_type.strip_prefix("trigger.")?;
    match key {
        "manual" => Some(WorkflowTrigger::Manual),
        "webhook" => Some(WorkflowTrigger::Webhook),
        "email" => Some(WorkflowTrigger::Email),
        "schedule" => {
            let schedule = serde_json::from_value(node.config.get("schedule")?.clone()).ok()?;
            Some(WorkflowTrigger::Schedule(schedule))
        }
        "publication_alert" => {
            let schedule = node
                .config
                .get("schedule")
                .and_then(|value| serde_json::from_value(value.clone()).ok())
                .unwrap_or_else(crate::workflows::schedule::Schedule::daily_default);
            Some(WorkflowTrigger::PublicationAlert { schedule })
        }
        event if EVENT_TRIGGER_KEYS.contains(&event) => {
            Some(WorkflowTrigger::Event(event.to_owned()))
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn port_compatibility_is_explicit() {
        assert!(PortType::Records.connects_to(PortType::Records));
        assert!(PortType::Record.connects_to(PortType::Records));
        assert!(PortType::Report.connects_to(PortType::Json));
        assert!(!PortType::Records.connects_to(PortType::Record));
        assert!(!PortType::Text.connects_to(PortType::Records));
        assert!(!PortType::Study.connects_to(PortType::Document));
    }
}

#[cfg(test)]
mod publication_query_tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn simple_fields_win_over_combined_terms_and_crossref_gets_plain_words() {
        let query = PublicationQuery::from_config(Some(&json!({
            "terms": "",
            "keywords": ["auditory neuropathy", "OTOF"],
            "authors": ["Silva J"],
            "journal": " "
        })));
        assert_eq!(
            query.pubmed_terms(),
            "(\"auditory neuropathy\" OR OTOF) AND (\"Silva J\"[Author])"
        );
        assert_eq!(query.crossref_terms(), "auditory neuropathy OTOF Silva J");
        assert!(!query.is_empty());
    }

    #[test]
    fn older_automations_keep_their_combined_terms() {
        let query = PublicationQuery::from_config(Some(&json!({"terms": " Auditory Neuropathy "})));
        assert_eq!(query.pubmed_terms(), "Auditory Neuropathy");
        assert_eq!(query.crossref_terms(), "Auditory Neuropathy");
        assert!(PublicationQuery::from_config(Some(&json!({"terms": "  "}))).is_empty());
        assert!(PublicationQuery::from_config(None).is_empty());
    }
}
