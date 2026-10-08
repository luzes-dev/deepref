//! Publish-time validation. Problems are returned per node, in plain
//! language, so the canvas can mark the offending block.

use std::collections::{HashMap, HashSet, VecDeque};

use serde::Serialize;
use serde_json::Value;

use super::{
    catalog::{ConfigField, ConfigFieldKind, NodeTypeDef, node_type},
    conditions::Condition,
    details::{TemplateProblem, scope_shape, template_problems, template_scope},
    model::{GraphNode, MAX_GRAPH_EDGES, MAX_GRAPH_NODES, WorkflowGraph},
    preflight::{
        EMAIL_NOT_CONFIGURED, SLACK_WEBHOOK_MISSING, email_address_is_valid, slack_webhook_problem,
        split_recipients, web_address_problem,
    },
    schedule::Schedule,
};

/// Placeholder stored in a graph in place of a secret value that is kept
/// separately and never returned to the browser.
pub const SECRET_PLACEHOLDER: &str = "__set__";

/// Facts a check needs that the graph does not carry: whether this server can
/// send e-mail, and the stored values of the secrets a block refers to. The
/// layer that can read them builds this; the checks themselves stay pure.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ValidationContext {
    /// Whether the server's outgoing mail settings are in place.
    pub email_configured: bool,
    /// Stored secret values by (block id, field key).
    pub secrets: HashMap<(String, String), String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct GraphIssue {
    /// The block the problem belongs to; `None` for whole-workflow problems.
    pub node_id: Option<String>,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Default)]
pub struct ValidationReport {
    pub issues: Vec<GraphIssue>,
}

impl ValidationReport {
    pub fn is_ok(&self) -> bool {
        self.issues.is_empty()
    }

    fn graph(&mut self, message: impl Into<String>) {
        self.issues.push(GraphIssue {
            node_id: None,
            message: message.into(),
        });
    }

    fn node(&mut self, node_id: &str, message: impl Into<String>) {
        self.issues.push(GraphIssue {
            node_id: Some(node_id.to_owned()),
            message: message.into(),
        });
    }
}

fn is_blank(value: Option<&Value>) -> bool {
    match value {
        None | Some(Value::Null) => true,
        Some(Value::String(text)) => text.trim().is_empty(),
        Some(Value::Array(items)) => items.is_empty(),
        Some(_) => false,
    }
}

