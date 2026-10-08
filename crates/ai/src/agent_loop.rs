//! Bounded tool-calling loop for the project assistant.
//!
//! Read tools run immediately. Every write-capable tool is intercepted: it is
//! authorized by the shared [`PolicyEngine`] and, if allowed, recorded as a
//! [`PlanAction`] instead of being executed. The turn ends with a plan that a
//! human must confirm. Tools the policy never allows (final exclusion,
//! protocol publishing) become manual steps with a link target.

use std::collections::BTreeSet;

use deepref_domain::{Actor, ProjectId};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use uuid::Uuid;

use crate::{
    AgentToolName, AiError, AiFuture, AuthorityTier, ChatGateway, ChatMessage, ChatRequest,
    ChatToolCall, PolicyDecision, PolicyEngine, PolicyInput, ProjectAiPolicy, RequestedAction,
    ResolvedModel, ToolDeclaration, assistant_tool_declarations,
};

pub const ASSISTANT_PROMPT_VERSION: &str = "assistant.agent.v2";

/// Write tool that records title/abstract decisions for several reports.
pub const TOOL_SCREEN_REPORTS: &str = "screen_reports";
/// Never executable by the assistant; surfaced as a manual step.
pub const TOOL_FINAL_EXCLUSION: &str = "request_final_exclusion";
pub const TOOL_PUBLISH_PROTOCOL: &str = "request_protocol_publish";
pub const TOOL_PROJECT_OVERVIEW: &str = "get_project_overview";
pub const TOOL_LIST_REPORTS: &str = "list_reports_by_screening_status";

const MAX_TOOL_OUTPUT_CHARS: usize = 12_000;
const MAX_ACTIONS_PER_PLAN: usize = 25;
pub const MAX_REPORTS_PER_ACTION: usize = 200;

