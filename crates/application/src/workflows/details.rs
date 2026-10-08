//! The details a block can read from the steps before it.
//!
//! A block sees the value on each input port: the output of the step it is
//! connected to. From the graph alone we can tell the shape of most values:
//! a record has these fields, a list of records has a count, an HTTP answer has
//! a status and a body. Webhook bodies and the data of a manual run are open:
//! their keys are whatever the caller sends, so validation cannot judge them.
//!
//! Validation uses the shapes to flag `{{names}}` that the previous step does
//! not provide, and the inspector uses them to offer the names to insert.

use serde::Serialize;
use serde_json::Value;
use uuid::Uuid;

use super::{
    catalog::node_type,
    conditions::template_tokens,
    model::{GraphNode, PortType, WorkflowGraph},
};

/// Guards the walk against a malformed graph; real flows are a few steps deep.
const MAX_DEPTH: usize = 64;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DetailKind {
    Text,
    Number,
    YesNo,
    List,
    Any,
}

/// One name a block can use, such as `title` or `screening.final`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Detail {
    pub key: String,
    pub label: String,
    pub kind: DetailKind,
}

/// What is known about the value on a port.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Shape {
    /// An object whose keys are all known.
    Object(Vec<Detail>),
    /// A list whose items have the given shape.
    List(Box<Shape>),
    /// An object whose keys nobody can know in advance.
    Open,
    /// A single piece of text.
    Text,
}

fn detail(key: &str, label: &str, kind: DetailKind) -> Detail {
    Detail {
        key: key.to_owned(),
        label: label.to_owned(),
        kind,
    }
}

/// The keys of a record as the blocks pass it around.
fn record_fields() -> Vec<Detail> {
    use DetailKind::{List, Number, Text};
    vec![
        detail("report_id", "Record id", Text),
        detail("title", "Title", Text),
        detail("abstract", "Abstract", Text),
        detail("year", "Year", Number),
        detail("journal", "Journal", Text),
        detail("url", "Web address", Text),
        detail("doi", "DOI", Text),
        detail("pmid", "PubMed ID", Text),
        detail("authors", "Authors", List),
        detail(
            "screening.title_abstract",
            "Title and abstract decision",
            Text,
        ),
        detail("screening.full_text", "Full text decision", Text),
        detail("screening.final", "Overall decision", Text),
        detail("study_id", "Study id", Text),
    ]
}

/// A record the AI has judged, as the verdict ports carry it.
fn screened_record_fields() -> Vec<Detail> {
    let mut fields = record_fields();
    fields.push(detail(
        "ai_decision",
        "AI verdict (include, exclude or unsure)",
        DetailKind::Text,
    ));
    fields.push(detail(
        "ai_rationale",
        "Why the AI decided",
        DetailKind::Text,
    ));
    fields
}

fn study_fields() -> Vec<Detail> {
    vec![
        detail("study_id", "Study id", DetailKind::Text),
        detail("title", "Study name", DetailKind::Text),
        detail("design", "Study design", DetailKind::Text),
    ]
}

fn count_detail() -> Detail {
    detail("count", "Number of items", DetailKind::Number)
}

fn record_list() -> Shape {
    Shape::List(Box::new(Shape::Object(record_fields())))
}

fn email_fields() -> Vec<Detail> {
    use DetailKind::{Any, List, Text};
    vec![
        detail("from", "Sender", Text),
        detail("to", "Recipient", Text),
        detail("subject", "Subject", Text),
        detail("text", "Message text", Text),
        detail("html", "Message as HTML", Text),
        detail("attachments", "Attachments", Any),
        detail("dois", "DOIs found in the message", List),
        detail("pmids", "PubMed IDs found in the message", List),
        detail("identifiers", "Identifiers found in the message", List),
    ]
}

fn report_fields() -> Vec<Detail> {
    let mut fields = record_fields();
    fields.push(detail("identifiers", "All identifiers", DetailKind::List));
    fields.push(detail("documents", "Full texts", DetailKind::List));
    fields
}