fn check_field(
    report: &mut ValidationReport,
    node_id: &str,
    title: &str,
    field: &ConfigField,
    value: Option<&Value>,
) {
    if is_blank(value) {
        if field.required {
            report.node(
                node_id,
                format!("\"{title}\" needs \"{}\" to be filled in.", field.label),
            );
        }
        return;
    }
    let Some(value) = value else { return };
    match field.kind {
        ConfigFieldKind::Select => {
            let chosen = value.as_str().unwrap_or_default();
            if !field.options.iter().any(|option| option.value == chosen) {
                report.node(
                    node_id,
                    format!(
                        "\"{title}\": \"{}\" has a value that is not one of the choices.",
                        field.label
                    ),
                );
            }
        }
        ConfigFieldKind::Number => match value.as_f64() {
            Some(number) => {
                if field.min.is_some_and(|min| number < min)
                    || field.max.is_some_and(|max| number > max)
                {
                    report.node(
                        node_id,
                        format!(
                            "\"{title}\": \"{}\" must be between {} and {}.",
                            field.label,
                            field.min.unwrap_or(f64::MIN),
                            field.max.unwrap_or(f64::MAX)
                        ),
                    );
                }
            }
            None => report.node(
                node_id,
                format!("\"{title}\": \"{}\" must be a number.", field.label),
            ),
        },
        ConfigFieldKind::Schedule => match serde_json::from_value::<Schedule>(value.clone()) {
            Ok(schedule) => {
                if let Err(error) = schedule.validate() {
                    report.node(node_id, format!("\"{title}\": {error}."));
                }
            }
            Err(_) => report.node(node_id, format!("\"{title}\": the schedule is incomplete.")),
        },
        ConfigFieldKind::Condition => {
            if serde_json::from_value::<Condition>(value.clone()).is_err() {
                report.node(node_id, format!("\"{title}\": the rule is incomplete."));
            }
        }
        ConfigFieldKind::Duration => {
            let amount = value.get("value").and_then(Value::as_f64);
            let unit = value.get("unit").and_then(Value::as_str);
            if !(amount.is_some_and(|amount| amount > 0.0)
                && matches!(unit, Some("seconds" | "minutes" | "hours" | "days")))
            {
                report.node(
                    node_id,
                    format!(
                        "\"{title}\": \"{}\" needs an amount and a unit.",
                        field.label
                    ),
                );
            }
        }
        ConfigFieldKind::FieldList => {
            let valid = value.as_array().is_some_and(|rows| {
                !rows.is_empty()
                    && rows.iter().all(|row| {
                        row.get("name")
                            .and_then(Value::as_str)
                            .is_some_and(|name| !name.trim().is_empty())
                    })
            });
            if !valid {
                report.node(node_id, format!("\"{title}\": every answer needs a name."));
            }
        }
        ConfigFieldKind::PublicationQuery => {
            if crate::workflows::model::PublicationQuery::from_config(Some(value)).is_empty() {
                report.node(
                    node_id,
                    format!("\"{title}\": write the words to search for."),
                );
            }
        }
        ConfigFieldKind::Text
        | ConfigFieldKind::LongText
        | ConfigFieldKind::Boolean
        | ConfigFieldKind::ProjectField
        | ConfigFieldKind::Secret => {}
    }
}

/// Convert a duration config value to seconds.
pub fn duration_seconds(value: &Value) -> Option<u64> {
    let amount = value.get("value")?.as_f64()?;
    if amount <= 0.0 {
        return None;
    }
    let factor = match value.get("unit")?.as_str()? {
        "seconds" => 1.0,
        "minutes" => 60.0,
        "hours" => 3600.0,
        "days" => 86_400.0,
        _ => return None,
    };
    Some((amount * factor) as u64)
}

/// Checks that follow from what a block's settings lead to: the address it
/// calls, its Slack webhook, the server's mail setup, and the `{{names}}` its
/// text uses. Each one is worded like the message a real run would fail with.
fn check_block_settings(
    report: &mut ValidationReport,
    graph: &WorkflowGraph,
    node: &GraphNode,
    title: &str,
    def: &NodeTypeDef,
    ctx: &ValidationContext,
) {
    let text = |key: &str| {
        node.config
            .get(key)
            .and_then(Value::as_str)
            .unwrap_or_default()
    };
    match def.id.as_str() {
        "integration.http_request" => {
            let url = text("url");
            if !url.trim().is_empty()
                && !url.contains("{{")
                && let Some(problem) = web_address_problem(url)
            {
                report.node(&node.id, format!("\"{title}\": {problem}"));
            }
        }
        "integration.slack" => {
            let configured = text("webhook");
            if configured == SECRET_PLACEHOLDER {
                let stored = ctx
                    .secrets
                    .get(&(node.id.clone(), "webhook".to_owned()))
                    .map(String::as_str);
                match stored {
                    Some(address) => {
                        if let Some(problem) = slack_webhook_problem(address) {
                            report.node(&node.id, format!("\"{title}\": {problem}"));
                        }
                    }
                    None => report.node(&node.id, format!("\"{title}\": {SLACK_WEBHOOK_MISSING}")),
                }
            } else if !configured.trim().is_empty()
                && let Some(problem) = slack_webhook_problem(configured)
            {
                report.node(&node.id, format!("\"{title}\": {problem}"));
            }
        }
        "integration.email" => {
            if !ctx.email_configured {
                report.node(&node.id, format!("\"{title}\": {EMAIL_NOT_CONFIGURED}"));
            }
            let to = text("to");
            if !to.contains("{{") {
                for address in split_recipients(to) {
                    if !email_address_is_valid(&address) {
                        report.node(
                            &node.id,
                            format!("\"{title}\": \"{address}\" is not a valid e-mail address."),
                        );
                    }
                }
            }
        }
        _ => {}
    }
    for field in &def.config {
        let Some(scope) = template_scope(&def.id, &field.key) else {
            continue;
        };
        let Some(template) = node.config.get(&field.key).and_then(Value::as_str) else {
            continue;
        };
        if !template.contains("{{") {
            continue;
        }
        let shape = scope_shape(graph, &node.id, scope);
        for problem in template_problems(shape.as_ref(), template) {
            report.node(&node.id, template_message(title, &field.label, &problem));
        }
    }
}

