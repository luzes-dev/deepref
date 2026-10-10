//! One DeepRef assistant turn on the Rig agent runtime.
//!
//! Rig owns the agent state machine (model → tool → model execution, tool
//! dispatch, streaming, lifecycle); DeepRef owns everything semantic through
//! [`DeepRefAgentContext`], [`AssistantToolHost`], [`DeepRefHooks`] and the
//! tool adapter. The turn driver maps Rig events back onto DeepRef's
//! assistant contract: progress events for live display, a trace for
//! persistence, plan actions for human confirmation, and token accounting.
//!
//! Production callers should prefer [`run_rig_turn_channel`]: execution and
//! observation get separate lifetimes, so a slow observer never stalls the
//! run and dropping observation never cancels it.

use std::sync::Arc;

use futures::StreamExt;
use rig_agent::agent::{
    Agent, AgentBuilder, AgentRunner, MultiTurnStreamItem,
    run::response::{PromptError, PromptResponse},
};
use rig_core::{
    DynModel,
    completion::Message,
    message::AssistantContent,
    operation::Completion,
    streaming::{Item, StreamEvent},
    tool::ToolContext,
};
use serde_json::Value;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};

use uuid::Uuid;

use crate::{
    AgentLoopConfig, AiError, AssistantStreamEvent, ChatMessage, ChatToolCall, PlanAction,
    PriceBook, ToolTraceEntry, UsageLedger, assistant_system_prompt, claims_pending_change,
    runtime::{
        cassette::AssistantCassette,
        context::{AssistantToolHost, DeepRefAgentContext},
        hooks::{BUDGET_STOP_REASON, DeepRefHooks, TOKEN_STOP_REASON},
        model::AgentModelFactory,
        tools::{PlanCollector, ToolHostScope, deepref_dynamic_tools},
    },
};

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

/// Inputs for one Rig-driven assistant turn.
pub struct RigTurn<'a> {
    pub factory: Arc<dyn AgentModelFactory>,
    pub context: DeepRefAgentContext,
    pub history: Vec<ChatMessage>,
    pub user_message: String,
    pub host: Arc<dyn AssistantToolHost>,
    pub ledger: Arc<dyn UsageLedger>,
    pub prices: PriceBook,
    pub config: AgentLoopConfig,
    /// Identity of the durable assistant run this turn executes for.
    pub run_id: Uuid,
    /// Calibration contract this run executes under, for cassette provenance.
    pub semantic_contract_id: Option<String>,
    /// Build provenance for cassette provenance, where the caller tracks it.
    pub build_provenance: Option<Value>,
    /// Records every served dispatch for cassette replay. `None` runs bare.
    pub recorder: Option<rig_cassette::effect_log::EffectLogRecorder>,
    /// Receives progress as the turn runs. `None` keeps the turn silent.
    /// Only [`run_rig_turn`] observes it; channel callers drain the returned
    /// receiver instead.
    pub progress: Option<&'a mut (dyn AgentProgress + Send)>,
}

/// Outcome of one Rig-driven turn: reply, plan actions, tool trace, token
/// accounting, step count and truncation.
#[derive(Debug, Clone)]
pub struct RigTurnOutcome {
    pub reply: String,
    pub actions: Vec<PlanAction>,
    pub trace: Vec<ToolTraceEntry>,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub steps: usize,
    /// The loop stopped on a step or token cap rather than a final answer.
    pub truncated: bool,
    /// The recorded cassette: effect log plus run association. Present only
    /// when the turn ran with a recorder.
    pub cassette: Option<AssistantCassette>,
}

