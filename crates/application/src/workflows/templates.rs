//! Ready-made workflows. Every built-in automation recipe has a template, so
//! the canvas can offer the same capabilities as the old fixed recipes.

use serde_json::{Value, json};

use crate::automations::BuiltInAutomationRecipe;

use super::model::{Endpoint, GraphEdge, GraphNode, Position, WorkflowGraph};

#[derive(Debug, Clone, PartialEq)]
pub struct WorkflowTemplate {
    pub id: &'static str,
    /// The built-in recipe this template replaces, when there is one.
    pub recipe: Option<BuiltInAutomationRecipe>,
    pub name: &'static str,
    pub description: &'static str,
    pub graph: WorkflowGraph,
}

struct Builder {
    nodes: Vec<GraphNode>,
    edges: Vec<GraphEdge>,
}

impl Builder {
    fn new() -> Self {
        Self {
            nodes: Vec::new(),
            edges: Vec::new(),
        }
    }

    fn add(mut self, id: &str, node_type: &str, config: Value) -> Self {
        let x = 80.0 + 300.0 * self.nodes.len() as f64;
        self.nodes.push(GraphNode {
            id: id.to_owned(),
            node_type: node_type.to_owned(),
            position: Position { x, y: 120.0 },
            label: None,
            config,
        });
        self
    }

    fn connect(mut self, from: &str, from_port: &str, to: &str, to_port: &str) -> Self {
        self.edges.push(GraphEdge {
            id: Some(format!("{from}:{from_port}->{to}:{to_port}")),
            from: Endpoint {
                node: from.to_owned(),
                port: from_port.to_owned(),
            },
            to: Endpoint {
                node: to.to_owned(),
                port: to_port.to_owned(),
            },
        });
        self
    }

    fn build(self) -> WorkflowGraph {
        WorkflowGraph {
            nodes: self.nodes,
            edges: self.edges,
        }
    }
}