fn template_message(title: &str, label: &str, problem: &TemplateProblem) -> String {
    match problem {
        TemplateProblem::Unclosed => {
            format!("\"{title}\": \"{label}\" has {{{{ with no matching }}}}.")
        }
        TemplateProblem::Empty => format!("\"{title}\": \"{label}\" has an empty {{{{}}}}."),
        TemplateProblem::Unknown(path) => format!(
            "\"{title}\": \"{label}\" uses {{{{{path}}}}}, but the step before does not give that detail. Check the spelling, or pick a detail under \"Insert value\"."
        ),
        TemplateProblem::WholeList(path) => format!(
            "\"{title}\": \"{label}\" uses {{{{{path}}}}}, but the step before gives a list. Use {{{{count}}}} for its size, or add \"Do this for each record\" before this step."
        ),
        TemplateProblem::PlainText(path) => format!(
            "\"{title}\": \"{label}\" uses {{{{{path}}}}}, but the step before gives plain text, which has no details to use."
        ),
    }
}

pub fn display_name(node: &super::model::GraphNode, def: &NodeTypeDef) -> String {
    node.label
        .as_deref()
        .filter(|label| !label.trim().is_empty())
        .unwrap_or(&def.label)
        .to_owned()
}

/// Validate a graph for publishing. The same checks run for Check, test runs
/// and publishing, so a problem that would stop a real run is reported here.
pub fn validate_graph(graph: &WorkflowGraph, ctx: &ValidationContext) -> ValidationReport {
    let mut report = ValidationReport::default();
    if graph.nodes.is_empty() {
        report.graph("The workflow is empty. Start by adding something that triggers it.");
        return report;
    }
    if graph.nodes.len() > MAX_GRAPH_NODES {
        report.graph(format!(
            "A workflow can have at most {MAX_GRAPH_NODES} blocks."
        ));
        return report;
    }
    if graph.edges.len() > MAX_GRAPH_EDGES {
        report.graph(format!(
            "A workflow can have at most {MAX_GRAPH_EDGES} connections."
        ));
        return report;
    }

    let mut seen = HashSet::new();
    let mut defs: HashMap<&str, (&NodeTypeDef, String)> = HashMap::new();
    for node in &graph.nodes {
        if node.id.trim().is_empty() || !seen.insert(node.id.as_str()) {
            report
                .graph("Two blocks share the same identity. Remove one of them and add it again.");
            continue;
        }
        match node_type(&node.node_type) {
            Some(def) => {
                defs.insert(node.id.as_str(), (def, display_name(node, def)));
            }
            None => report.node(&node.id, "This block is not available any more. Remove it."),
        }
    }

    // Exactly one trigger.
    let triggers: Vec<_> = graph
        .nodes
        .iter()
        .filter(|node| node.node_type.starts_with("trigger."))
        .collect();
    match triggers.len() {
        0 => {
            report.graph("Add something that starts the workflow, such as a schedule or an event.")
        }
        1 => {}
        _ => {
            for node in triggers.iter().skip(1) {
                report.node(
                    &node.id,
                    "A workflow can only have one trigger. Remove this one or the other.",
                );
            }
        }
    }

    // Edges.
    let mut incoming: HashMap<(&str, &str), usize> = HashMap::new();
    let mut adjacency: HashMap<&str, Vec<&str>> = HashMap::new();
    for edge in &graph.edges {
        let (Some((from_def, from_title)), Some((to_def, to_title))) = (
            defs.get(edge.from.node.as_str()),
            defs.get(edge.to.node.as_str()),
        ) else {
            report.graph("A connection points to a block that no longer exists.");
            continue;
        };
        let Some(out_port) = from_def
            .outputs
            .iter()
            .find(|port| port.id == edge.from.port)
        else {
            report.node(
                &edge.from.node,
                format!(
                    "\"{from_title}\" has no output called \"{}\".",
                    edge.from.port
                ),
            );
            continue;
        };
        let Some(in_port) = to_def.inputs.iter().find(|port| port.id == edge.to.port) else {
            report.node(
                &edge.to.node,
                format!("\"{to_title}\" has no input called \"{}\".", edge.to.port),
            );
            continue;
        };
        if !out_port.port_type.connects_to(in_port.port_type) {
            report.node(
                &edge.to.node,
                format!(
                    "\"{to_title}\" cannot use what \"{from_title}\" produces: it expects {} but gets {}.",
                    in_port.port_type.as_str(),
                    out_port.port_type.as_str()
                ),
            );
        }
        let count = incoming
            .entry((edge.to.node.as_str(), edge.to.port.as_str()))
            .or_default();
        *count += 1;
        if *count > 1 && !in_port.multiple {
            report.node(
                &edge.to.node,
                format!(
                    "\"{to_title}\" can only receive one connection into \"{}\".",
                    in_port.label
                ),
            );
        }
        adjacency
            .entry(edge.from.node.as_str())
            .or_default()
            .push(edge.to.node.as_str());
    }

    // The verdict outputs only carry records for screening.
    for edge in &graph.edges {
        if !matches!(edge.from.port.as_str(), "included" | "excluded" | "unsure") {
            continue;
        }
        let Some(source) = graph.node(&edge.from.node) else {
            continue;
        };
        if source.node_type != "ai.run_review"
            || source.config.get("task").and_then(Value::as_str) == Some("screening")
        {
            continue;
        }
        let title = defs
            .get(source.id.as_str())
            .map_or("This block", |(_, title)| title.as_str());
        report.node(
            &source.id,
            format!(
                "\"{title}\" sorts records by the AI's verdict only for \"Screen against the protocol\". Connect \"All records\" instead."
            ),
        );
    }

    // Required inputs and configuration.
    for node in &graph.nodes {
        let Some((def, title)) = defs.get(node.id.as_str()) else {
            continue;
        };
        for port in def.inputs.iter().filter(|port| port.required) {
            if !incoming.contains_key(&(node.id.as_str(), port.id.as_str())) {
                report.node(
                    &node.id,
                    format!(
                        "\"{title}\" is missing something to work with: connect \"{}\".",
                        port.label
                    ),
                );
            }
        }
        for field in &def.config {
            check_field(
                &mut report,
                &node.id,
                title,
                field,
                node.config.get(&field.key),
            );
        }
        check_block_settings(&mut report, graph, node, title, def, ctx);
    }

    // Reachability from the trigger.
    if let [trigger, ..] = triggers.as_slice() {
        let mut reached: HashSet<&str> = HashSet::new();
        let mut queue = VecDeque::from([trigger.id.as_str()]);
        while let Some(current) = queue.pop_front() {
            if !reached.insert(current) {
                continue;
            }
            for next in adjacency.get(current).into_iter().flatten() {
                queue.push_back(next);
            }
        }
        for node in &graph.nodes {
            if defs.contains_key(node.id.as_str()) && !reached.contains(node.id.as_str()) {
                let title = defs
                    .get(node.id.as_str())
                    .map_or("This block", |(_, title)| title.as_str());
                report.node(
                    &node.id,
                    format!("\"{title}\" is not connected to the trigger, so it would never run."),
                );
            }
        }
    }

    // Cycles (Kahn). Repetition is expressed with the "for each" block, which
    // fans out instead of looping back, so loops in the graph are never valid.
    let mut indegree: HashMap<&str, usize> = graph
        .nodes
        .iter()
        .map(|node| (node.id.as_str(), 0))
        .collect();
    for targets in adjacency.values() {
        for target in targets {
            if let Some(degree) = indegree.get_mut(target) {
                *degree += 1;
            }
        }
    }
    let mut queue: VecDeque<&str> = indegree
        .iter()
        .filter(|(_, degree)| **degree == 0)
        .map(|(id, _)| *id)
        .collect();
    let mut visited = 0;
    while let Some(current) = queue.pop_front() {
        visited += 1;
        for target in adjacency.get(current).into_iter().flatten() {
            if let Some(degree) = indegree.get_mut(target) {
                *degree -= 1;
                if *degree == 0 {
                    queue.push_back(target);
                }
            }
        }
    }
    if visited < graph.nodes.len() {
        for node in &graph.nodes {
            if indegree
                .get(node.id.as_str())
                .is_some_and(|degree| *degree > 0)
            {
                let title = defs
                    .get(node.id.as_str())
                    .map_or("This block", |(_, title)| title.as_str());
                report.node(
                    &node.id,
                    format!("\"{title}\" is part of a loop. To repeat steps, use \"Do this for each record\" instead of connecting back."),
                );
            }
        }
    }

    report
}