/// Runs the turn to completion, driving observation inline. For separate
/// execution/observation lifetimes use [`run_rig_turn_channel`].
pub async fn run_rig_turn(
    mut turn: RigTurn<'_>,
    mut on_tool: impl FnMut(&ToolTraceEntry) + Send,
) -> Result<RigTurnOutcome, AiError> {
    let progress = turn.progress.take();
    let (future, mut events) = run_rig_turn_channel(turn)?;
    tokio::pin!(future);
    let mut pending_progress = progress;
    let mut pending_calls: Vec<ChatToolCall> = Vec::new();
    let result = loop {
        tokio::select! {
            settled = &mut future => break settled,
            event = events.recv() => {
                // A closed receiver without settlement just means observation
                // ended early; the run still settles. Never break here.
                if let Some(event) = event {
                    forward_event(event, &mut pending_progress, &mut pending_calls, &mut on_tool);
                }
            }
        }
    };
    // The relay drains the same way: events queued behind the settlement
    // still reach the observer.
    while let Ok(event) = events.try_recv() {
        forward_event(
            event,
            &mut pending_progress,
            &mut pending_calls,
            &mut on_tool,
        );
    }
    result
}

/// Builds the agent and splits execution from observation. The future drives
/// the run to a [`RigTurnOutcome`]; the receiver carries
/// [`AssistantStreamEvent`] for the observer. Dropping the receiver lets the
/// run continue unobserved.
pub fn run_rig_turn_channel(
    turn: RigTurn<'_>,
) -> Result<
    (
        impl Future<Output = Result<RigTurnOutcome, AiError>> + Send,
        UnboundedReceiver<AssistantStreamEvent>,
    ),
    AiError,
> {
    let built = build_turn(turn)?;
    let (progress_tx, progress_rx) = unbounded_channel();
    let future = async move { drive_turn(built, progress_tx).await };
    Ok((future, progress_rx))
}

/// Forwards one driver-emitted event to inline progress observation,
/// tracking tool calls so completed entries keep their arguments.
fn forward_event(
    event: AssistantStreamEvent,
    progress: &mut Option<&mut (dyn AgentProgress + Send)>,
    pending_calls: &mut Vec<ChatToolCall>,
    on_tool: &mut (impl FnMut(&ToolTraceEntry) + Send),
) {
    match event {
        AssistantStreamEvent::Token { delta } => {
            if let Some(progress) = progress.as_mut() {
                progress.text_delta(&delta);
            }
        }
        AssistantStreamEvent::Replace { .. } => {
            if let Some(progress) = progress.as_mut() {
                progress.text_discarded();
            }
        }
        AssistantStreamEvent::ToolStart {
            tool,
            tool_call_id,
            args,
        } => {
            let call = ChatToolCall {
                id: tool_call_id,
                name: tool,
                arguments: args,
            };
            if let Some(progress) = progress.as_mut() {
                progress.tool_started(&call);
            }
            pending_calls.push(call);
        }
        AssistantStreamEvent::ToolComplete {
            tool,
            tool_call_id,
            output,
        } => {
            if let Some(index) = pending_calls
                .iter()
                .position(|call| call.id == tool_call_id)
            {
                let call = pending_calls.remove(index);
                on_tool(&ToolTraceEntry { call, output });
            } else {
                on_tool(&ToolTraceEntry {
                    call: ChatToolCall {
                        id: tool_call_id,
                        name: tool,
                        arguments: Value::Null,
                    },
                    output,
                });
            }
        }
        _ => {}
    }
}

/// Stable owner naming this assistant's bus keys (`<owner>/model:…`,
/// `<owner>/tool:…`). Replay requires the same owner at record and replay
/// time; a process-local counter would break cross-process compatibility.
pub const DEEPREF_ASSISTANT_OWNER: &str = "deepref-assistant";

struct BuiltTurn {
    agent: Agent,
    context: DeepRefAgentContext,
    history: Vec<Message>,
    user_message: String,
    scope: ToolHostScope,
    hooks: DeepRefHooks,
    config: AgentLoopConfig,
    run_id: Uuid,
    semantic_contract_id: Option<String>,
    build_provenance: Option<Value>,
    recorder: Option<rig_cassette::effect_log::EffectLogRecorder>,
}

