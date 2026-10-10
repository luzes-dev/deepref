//! Shared plan mechanics for the project assistant: tool declarations,
//! proposal validation, policy-checked plan actions, and the plan-consistency
//! helpers. Reads run immediately; every write-capable tool is intercepted,
//! authorized by the shared [`PolicyEngine`] and, if allowed, recorded as a
//! [`PlanAction`] instead of being executed. The turn ends with a plan that a
//! human must confirm. Tools the policy never allows (final exclusion,
//! protocol publishing) become manual steps with a link target.
//!
//! The Rig runtime owns these exact semantics; the removed custom loop once
//! shared them. Parity by construction, not by parallel implementation.

use std::collections::BTreeSet;

use deepref_domain::{Actor, ProjectId};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use uuid::Uuid;

use crate::{
    AgentToolName, AuthorityTier, PolicyDecision, PolicyEngine, PolicyInput, ProjectAiPolicy,
    RequestedAction, ToolDeclaration, assistant_tool_declarations,
};

/// One turn-record message for assistant history. Transports persist these;
/// the Rig adapter maps user/assistant text onto provider messages.
#[derive(Debug, Clone, PartialEq)]
pub enum ChatMessage {
    System(String),
    User(String),
    Assistant {
        content: String,
        tool_calls: Vec<ChatToolCall>,
    },
    Tool {
        tool_call_id: String,
        content: String,
    },
}

/// One model-emitted tool call with its arguments.
#[derive(Debug, Clone, PartialEq)]
pub struct ChatToolCall {
    pub id: String,
    pub name: String,
    pub arguments: Value,
}

pub const ASSISTANT_PROMPT_VERSION: &str = "assistant.agent.v2";

/// Write tool that records title/abstract decisions for several reports.
pub const TOOL_SCREEN_REPORTS: &str = "screen_reports";
/// Never executable by the assistant; surfaced as a manual step.
pub const TOOL_FINAL_EXCLUSION: &str = "request_final_exclusion";
pub const TOOL_PUBLISH_PROTOCOL: &str = "request_protocol_publish";
pub const TOOL_PROJECT_OVERVIEW: &str = "get_project_overview";
pub const TOOL_LIST_REPORTS: &str = "list_reports_by_screening_status";

const MAX_TOOL_OUTPUT_CHARS: usize = 12_000;
pub(crate) const MAX_ACTIONS_PER_PLAN: usize = 25;
pub const MAX_REPORTS_PER_ACTION: usize = 200;

#[derive(Debug, Clone, Copy)]
pub struct AgentLoopConfig {
    pub max_steps: usize,
    /// Hard cap on input+output tokens spent by one turn.
    pub max_total_tokens: u64,
    pub max_output_tokens_per_call: u32,
}

impl Default for AgentLoopConfig {
    fn default() -> Self {
        Self {
            max_steps: 8,
            max_total_tokens: 60_000,
            max_output_tokens_per_call: 2_048,
        }
    }
}