/// Positive phrases that tell the user something was queued, planned or is
/// waiting for their confirmation. Matched per clause against the lower-cased
/// reply, in English and Portuguese, and only when the turn created no plan.
const PENDING_CHANGE_CLAIMS: &[&str] = &[
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
const NEGATIONS: &[&str] = &[
    "nothing", " not ", "n't", " no ", "never", "none", "nada", "n\u{e3}o", "nenhum",
];

const NO_PLAN_CORRECTION: &str = "[System correction] Your last reply says something was queued, planned or is waiting for confirmation, but no change tool was called in this turn, so nothing is in a plan. Do not say that anything was queued. If the user wants a change, call the right change tool now. Otherwise reply again in the user's language and say plainly that nothing has been changed.";

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

#[derive(Debug, Clone, PartialEq)]
pub struct AgentTurnOutcome {
    pub reply: String,
    pub actions: Vec<PlanAction>,
    pub trace: Vec<ToolTraceEntry>,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub steps: usize,
    /// The loop stopped on a step or token cap rather than a final answer.
    pub truncated: bool,
}

/// Executes read tools. Implementations must enforce project scope.
pub trait AgentReadTools: Send + Sync {
    fn read<'a>(&'a self, tool: &'a str, args: Value) -> AiFuture<'a, Value>;
}

pub fn is_read_tool(name: &str) -> bool {
    name == TOOL_PROJECT_OVERVIEW
        || name == TOOL_LIST_REPORTS
        || AgentToolName::parse(name).is_some_and(AgentToolName::is_read)
}

fn is_write_tool(name: &str) -> bool {
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

fn truncate_for_model(value: &Value) -> String {
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
fn plan_write_call(
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

fn error_output(message: &str) -> Value {
    json!({"error": message})
}

/// Live progress of one turn, for a transport that shows it while the turn
/// runs. Every method defaults to doing nothing, so an observer implements only
/// what it shows.
pub trait AgentProgress: Send {
    /// Answer text as the model generates it. Text of a step that ends in tool
    /// calls is reported as well, so the transport must clear it when a tool
    /// starts; the final answer is the text after the last clear.
    fn text_delta(&mut self, _delta: &str) {}
    /// The text reported so far is not part of the answer: the loop discarded
    /// that step and is asking the model again.
    fn text_discarded(&mut self) {}
    /// A tool call is about to run, before it has any effect.
    fn tool_started(&mut self, _call: &ChatToolCall) {}
}

pub struct AgentTurn<'a> {
    pub chat: &'a dyn ChatGateway,
    pub route: ResolvedModel,
    pub project_id: ProjectId,
    pub actor: Actor,
    pub history: Vec<ChatMessage>,
    pub user_message: String,
    pub reads: &'a dyn AgentReadTools,
    pub config: AgentLoopConfig,
    /// Receives progress as the turn runs. `None` keeps the turn silent.
    pub progress: Option<&'a mut (dyn AgentProgress + Send)>,
}

/// Runs the loop. `on_tool` is called after every executed or queued tool call
/// so the transport can stream progress.
pub async fn run_agent_loop(
    mut turn: AgentTurn<'_>,
    mut on_tool: impl FnMut(&ToolTraceEntry) + Send,
) -> Result<AgentTurnOutcome, AiError> {
    let tools = agent_tool_declarations();
    let notice = no_plan_notice(&turn.user_message);
    let mut messages = vec![ChatMessage::System(assistant_system_prompt(
        turn.project_id,
    ))];
    messages.extend(turn.history);
    messages.push(ChatMessage::User(turn.user_message));

    let mut actions: Vec<PlanAction> = Vec::new();
    let mut trace = Vec::new();
    let (mut input_tokens, mut output_tokens) = (0_u64, 0_u64);
    let mut reply = String::new();
    let mut truncated = true;
    let mut steps = 0;
    // A reply that claims a queued change without any plan action gets one
    // corrective re-prompt; if the claim survives it, the reply is annotated.
    let mut claim_retried = false;

    while steps < turn.config.max_steps {
        if input_tokens + output_tokens >= turn.config.max_total_tokens {
            break;
        }
        steps += 1;
        let mut on_text = |delta: &str| {
            if let Some(progress) = turn.progress.as_mut() {
                progress.text_delta(delta);
            }
        };
        let completion = turn
            .chat
            .chat_streaming(
                ChatRequest {
                    project_id: Some(turn.project_id),
                    route: turn.route.clone(),
                    messages: messages.clone(),
                    tools: tools.clone(),
                    max_output_tokens: Some(turn.config.max_output_tokens_per_call),
                },
                &mut on_text,
            )
            .await?;
        input_tokens += completion.input_tokens;
        output_tokens += completion.output_tokens;

        if completion.tool_calls.is_empty() {
            if actions.is_empty() && !claim_retried && claims_pending_change(&completion.content) {
                claim_retried = true;
                if let Some(progress) = turn.progress.as_mut() {
                    progress.text_discarded();
                }
                messages.push(ChatMessage::Assistant {
                    content: completion.content,
                    tool_calls: Vec::new(),
                });
                messages.push(ChatMessage::User(NO_PLAN_CORRECTION.to_owned()));
                continue;
            }
            reply = completion.content;
            truncated = false;
            break;
        }
        messages.push(ChatMessage::Assistant {
            content: completion.content.clone(),
            tool_calls: completion.tool_calls.clone(),
        });
        for call in completion.tool_calls {
            if let Some(progress) = turn.progress.as_mut() {
                progress.tool_started(&call);
            }
            let output = if is_read_tool(&call.name) {
                let mut args = call.arguments.clone();
                if let Some(object) = args.as_object_mut() {
                    object.insert("project_id".to_owned(), json!(turn.project_id.as_uuid()));
                    args = Value::Object(object.clone());
                } else if args.is_null() {
                    args = json!({"project_id": turn.project_id.as_uuid()});
                }
                match turn.reads.read(&call.name, args).await {
                    Ok(value) => value,
                    Err(AiError::BudgetExceeded) => return Err(AiError::BudgetExceeded),
                    Err(error) => error_output(&match error {
                        AiError::InvalidContext(message) => message,
                        _ => "the tool failed".to_owned(),
                    }),
                }
            } else if is_write_tool(&call.name) {
                if actions.len() >= MAX_ACTIONS_PER_PLAN {
                    error_output("the plan is full; ask the user to confirm it before adding more")
                } else {
                    match plan_write_call(turn.project_id, &turn.actor, &call, actions.len()) {
                        Ok(action) => {
                            let output = if action.executable {
                                json!({"status": "queued_in_plan", "note": "Not executed. It only runs if the user confirms the plan."})
                            } else {
                                json!({"status": "user_must_do_this", "note": "You may not do this. The user is shown a link to do it themselves."})
                            };
                            actions.push(action);
                            output
                        }
                        Err(message) => error_output(&message),
                    }
                }
            } else {
                error_output("unknown tool")
            };
            let entry = ToolTraceEntry { call, output };
            on_tool(&entry);
            messages.push(ChatMessage::Tool {
                tool_call_id: entry.call.id.clone(),
                content: truncate_for_model(&entry.output),
            });
            trace.push(entry);
        }
    }

    if truncated {
        reply = if actions.is_empty() {
            "I ran out of room to finish this in one go. Try a narrower question.".to_owned()
        } else {
            "I reached my working limit, so the plan below may be incomplete. Review it, then ask me to continue.".to_owned()
        };
    } else if reply.trim().is_empty() {
        reply = if actions.is_empty() {
            "I have nothing to add.".to_owned()
        } else {
            "I prepared a plan. Nothing has been changed yet.".to_owned()
        };
    } else if actions.is_empty() && claim_retried && claims_pending_change(&reply) {
        reply = format!("{}\n\n{notice}", reply.trim_end());
    }
    Ok(AgentTurnOutcome {
        reply,
        actions,
        trace,
        input_tokens,
        output_tokens,
        steps,
        truncated,
    })
}

#[cfg(test)]
mod tests {
    use std::sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    };

    use deepref_domain::ActorKind;

    use super::*;
    use crate::{ChatCompletion, ChatTextSink, ModelParameters, ModelProfile};

    struct Scripted {
        replies: Mutex<Vec<ChatCompletion>>,
        calls: AtomicUsize,
    }
    impl ChatGateway for Scripted {
        fn chat<'a>(&'a self, _request: ChatRequest) -> AiFuture<'a, ChatCompletion> {
            Box::pin(async move {
                self.calls.fetch_add(1, Ordering::SeqCst);
                let mut replies = self
                    .replies
                    .lock()
                    .map_err(|_| AiError::Gateway(String::new()))?;
                if replies.is_empty() {
                    return Err(AiError::Gateway("script exhausted".to_owned()));
                }
                Ok(replies.remove(0))
            })
        }
    }

    #[derive(Default)]
    struct Reads(Arc<Mutex<Vec<String>>>);
    impl AgentReadTools for Reads {
        fn read<'a>(&'a self, tool: &'a str, _args: Value) -> AiFuture<'a, Value> {
            self.0.lock().map(|mut log| log.push(tool.to_owned())).ok();
            Box::pin(async { Ok(json!({"reports": 3})) })
        }
    }

    fn reply(content: &str, calls: Vec<ChatToolCall>) -> ChatCompletion {
        ChatCompletion {
            content: content.to_owned(),
            tool_calls: calls,
            input_tokens: 100,
            output_tokens: 10,
            cost_micros: None,
        }
    }
    fn call(id: &str, name: &str, arguments: Value) -> ChatToolCall {
        ChatToolCall {
            id: id.to_owned(),
            name: name.to_owned(),
            arguments,
        }
    }
    fn route() -> ResolvedModel {
        ResolvedModel {
            profile: ModelProfile::Reasoning,
            provider: "opencode-go".to_owned(),
            model: "glm-5.3-flash".to_owned(),
            model_version: "glm-5.3-flash".to_owned(),
            parameters: ModelParameters::default(),
            route_id: None,
        }
    }
    async fn run(script: Vec<ChatCompletion>, config: AgentLoopConfig) -> AgentTurnOutcome {
        let chat = Scripted {
            replies: Mutex::new(script),
            calls: AtomicUsize::new(0),
        };
        let reads = Reads::default();
        let outcome = run_agent_loop(
            AgentTurn {
                chat: &chat,
                route: route(),
                project_id: ProjectId::new(Uuid::from_u128(7)),
                actor: Actor::new(ActorKind::User, "tester").unwrap_or_else(|_| unreachable!()),
                history: Vec::new(),
                user_message: "exclude the animal studies".to_owned(),
                reads: &reads,
                config,
                progress: None,
            },
            |_| {},
        )
        .await;
        outcome.unwrap_or_else(|_| unreachable!())
    }

    #[tokio::test]
    async fn reads_run_and_writes_become_a_plan() {
        let ids = json!([Uuid::from_u128(1), Uuid::from_u128(2)]);
        let outcome = run(
            vec![
                reply("", vec![call("1", TOOL_PROJECT_OVERVIEW, json!({}))]),
                reply(
                    "",
                    vec![
                        call("2", TOOL_SCREEN_REPORTS, json!({"report_ids": ids, "decision": "exclude", "summary": "Exclude 2 animal studies", "rationale": "Protocol requires humans"})),
                        call("3", TOOL_PUBLISH_PROTOCOL, json!({"summary": "Publish the protocol", "rationale": "x"})),
                    ],
                ),
                reply("I prepared a plan.", vec![]),
            ],
            AgentLoopConfig::default(),
        )
        .await;
        assert_eq!(outcome.reply, "I prepared a plan.");
        assert_eq!(outcome.actions.len(), 2);
        assert!(outcome.actions[0].executable);
        assert_eq!(outcome.actions[0].affected_count, 2);
        assert!(!outcome.actions[1].executable);
        assert_eq!(
            outcome.actions[1]
                .manual
                .as_ref()
                .map(|m| m.link_target.as_str()),
            Some("protocol")
        );
        assert!(!outcome.truncated);
    }

    #[tokio::test]
    async fn invalid_write_is_rejected_back_to_the_model() {
        let outcome = run(
            vec![
                reply("", vec![call("1", TOOL_SCREEN_REPORTS, json!({"report_ids": ["nope"], "decision": "exclude", "summary": "s", "rationale": "r"}))]),
                reply("Sorry.", vec![]),
            ],
            AgentLoopConfig::default(),
        )
        .await;
        assert!(outcome.actions.is_empty());
        assert!(outcome.trace[0].output.get("error").is_some());
    }

    #[tokio::test]
    async fn step_cap_bounds_the_loop() {
        let looping = (0..20)
            .map(|index| {
                reply(
                    "",
                    vec![call(&index.to_string(), TOOL_PROJECT_OVERVIEW, json!({}))],
                )
            })
            .collect();
        let outcome = run(
            looping,
            AgentLoopConfig {
                max_steps: 3,
                ..AgentLoopConfig::default()
            },
        )
        .await;
        assert_eq!(outcome.steps, 3);
        assert!(outcome.truncated);
    }

    #[tokio::test]
    async fn token_cap_bounds_the_loop() {
        let looping = (0..20)
            .map(|index| {
                reply(
                    "",
                    vec![call(&index.to_string(), TOOL_PROJECT_OVERVIEW, json!({}))],
                )
            })
            .collect();
        let outcome = run(
            looping,
            AgentLoopConfig {
                max_total_tokens: 250,
                ..AgentLoopConfig::default()
            },
        )
        .await;
        assert_eq!(outcome.steps, 3);
        assert!(outcome.truncated);
    }

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

    async fn run_message(script: Vec<ChatCompletion>, message: &str) -> (AgentTurnOutcome, usize) {
        let chat = Scripted {
            replies: Mutex::new(script),
            calls: AtomicUsize::new(0),
        };
        let reads = Reads::default();
        let outcome = run_agent_loop(
            AgentTurn {
                chat: &chat,
                route: route(),
                project_id: ProjectId::new(Uuid::from_u128(7)),
                actor: Actor::new(ActorKind::User, "tester").unwrap_or_else(|_| unreachable!()),
                history: Vec::new(),
                user_message: message.to_owned(),
                reads: &reads,
                config: AgentLoopConfig::default(),
                progress: None,
            },
            |_| {},
        )
        .await
        .unwrap_or_else(|_| unreachable!());
        (outcome, chat.calls.load(Ordering::SeqCst))
    }

    #[tokio::test]
    async fn false_queue_claim_gets_one_corrective_reprompt() {
        let (outcome, calls) = run_message(
            vec![
                reply(
                    "I've queued a plan to propose the sample size. Confirm to apply it.",
                    vec![],
                ),
                reply(
                    "I could not find a change to make, so nothing has been queued.",
                    vec![],
                ),
            ],
            "propose the sample size extraction",
        )
        .await;
        assert_eq!(calls, 2, "exactly one corrective re-prompt");
        assert!(outcome.actions.is_empty());
        assert_eq!(
            outcome.reply,
            "I could not find a change to make, so nothing has been queued."
        );
    }

    #[tokio::test]
    async fn claim_that_survives_the_reprompt_is_annotated() {
        let (outcome, calls) = run_message(
            vec![
                reply("I've queued the exclusion; confirm to apply.", vec![]),
                reply("The exclusion is queued in the plan.", vec![]),
            ],
            "exclude it",
        )
        .await;
        assert_eq!(calls, 2);
        assert!(outcome.actions.is_empty());
        assert!(
            outcome
                .reply
                .starts_with("The exclusion is queued in the plan.")
        );
        assert!(outcome.reply.ends_with(
            "Nothing was queued: no change was added to a plan, so nothing is waiting for your confirmation."
        ));
    }

    #[tokio::test]
    async fn real_plan_is_not_treated_as_a_false_claim() {
        let ids = json!([Uuid::from_u128(1)]);
        let (outcome, calls) = run_message(
            vec![
                reply(
                    "",
                    vec![call("1", TOOL_SCREEN_REPORTS, json!({"report_ids": ids, "decision": "exclude", "summary": "Exclude 1 record", "rationale": "off topic"}))],
                ),
                reply("I've queued the exclusion; confirm to apply.", vec![]),
            ],
            "exclude it",
        )
        .await;
        assert_eq!(calls, 2, "a plan exists, so there is no re-prompt");
        assert_eq!(outcome.actions.len(), 1);
        assert_eq!(
            outcome.reply,
            "I've queued the exclusion; confirm to apply."
        );
    }

    #[tokio::test]
    async fn plain_answers_are_not_retried() {
        let (outcome, calls) = run_message(
            vec![reply(
                "There are 50 unscreened records. The plan of the protocol has four criteria.",
                vec![],
            )],
            "how many are left?",
        )
        .await;
        assert_eq!(calls, 1);
        assert!(!outcome.reply.contains("Nothing was queued"));
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

    /// Streams each scripted reply word by word, as the provider does.
    struct StreamingScript {
        replies: Mutex<Vec<ChatCompletion>>,
    }
    impl ChatGateway for StreamingScript {
        fn chat<'a>(&'a self, _request: ChatRequest) -> AiFuture<'a, ChatCompletion> {
            Box::pin(async move { next_scripted(&self.replies) })
        }
        fn chat_streaming<'a>(
            &'a self,
            _request: ChatRequest,
            on_text: ChatTextSink<'a>,
        ) -> AiFuture<'a, ChatCompletion> {
            Box::pin(async move {
                let completion = next_scripted(&self.replies)?;
                for word in completion.content.split_inclusive(' ') {
                    on_text(word);
                }
                Ok(completion)
            })
        }
    }

    fn next_scripted(replies: &Mutex<Vec<ChatCompletion>>) -> Result<ChatCompletion, AiError> {
        let mut replies = replies
            .lock()
            .map_err(|_| AiError::Gateway(String::new()))?;
        if replies.is_empty() {
            return Err(AiError::Gateway("script exhausted".to_owned()));
        }
        Ok(replies.remove(0))
    }

    /// One log shared by reads, progress and the tool callback, so the order
    /// of events across them can be asserted.
    #[derive(Clone, Default)]
    struct OrderedLog(Arc<Mutex<Vec<String>>>);
    impl OrderedLog {
        fn push(&self, entry: String) {
            if let Ok(mut log) = self.0.lock() {
                log.push(entry);
            }
        }
        fn entries(&self) -> Vec<String> {
            self.0.lock().map(|log| log.clone()).unwrap_or_default()
        }
    }

    struct OrderedReads(OrderedLog);
    impl AgentReadTools for OrderedReads {
        fn read<'a>(&'a self, tool: &'a str, _args: Value) -> AiFuture<'a, Value> {
            self.0.push(format!("run:{tool}"));
            Box::pin(async { Ok(json!({"reports": 2})) })
        }
    }

    struct OrderedProgress(OrderedLog);
    impl AgentProgress for OrderedProgress {
        fn text_delta(&mut self, delta: &str) {
            self.0.push(format!("text:{delta}"));
        }
        fn text_discarded(&mut self) {
            self.0.push("discarded".to_owned());
        }
        fn tool_started(&mut self, call: &ChatToolCall) {
            self.0.push(format!("start:{}", call.name));
        }
    }

    #[tokio::test]
    async fn progress_streams_text_and_announces_each_tool_before_it_runs() {
        let log = OrderedLog::default();
        let chat = StreamingScript {
            replies: Mutex::new(vec![
                reply("", vec![call("1", TOOL_PROJECT_OVERVIEW, json!({}))]),
                reply("Two reports match the query.", vec![]),
            ]),
        };
        let reads = OrderedReads(log.clone());
        let mut progress = OrderedProgress(log.clone());
        let tool_log = log.clone();
        let outcome = run_agent_loop(
            AgentTurn {
                chat: &chat,
                route: route(),
                project_id: ProjectId::new(Uuid::from_u128(7)),
                actor: Actor::new(ActorKind::User, "tester").unwrap_or_else(|_| unreachable!()),
                history: Vec::new(),
                user_message: "how many reports match?".to_owned(),
                reads: &reads,
                config: AgentLoopConfig::default(),
                progress: Some(&mut progress),
            },
            move |entry: &ToolTraceEntry| tool_log.push(format!("done:{}", entry.call.name)),
        )
        .await
        .unwrap_or_else(|_| unreachable!());
        assert_eq!(outcome.reply, "Two reports match the query.");
        assert_eq!(
            log.entries(),
            vec![
                "start:get_project_overview",
                "run:get_project_overview",
                "done:get_project_overview",
                "text:Two ",
                "text:reports ",
                "text:match ",
                "text:the ",
                "text:query.",
            ]
        );
    }

    #[tokio::test]
    async fn a_retried_claim_discards_the_text_streamed_for_it() {
        let log = OrderedLog::default();
        let chat = StreamingScript {
            replies: Mutex::new(vec![
                reply("I've queued it.", vec![]),
                reply("I found no change to make.", vec![]),
            ]),
        };
        let reads = OrderedReads(log.clone());
        let mut progress = OrderedProgress(log.clone());
        let outcome = run_agent_loop(
            AgentTurn {
                chat: &chat,
                route: route(),
                project_id: ProjectId::new(Uuid::from_u128(7)),
                actor: Actor::new(ActorKind::User, "tester").unwrap_or_else(|_| unreachable!()),
                history: Vec::new(),
                user_message: "propose the sample size".to_owned(),
                reads: &reads,
                config: AgentLoopConfig::default(),
                progress: Some(&mut progress),
            },
            |_| {},
        )
        .await
        .unwrap_or_else(|_| unreachable!());
        assert_eq!(outcome.reply, "I found no change to make.");
        assert_eq!(
            log.entries(),
            vec![
                "text:I've ",
                "text:queued ",
                "text:it.",
                "discarded",
                "text:I ",
                "text:found ",
                "text:no ",
                "text:change ",
                "text:to ",
                "text:make.",
            ]
        );
    }
}