/// Resolves the model, builds hook state and validates inputs. Fail-fast
/// errors (unroutable model) surface here, before any stream opens.
fn build_turn(turn: RigTurn<'_>) -> Result<BuiltTurn, AiError> {
    let model = turn.factory.model(&turn.context.route)?;
    let plans = PlanCollector::new();
    let scope = ToolHostScope::new(turn.context.clone(), turn.host.clone(), plans);
    let hooks = DeepRefHooks::new(scope.clone(), turn.ledger.clone(), turn.prices, turn.config);
    let history = turn
        .history
        .iter()
        .filter_map(|message| match message {
            ChatMessage::User(text) => Some(Message::user(text.clone())),
            ChatMessage::Assistant { content, .. } => Some(Message::assistant(content.clone())),
            ChatMessage::System(_) | ChatMessage::Tool { .. } => None,
        })
        .collect();
    let agent = build_live_agent(
        model,
        &turn.context,
        &scope,
        hooks.clone(),
        turn.config,
        turn.recorder.clone(),
    );
    Ok(BuiltTurn {
        agent,
        context: turn.context,
        history,
        user_message: turn.user_message,
        scope,
        hooks,
        config: turn.config,
        run_id: turn.run_id,
        semantic_contract_id: turn.semantic_contract_id,
        build_provenance: turn.build_provenance,
        recorder: turn.recorder,
    })
}

/// Assembles the live assistant agent: preamble, model parameters, the
/// DeepRef tool catalog, DeepRef hooks, a stable owner and optional
/// cassette recording.
fn build_live_agent(
    model: DynModel<Completion>,
    context: &DeepRefAgentContext,
    scope: &ToolHostScope,
    hooks: DeepRefHooks,
    config: AgentLoopConfig,
    recorder: Option<rig_cassette::effect_log::EffectLogRecorder>,
) -> Agent {
    let mut builder = AgentBuilder::new(model)
        .owner(DEEPREF_ASSISTANT_OWNER)
        .preamble(assistant_system_prompt(context.project_id))
        .max_tokens(u64::from(config.max_output_tokens_per_call))
        .dynamic_tools(deepref_dynamic_tools(scope))
        .add_hook(hooks);
    if let Some(temperature) = context.route.parameters.temperature {
        builder = builder.temperature(f64::from(temperature));
    }
    if let Some(top_p) = context.route.parameters.top_p {
        builder = builder.top_p(f64::from(top_p));
    }
    if !context.route.parameters.additional.is_empty() {
        builder = builder.additional_params(Value::Object(
            context
                .route
                .parameters
                .additional
                .clone()
                .into_iter()
                .collect(),
        ));
    }
    match recorder {
        Some(recorder) => builder.record_to(recorder).build(),
        None => builder.build(),
    }
}

struct Driver {
    scope: ToolHostScope,
    hooks: DeepRefHooks,
    user_message: String,
    progress_tx: UnboundedSender<AssistantStreamEvent>,
    /// Rig tool calls announced but not yet committed, in event order.
    pending: Vec<ChatToolCall>,
    /// Committed trace entries, in commit order.
    trace: Vec<ToolTraceEntry>,
    /// Completed model calls observed.
    steps: usize,
}

async fn drive_turn(
    built: BuiltTurn,
    progress_tx: UnboundedSender<AssistantStreamEvent>,
) -> Result<RigTurnOutcome, AiError> {
    let tool_scope: Arc<dyn std::any::Any + Send + Sync> = Arc::new(built.scope.clone());
    let runner = built
        .agent
        .prompt(Message::user(built.user_message.clone()))
        .history(built.history.clone())
        .max_turns(built.config.max_steps)
        .tool_context(ToolContext::new().with_scope(tool_scope))
        .tool_concurrency(1)
        .max_invalid_tool_call_retries(built.config.max_steps);
    let mut outcome = run_assistant_runner(
        built.scope.clone(),
        built.hooks.clone(),
        built.user_message.clone(),
        runner,
        progress_tx,
    )
    .await?;
    if let Some(recorder) = &built.recorder {
        outcome.cassette = Some(AssistantCassette::assemble(
            &built.agent,
            recorder,
            &built.context,
            built.run_id,
            built.semantic_contract_id.clone(),
            built.build_provenance.clone(),
        ));
    }
    Ok(outcome)
}

