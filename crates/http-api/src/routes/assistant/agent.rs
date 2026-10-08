//! Free-form assistant turns: a bounded tool-calling loop whose writes end in
//! a plan that the user must confirm.

use std::{future::Future, sync::Arc};

use deepref_ai::{
    ASSISTANT_PROMPT_VERSION, AgentLoopConfig, AgentProgress, AgentReadTools, AgentTool,
    AgentToolError, AgentToolName, AgentTurn, AgentTurnOutcome, AiError, AiFuture,
    AssistantStreamEvent, AssistantToolCall, AssistantToolResult, BoundedAgentJson, ChatGateway,
    ChatMessage, ChatToolCall, ModelProfile, ModelRouter, ResolvedModel, TOOL_LIST_REPORTS,
    TOOL_PROJECT_OVERVIEW, ToolTraceEntry, hash_json, run_agent_loop,
};
use deepref_domain::{Actor, ProjectId};
use serde_json::{Value, json};
use tokio::sync::mpsc::Sender;
use uuid::Uuid;

use super::{AssistantFailure, execute_read, plans};
use crate::{error::ApiError, state::AppState};

const HISTORY_LIMIT: usize = 12;
const STORED_OUTPUT_CHARS: usize = 4_000;
const ASSISTANT_TEMPERATURE: f32 = 0.2;

pub(super) struct AgentSetup {
    chat: Arc<dyn ChatGateway>,
    route: ResolvedModel,
}

/// Fails fast (as a plain HTTP error) when the assistant cannot run at all, so
/// the user message is not persisted and no stream is opened.
pub(super) async fn prepare_agent_turn(
    state: &AppState,
    project_id: Uuid,
) -> Result<AgentSetup, ApiError> {
    let Some(chat) = state.chat_gateway.clone() else {
        return Err(ApiError::Configuration(
            "The assistant needs an AI provider, which is not configured for this workspace."
                .to_owned(),
        ));
    };
    let mut route = deepref_postgres::PostgresAiStore::new(&state.pool)
        .resolve(ModelProfile::Reasoning)
        .await
        .map_err(|_| {
            ApiError::Configuration("No AI model is configured for the assistant.".to_owned())
        })?;
    route.parameters.temperature = Some(ASSISTANT_TEMPERATURE);
    let budget = deepref_postgres::get_ai_budget(&state.pool, project_id)
        .await
        .map_err(|error| match error {
            deepref_postgres::AiUsageError::ProjectNotFound => {
                ApiError::NotFound("project not found".to_owned())
            }
            _ => ApiError::Internal(anyhow::anyhow!("AI budget lookup failed")),
        })?;
    if budget.exhausted() {
        return Err(super::budget_exceeded_error());
    }
    Ok(AgentSetup { chat, route })
}

struct ProjectReadTools {
    state: AppState,
}

fn tool_failure(error: &AssistantFailure) -> AiError {
    AiError::InvalidContext(
        match error {
            AssistantFailure::NotFound => "that item was not found in this project",
            AssistantFailure::Conflict => "that request conflicts with the current state",
            AssistantFailure::Internal => "the tool failed",
        }
        .to_owned(),
    )
}

impl AgentReadTools for ProjectReadTools {
    fn read<'a>(&'a self, tool: &'a str, args: Value) -> AiFuture<'a, Value> {
        Box::pin(async move {
            let project_id = args
                .get("project_id")
                .and_then(Value::as_str)
                .and_then(|text| Uuid::parse_str(text).ok())
                .ok_or_else(|| AiError::InvalidContext("missing project".to_owned()))?;
            let read_error = |error: deepref_postgres::AgentReadError| match error {
                deepref_postgres::AgentReadError::NotFound => {
                    AiError::InvalidContext("that item was not found in this project".to_owned())
                }
                deepref_postgres::AgentReadError::InvalidData => {
                    AiError::InvalidContext("invalid argument".to_owned())
                }
                deepref_postgres::AgentReadError::Database(_) => {
                    AiError::Persistence("read failed".to_owned())
                }
            };
            match tool {
                TOOL_PROJECT_OVERVIEW => {
                    deepref_postgres::get_agent_project_overview(&self.state.pool, project_id)
                        .await
                        .map_err(read_error)
                }
                TOOL_LIST_REPORTS => {
                    let limit = args.get("limit").and_then(Value::as_i64).unwrap_or(25);
                    deepref_postgres::list_agent_reports_by_screening(
                        &self.state.pool,
                        project_id,
                        args.get("status").and_then(Value::as_str),
                        limit,
                    )
                    .await
                    .map_err(read_error)
                }
                other => {
                    let name = AgentToolName::parse(other)
                        .ok_or_else(|| AiError::InvalidContext("unknown tool".to_owned()))?;
                    let tool = AgentTool::from_name_and_args(name, args).map_err(|_| {
                        AiError::InvalidContext("the arguments are malformed".to_owned())
                    })?;
                    tool.validate().map_err(|_| {
                        AiError::InvalidContext("the arguments are invalid".to_owned())
                    })?;
                    let operation = tool
                        .into_read_operation()
                        .map_err(|_| AiError::InvalidContext("not a read tool".to_owned()))?;
                    let value = execute_read(&self.state, operation)
                        .await
                        .map_err(|error| tool_failure(&error))?;
                    BoundedAgentJson::new(value)
                        .map(BoundedAgentJson::into_value)
                        .map_err(|_| AiError::InvalidContext("the result was too large".to_owned()))
                }
            }
        })
    }
}