/// Where the UI should send the user for a step the assistant may not perform.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ManualStep {
    pub reason: String,
    /// `protocol` or `full_text_screening`; the web app maps it to a route.
    pub link_target: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PlanAction {
    pub id: String,
    pub tool: String,
    pub summary: String,
    pub rationale: String,
    pub affected_count: u32,
    pub args: Value,
    /// `false` when the policy forbids the assistant from ever running it.
    pub executable: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub manual: Option<ManualStep>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ToolTraceEntry {
    pub call: ChatToolCall,
    pub output: Value,
}
pub fn is_read_tool(name: &str) -> bool {
    name == TOOL_PROJECT_OVERVIEW
        || name == TOOL_LIST_REPORTS
        || AgentToolName::parse(name).is_some_and(AgentToolName::is_read)
}

pub(crate) fn is_write_tool(name: &str) -> bool {
    name == TOOL_SCREEN_REPORTS
        || name == TOOL_FINAL_EXCLUSION
        || name == TOOL_PUBLISH_PROTOCOL
        || AgentToolName::parse(name).is_some_and(AgentToolName::is_proposal)
}
fn strip_project_id(mut declaration: ToolDeclaration) -> ToolDeclaration {
    if let Some(properties) = declaration
        .parameters
        .get_mut("properties")
        .and_then(Value::as_object_mut)
    {
        properties.remove("project_id");
    }
    if let Some(required) = declaration
        .parameters
        .get_mut("required")
        .and_then(Value::as_array_mut)
    {
        required.retain(|name| name != "project_id");
    }
    declaration
}

fn as_write_tool(mut declaration: ToolDeclaration, kind: &str) -> ToolDeclaration {
    declaration.description = format!(
        "{} This does NOT run immediately: it is added to a plan the user must confirm. {kind}",
        declaration.description
    );
    if let Some(properties) = declaration
        .parameters
        .get_mut("properties")
        .and_then(Value::as_object_mut)
    {
        properties.insert(
            "summary".to_owned(),
            json!({"type": "string", "description": "One plain-language sentence saying what will be done, e.g. 'Exclude 14 records about animal studies'."}),
        );
        properties.insert(
            "rationale".to_owned(),
            json!({"type": "string", "description": "Why, in one or two sentences, grounded in the protocol or the data you read."}),
        );
    }
    if let Some(required) = declaration
        .parameters
        .get_mut("required")
        .and_then(Value::as_array_mut)
    {
        required.push(json!("summary"));
        required.push(json!("rationale"));
    }
    declaration
}

pub fn agent_tool_declarations() -> Vec<ToolDeclaration> {
    let mut tools = Vec::new();
    for declaration in assistant_tool_declarations() {
        let Some(name) = AgentToolName::parse(&declaration.name) else {
            continue; // trigger_workflow is intentionally not offered to the model
        };
        let declaration = strip_project_id(declaration);
        tools.push(if name.is_read() {
            declaration
        } else {
            as_write_tool(declaration, "")
        });
    }
    tools.push(ToolDeclaration {
        name: TOOL_PROJECT_OVERVIEW.to_owned(),
        description: "Counts of reports, studies and title/abstract screening status in this project. Call this first when the user asks about progress or what to do next.".to_owned(),
        parameters: json!({"type": "object", "properties": {}}),
    });
    tools.push(ToolDeclaration {
        name: TOOL_LIST_REPORTS.to_owned(),
        description: "List reports with their title/abstract screening status, optionally filtered. Returns report ids and titles (max 50).".to_owned(),
        parameters: json!({
            "type": "object",
            "properties": {
                "status": {"type": "string", "enum": ["unscreened", "include", "exclude", "maybe"]},
                "limit": {"type": "integer", "minimum": 1, "maximum": 50}
            }
        }),
    });
    tools.push(as_write_tool(
        ToolDeclaration {
            name: TOOL_SCREEN_REPORTS.to_owned(),
            description: "Record title/abstract screening decisions for specific reports on behalf of the user.".to_owned(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "report_ids": {"type": "array", "items": {"type": "string", "format": "uuid"}, "minItems": 1, "maxItems": MAX_REPORTS_PER_ACTION},
                    "decision": {"type": "string", "enum": ["include", "exclude", "maybe"]},
                    "notes": {"type": "string", "description": "Optional note stored with each decision."}
                },
                "required": ["report_ids", "decision"]
            }),
        },
        "Only for the title/abstract stage.",
    ));
    tools.push(as_write_tool(
        ToolDeclaration {
            name: TOOL_FINAL_EXCLUSION.to_owned(),
            description: "Use when the user wants a study finally excluded (full-text exclusion). You may NOT do this; it tells the user to do it themselves.".to_owned(),
            parameters: json!({
                "type": "object",
                "properties": {"report_ids": {"type": "array", "items": {"type": "string", "format": "uuid"}}},
                "required": ["report_ids"]
            }),
        },
        "",
    ));
    tools.push(as_write_tool(
        ToolDeclaration {
            name: TOOL_PUBLISH_PROTOCOL.to_owned(),
            description: "Use when the user wants the protocol published or amended. You may NOT do this; it tells the user to do it themselves.".to_owned(),
            parameters: json!({"type": "object", "properties": {}}),
        },
        "",
    ));
    tools
}