/// A secret value taken out of a graph so it is stored separately.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SecretWrite {
    pub node_id: String,
    pub key: String,
    /// `None` clears the stored secret.
    pub value: Option<String>,
}

/// Replace secret values in node configs with [`SECRET_PLACEHOLDER`] and
/// return what must be stored (or cleared) separately.
pub fn extract_secrets(graph: &mut WorkflowGraph) -> Vec<SecretWrite> {
    let mut writes = Vec::new();
    for node in &mut graph.nodes {
        let Some(def) = node_type(&node.node_type) else {
            continue;
        };
        let Some(config) = node.config.as_object_mut() else {
            continue;
        };
        for field in def
            .config
            .iter()
            .filter(|field| field.kind == ConfigFieldKind::Secret)
        {
            match config.get(&field.key) {
                Some(Value::String(text)) if text == SECRET_PLACEHOLDER => {}
                Some(Value::String(text)) if !text.trim().is_empty() => {
                    writes.push(SecretWrite {
                        node_id: node.id.clone(),
                        key: field.key.clone(),
                        value: Some(text.clone()),
                    });
                    config.insert(
                        field.key.clone(),
                        Value::String(SECRET_PLACEHOLDER.to_owned()),
                    );
                }
                _ => {
                    config.remove(&field.key);
                    writes.push(SecretWrite {
                        node_id: node.id.clone(),
                        key: field.key.clone(),
                        value: None,
                    });
                }
            }
        }
    }
    writes
}