fn history_messages(records: &[deepref_postgres::AssistantMessageRecord]) -> Vec<ChatMessage> {
    let mut messages: Vec<ChatMessage> = records
        .iter()
        .filter(|record| !record.content.trim().is_empty())
        .filter_map(|record| match record.role.as_str() {
            "user" => Some(ChatMessage::User(record.content.clone())),
            "assistant" => Some(ChatMessage::Assistant {
                content: record.content.clone(),
                tool_calls: Vec::new(),
            }),
            _ => None,
        })
        .collect();
    let excess = messages.len().saturating_sub(HISTORY_LIMIT);
    messages.drain(..excess);
    messages
}

fn stored_output(output: &Value) -> Value {
    let text = output.to_string();
    if text.chars().count() <= STORED_OUTPUT_CHARS {
        output.clone()
    } else {
        json!({
            "truncated": true,
            "preview": text.chars().take(STORED_OUTPUT_CHARS).collect::<String>(),
        })
    }
}

fn evidence_from(trace: &[ToolTraceEntry]) -> Value {
    Value::Array(
        trace
            .iter()
            .filter(|entry| deepref_ai::is_read_tool(&entry.call.name))
            .map(|entry| {
                json!({
                    "tool": entry.call.name,
                    "args": entry.call.arguments,
                    "output_sha256": hash_json(&entry.output).ok(),
                })
            })
            .collect(),
    )
}

fn map_loop_error(error: &AiError) -> AgentToolError {
    match error {
        AiError::BudgetExceeded => AgentToolError::BudgetExceeded,
        AiError::SubscriptionLimit => AgentToolError::SubscriptionLimit,
        AiError::Gateway(_) | AiError::Route(_) => AgentToolError::ProviderFailed,
        _ => AgentToolError::ExecutionFailed,
    }
}

/// Ends a stopped answer. The web app shows the same sentence, so reloading the
/// conversation shows what the user saw.
const STOPPED_NOTE: &str = "Stopped. Nothing has been changed.";

/// Carries the loop's progress into the turn's event queue without waiting.
struct ChannelProgress {
    events: tokio::sync::mpsc::UnboundedSender<AssistantStreamEvent>,
}

impl AgentProgress for ChannelProgress {
    fn text_delta(&mut self, delta: &str) {
        let _ = self.events.send(AssistantStreamEvent::Token {
            delta: delta.to_owned(),
        });
    }

    fn text_discarded(&mut self) {
        let _ = self.events.send(AssistantStreamEvent::Replace {
            text: String::new(),
        });
    }

    fn tool_started(&mut self, call: &ChatToolCall) {
        let _ = self.events.send(AssistantStreamEvent::ToolStart {
            tool: call.name.clone(),
            tool_call_id: call.id.clone(),
            args: call.arguments.clone(),
        });
    }
}

/// What the client has been shown for the answer, and the tool work behind it.
/// The text restarts when a tool starts or a step is discarded, as it does in
/// the web app. A stopped turn is stored from this.
#[derive(Default)]
struct TurnRelay {
    text: String,
    calls: Vec<AssistantToolCall>,
    results: Vec<AssistantToolResult>,
}