fn build_templates() -> Vec<WorkflowTemplate> {
    vec![
        WorkflowTemplate {
            id: "project_maintenance",
            recipe: Some(BuiltInAutomationRecipe::ProjectMaintenanceV1),
            name: "Refresh project statistics",
            description: "Recalculates the project's counts and rankings whenever you run it.",
            graph: Builder::new()
                .add("start", "trigger.manual", json!({}))
                .add("refresh", "action.recompute_metrics", json!({}))
                .connect("start", "data", "refresh", "start")
                .build(),
        },
        WorkflowTemplate {
            id: "review_screening",
            recipe: Some(BuiltInAutomationRecipe::ReviewScreeningV1),
            name: "AI screening of the waiting records",
            description: "Takes the records still waiting for a title and abstract decision and asks the AI to screen them against the protocol.",
            graph: Builder::new()
                .add("start", "trigger.manual", json!({}))
                .add("queue", "data.screening_queue", json!({"stage": "title_abstract", "limit": 100}))
                .add("screen", "ai.classify", json!({"stage": "title_abstract"}))
                .connect("start", "data", "queue", "start")
                .connect("queue", "records", "screen", "records")
                .build(),
        },
        WorkflowTemplate {
            id: "review_duplicate_detection",
            recipe: Some(BuiltInAutomationRecipe::ReviewDuplicateDetectionV1),
            name: "Look for duplicate records",
            description: "Checks newly added records against the rest of the project.",
            graph: Builder::new()
                .add("start", "trigger.manual", json!({}))
                .add("dedupe", "action.run_deduplication", json!({"limit": 500}))
                .connect("start", "data", "dedupe", "start")
                .build(),
        },
        WorkflowTemplate {
            id: "review_study_classification",
            recipe: Some(BuiltInAutomationRecipe::ReviewStudyClassificationV1),
            name: "Suggest the design of included studies",
            description: "Asks the AI to suggest a study design for the included records.",
            graph: Builder::new()
                .add("start", "trigger.manual", json!({}))
                .add("find", "data.find_records", json!({"stage": "final", "status": "include", "limit": 100}))
                .add("classify", "ai.run_review", json!({"task": "study_classification"}))
                .connect("start", "data", "find", "start")
                .connect("find", "records", "classify", "records")
                .build(),
        },
        WorkflowTemplate {
            id: "review_study_grouping",
            recipe: Some(BuiltInAutomationRecipe::ReviewStudyGroupingV1),
            name: "Suggest study groups",
            description: "Asks the AI which included records belong to the same study.",
            graph: Builder::new()
                .add("start", "trigger.manual", json!({}))
                .add("find", "data.find_records", json!({"stage": "final", "status": "include", "limit": 100}))
                .add("group", "ai.run_review", json!({"task": "study_grouping"}))
                .connect("start", "data", "find", "start")
                .connect("find", "records", "group", "records")
                .build(),
        },
        WorkflowTemplate {
            id: "review_appraisal_prefill",
            recipe: Some(BuiltInAutomationRecipe::ReviewAppraisalPrefillV1),
            name: "Pre-fill quality appraisal",
            description: "Asks the AI to draft the quality appraisal of the included records for you to check.",
            graph: Builder::new()
                .add("start", "trigger.manual", json!({}))
                .add("find", "data.find_records", json!({"stage": "final", "status": "include", "limit": 50}))
                .add("appraise", "ai.run_review", json!({"task": "appraisal_prefill"}))
                .connect("start", "data", "find", "start")
                .connect("find", "records", "appraise", "records")
                .build(),
        },
        WorkflowTemplate {
            id: "review_data_extraction",
            recipe: Some(BuiltInAutomationRecipe::ReviewDataExtractionV1),
            name: "Extract data from included studies",
            description: "Asks the AI to fill in your extraction fields, citing the passages it used.",
            graph: Builder::new()
                .add("start", "trigger.manual", json!({}))
                .add("find", "data.find_records", json!({"stage": "final", "status": "include", "limit": 50}))
                .add("extract", "ai.run_review", json!({"task": "data_extraction"}))
                .connect("start", "data", "find", "start")
                .connect("find", "records", "extract", "records")
                .build(),
        },
        WorkflowTemplate {
            id: "weekly_literature_alert",
            recipe: None,
            name: "Weekly literature alert",
            description: "Every Monday, looks for new publications on your topic, adds them to the project and tells you.",
            graph: Builder::new()
                .add(
                    "alert",
                    "trigger.publication_alert",
                    json!({
                        "query": {"terms": ""},
                        "sources": "both",
                        "schedule": {"every": "week", "weekdays": ["monday"], "at": "08:00", "timezone": "UTC"}
                    }),
                )
                .add("import", "action.import_identifiers", json!({"label": "Literature alert"}))
                .add("notify", "integration.notify", json!({"title": "New publications added", "message": "The weekly literature alert added new records.", "severity": "info"}))
                .connect("alert", "records", "import", "items")
                .connect("import", "records", "notify", "data")
                .build(),
        },
        WorkflowTemplate {
            id: "literature_alert_ai_included",
            recipe: None,
            name: "Literature alert, notify only what the AI would include",
            description: "Looks for new publications on your topic, adds them to the project, lets the AI screen them and notifies you only about the ones it would include, with its reason.",
            graph: Builder::new()
                .add(
                    "alert",
                    "trigger.publication_alert",
                    json!({
                        "query": {"terms": ""},
                        "sources": "both",
                        "schedule": {"every": "day", "at": "08:00", "timezone": "UTC"}
                    }),
                )
                .add("import", "action.import_identifiers", json!({"label": "Literature alert"}))
                .add("screen", "ai.run_review", json!({"task": "screening", "stage": "title_abstract"}))
                .add("each", "logic.for_each", json!({"max_items": 50}))
                .add(
                    "notify",
                    "integration.notify",
                    json!({"title": "AI would include: {{title}}", "message": "{{ai_rationale}}", "severity": "info"}),
                )
                .connect("alert", "records", "import", "items")
                .connect("import", "records", "screen", "records")
                .connect("screen", "included", "each", "items")
                .connect("each", "item", "notify", "data")
                .build(),
        },
        WorkflowTemplate {
            id: "email_to_library",
            recipe: None,
            name: "E-mail papers into the project",
            description: "Forward an e-mail with DOIs or PubMed IDs to this workflow and the papers are added to the project.",
            graph: Builder::new()
                .add("mail", "trigger.email", json!({}))
                .add("import", "action.import_identifiers", json!({"label": "Added by e-mail"}))
                .connect("mail", "data", "import", "items")
                .build(),
        },
        WorkflowTemplate {
            id: "included_record_notice",
            recipe: None,
            name: "Tell me when a record is included",
            description: "Posts a notification each time a record is included after screening.",
            graph: Builder::new()
                .add("included", "trigger.report_included", json!({}))
                .add("notify", "integration.notify", json!({"title": "A record was included", "message": "{{title}}", "severity": "success"}))
                .connect("included", "record", "notify", "data")
                .build(),
        },
    ]
}