pub fn assistant_system_prompt(project_id: ProjectId) -> String {
    format!(
        "You are DeepRef's assistant for a systematic literature review. Project id: {project_id}.\n\
         \n\
         Rules:\n\
         - Answer from data you read with tools; never invent report ids, titles, documents or counts.\n\
         - Read tools run immediately. Use them freely but economically.\n\
         - You cannot change anything directly. Every change tool only adds an item to a plan that the user must confirm. A change tool result with status queued_in_plan means the item is in the plan; any other result means it is not.\n\
         - Only say that something was queued, planned or is waiting for confirmation when a change tool returned queued_in_plan in this turn. If no change tool was called, say plainly that nothing was queued and nothing has changed. After queuing changes, say what the plan contains and that nothing has been changed yet.\n\
         - Final exclusion of studies and publishing/amending the protocol are never yours to do. If asked, call the matching request tool so the user is pointed to the right screen.\n\
         - Prefer one batched action (screen_reports with many report ids) over many single ones. Give each change a clear summary and rationale.\n\
         - Counts: use get_project_overview as it is. The title_abstract buckets add up to the report total. full_text.awaiting_decision is a part of title_abstract.include, not an extra bucket.\n\
         - Full text: a report has full text only when get_report lists a document with status available. A report without such a document has no full text, even when it is included.\n\
         - Searching: search_project_reports matches any of the words given and ranks the best matches first. Pass key terms, not a whole sentence. If nothing matches, try other key terms before saying that no record mentions it.\n\
         - Studies: use list_studies to find a study id. Never pass the project id where a study or report id is expected.\n\
         - Language: reply in the same language as the user's latest message, even when the protocol or the records are in another language. Quote records in their original language and explain in the user's language.\n\
         - Be concise and plain; the user is a researcher, not an engineer. Do not mention tool names, JSON or ids unless useful.\n\
         - If a tool returns an error, adapt or explain; do not retry the same call.",
        project_id = project_id.as_uuid()
    )
}

/// Positive phrases that tell the user something was queued, planned or is
/// waiting for their confirmation. Matched per clause against the lower-cased
/// reply, in English and Portuguese, and only when the turn created no plan.
pub(crate) const PENDING_CHANGE_CLAIMS: &[&str] = &[
    "i've queued",
    "i have queued",
    "queued a plan",
    "queued the",
    "queued this",
    "queued it",
    "queued these",
    "queued for",
    "queued in",
    "is queued",
    "are queued",
    "in the plan",
    "to the plan",
    "the plan below",
    "prepared a plan",
    "i've prepared",
    "i have prepared",
    "confirm to apply",
    "once you confirm",
    "after you confirm",
    "when you confirm",
    "waiting for your confirmation",
    "awaiting your confirmation",
    "waiting for your ok",
    "will only run once",
    "will only run after",
    "na fila",
    "enfileir",
    "ao plano",
    "plano pendente",
    "preparei um plano",
    "preparei o plano",
    "aguardando sua confirma",
    "aguardando a sua confirma",
    "aguardando confirma",
    "quando voc\u{ea} confirmar",
    "depois que voc\u{ea} confirmar",
    "confirme para aplicar",
];

/// Words that turn a clause into a denial, such as "nothing has been queued".
pub(crate) const NEGATIONS: &[&str] = &[
    "nothing", " not ", "n't", " no ", "never", "none", "nada", "n\u{e3}o", "nenhum",
];

pub(crate) const NO_PLAN_CORRECTION: &str = "[System correction] Your last reply says something was queued, planned or is waiting for confirmation, but no change tool was called in this turn, so nothing is in a plan. Do not say that anything was queued. If the user wants a change, call the right change tool now. Otherwise reply again in the user's language and say plainly that nothing has been changed.";

/// True when a reply that created no plan nevertheless tells the user that
/// something was queued, planned or is waiting for their confirmation. A
/// clause that denies it ("nothing has been queued") is not a claim.
pub fn claims_pending_change(reply: &str) -> bool {
    reply
        .to_lowercase()
        .split(['.', '!', '?', ';', ':', '\n', '\u{2014}', '\u{2013}'])
        .any(|clause| {
            let padded = format!(" {clause} ");
            PENDING_CHANGE_CLAIMS
                .iter()
                .any(|claim| padded.contains(claim))
                && !NEGATIONS.iter().any(|word| padded.contains(word))
        })
}