/// Drives one runner to settlement, sharing the select/drain/finish loop
/// between live runs and cassette replays.
pub(crate) async fn run_assistant_runner(
    scope: ToolHostScope,
    hooks: DeepRefHooks,
    user_message: String,
    runner: AgentRunner,
    progress_tx: UnboundedSender<AssistantStreamEvent>,
) -> Result<RigTurnOutcome, AiError> {
    let (run_future, mut run_events) = runner.run_channel();
    tokio::pin!(run_future);
    let mut driver = Driver {
        scope: scope.clone(),
        hooks: hooks.clone(),
        user_message,
        progress_tx,
        pending: Vec::new(),
        trace: Vec::new(),
        steps: 0,
    };
    let mut response = None;
    let mut run_error = None;
    loop {
        tokio::select! {
            settled = &mut run_future => {
                match settled {
                    Ok(finished) => response = Some(finished),
                    Err(error) => run_error = Some(error),
                }
                break;
            }
            item = run_events.next() => {
                // A closed feed without settlement just means the sender went
                // away; the future still resolves. Never break early here,
                // or buffered events (tool commits, completion calls) are lost.
                if let Some(item) = item {
                    observe_item(&mut driver, item);
                }
            }
        }
    }
    // The relay drains the same way: events queued behind the settlement
    // still reach the observer before the outcome is built.
    while let Some(item) = run_events.next().await {
        observe_item(&mut driver, item);
    }
    finish_turn(driver, response, run_error)
}

/// Maps one Rig stream item onto driver progress. The final reply comes
/// from the settled future; these events only feed live display and the
/// tool trace.
fn observe_item(driver: &mut Driver, item: MultiTurnStreamItem) {
    match item {
        MultiTurnStreamItem::StreamAssistantItem(Item::Event(StreamEvent::Text {
            part: _,
            text,
        })) => {
            let _ = driver
                .progress_tx
                .send(AssistantStreamEvent::Token { delta: text });
        }
        MultiTurnStreamItem::ToolCall { tool_call } => {
            let call = ChatToolCall {
                id: tool_call.id.wire().into_owned(),
                name: tool_call.function.name.as_str().to_owned(),
                arguments: Value::Object(tool_call.function.arguments.clone()),
            };
            let _ = driver.progress_tx.send(AssistantStreamEvent::ToolStart {
                tool: call.name.clone(),
                tool_call_id: call.id.clone(),
                args: call.arguments.clone(),
            });
            driver.pending.push(call);
        }
        MultiTurnStreamItem::ToolExecutionCommitted { tool_call } => {
            let name = tool_call.function.name.as_str();
            let args = Value::Object(tool_call.function.arguments.clone());
            let id = tool_call.id.wire().into_owned();
            if let Some(output) = driver.scope.trace.take_matching(name, &args) {
                // The committed call leaves the pending set: leftovers can
                // only match calls that never committed (a stopped run).
                if let Some(index) = driver.pending.iter().position(|call| call.id == id) {
                    let call = driver.pending.remove(index);
                    driver.trace.push(ToolTraceEntry {
                        call: call.clone(),
                        output: output.output.clone(),
                    });
                }
                let _ = driver.progress_tx.send(AssistantStreamEvent::ToolComplete {
                    tool: name.to_owned(),
                    tool_call_id: id,
                    output: output.output,
                });
            }
        }
        MultiTurnStreamItem::ModelTurnRetried { .. } => {
            let _ = driver.progress_tx.send(AssistantStreamEvent::Replace {
                text: String::new(),
            });
        }
        MultiTurnStreamItem::CompletionCall(_) => {
            driver.steps += 1;
        }
        MultiTurnStreamItem::FinalResponse(_)
        | MultiTurnStreamItem::ToolResult { .. }
        | MultiTurnStreamItem::StreamAssistantItem(_)
        | _ => {}
    }
}