#[cfg(test)]
pub(crate) mod tests {
    use serde_json::json;

    use super::*;
    use crate::workflows::model::{Endpoint, GraphEdge, GraphNode, Position};

    pub(crate) fn node(id: &str, node_type: &str, config: Value) -> GraphNode {
        GraphNode {
            id: id.to_owned(),
            node_type: node_type.to_owned(),
            position: Position::default(),
            label: None,
            config,
        }
    }

    pub(crate) fn edge(from: &str, from_port: &str, to: &str, to_port: &str) -> GraphEdge {
        GraphEdge {
            id: None,
            from: Endpoint {
                node: from.to_owned(),
                port: from_port.to_owned(),
            },
            to: Endpoint {
                node: to.to_owned(),
                port: to_port.to_owned(),
            },
        }
    }

    pub(crate) fn messages(report: &ValidationReport) -> Vec<String> {
        report
            .issues
            .iter()
            .map(|issue| issue.message.clone())
            .collect()
    }

    /// A server with outgoing mail set up and no stored secrets.
    pub(crate) fn ready() -> ValidationContext {
        ValidationContext {
            email_configured: true,
            ..ValidationContext::default()
        }
    }

    #[test]
    fn valid_graph_passes() {
        let graph = WorkflowGraph {
            nodes: vec![
                node("t", "trigger.report_added", json!({})),
                node("n", "integration.notify", json!({"title": "New record"})),
            ],
            edges: vec![edge("t", "record", "n", "data")],
        };
        let report = validate_graph(&graph, &ready());
        assert!(report.is_ok(), "{:?}", messages(&report));
    }

