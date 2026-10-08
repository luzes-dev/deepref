//! The node catalog: every block a workflow can use, described in plain
//! language so the canvas can render a palette and generate inspector forms
//! without hard-coding any block.

use std::sync::OnceLock;

use serde::Serialize;
use serde_json::{Value, json};

use super::model::PortType;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum NodeCategory {
    Trigger,
    Data,
    Action,
    Ai,
    Logic,
    Integration,
}

impl NodeCategory {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Trigger => "trigger",
            Self::Data => "data",
            Self::Action => "action",
            Self::Ai => "ai",
            Self::Logic => "logic",
            Self::Integration => "integration",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct PortDef {
    pub id: String,
    pub label: String,
    #[serde(rename = "type")]
    pub port_type: PortType,
    /// An input that must be connected for the workflow to publish.
    pub required: bool,
    /// An input that accepts several incoming connections.
    pub multiple: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ConfigFieldKind {
    Text,
    LongText,
    Number,
    Select,
    Boolean,
    /// Pick one of the project's data-extraction fields.
    ProjectField,
    /// Write-only value (tokens, webhook URLs with credentials).
    Secret,
    /// `{ "value": 5, "unit": "minutes" }`
    Duration,
    /// Friendly schedule, see [`super::schedule::Schedule`].
    Schedule,
    /// Rule builder, see [`super::conditions::Condition`].
    Condition,
    /// Saved publication search, see the publication alert trigger.
    PublicationQuery,
    /// List of `{ "name", "kind", "description" }` rows describing the
    /// answers an AI step must return.
    FieldList,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ConfigOption {
    pub value: String,
    pub label: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ConfigField {
    pub key: String,
    pub label: String,
    pub help: Option<String>,
    pub kind: ConfigFieldKind,
    pub required: bool,
    pub default: Option<Value>,
    pub options: Vec<ConfigOption>,
    pub placeholder: Option<String>,
    pub min: Option<f64>,
    pub max: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct NodeTypeDef {
    pub id: String,
    pub category: NodeCategory,
    pub label: String,
    pub description: String,
    pub inputs: Vec<PortDef>,
    pub outputs: Vec<PortDef>,
    pub config: Vec<ConfigField>,
    /// Changes something outside the workflow (project data, a message, a web
    /// request). Test runs never execute these for real.
    pub has_side_effects: bool,
    /// Several outputs, only one of which carries data on each run.
    pub branches: bool,
}

fn port(id: &str, label: &str, port_type: PortType, required: bool) -> PortDef {
    PortDef {
        id: id.to_owned(),
        label: label.to_owned(),
        port_type,
        required,
        multiple: false,
    }
}

fn field(key: &str, label: &str, kind: ConfigFieldKind, required: bool) -> ConfigField {
    ConfigField {
        key: key.to_owned(),
        label: label.to_owned(),
        help: None,
        kind,
        required,
        default: None,
        options: Vec::new(),
        placeholder: None,
        min: None,
        max: None,
    }
}

impl ConfigField {
    fn help(mut self, help: &str) -> Self {
        self.help = Some(help.to_owned());
        self
    }
    fn default(mut self, value: Value) -> Self {
        self.default = Some(value);
        self
    }
    fn placeholder(mut self, text: &str) -> Self {
        self.placeholder = Some(text.to_owned());
        self
    }
    fn range(mut self, min: f64, max: f64) -> Self {
        self.min = Some(min);
        self.max = Some(max);
        self
    }
    fn options(mut self, options: &[(&str, &str)]) -> Self {
        self.options = options
            .iter()
            .map(|(value, label)| ConfigOption {
                value: (*value).to_owned(),
                label: (*label).to_owned(),
            })
            .collect();
        self
    }
}

fn node(
    id: &str,
    category: NodeCategory,
    label: &str,
    description: &str,
    inputs: Vec<PortDef>,
    outputs: Vec<PortDef>,
    config: Vec<ConfigField>,
) -> NodeTypeDef {
    NodeTypeDef {
        id: id.to_owned(),
        category,
        label: label.to_owned(),
        description: description.to_owned(),
        inputs,
        outputs,
        config,
        has_side_effects: false,
        branches: false,
    }
}

fn effect(mut def: NodeTypeDef) -> NodeTypeDef {
    def.has_side_effects = true;
    def
}

fn stage_field() -> ConfigField {
    field("stage", "Screening stage", ConfigFieldKind::Select, true)
        .options(&[
            ("title_abstract", "Title and abstract"),
            ("full_text", "Full text"),
        ])
        .default(json!("title_abstract"))
}

fn trigger(id: &str, label: &str, description: &str, outputs: Vec<PortDef>) -> NodeTypeDef {
    node(
        id,
        NodeCategory::Trigger,
        label,
        description,
        Vec::new(),
        outputs,
        Vec::new(),
    )
}

fn build_catalog() -> Vec<NodeTypeDef> {
    use ConfigFieldKind as K;
    use NodeCategory as C;
    use PortType as P;

    let mut multiple_input = port("inputs", "Steps to combine", P::Json, true);
    multiple_input.multiple = true;

    let mut nodes = vec![
        // Triggers -------------------------------------------------------
        trigger(
            "trigger.report_added",
            "A record is added",
            "Starts when a new record joins the project, from an import or a search.",
            vec![port("record", "New record", P::Record, false)],
        ),
        trigger(
            "trigger.acquisition_completed",
            "An import or search finishes",
            "Starts when a batch of records has been fully brought into the project.",
            vec![port("data", "Import details", P::Json, false)],
        ),
        trigger(
            "trigger.full_text_attached",
            "A full text is attached",
            "Starts when a PDF is added to a record.",
            vec![port("document", "Attached document", P::Document, false)],
        ),
        trigger(
            "trigger.report_included",
            "A record is included",
            "Starts when a record is marked as included after screening.",
            vec![port("record", "Included record", P::Record, false)],
        ),
        trigger(
            "trigger.study_created",
            "A study is created",
            "Starts when records are grouped into a new study.",
            vec![port("study", "New study", P::Study, false)],
        ),
        trigger(
            "trigger.appraisal_completed",
            "A quality appraisal is completed",
            "Starts when someone finishes appraising a record.",
            vec![port("record", "Appraised record", P::Record, false)],
        ),
        trigger(
            "trigger.screening_decision_recorded",
            "A screening decision is recorded",
            "Starts every time a reviewer, or an automation, records a screening decision.",
            vec![port("record", "Screened record", P::Record, false)],
        ),
        trigger(
            "trigger.protocol_published",
            "The protocol is published",
            "Starts when a new version of the review protocol is published.",
            vec![port("data", "Protocol details", P::Json, false)],
        ),
        trigger(
            "trigger.document_parsed",
            "A full text finishes processing",
            "Starts when a PDF has been read and its text is ready.",
            vec![port("document", "Processed document", P::Document, false)],
        ),
        trigger(
            "trigger.ai_proposal_created",
            "The AI makes a suggestion",
            "Starts when the AI prepares a suggestion that waits for your review.",
            vec![port("data", "Suggestion details", P::Json, false)],
        ),
        trigger(
            "trigger.manual",
            "Run by hand",
            "Starts when someone presses the run button.",
            vec![port("data", "Run details", P::Json, false)],
        ),
        {
            let mut def = trigger(
                "trigger.schedule",
                "On a schedule",
                "Starts every day, week or month at a time you choose.",
                vec![port("data", "Run details", P::Json, false)],
            );
            def.config = vec![
                field("schedule", "When", K::Schedule, true)
                    .default(json!({"every": "day", "at": "09:00", "timezone": "UTC"})),
            ];
            def
        },
        trigger(
            "trigger.webhook",
            "A web address is called",
            "Starts when another tool sends information to this workflow's private address.",
            vec![port("data", "Received information", P::Json, false)],
        ),
        {
            let mut def = trigger(
                "trigger.publication_alert",
                "New publications match a search",
                "Checks PubMed and Crossref on a schedule and starts when it finds publications you have not seen yet.",
                vec![port("records", "New publications", P::Records, false)],
            );
            def.config = vec![
                field("query", "Search words", K::PublicationQuery, true)
                    .help("What to look for, for example a topic or a title phrase."),
                field("sources", "Where to look", K::Select, true)
                    .options(&[
                        ("both", "PubMed and Crossref"),
                        ("pubmed", "PubMed only"),
                        ("crossref", "Crossref only"),
                    ])
                    .default(json!("both")),
                field("schedule", "How often", K::Schedule, true)
                    .default(json!({"every": "day", "at": "08:00", "timezone": "UTC"})),
            ];
            def
        },
        trigger(
            "trigger.email",
            "An e-mail arrives",
            "Starts when a message reaches this workflow's private e-mail address. DOIs and PubMed IDs in the message are picked up for you.",
            vec![
                port("data", "The message", P::Json, false),
                port("text", "Message text", P::Text, false),
            ],
        ),
        // Data -----------------------------------------------------------
        node(
            "data.find_records",
            C::Data,
            "Find records",
            "Looks up records in this project by their screening status or by words in the title or abstract.",
            vec![port("start", "Start", P::Json, true)],
            vec![port("records", "Matching records", P::Records, false)],
            vec![
                field("stage", "Look at", K::Select, false)
                    .options(&[
                        ("final", "Overall decision"),
                        ("title_abstract", "Title and abstract decision"),
                        ("full_text", "Full text decision"),
                    ])
                    .default(json!("final")),
                field("status", "Status", K::Select, false)
                    .options(&[
                        ("any", "Any"),
                        ("unscreened", "Not screened yet"),
                        ("include", "Included"),
                        ("exclude", "Excluded"),
                        ("maybe", "Maybe"),
                    ])
                    .default(json!("any")),
                field("text", "Contains words", K::Text, false)
                    .help("Matches the title or abstract."),
                field("limit", "At most", K::Number, false)
                    .default(json!(100))
                    .range(1.0, 500.0),
            ],
        ),
        node(
            "data.get_report",
            C::Data,
            "Get record details",
            "Loads the full details of a record: abstract, authors, identifiers and screening state.",
            vec![port("record", "Record", P::Record, true)],
            vec![port("report", "Details", P::Report, false)],
            Vec::new(),
        ),
        node(
            "data.get_study",
            C::Data,
            "Get the study of a record",
            "Finds the study a record belongs to. Stops this branch when the record is not in a study.",
            vec![port("record", "Record", P::Record, true)],
            vec![port("study", "Study", P::Study, false)],
            Vec::new(),
        ),
        node(
            "data.screening_queue",
            C::Data,
            "List the screening queue",
            "Lists the records still waiting to be screened.",
            vec![port("start", "Start", P::Json, true)],
            vec![port("records", "Waiting records", P::Records, false)],
            vec![
                stage_field(),
                field("limit", "At most", K::Number, false)
                    .default(json!(100))
                    .range(1.0, 500.0),
            ],
        ),
        // Actions --------------------------------------------------------
        effect(node(
            "action.import_identifiers",
            C::Action,
            "Add records from DOIs or PubMed IDs",
            "Finds DOIs and PubMed IDs in whatever arrives and adds those papers to the project.",
            vec![port("items", "Where to look", P::Json, true)],
            vec![port("records", "Added records", P::Records, false)],
            vec![field("label", "Name for this batch", K::Text, false)
                .placeholder("Added by automation")],
        )),
        effect(node(
            "action.record_decision",
            C::Action,
            "Record a screening decision",
            "Includes, excludes or marks records as maybe. How far the automation may go on its own follows the project's AI autonomy setting.",
            vec![port("records", "Records", P::Records, true)],
            vec![port("records", "Decided records", P::Records, false)],
            vec![
                stage_field(),
                field("decision", "Decision", K::Select, true).options(&[
                    ("include", "Include"),
                    ("exclude", "Exclude"),
                    ("maybe", "Maybe"),
                ]),
                field("reason", "Reason for excluding", K::Text, false)
                    .help("Name of one of the protocol's exclusion reasons. Required when excluding at full text."),
                field("note", "Note", K::LongText, false),
            ],
        )),
        effect(node(
            "action.group_into_study",
            C::Action,
            "Group records into a study",
            "Puts the records together in one study, for example several reports of the same trial.",
            vec![port("records", "Records", P::Records, true)],
            vec![port("study", "Study", P::Study, false)],
            vec![field("title", "Study name", K::Text, false)
                .help("Leave empty to use the first record's title.")],
        )),
        effect(node(
            "action.attach_pdf",
            C::Action,
            "Attach the full text",
            "Downloads the PDF of a record from a web address, or looks for a free copy.",
            vec![port("records", "Records", P::Records, true)],
            vec![port("records", "Records", P::Records, false)],
            vec![
                field("source", "Where from", K::Select, true)
                    .options(&[
                        ("open_access", "Look for a free copy"),
                        ("url", "A web address"),
                    ])
                    .default(json!("open_access")),
                field("url", "Web address", K::Text, false)
                    .help("Only for \"A web address\". You can use details from earlier steps."),
            ],
        )),
        effect(node(
            "action.run_deduplication",
            C::Action,
            "Look for duplicates",
            "Checks newly added records against the rest of the project and links exact duplicates.",
            vec![port("start", "Start", P::Json, true)],
            vec![port("summary", "What was found", P::Json, false)],
            vec![field("limit", "Check at most", K::Number, false)
                .default(json!(500))
                .range(1.0, 5000.0)],
        )),
        effect(node(
            "action.recompute_metrics",
            C::Action,
            "Refresh project statistics",
            "Recalculates the project's counts and rankings.",
            vec![port("start", "Start", P::Json, true)],
            vec![port("summary", "Result", P::Json, false)],
            Vec::new(),
        )),
        effect(node(
            "action.export",
            C::Action,
            "Export records to a file",
            "Saves the records as a file you can download from the run.",
            vec![port("records", "Records", P::Records, true)],
            vec![port("file", "File", P::Json, false)],
            vec![
                field("format", "Format", K::Select, true)
                    .options(&[("ris", "RIS (reference managers)"), ("csv", "CSV (spreadsheets)")])
                    .default(json!("ris")),
                field("name", "File name", K::Text, false).placeholder("records"),
            ],
        )),
        // AI -------------------------------------------------------------
        node(
            "ai.prompt",
            C::Ai,
            "Ask the AI",
            "Gives the AI some information and your instructions and gets back structured answers you can use in later steps.",
            vec![port("data", "Information", P::Json, true)],
            vec![port("result", "Answers", P::Json, false)],
            vec![
                field("instructions", "What should the AI do?", K::LongText, true),
                field("answers", "Answers to return", K::FieldList, true)
                    .help("Each answer has a name and a kind, such as text, number or yes/no."),
            ],
        ),
        effect(node(
            "ai.classify",
            C::Ai,
            "Screen records with the AI",
            "Asks the AI to judge records against the protocol's criteria. The result appears as a suggestion or a decision depending on the project's AI autonomy setting.",
            vec![port("records", "Records", P::Records, true)],
            vec![port("records", "Records", P::Records, false)],
            vec![stage_field()],
        )),
        effect(node(
            "ai.extract_field",
            C::Ai,
            "Extract a data field",
            "Asks the AI to read the full text and fill in one extraction field, citing the passage it used.",
            vec![port("records", "Records", P::Records, true)],
            vec![port("records", "Records", P::Records, false)],
            vec![field("field", "Field", K::ProjectField, true)],
        )),
        effect(node(
            "ai.run_review",
            C::Ai,
            "Run an AI review",
            "Runs one of the built-in AI review tasks over the records. Screening waits for the AI and sends each record out by its verdict; the other tasks run in the background.",
            vec![port("records", "Records", P::Records, true)],
            vec![
                port("records", "All records", P::Records, false),
                port("included", "AI would include", P::Records, false),
                port("excluded", "AI would exclude", P::Records, false),
                port("unsure", "Not sure", P::Records, false),
            ],
            vec![
                field("task", "Task", K::Select, true)
                    .options(&[
                        ("screening", "Screen against the protocol"),
                        ("study_grouping", "Suggest study groups"),
                        ("study_classification", "Suggest the study design"),
                        ("appraisal_prefill", "Pre-fill quality appraisal"),
                        ("data_extraction", "Extract data"),
                    ])
                    .help("Screening waits for the AI, up to 30 minutes, and sends each record out on \"AI would include\", \"AI would exclude\" or \"Not sure\". The other tasks only add their results for a person to review."),
                {
                    let mut stage = stage_field().help("Only used when screening.");
                    stage.required = false;
                    stage
                },
                field("appraisal_tool", "Appraisal tool", K::Text, false)
                    .help("Only used for quality appraisal. Leave empty for the default tool."),
            ],
        )),
        // Logic ----------------------------------------------------------
        {
            let mut def = node(
                "logic.if",
                C::Logic,
                "If … then … otherwise",
                "Sends the flow one way or the other depending on a rule you build. For a list you can test its size with \"count\".",
                vec![port("data", "Information", P::Json, true)],
                vec![
                    port("yes", "Yes", P::Json, false),
                    port("no", "No", P::Json, false),
                ],
                vec![field("condition", "Rule", K::Condition, true)],
            );
            def.branches = true;
            def
        },
        node(
            "logic.filter",
            C::Logic,
            "Keep only some records",
            "Keeps the records that match your rule and sets the others aside.",
            vec![port("records", "Records", P::Records, true)],
            vec![
                port("records", "Kept", P::Records, false),
                port("rejected", "Set aside", P::Records, false),
            ],
            vec![
                field("condition", "Rule", K::Condition, true),
                field("stop_if_empty", "Stop when nothing is kept", K::Boolean, false)
                    .default(json!(true)),
            ],
        ),
        node(
            "logic.for_each",
            C::Logic,
            "Do this for each record",
            "Repeats the steps that follow once for every record in the list.",
            vec![port("items", "Records", P::Records, true)],
            vec![port("item", "Each record", P::Record, false)],
            vec![field("max_items", "At most", K::Number, false)
                .default(json!(50))
                .range(1.0, 200.0)],
        ),
        node(
            "logic.wait",
            C::Logic,
            "Wait",
            "Pauses the flow for a while before continuing.",
            vec![port("data", "Information", P::Json, true)],
            vec![port("data", "Information", P::Json, false)],
            vec![field("duration", "How long", K::Duration, true)
                .default(json!({"value": 5, "unit": "minutes"}))],
        ),
        node(
            "logic.merge",
            C::Logic,
            "Wait for everything",
            "Waits until every connected step, or every repeat of a loop, has finished, then continues once.",
            vec![multiple_input],
            vec![port("merged", "Everything gathered", P::Json, false)],
            Vec::new(),
        ),
        // Integrations ---------------------------------------------------
        effect(node(
            "integration.http_request",
            C::Integration,
            "Call a web address",
            "Sends a request to another service and keeps its answer.",
            vec![port("data", "Information", P::Json, true)],
            vec![port("response", "Answer", P::Json, false)],
            vec![
                field("method", "Method", K::Select, true)
                    .options(&[
                        ("GET", "Read (GET)"),
                        ("POST", "Send (POST)"),
                        ("PUT", "Replace (PUT)"),
                        ("PATCH", "Update (PATCH)"),
                        ("DELETE", "Delete (DELETE)"),
                    ])
                    .default(json!("POST")),
                field("url", "Web address", K::Text, true),
                field("headers", "Extra headers", K::LongText, false)
                    .help("One per line, written as Name: value."),
                field("auth", "Authorization header value", K::Secret, false)
                    .help("Sent as the Authorization header. Never shown again after saving."),
                field("body", "Message to send", K::LongText, false),
            ],
        )),
        effect(node(
            "integration.notify",
            C::Integration,
            "Notify me in DeepRef",
            "Adds a message to your DeepRef notifications.",
            vec![port("data", "Information", P::Json, true)],
            vec![port("data", "Information", P::Json, false)],
            vec![
                field("title", "Title", K::Text, true),
                field("message", "Message", K::LongText, false),
                field("severity", "Importance", K::Select, false)
                    .options(&[
                        ("info", "Information"),
                        ("success", "Good news"),
                        ("warning", "Needs attention"),
                        ("error", "Problem"),
                    ])
                    .default(json!("info")),
            ],
        )),
        effect(node(
            "integration.email",
            C::Integration,
            "Send an e-mail",
            "Sends an e-mail. Requires the server's outgoing mail settings to be set up.",
            vec![port("data", "Information", P::Json, true)],
            vec![port("data", "Information", P::Json, false)],
            vec![
                field("to", "To", K::Text, true).help("One or more addresses separated by commas."),
                field("subject", "Subject", K::Text, true),
                field("body", "Message", K::LongText, true),
            ],
        )),
        effect(node(
            "integration.slack",
            C::Integration,
            "Post to Slack",
            "Posts a message to a Slack channel through an incoming webhook.",
            vec![port("data", "Information", P::Json, true)],
            vec![port("data", "Information", P::Json, false)],
            vec![
                field("webhook", "Slack webhook address", K::Secret, true)
                    .help("Create an incoming webhook in Slack and paste its address here."),
                field("message", "Message", K::LongText, true),
            ],
        )),
    ];
    // Triggers carry no required inputs; keep ordering stable for the UI.
    nodes.shrink_to_fit();
    nodes
}

/// All blocks, triggers first.
pub fn catalog() -> &'static [NodeTypeDef] {
    static CATALOG: OnceLock<Vec<NodeTypeDef>> = OnceLock::new();
    CATALOG.get_or_init(build_catalog)
}

pub fn node_type(id: &str) -> Option<&'static NodeTypeDef> {
    catalog().iter().find(|def| def.id == id)
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::*;

    #[test]
    fn ids_are_unique_and_every_category_is_present() {
        let mut seen = HashSet::new();
        for def in catalog() {
            assert!(seen.insert(def.id.clone()), "duplicate {}", def.id);
            let mut ports = HashSet::new();
            for port in &def.outputs {
                assert!(ports.insert(port.id.clone()), "duplicate output {}", def.id);
            }
        }
        for category in [
            NodeCategory::Trigger,
            NodeCategory::Data,
            NodeCategory::Action,
            NodeCategory::Ai,
            NodeCategory::Logic,
            NodeCategory::Integration,
        ] {
            assert!(catalog().iter().any(|def| def.category == category));
        }
    }

    #[test]
    fn triggers_have_no_inputs_and_others_have_some() {
        for def in catalog() {
            if def.category == NodeCategory::Trigger {
                assert!(def.inputs.is_empty(), "{}", def.id);
                assert!(def.id.starts_with("trigger."));
            } else {
                assert!(!def.inputs.is_empty(), "{}", def.id);
            }
        }
    }

    #[test]
    fn side_effect_blocks_are_marked() {
        assert!(node_type("action.record_decision").is_some_and(|def| def.has_side_effects));
        assert!(node_type("integration.slack").is_some_and(|def| def.has_side_effects));
        assert!(node_type("logic.filter").is_some_and(|def| !def.has_side_effects));
    }
}