/// The correction appended to a reply that still claims a change after the
/// one allowed re-prompt. Written in the language the user is using.
pub fn no_plan_notice(user_message: &str) -> &'static str {
    if looks_portuguese(user_message) {
        "Nada foi enfileirado: nenhuma altera\u{e7}\u{e3}o foi colocada em um plano, ent\u{e3}o nada aguarda a sua confirma\u{e7}\u{e3}o."
    } else {
        "Nothing was queued: no change was added to a plan, so nothing is waiting for your confirmation."
    }
}

fn looks_portuguese(text: &str) -> bool {
    const PORTUGUESE: &[&str] = &[
        " n\u{e3}o ",
        " que ",
        " para ",
        " uma ",
        " voc\u{ea}",
        " s\u{e3}o ",
        " dos ",
        " das ",
        " com ",
        "\u{e7}\u{e3}o",
    ];
    const ENGLISH: &[&str] = &[
        " the ", " and ", " what ", " which ", " how ", " is ", " are ",
    ];
    let padded = format!(" {} ", text.to_lowercase());
    let score = |markers: &[&str]| markers.iter().filter(|m| padded.contains(*m)).count();
    score(PORTUGUESE) > score(ENGLISH)
}
pub(crate) fn truncate_for_model(value: &Value) -> String {
    let text = value.to_string();
    if text.chars().count() <= MAX_TOOL_OUTPUT_CHARS {
        return text;
    }
    let cut: String = text.chars().take(MAX_TOOL_OUTPUT_CHARS).collect();
    format!("{cut}... [truncated]")
}

fn take_text(args: &mut Value, key: &str) -> String {
    args.as_object_mut()
        .and_then(|object| object.remove(key))
        .and_then(|value| value.as_str().map(|text| text.trim().to_owned()))
        .unwrap_or_default()
}

fn manual_for(tool: &str) -> Option<ManualStep> {
    match tool {
        TOOL_FINAL_EXCLUSION => Some(ManualStep {
            reason: "Final exclusion of a study must be recorded by you, with a reason, during full-text screening.".to_owned(),
            link_target: "full_text_screening".to_owned(),
        }),
        TOOL_PUBLISH_PROTOCOL => Some(ManualStep {
            reason: "Publishing or amending the protocol must be done by you in the protocol editor.".to_owned(),
            link_target: "protocol".to_owned(),
        }),
        _ => None,
    }
}

fn affected_count(tool: &str, args: &Value) -> u32 {
    match tool {
        TOOL_SCREEN_REPORTS | TOOL_FINAL_EXCLUSION => args
            .get("report_ids")
            .and_then(Value::as_array)
            .map_or(1, |ids| u32::try_from(ids.len()).unwrap_or(u32::MAX)),
        _ => 1,
    }
}

/// Re-validates a persisted plan action against the current policy. Used when
/// a plan is confirmed, so a tampered or stale row can never execute.
pub fn plan_action_is_executable(
    project_id: ProjectId,
    actor: &Actor,
    action: &PlanAction,
) -> bool {
    if !action.executable || action.manual.is_some() {
        return false;
    }
    let mut arguments = action.args.clone();
    if let Some(object) = arguments.as_object_mut() {
        object.insert("summary".to_owned(), json!(action.summary));
    }
    let call = ChatToolCall {
        id: action.id.clone(),
        name: action.tool.clone(),
        arguments,
    };
    plan_write_call(project_id, actor, &call, 0).is_ok_and(|checked| checked.executable)
}