pub fn templates() -> &'static [WorkflowTemplate] {
    static TEMPLATES: std::sync::OnceLock<Vec<WorkflowTemplate>> = std::sync::OnceLock::new();
    TEMPLATES.get_or_init(build_templates)
}

pub fn template(id: &str) -> Option<&'static WorkflowTemplate> {
    templates().iter().find(|template| template.id == id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workflows::validate::{ValidationContext, validate_graph};

    /// A server with outgoing mail set up, so templates are judged on their own terms.
    fn ready() -> ValidationContext {
        ValidationContext {
            email_configured: true,
            ..ValidationContext::default()
        }
    }

    #[test]
    fn every_builtin_recipe_has_a_valid_template() {
        for recipe in BuiltInAutomationRecipe::ALL {
            let found = templates()
                .iter()
                .find(|template| template.recipe == Some(recipe));
            let Some(found) = found else {
                unreachable!("recipe {} has no template", recipe.id());
            };
            assert_eq!(found.id, recipe.id());
        }
    }

    #[test]
    fn templates_validate_except_for_fields_the_user_must_fill() {
        for template in templates() {
            let report = validate_graph(&template.graph, &ready());
            for issue in &report.issues {
                // The two literature alerts need the user's own search words,
                // and nothing else.
                let search_words_only = issue.message.ends_with("write the words to search for.");
                assert!(
                    search_words_only
                        && matches!(
                            template.id,
                            "weekly_literature_alert" | "literature_alert_ai_included"
                        ),
                    "{}: {}",
                    template.id,
                    issue.message
                );
            }
        }
    }

    #[test]
    fn literature_alert_that_screens_and_notifies_each_record_is_publishable() {
        let graph = Builder::new()
            .add(
                "alert",
                "trigger.publication_alert",
                json!({
                    "query": {"terms": "Auditory Neuropathy"},
                    "sources": "both",
                    "schedule": {"every": "day", "at": "08:00", "timezone": "UTC"}
                }),
            )
            .add("import", "action.import_identifiers", json!({}))
            .add("screen", "ai.run_review", json!({"task": "screening"}))
            .add("each", "logic.for_each", json!({"max_items": 50}))
            .add(
                "notify",
                "integration.notify",
                json!({"title": "New matching article found"}),
            )
            .connect("alert", "records", "import", "items")
            .connect("import", "records", "screen", "records")
            .connect("screen", "records", "each", "items")
            .connect("each", "item", "notify", "data")
            .build();
        let report = validate_graph(&graph, &ready());
        let messages: Vec<_> = report.issues.iter().map(|issue| &issue.message).collect();
        assert!(messages.is_empty(), "{messages:?}");
    }

    #[test]
    fn template_ids_are_unique() {
        let mut ids: Vec<_> = templates().iter().map(|template| template.id).collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), templates().len());
    }
}