    #[test]
    fn reports_missing_trigger_dangling_inputs_and_missing_config() {
        let graph = WorkflowGraph {
            nodes: vec![node("n", "integration.notify", json!({}))],
            edges: vec![],
        };
        let text = messages(&validate_graph(&graph, &ready())).join("|");
        assert!(text.contains("starts the workflow"));
        assert!(text.contains("connect \"Information\""));
        assert!(text.contains("needs \"Title\""));
    }

    #[test]
    fn rejects_incompatible_edges_and_multiple_triggers() {
        let graph = WorkflowGraph {
            nodes: vec![
                node("t", "trigger.manual", json!({})),
                node("t2", "trigger.webhook", json!({})),
                node("f", "logic.filter", json!({"condition": {"rules": []}})),
            ],
            edges: vec![edge("t", "data", "f", "records")],
        };
        let report = validate_graph(&graph, &ready());
        let text = messages(&report).join("|");
        assert!(text.contains("expects records but gets json"), "{text}");
        assert!(text.contains("only have one trigger"));
        assert!(
            report
                .issues
                .iter()
                .any(|issue| issue.node_id.as_deref() == Some("f"))
        );
    }

    #[test]
    fn rejects_cycles_and_unreachable_blocks() {
        let graph = WorkflowGraph {
            nodes: vec![
                node("t", "trigger.manual", json!({})),
                node(
                    "a",
                    "logic.wait",
                    json!({"duration": {"value": 1, "unit": "minutes"}}),
                ),
                node(
                    "b",
                    "logic.wait",
                    json!({"duration": {"value": 1, "unit": "minutes"}}),
                ),
                node(
                    "island",
                    "logic.wait",
                    json!({"duration": {"value": 1, "unit": "minutes"}}),
                ),
            ],
            edges: vec![
                edge("t", "data", "a", "data"),
                edge("a", "data", "b", "data"),
                edge("b", "data", "a", "data"),
            ],
        };
        let text = messages(&validate_graph(&graph, &ready())).join("|");
        assert!(text.contains("part of a loop"), "{text}");
        assert!(text.contains("not connected to the trigger"), "{text}");
        assert!(text.contains("only receive one connection"), "{text}");
    }

    #[test]
    fn validates_select_number_schedule_and_duration_fields() {
        let graph = WorkflowGraph {
            nodes: vec![
                node(
                    "t",
                    "trigger.schedule",
                    json!({"schedule": {"every": "day", "at": "99:99", "timezone": "UTC"}}),
                ),
                node(
                    "q",
                    "data.find_records",
                    json!({"status": "weird", "limit": 9999}),
                ),
                node(
                    "w",
                    "logic.wait",
                    json!({"duration": {"value": 0, "unit": "minutes"}}),
                ),
            ],
            edges: vec![
                edge("t", "data", "q", "start"),
                edge("q", "records", "w", "data"),
            ],
        };
        let text = messages(&validate_graph(&graph, &ready())).join("|");
        assert!(text.contains("time of day"), "{text}");
        assert!(text.contains("not one of the choices"), "{text}");
        assert!(text.contains("must be between"), "{text}");
        assert!(text.contains("needs an amount and a unit"), "{text}");
    }