/// The shape of the value a node writes on one of its output ports.
fn output_shape(graph: &WorkflowGraph, node: &GraphNode, port: &str, depth: usize) -> Shape {
    if depth > MAX_DEPTH {
        return Shape::Open;
    }
    let passthrough =
        |input: &str| input_shape(graph, &node.id, input, depth + 1).unwrap_or(Shape::Open);
    let object = |fields: Vec<Detail>| Shape::Object(fields);
    let id_only = |key: &str, label: &str| object(vec![detail(key, label, DetailKind::Text)]);
    match (node.node_type.as_str(), port) {
        (
            "trigger.report_added"
            | "trigger.report_included"
            | "trigger.appraisal_completed"
            | "trigger.screening_decision_recorded",
            "record",
        ) => object(record_fields()),
        ("trigger.full_text_attached" | "trigger.document_parsed", "document") => object(vec![
            detail("document_id", "Full text id", DetailKind::Text),
            detail("report_id", "Record id", DetailKind::Text),
        ]),
        ("trigger.study_created", "study") | ("data.get_study", "study") => object(study_fields()),
        ("trigger.acquisition_completed", "data") => id_only("acquisition_id", "Import id"),
        ("trigger.protocol_published", "data") => {
            id_only("protocol_version_id", "Protocol version id")
        }
        ("trigger.ai_proposal_created", "data") => id_only("proposal_id", "Suggestion id"),
        ("trigger.schedule", "data") => object(vec![
            detail("scheduled_for", "Scheduled time", DetailKind::Text),
            detail("fired_at", "Started at", DetailKind::Text),
        ]),
        ("trigger.email", "data") => object(email_fields()),
        ("trigger.email", "text") => Shape::Text,
        ("trigger.publication_alert", "records") => Shape::List(Box::new(Shape::Open)),
        ("trigger.manual" | "trigger.webhook", "data") => Shape::Open,
        ("data.find_records" | "data.screening_queue", "records") => record_list(),
        ("data.get_report", "report") => object(report_fields()),
        ("action.import_identifiers", "records") => record_list(),
        ("action.record_decision" | "action.attach_pdf", "records") => passthrough("records"),
        ("action.group_into_study", "study") => object(vec![
            detail("title", "Study name", DetailKind::Text),
            detail(
                "suggested",
                "Suggested for a person to confirm",
                DetailKind::YesNo,
            ),
        ]),
        ("action.run_deduplication", "summary") => object(vec![
            detail("checked", "Records checked", DetailKind::Number),
            detail("linked", "Duplicates linked", DetailKind::Number),
            detail("new_reports", "New records", DetailKind::Number),
            detail(
                "suggestions",
                "Suggestions for a person",
                DetailKind::Number,
            ),
        ]),
        ("action.recompute_metrics", "summary") => object(vec![detail(
            "refreshed",
            "Statistics refreshed",
            DetailKind::YesNo,
        )]),
        ("action.export", "file") => object(vec![
            detail("file_id", "File id", DetailKind::Text),
            detail("name", "File name", DetailKind::Text),
            detail("records", "Number of records", DetailKind::Number),
        ]),
        ("ai.classify" | "ai.extract_field", "records") => passthrough("records"),
        ("ai.run_review", "records")
            if node.config.get("task").and_then(Value::as_str) != Some("screening") =>
        {
            passthrough("records")
        }
        ("ai.run_review", "records" | "included" | "excluded" | "unsure") => {
            Shape::List(Box::new(Shape::Object(screened_record_fields())))
        }
        ("ai.prompt", "result") => answers_shape(node),
        ("logic.if" | "logic.wait", _) => passthrough("data"),
        ("logic.filter", _) => passthrough("records"),
        ("logic.for_each", "item") => match input_shape(graph, &node.id, "items", depth + 1) {
            Some(Shape::List(item)) => *item,
            _ => Shape::Open,
        },
        ("logic.merge", "merged") => Shape::List(Box::new(
            input_shape(graph, &node.id, "inputs", depth + 1).unwrap_or(Shape::Open),
        )),
        ("integration.http_request", "response") => object(vec![
            detail("status", "Web status code", DetailKind::Number),
            detail("body", "Answer body", DetailKind::Any),
        ]),
        ("integration.notify" | "integration.email" | "integration.slack", "data") => {
            passthrough("data")
        }
        _ => port_type_shape(node, port),
    }
}

/// Answers of an AI step, as the schema the step asks the model for.
fn answers_shape(node: &GraphNode) -> Shape {
    let fields: Vec<Detail> = node
        .config
        .get("answers")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|answer| {
            let name = answer.get("name").and_then(Value::as_str)?.trim();
            if name.is_empty() {
                return None;
            }
            let kind = match answer.get("kind").and_then(Value::as_str) {
                Some("number") => DetailKind::Number,
                Some("yes_no") => DetailKind::YesNo,
                Some("list") => DetailKind::List,
                _ => DetailKind::Text,
            };
            Some(detail(name, name, kind))
        })
        .collect();
    if fields.is_empty() {
        Shape::Open
    } else {
        Shape::Object(fields)
    }
}