impl TurnRelay {
    fn observe(&mut self, event: &AssistantStreamEvent) {
        match event {
            AssistantStreamEvent::Token { delta } => self.text.push_str(delta),
            AssistantStreamEvent::Replace { text } => self.text.clone_from(text),
            AssistantStreamEvent::ToolStart {
                tool,
                tool_call_id,
                args,
            } => {
                self.text.clear();
                self.calls.push(AssistantToolCall {
                    id: tool_call_id.clone(),
                    tool: tool.clone(),
                    args: args.clone(),
                });
            }
            AssistantStreamEvent::ToolComplete {
                tool,
                tool_call_id,
                output,
            } => self.results.push(AssistantToolResult {
                tool_call_id: tool_call_id.clone(),
                tool: tool.clone(),
                output: output.clone(),
                proposal_review_run_id: None,
            }),
            _ => {}
        }
    }
}

enum TurnEnd {
    Finished(Result<AgentTurnOutcome, AiError>),
    /// The client went away before the turn finished.
    Stopped,
}

async fn forward(
    events: &Sender<AssistantStreamEvent>,
    relay: &mut TurnRelay,
    event: AssistantStreamEvent,
) {
    relay.observe(&event);
    let _ = events.send(event).await;
}

/// Relays the loop's progress to the client until the turn finishes. When the
/// client disconnects first, the turn future is dropped, which abandons the
/// provider request in flight.
async fn relay_turn(
    events: &Sender<AssistantStreamEvent>,
    progress: &mut tokio::sync::mpsc::UnboundedReceiver<AssistantStreamEvent>,
    relay: &mut TurnRelay,
    turn: impl Future<Output = Result<AgentTurnOutcome, AiError>>,
) -> TurnEnd {
    tokio::pin!(turn);
    loop {
        tokio::select! {
            biased;
            () = events.closed() => return TurnEnd::Stopped,
            Some(event) = progress.recv() => forward(events, relay, event).await,
            result = &mut turn => {
                while let Ok(event) = progress.try_recv() {
                    forward(events, relay, event).await;
                }
                return TurnEnd::Finished(result);
            }
        }
    }
}

/// The event that makes the client's answer equal the stored reply: the missing
/// tail when the reply extends what was shown, otherwise a replacement.
fn answer_correction(shown: &str, reply: &str) -> Option<AssistantStreamEvent> {
    if shown == reply {
        return None;
    }
    Some(match reply.strip_prefix(shown) {
        Some(tail) => AssistantStreamEvent::Token {
            delta: tail.to_owned(),
        },
        None => AssistantStreamEvent::Replace {
            text: reply.to_owned(),
        },
    })
}

/// Stores a stopped turn: the answer text the user saw, marked as stopped, with
/// the tool work done so far. A stopped turn creates no plan, so nothing the
/// partial answer mentions was queued.
async fn persist_stopped_turn(
    state: &AppState,
    project_id: ProjectId,
    conversation_id: Uuid,
    model: &str,
    relay: TurnRelay,
) {
    let message_id = Uuid::new_v4();
    let content = match relay.text.trim_end() {
        "" => STOPPED_NOTE.to_owned(),
        text => format!("{text}\n\n{STOPPED_NOTE}"),
    };
    let message = deepref_postgres::AppendAssistantMessage {
        id: Some(message_id),
        conversation_id,
        role: "assistant".to_owned(),
        content,
        tool_calls: (!relay.calls.is_empty()).then(|| json!(relay.calls)),
        tool_results: (!relay.results.is_empty()).then(|| json!(relay.results)),
        metadata: Some(json!({
            "message_id": message_id,
            "plan_id": null,
            "model": model,
            "prompt_version": ASSISTANT_PROMPT_VERSION,
            "stopped": true,
        })),
    };
    if let Err(error) = deepref_postgres::append_assistant_message(&state.pool, &message).await {
        tracing::warn!(%error, "failed to persist a stopped assistant turn");
    }
    tracing::info!(project_id = %project_id.as_uuid(), "assistant turn stopped by the client");
}