    #[test]
    fn secrets_are_moved_out_of_the_graph() {
        let mut graph = WorkflowGraph {
            nodes: vec![
                node("t", "trigger.manual", json!({})),
                node(
                    "s",
                    "integration.slack",
                    json!({"webhook": "https://hooks.slack.com/x", "message": "hi"}),
                ),
            ],
            edges: vec![edge("t", "data", "s", "data")],
        };
        let writes = extract_secrets(&mut graph);
        assert_eq!(writes.len(), 1);
        assert_eq!(
            writes[0].value.as_deref(),
            Some("https://hooks.slack.com/x")
        );
        assert_eq!(graph.nodes[1].config["webhook"], SECRET_PLACEHOLDER);
        // The stored value is looked up by the layer that reads the database.
        let mut stored = ValidationContext {
            email_configured: true,
            ..ValidationContext::default()
        };
        assert!(!validate_graph(&graph, &stored).is_ok());
        stored.secrets.insert(
            ("s".to_owned(), "webhook".to_owned()),
            "https://hooks.slack.com/x".to_owned(),
        );
        assert!(validate_graph(&graph, &stored).is_ok());
        // Saving the masked graph again keeps the stored secret.
        assert!(extract_secrets(&mut graph).is_empty());
    }

    /// The messages attached to one block.
    fn issues_for(report: &ValidationReport, id: &str) -> String {
        report
            .issues
            .iter()
            .filter(|issue| issue.node_id.as_deref() == Some(id))
            .map(|issue| issue.message.clone())
            .collect::<Vec<_>>()
            .join("|")
    }

    #[test]
    fn addresses_webhooks_and_mail_that_cannot_work_are_reported() {
        use crate::workflows::preflight::{
            EMAIL_NOT_CONFIGURED, NOT_SLACK_WEBHOOK, PRIVATE_ADDRESS, SLACK_WEBHOOK_MISSING,
        };
        let blocks = ["private", "public", "wrong", "unset", "mail"];
        let graph = WorkflowGraph {
            nodes: vec![
                node("t", "trigger.manual", json!({})),
                node(
                    "private",
                    "integration.http_request",
                    json!({"method": "GET", "url": "http://127.0.0.1:8099/"}),
                ),
                node(
                    "public",
                    "integration.http_request",
                    json!({"method": "GET", "url": "https://example.com/hook"}),
                ),
                node(
                    "wrong",
                    "integration.slack",
                    json!({"webhook": SECRET_PLACEHOLDER, "message": "hi"}),
                ),
                node(
                    "unset",
                    "integration.slack",
                    json!({"webhook": SECRET_PLACEHOLDER, "message": "hi"}),
                ),
                node(
                    "mail",
                    "integration.email",
                    json!({"to": "ana@example", "subject": "s", "body": "b"}),
                ),
            ],
            edges: blocks
                .iter()
                .map(|id| edge("t", "data", id, "data"))
                .collect(),
        };
        let mut ctx = ValidationContext::default();
        ctx.secrets.insert(
            ("wrong".to_owned(), "webhook".to_owned()),
            "https://example.com/not-slack".to_owned(),
        );
        let report = validate_graph(&graph, &ctx);
        assert!(issues_for(&report, "private").contains(PRIVATE_ADDRESS));
        assert_eq!(issues_for(&report, "public"), "");
        assert!(issues_for(&report, "wrong").contains(NOT_SLACK_WEBHOOK));
        assert!(issues_for(&report, "unset").contains(SLACK_WEBHOOK_MISSING));
        let mail = issues_for(&report, "mail");
        assert!(mail.contains(EMAIL_NOT_CONFIGURED), "{mail}");
        assert!(
            mail.contains("\"ana@example\" is not a valid e-mail address."),
            "{mail}"
        );

        ctx.email_configured = true;
        let report = validate_graph(&graph, &ctx);
        let mail = issues_for(&report, "mail");
        assert!(!mail.contains(EMAIL_NOT_CONFIGURED), "{mail}");
        assert!(mail.contains("is not a valid e-mail address"), "{mail}");
    }