/// For a block without a known layout, the shape its port type implies.
fn port_type_shape(node: &GraphNode, port: &str) -> Shape {
    let port_type = node_type(&node.node_type)
        .and_then(|def| def.outputs.iter().find(|candidate| candidate.id == port))
        .map(|candidate| candidate.port_type);
    match port_type {
        Some(PortType::Records) => Shape::List(Box::new(Shape::Open)),
        Some(PortType::Text) => Shape::Text,
        _ => Shape::Open,
    }
}

/// The shape of the value that arrives on an input port, from its first
/// incoming connection. `None` when nothing is connected to it.
fn input_shape(graph: &WorkflowGraph, node_id: &str, port: &str, depth: usize) -> Option<Shape> {
    if depth > MAX_DEPTH {
        return Some(Shape::Open);
    }
    let edge = graph.incoming(node_id).find(|edge| edge.to.port == port)?;
    Some(match graph.node(&edge.from.node) {
        Some(source) => output_shape(graph, source, &edge.from.port, depth),
        None => Shape::Open,
    })
}

/// Where a block's text or rule reads its values from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Scope {
    /// The input port the value comes in on.
    pub port: &'static str,
    /// The field is filled in once per record of a list, not with the whole list.
    pub each_item: bool,
}

/// The input a text field of a block is a template over, when it is one.
pub fn template_scope(node_type: &str, field: &str) -> Option<Scope> {
    let whole = Scope {
        port: "data",
        each_item: false,
    };
    let each_record = Scope {
        port: "records",
        each_item: true,
    };
    match (node_type, field) {
        ("integration.notify", "title" | "message") => Some(whole),
        ("integration.email", "to" | "subject" | "body") => Some(whole),
        ("integration.slack", "message") => Some(whole),
        ("integration.http_request", "url" | "headers" | "body") => Some(whole),
        ("ai.prompt", "instructions") => Some(whole),
        ("action.attach_pdf", "url") => Some(each_record),
        ("action.group_into_study", "title") => Some(each_record),
        _ => None,
    }
}

/// The input a rule of a condition block reads from.
pub fn condition_scope(node_type: &str, field: &str) -> Option<Scope> {
    match (node_type, field) {
        ("logic.if", "condition") => Some(Scope {
            port: "data",
            each_item: false,
        }),
        ("logic.filter", "condition") => Some(Scope {
            port: "records",
            each_item: true,
        }),
        _ => None,
    }
}

/// The shape a scope reads, or `None` when nothing is connected to it.
pub fn scope_shape(graph: &WorkflowGraph, node_id: &str, scope: Scope) -> Option<Shape> {
    let shape = input_shape(graph, node_id, scope.port, 0)?;
    Some(match (scope.each_item, shape) {
        (true, Shape::List(item)) => *item,
        (_, shape) => shape,
    })
}

/// A problem with one `{{...}}` token of a template.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TemplateProblem {
    /// A `{{` with no `}}` after it.
    Unclosed,
    /// `{{}}` with no name in it.
    Empty,
    /// The name is not something the previous step provides.
    Unknown(String),
    /// The name needs one record at a time, but the step gives the whole list.
    WholeList(String),
    /// The previous step gives plain text, which has no names to use.
    PlainText(String),
}

/// Check the tokens of a template against the shape of the value it reads.
/// Nothing is reported when the shape is unknown.
pub fn template_problems(shape: Option<&Shape>, template: &str) -> Vec<TemplateProblem> {
    let tokens = template_tokens(template);
    let mut problems = Vec::new();
    if tokens.unclosed {
        problems.push(TemplateProblem::Unclosed);
    }
    for path in tokens.paths {
        if path.is_empty() {
            problems.push(TemplateProblem::Empty);
            continue;
        }
        match shape {
            None | Some(Shape::Open) => {}
            Some(Shape::Object(fields)) if fields.is_empty() => {}
            Some(Shape::Object(fields)) => {
                let root = path.split('.').next().unwrap_or_default();
                let known = fields
                    .iter()
                    .any(|field| field.key.split('.').next() == Some(root));
                if !known {
                    problems.push(TemplateProblem::Unknown(path));
                }
            }
            Some(Shape::List(_)) => {
                if path != "count" {
                    problems.push(TemplateProblem::WholeList(path));
                }
            }
            Some(Shape::Text) => problems.push(TemplateProblem::PlainText(path)),
        }
    }
    problems
}