/// Settles the turn: budget errors first, then the settled response or the
/// mapped run error.
fn finish_turn(
    mut driver: Driver,
    response: Option<PromptResponse>,
    run_error: Option<PromptError>,
) -> Result<RigTurnOutcome, AiError> {
    if driver
        .scope
        .budget_exceeded
        .load(std::sync::atomic::Ordering::SeqCst)
    {
        return Err(AiError::BudgetExceeded);
    }
    if let Some(response) = response {
        return Ok(success_outcome(&mut driver, &response));
    }
    let Some(error) = run_error else {
        return Err(AiError::Gateway(
            "assistant run ended without producing a final response".to_owned(),
        ));
    };
    map_run_error(&mut driver, error)
}

fn success_outcome(driver: &mut Driver, response: &PromptResponse) -> RigTurnOutcome {
    let usage = driver.hooks.usage();
    let mut reply = response
        .content
        .iter()
        .filter_map(|block| match block {
            AssistantContent::Text(text) => Some(text.text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("");
    let actions = driver.scope.plans.actions();
    let trace = drain_trace(driver);
    let mut outcome = RigTurnOutcome {
        reply: reply.clone(),
        actions,
        trace,
        input_tokens: usage.input_tokens,
        output_tokens: usage.output_tokens,
        steps: driver.steps,
        truncated: false,
        cassette: None,
    };
    if reply.trim().is_empty() {
        reply = if outcome.actions.is_empty() {
            "I have nothing to add.".to_owned()
        } else {
            "I prepared a plan. Nothing has been changed yet.".to_owned()
        };
    } else if outcome.actions.is_empty()
        && driver.hooks.consistency_retried()
        && claims_pending_change(&reply)
    {
        reply = format!(
            "{}\n\n{}",
            reply.trim_end(),
            super::plan::no_plan_notice(&driver.user_message)
        );
    }
    outcome.reply = reply;
    outcome
}

/// Remaining trace: committed calls were collected during the run; anything
/// left executed but never committed (a stopped run).
fn drain_trace(driver: &mut Driver) -> Vec<ToolTraceEntry> {
    let mut trace = std::mem::take(&mut driver.trace);
    for record in driver.scope.trace.drain() {
        if let Some(index) = driver
            .pending
            .iter()
            .position(|call| call.name == record.tool && call.arguments == record.args)
        {
            let call = driver.pending.remove(index);
            trace.push(ToolTraceEntry {
                call,
                output: record.output,
            });
        }
    }
    trace
}

fn map_run_error(driver: &mut Driver, error: PromptError) -> Result<RigTurnOutcome, AiError> {
    match error {
        PromptError::MaxTurns { .. } => Ok(truncated_outcome(driver)),
        PromptError::Cancelled { reason, .. } => {
            if reason == BUDGET_STOP_REASON {
                Err(AiError::BudgetExceeded)
            } else if reason == TOKEN_STOP_REASON {
                Ok(truncated_outcome(driver))
            } else {
                Err(AiError::Gateway(reason))
            }
        }
        PromptError::UnknownToolCall { .. } => {
            // Invalid-call recovery exhausted: equivalent to step exhaustion.
            Ok(truncated_outcome(driver))
        }
        PromptError::Provider(error) => Err(AiError::Gateway(error.to_string())),
        PromptError::Report(error) => Err(AiError::Gateway(error.to_string())),
        PromptError::Memory(error) => Err(AiError::Gateway(error.to_string())),
        other => Err(AiError::Gateway(format!("assistant run failed: {other}"))),
    }
}

fn truncated_outcome(driver: &mut Driver) -> RigTurnOutcome {
    let usage = driver.hooks.usage();
    let actions = driver.scope.plans.actions();
    let trace = drain_trace(driver);
    let reply = if actions.is_empty() {
        "I ran out of room to finish this in one go. Try a narrower question.".to_owned()
    } else {
        "I reached my working limit, so the plan below may be incomplete. Review it, then ask me to continue.".to_owned()
    };
    RigTurnOutcome {
        reply,
        actions,
        trace,
        input_tokens: usage.input_tokens,
        output_tokens: usage.output_tokens,
        steps: driver.steps,
        truncated: true,
        cassette: None,
    }
}