    #[test]
    fn template_names_must_be_given_by_the_step_before() {
        let graph = WorkflowGraph {
            nodes: vec![
                node("t", "trigger.manual", json!({})),
                node("f", "data.find_records", json!({})),
                node(
                    "counted",
                    "integration.notify",
                    json!({"title": "Found {{count}} new", "message": "{{ttle}}"}),
                ),
                node("whole", "integration.notify", json!({"title": "{{title}}"})),
                node("each", "logic.for_each", json!({})),
                node(
                    "one",
                    "integration.notify",
                    json!({"title": "{{title}} / {{screening.final}}", "message": "{{ai_decision}}"}),
                ),
                node(
                    "broken",
                    "integration.notify",
                    json!({"title": "x {{ y", "message": "{{}}"}),
                ),
            ],
            edges: vec![
                edge("t", "data", "f", "start"),
                edge("f", "records", "counted", "data"),
                edge("f", "records", "whole", "data"),
                edge("f", "records", "each", "items"),
                edge("each", "item", "one", "data"),
                edge("f", "records", "broken", "data"),
            ],
        };
        let report = validate_graph(&graph, &ready());
        // `count` is known for a list, so only the misspelt name is reported.
        let counted = issues_for(&report, "counted");
        assert!(counted.contains("uses {{ttle}}"), "{counted}");
        assert_eq!(counted.matches("uses {{").count(), 1, "{counted}");
        let whole = issues_for(&report, "whole");
        assert!(
            whole.contains("{{title}}") && whole.contains("gives a list"),
            "{whole}"
        );
        // Each record has the record fields; AI verdicts are not among them.
        let one = issues_for(&report, "one");
        assert!(one.contains("{{ai_decision}}"), "{one}");
        assert!(
            !one.contains("screening.final") && !one.contains("{{title}}"),
            "{one}"
        );
        let broken = issues_for(&report, "broken");
        assert!(broken.contains("has {{ with no matching }}"), "{broken}");
        assert!(broken.contains("empty {{}}"), "{broken}");
    }

    #[test]
    fn ai_verdicts_are_known_after_a_screening_review() {
        let graph = WorkflowGraph {
            nodes: vec![
                node("t", "trigger.manual", json!({})),
                node("f", "data.find_records", json!({})),
                node("s", "ai.run_review", json!({"task": "screening"})),
                node("each", "logic.for_each", json!({})),
                node(
                    "n",
                    "integration.notify",
                    json!({"title": "{{ai_decision}}: {{ai_rationale}} {{title}}"}),
                ),
                node(
                    "review",
                    "ai.run_review",
                    json!({"task": "data_extraction"}),
                ),
                node("each2", "logic.for_each", json!({})),
                node(
                    "m",
                    "integration.notify",
                    json!({"title": "{{ai_decision}}"}),
                ),
            ],
            edges: vec![
                edge("t", "data", "f", "start"),
                edge("f", "records", "s", "records"),
                edge("s", "included", "each", "items"),
                edge("each", "item", "n", "data"),
                edge("f", "records", "review", "records"),
                edge("review", "records", "each2", "items"),
                edge("each2", "item", "m", "data"),
            ],
        };
        let report = validate_graph(&graph, &ready());
        assert_eq!(issues_for(&report, "n"), "");
        let m = issues_for(&report, "m");
        assert!(m.contains("{{ai_decision}}"), "{m}");
    }

    #[test]
    fn a_webhook_body_is_not_judged_by_its_names() {
        let graph = WorkflowGraph {
            nodes: vec![
                node("t", "trigger.webhook", json!({})),
                node(
                    "n",
                    "integration.notify",
                    json!({"title": "{{status}} {{anything}}", "message": "{{body.title}}"}),
                ),
            ],
            edges: vec![edge("t", "data", "n", "data")],
        };
        assert_eq!(issues_for(&validate_graph(&graph, &ready()), "n"), "");
    }

    #[test]
    fn duration_conversion() {
        assert_eq!(
            duration_seconds(&json!({"value": 2, "unit": "hours"})),
            Some(7200)
        );
        assert_eq!(
            duration_seconds(&json!({"value": 0, "unit": "hours"})),
            None
        );
    }
}