/// What the inspector offers for one field of a block.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FieldDetails {
    /// Label of the block the value comes from, when one is connected.
    pub source: Option<String>,
    /// Names the field can use.
    pub fields: Vec<Detail>,
    /// The value is a whole list; only its size is available.
    pub list: bool,
    /// The value has keys that cannot be known; the name is typed by hand.
    pub open: bool,
    /// The field is filled in once per record of a list.
    pub each_item: bool,
}

/// What the inspector may offer for a text or rule field of a block.
pub fn field_details(graph: &WorkflowGraph, node_id: &str, field: &str) -> Option<FieldDetails> {
    let node = graph.node(node_id)?;
    let scope = template_scope(&node.node_type, field)
        .or_else(|| condition_scope(&node.node_type, field))?;
    let source = graph
        .incoming(node_id)
        .find(|edge| edge.to.port == scope.port)
        .and_then(|edge| graph.node(&edge.from.node))
        .map(|source| {
            source
                .label
                .as_deref()
                .filter(|label| !label.trim().is_empty())
                .or_else(|| node_type(&source.node_type).map(|def| def.label.as_str()))
                .unwrap_or("Previous step")
                .to_owned()
        });
    let shape = scope_shape(graph, node_id, scope);
    let mut details = FieldDetails {
        source,
        fields: Vec::new(),
        list: false,
        open: false,
        each_item: scope.each_item,
    };
    match shape {
        None => {}
        Some(Shape::Object(fields)) => details.fields = fields,
        Some(Shape::List(_)) => {
            details.list = true;
            details.fields = vec![count_detail()];
        }
        Some(Shape::Open) => details.open = true,
        Some(Shape::Text) => {}
    }
    Some(details)
}