#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
pub(super) async fn run_agent_turn(
    state: &AppState,
    setup: AgentSetup,
    actor: Actor,
    project_id: ProjectId,
    conversation_id: Uuid,
    records: &[deepref_postgres::AssistantMessageRecord],
    user_message: &str,
    events: &Sender<AssistantStreamEvent>,
) -> Result<(), AgentToolError> {
    let _ = events
        .send(AssistantStreamEvent::Status {
            message: "Thinking".to_owned(),
        })
        .await;
    let reads = ProjectReadTools {
        state: state.clone(),
    };
    let model = setup.route.model.clone();
    // Progress goes through an unbounded queue, so the loop never waits on the
    // client and no token or tool event is dropped.
    let (progress_tx, mut progress_rx) =
        tokio::sync::mpsc::unbounded_channel::<AssistantStreamEvent>();
    let mut progress = ChannelProgress {
        events: progress_tx.clone(),
    };
    let turn = run_agent_loop(
        AgentTurn {
            chat: setup.chat.as_ref(),
            route: setup.route,
            project_id,
            actor: actor.clone(),
            history: history_messages(records),
            user_message: user_message.to_owned(),
            reads: &reads,
            config: AgentLoopConfig::default(),
            progress: Some(&mut progress),
        },
        |entry: &ToolTraceEntry| {
            let _ = progress_tx.send(AssistantStreamEvent::ToolComplete {
                tool: entry.call.name.clone(),
                tool_call_id: entry.call.id.clone(),
                output: stored_output(&entry.output),
            });
        },
    );
    let mut relay = TurnRelay::default();
    let outcome = match relay_turn(events, &mut progress_rx, &mut relay, turn).await {
        TurnEnd::Finished(result) => result.map_err(|error| {
            tracing::warn!(?error, "assistant turn failed");
            map_loop_error(&error)
        })?,
        TurnEnd::Stopped => {
            persist_stopped_turn(state, project_id, conversation_id, &model, relay).await;
            return Ok(());
        }
    };

    let message_id = Uuid::new_v4();
    let mut plan_id = None;
    if !outcome.actions.is_empty() {
        let id = Uuid::new_v4();
        let summary = outcome
            .actions
            .iter()
            .map(|action| action.summary.as_str())
            .collect::<Vec<_>>()
            .join("; ");
        let record = deepref_postgres::create_assistant_plan(
            &state.pool,
            &deepref_postgres::NewAssistantPlan {
                id,
                project_id: project_id.as_uuid(),
                conversation_id,
                summary,
                actions: serde_json::to_value(&outcome.actions)
                    .map_err(|_| AgentToolError::ExecutionFailed)?,
                created_by_kind: actor.kind().as_str().to_owned(),
                created_by_id: actor.id().to_owned(),
                model: model.clone(),
                prompt_version: ASSISTANT_PROMPT_VERSION.to_owned(),
                evidence: evidence_from(&outcome.trace),
            },
        )
        .await
        .map_err(|error| {
            tracing::warn!(%error, "failed to persist assistant plan");
            AgentToolError::ExecutionFailed
        })?;
        plan_id = Some(id);
        let _ = events
            .send(AssistantStreamEvent::PlanProposed {
                plan: serde_json::to_value(plans::plan_dto(&record))
                    .map_err(|_| AgentToolError::ExecutionFailed)?,
            })
            .await;
    }

    // The answer already streamed while the loop ran. Bring the client's text in
    // line with the stored reply, which can differ when the loop annotated it.
    if let Some(event) = answer_correction(&relay.text, &outcome.reply) {
        forward(events, &mut relay, event).await;
    }

    let calls: Vec<ChatToolCall> = outcome
        .trace
        .iter()
        .map(|entry| entry.call.clone())
        .collect();
    let message = deepref_postgres::AppendAssistantMessage {
        id: Some(message_id),
        conversation_id,
        role: "assistant".to_owned(),
        content: outcome.reply.clone(),
        tool_calls: (!calls.is_empty()).then(|| {
            json!(
                calls
                    .iter()
                    .map(|call| AssistantToolCall {
                        id: call.id.clone(),
                        tool: call.name.clone(),
                        args: call.arguments.clone(),
                    })
                    .collect::<Vec<_>>()
            )
        }),
        tool_results: (!outcome.trace.is_empty()).then(|| {
            json!(
                outcome
                    .trace
                    .iter()
                    .map(|entry| AssistantToolResult {
                        tool_call_id: entry.call.id.clone(),
                        tool: entry.call.name.clone(),
                        output: stored_output(&entry.output),
                        proposal_review_run_id: None,
                    })
                    .collect::<Vec<_>>()
            )
        }),
        metadata: Some(json!({
            "message_id": message_id,
            "plan_id": plan_id,
            "model": model,
            "prompt_version": ASSISTANT_PROMPT_VERSION,
            "input_tokens": outcome.input_tokens,
            "output_tokens": outcome.output_tokens,
            "steps": outcome.steps,
            "truncated": outcome.truncated,
        })),
    };
    if let Err(error) = deepref_postgres::append_assistant_message(&state.pool, &message).await {
        tracing::warn!(%error, "failed to persist assistant turn");
    }
    let _ = events
        .send(AssistantStreamEvent::Done {
            message_id,
            input_tokens: outcome.input_tokens,
            output_tokens: outcome.output_tokens,
        })
        .await;
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    };

    use super::*;

    fn token(delta: &str) -> AssistantStreamEvent {
        AssistantStreamEvent::Token {
            delta: delta.to_owned(),
        }
    }

    #[test]
    fn a_reply_that_extends_the_shown_text_sends_only_the_tail() {
        assert_eq!(
            answer_correction("I found", "I found two."),
            Some(token(" two."))
        );
        assert_eq!(answer_correction("Same", "Same"), None);
        assert_eq!(answer_correction("", "Whole"), Some(token("Whole")));
        assert_eq!(
            answer_correction("Old text", "New reply"),
            Some(AssistantStreamEvent::Replace {
                text: "New reply".to_owned()
            })
        );
    }

    #[test]
    fn the_relay_restarts_the_text_at_each_tool_and_keeps_the_tool_work() {
        let mut relay = TurnRelay::default();
        relay.observe(&token("Let me check. "));
        relay.observe(&AssistantStreamEvent::ToolStart {
            tool: "get_report".to_owned(),
            tool_call_id: "c1".to_owned(),
            args: json!({"report_id": "r1"}),
        });
        relay.observe(&AssistantStreamEvent::ToolComplete {
            tool: "get_report".to_owned(),
            tool_call_id: "c1".to_owned(),
            output: json!({"title": "T"}),
        });
        relay.observe(&token("It is "));
        relay.observe(&token("T."));
        assert_eq!(relay.text, "It is T.");
        assert_eq!(relay.calls.len(), 1);
        assert_eq!(relay.calls[0].id, "c1");
        assert_eq!(relay.results.len(), 1);
        assert_eq!(relay.results[0].tool_call_id, "c1");
        relay.observe(&AssistantStreamEvent::Replace {
            text: "Replaced".to_owned(),
        });
        assert_eq!(relay.text, "Replaced");
    }

    #[tokio::test]
    async fn progress_reaches_the_client_in_order_before_the_turn_result() {
        let (events, mut received) = tokio::sync::mpsc::channel::<AssistantStreamEvent>(8);
        let (progress_tx, mut progress) =
            tokio::sync::mpsc::unbounded_channel::<AssistantStreamEvent>();
        progress_tx
            .send(token("a"))
            .unwrap_or_else(|_| unreachable!());
        progress_tx
            .send(token("b"))
            .unwrap_or_else(|_| unreachable!());
        let turn = async {
            Ok(AgentTurnOutcome {
                reply: "ab".to_owned(),
                actions: Vec::new(),
                trace: Vec::new(),
                input_tokens: 1,
                output_tokens: 2,
                steps: 1,
                truncated: false,
            })
        };
        let mut relay = TurnRelay::default();
        let ended = relay_turn(&events, &mut progress, &mut relay, turn).await;
        let TurnEnd::Finished(Ok(outcome)) = ended else {
            panic!("a finished turn must report its outcome");
        };
        assert_eq!(outcome.reply, "ab");
        assert_eq!(relay.text, "ab");
        let mut delivered = Vec::new();
        while let Ok(event) = received.try_recv() {
            delivered.push(event);
        }
        assert_eq!(delivered, vec![token("a"), token("b")]);
    }

    struct DropFlag(Arc<AtomicBool>);
    impl Drop for DropFlag {
        fn drop(&mut self) {
            self.0.store(true, Ordering::SeqCst);
        }
    }

    #[tokio::test]
    async fn a_client_that_disconnects_abandons_the_turn_and_its_provider_request() {
        let (events, received) = tokio::sync::mpsc::channel::<AssistantStreamEvent>(8);
        let (_progress_tx, mut progress) =
            tokio::sync::mpsc::unbounded_channel::<AssistantStreamEvent>();
        drop(received);
        let dropped = Arc::new(AtomicBool::new(false));
        let guard = DropFlag(Arc::clone(&dropped));
        let turn = async move {
            // Stands in for a provider request that is still generating.
            let _guard = guard;
            std::future::pending::<Result<AgentTurnOutcome, AiError>>().await
        };
        let mut relay = TurnRelay::default();
        let ended = relay_turn(&events, &mut progress, &mut relay, turn).await;
        assert!(matches!(ended, TurnEnd::Stopped));
        assert!(
            dropped.load(Ordering::SeqCst),
            "the turn future was not dropped, so the provider request would keep running"
        );
    }
}