/// Validates and policy-checks one write tool call. Returns the action to add
/// to the plan, or a message for the model when the call is rejected.
pub(crate) fn plan_write_call(
    project_id: ProjectId,
    actor: &Actor,
    call: &ChatToolCall,
    index: usize,
) -> Result<PlanAction, String> {
    let mut args = call.arguments.clone();
    if !args.is_object() {
        return Err("arguments must be a JSON object".to_owned());
    }
    let summary = take_text(&mut args, "summary");
    let rationale = take_text(&mut args, "rationale");
    if summary.is_empty() {
        return Err("a plain-language `summary` is required".to_owned());
    }
    if let Some(object) = args.as_object_mut() {
        object.insert("project_id".to_owned(), json!(project_id.as_uuid()));
    }

    let (action, authority) = match call.name.as_str() {
        TOOL_FINAL_EXCLUSION => (
            RequestedAction::FinalExclusion,
            AuthorityTier::ScientificConclusion,
        ),
        TOOL_PUBLISH_PROTOCOL => (
            RequestedAction::ArbitrarySql,
            AuthorityTier::ScientificConclusion,
        ),
        TOOL_SCREEN_REPORTS => (
            RequestedAction::ScientificConclusion,
            AuthorityTier::ScientificConclusion,
        ),
        other => {
            let name = AgentToolName::parse(other).ok_or("unknown tool")?;
            let metadata = name.policy();
            (metadata.action, metadata.authority)
        }
    };

    if call.name == TOOL_SCREEN_REPORTS {
        validate_screen_reports(&args)?;
    } else if let Some(name) = AgentToolName::parse(&call.name) {
        crate::AgentTool::from_name_and_args(name, args.clone())
            .map_err(|error| error.to_owned())?
            .validate()
            .map_err(|error| error.to_string())?;
    }

    let decision = PolicyEngine.authorize(&PolicyInput {
        actor: actor.clone(),
        project_id,
        declared_project_id: project_id,
        tool: call.name.clone(),
        action,
        authority,
        args: args.clone(),
        project_policy: ProjectAiPolicy::default(),
    });
    let executable = decision == PolicyDecision::CreateProposal;
    let manual = if executable {
        None
    } else {
        manual_for(&call.name)
    };
    if !executable && manual.is_none() {
        return Err("the project policy does not allow this action".to_owned());
    }
    Ok(PlanAction {
        id: format!("a{}", index + 1),
        tool: call.name.clone(),
        affected_count: affected_count(&call.name, &args),
        summary,
        rationale,
        args,
        executable,
        manual,
    })
}

/// Shared with the executor so plan creation and confirmation agree.
pub fn validate_screen_reports(args: &Value) -> Result<(), String> {
    let ids = args
        .get("report_ids")
        .and_then(Value::as_array)
        .ok_or("`report_ids` must be an array")?;
    if ids.is_empty() || ids.len() > MAX_REPORTS_PER_ACTION {
        return Err(format!(
            "`report_ids` must hold 1 to {MAX_REPORTS_PER_ACTION} ids"
        ));
    }
    let mut seen = BTreeSet::new();
    for id in ids {
        let parsed = id
            .as_str()
            .and_then(|text| Uuid::parse_str(text).ok())
            .filter(|uuid| !uuid.is_nil())
            .ok_or("`report_ids` must contain report UUIDs from tool results")?;
        seen.insert(parsed);
    }
    if seen.len() != ids.len() {
        return Err("`report_ids` contains duplicates".to_owned());
    }
    if !matches!(
        args.get("decision").and_then(Value::as_str),
        Some("include" | "exclude" | "maybe")
    ) {
        return Err("`decision` must be include, exclude or maybe".to_owned());
    }
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn model_never_sees_workflow_trigger_or_project_id() {
        let tools = agent_tool_declarations();
        assert!(tools.iter().all(|tool| tool.name != "trigger_workflow"));
        assert!(
            tools
                .iter()
                .all(|tool| !tool.parameters.to_string().contains("project_id"))
        );
    }
    #[test]
    fn claim_detection_reads_english_and_portuguese_only_for_real_claims() {
        assert!(claims_pending_change("I've queued a plan to propose that."));
        assert!(claims_pending_change(
            "Nothing has changed yet: this is in the plan."
        ));
        assert!(claims_pending_change(
            "Vou colocar na fila; confirme para aplicar."
        ));
        assert!(claims_pending_change(
            "Preparei um plano. Nada foi alterado ainda."
        ));
        assert!(!claims_pending_change("There are 50 unscreened records."));
        assert!(!claims_pending_change(
            "H\u{e1} 50 registros sem triagem ainda."
        ));
        assert!(!claims_pending_change(
            "The protocol plan has four criteria."
        ));
        // Denials are honest replies, not claims.
        assert!(!claims_pending_change(
            "I could not find a change to make, so nothing has been queued."
        ));
        assert!(!claims_pending_change("Nada foi enfileirado."));
    }

    #[test]
    fn correction_notice_follows_the_user_language() {
        assert!(
            no_plan_notice("Quantos registros ainda n\u{e3}o foram triados?")
                .starts_with("Nada foi enfileirado")
        );
        assert!(
            no_plan_notice("How many records are left to screen?")
                .starts_with("Nothing was queued")
        );
    }
}