/// The report ids of one record, or of each record in a list. Values without
/// an id contribute nothing.
pub fn report_ids_in(value: &Value) -> Vec<Uuid> {
    let items: Vec<&Value> = match value {
        Value::Array(items) => items.iter().collect(),
        other => vec![other],
    };
    let mut ids: Vec<Uuid> = Vec::new();
    for item in items {
        let id = item
            .get("report_id")
            .and_then(Value::as_str)
            .and_then(|text| Uuid::parse_str(text).ok());
        if let Some(id) = id
            && !ids.contains(&id)
        {
            ids.push(id);
        }
    }
    ids
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::workflows::{
        model::GraphEdge,
        validate::tests::{edge, node},
    };

    fn graph(nodes: Vec<GraphNode>, edges: Vec<GraphEdge>) -> WorkflowGraph {
        WorkflowGraph { nodes, edges }
    }

    fn keys(fields: &[Detail]) -> Vec<&str> {
        fields.iter().map(|field| field.key.as_str()).collect()
    }

    #[test]
    fn a_notification_after_a_record_sees_the_record_fields() {
        let g = graph(
            vec![
                node("t", "trigger.manual", json!({})),
                node("f", "data.find_records", json!({})),
                node("n", "integration.notify", json!({"title": "{{title}}"})),
            ],
            vec![
                edge("t", "data", "f", "start"),
                edge("f", "records", "n", "data"),
            ],
        );
        let details = field_details(&g, "n", "title").expect("notify title");
        assert!(details.list && !details.each_item);
        assert_eq!(keys(&details.fields), vec!["count"]);
        // A list is only ever read as a count, unless the step is repeated per record.
        let each = graph(
            vec![
                node("t", "trigger.manual", json!({})),
                node("f", "data.find_records", json!({})),
                node("e", "logic.for_each", json!({})),
                node("n", "integration.notify", json!({"title": "{{title}}"})),
            ],
            vec![
                edge("t", "data", "f", "start"),
                edge("f", "records", "e", "items"),
                edge("e", "item", "n", "data"),
            ],
        );
        let each_details = field_details(&each, "n", "title").expect("notify title");
        assert!(keys(&each_details.fields).contains(&"title"));
        assert!(keys(&each_details.fields).contains(&"screening.final"));
    }

    #[test]
    fn details_pass_through_branches_and_waits() {
        let g = graph(
            vec![
                node("t", "trigger.email", json!({})),
                node("if", "logic.if", json!({"condition": {"rules": []}})),
                node(
                    "w",
                    "logic.wait",
                    json!({"duration": {"value": 1, "unit": "minutes"}}),
                ),
                node("n", "integration.notify", json!({"title": "{{subject}}"})),
            ],
            vec![
                edge("t", "data", "if", "data"),
                edge("if", "yes", "w", "data"),
                edge("w", "data", "n", "data"),
            ],
        );
        let shape = scope_shape(
            &g,
            "n",
            template_scope("integration.notify", "title").expect("scope"),
        );
        assert!(
            matches!(shape, Some(Shape::Object(ref fields)) if fields.iter().any(|f| f.key == "subject"))
        );
    }

    #[test]
    fn ai_verdicts_and_the_answers_of_an_ai_step_are_known_names() {
        let g = graph(
            vec![
                node("t", "trigger.manual", json!({})),
                node("r", "data.find_records", json!({})),
                node("s", "ai.run_review", json!({"task": "screening"})),
                node("each", "logic.for_each", json!({})),
                node(
                    "n",
                    "integration.notify",
                    json!({"title": "{{ai_decision}}"}),
                ),
                node(
                    "p",
                    "ai.prompt",
                    json!({"instructions": "Read it.", "answers": [{"name": "sample_size", "kind": "number"}]}),
                ),
                node(
                    "m",
                    "integration.notify",
                    json!({"title": "{{sample_size}}"}),
                ),
            ],
            vec![
                edge("t", "data", "r", "start"),
                edge("r", "records", "s", "records"),
                edge("s", "included", "each", "items"),
                edge("each", "item", "n", "data"),
                edge("t", "data", "p", "data"),
                edge("p", "result", "m", "data"),
            ],
        );
        let verdicts = field_details(&g, "n", "title").expect("verdict field");
        assert!(keys(&verdicts.fields).contains(&"ai_decision"));
        assert!(keys(&verdicts.fields).contains(&"ai_rationale"));
        let answers = field_details(&g, "m", "title").expect("answer field");
        assert_eq!(keys(&answers.fields), vec!["sample_size"]);
        assert_eq!(answers.fields[0].kind, DetailKind::Number);
    }

    #[test]
    fn a_manual_or_webhook_body_is_open() {
        let g = graph(
            vec![
                node("t", "trigger.webhook", json!({})),
                node("n", "integration.notify", json!({"title": "{{anything}}"})),
            ],
            vec![edge("t", "data", "n", "data")],
        );
        let details = field_details(&g, "n", "title").expect("title");
        assert!(details.open);
        assert!(details.fields.is_empty());
        assert!(
            template_problems(
                scope_shape(
                    &g,
                    "n",
                    template_scope("integration.notify", "title").expect("scope")
                )
                .as_ref(),
                "{{anything}}"
            )
            .is_empty()
        );
    }

    #[test]
    fn template_tokens_are_checked_against_the_shape() {
        let record = Shape::Object(record_fields());
        assert!(template_problems(Some(&record), "{{title}} {{ screening.final }}").is_empty());
        assert_eq!(
            template_problems(Some(&record), "{{ttle}}"),
            vec![TemplateProblem::Unknown("ttle".to_owned())]
        );
        let list = Shape::List(Box::new(Shape::Object(record_fields())));
        assert!(template_problems(Some(&list), "Found {{count}} new").is_empty());
        assert_eq!(
            template_problems(Some(&list), "{{title}}"),
            vec![TemplateProblem::WholeList("title".to_owned())]
        );
        assert_eq!(
            template_problems(Some(&Shape::Text), "{{x}}"),
            vec![TemplateProblem::PlainText("x".to_owned())]
        );
        assert_eq!(
            template_problems(Some(&record), "a {{title"),
            vec![TemplateProblem::Unclosed]
        );
        assert_eq!(
            template_problems(Some(&record), "{{}}"),
            vec![TemplateProblem::Empty]
        );
        // Nothing connected: nothing to judge (the required-input check reports it).
        assert!(template_problems(None, "{{anything}}").is_empty());
    }

    #[test]
    fn report_ids_come_from_one_record_or_a_list() {
        let a = Uuid::new_v4();
        let b = Uuid::new_v4();
        assert_eq!(report_ids_in(&json!({"report_id": a.to_string()})), vec![a]);
        assert_eq!(
            report_ids_in(
                &json!([{"report_id": a.to_string()}, {"report_id": b.to_string()}, {"report_id": a.to_string()}])
            ),
            vec![a, b]
        );
        assert!(report_ids_in(&json!({"title": "webhook"})).is_empty());
    }
}
