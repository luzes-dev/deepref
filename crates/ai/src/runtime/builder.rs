//! Shared assistant-agent construction for live runs and cassette replays.
//!
//! The live agent ([`AgentBuilder::new`]) and the replay agent
//! ([`AgentBuilder::over_bus`]) must be configured identically: preamble,
//! model parameters, the DeepRef tool catalog, DeepRef hooks, history mapping
//! and runner limits. Any drift between the two is a replay-compatibility
//! hazard, so every shared setting lives in exactly one helper here:
//!
//! * [`configure_assistant_builder`] — owner, preamble, token bound, tool
//!   catalog, hooks and model parameters. Both agents run through it, so the
//!   replay agent cannot drift from the recording agent by editing only one
//!   call site. [`AgentReplayExt::check_replayable`][check] still fails closed
//!   on any residual drift (different owner, hook stack, run spec).
//! * [`history_to_messages`] — the [`ChatMessage`] to Rig [`Message`] mapping.
//! * [`assistant_runner`] — prompt, history, turn cap, tool scope,
//!   single-flight tool execution and invalid-call recovery.
//!
//! [check]: rig_cassette::agent::AgentReplayExt::check_replayable

use std::sync::Arc;

use rig_agent::agent::{Agent, AgentBuilder, AgentRunner, NoToolConfig, WithBuilderTools};
use rig_core::{completion::Message, tool::ToolContext};
use serde_json::Value;

use crate::{
    AgentLoopConfig, ChatMessage, assistant_system_prompt,
    runtime::{
        context::DeepRefAgentContext,
        hooks::DeepRefHooks,
        tools::{ToolHostScope, deepref_dynamic_tools},
    },
};

/// Stable owner naming this assistant's bus keys (`<owner>/model:…`,
/// `<owner>/tool:…`). Replay requires the same owner at record and replay
/// time; a process-local counter would break cross-process compatibility.
///
/// `pub` only so [`crate::runtime::agent`] can re-export it on the
/// pre-existing path; the `builder` module itself is private, so effective
/// visibility stays inside this crate.
pub const DEEPREF_ASSISTANT_OWNER: &str = "deepref-assistant";

/// Applies every shared assistant setting to a builder, whatever bus it runs
/// over. Takes the builder before tools are attached and returns it after, so
/// live (own bus) and replay (host bus) construction share one preamble, one
/// tool catalog, one hook stack and one parameter mapping.
pub(crate) fn configure_assistant_builder(
    builder: AgentBuilder<NoToolConfig>,
    context: &DeepRefAgentContext,
    scope: &ToolHostScope,
    hooks: DeepRefHooks,
    config: AgentLoopConfig,
) -> AgentBuilder<WithBuilderTools> {
    // The replay builder already carries this owner from `over_bus`; setting
    // it again to the same value keeps both paths in one place on purpose.
    let builder = builder
        .owner(DEEPREF_ASSISTANT_OWNER)
        .preamble(assistant_system_prompt(context.project_id))
        .max_tokens(u64::from(config.max_output_tokens_per_call))
        .dynamic_tools(deepref_dynamic_tools(scope))
        .add_hook(hooks);
    with_model_parameters(builder, context)
}

/// Applies the resolved route's model parameters. Split out so temperature,
/// top-p and provider additions are mapped once for both agents.
fn with_model_parameters<S>(
    mut builder: AgentBuilder<S>,
    context: &DeepRefAgentContext,
) -> AgentBuilder<S> {
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
    builder
}

/// Maps persisted turn history onto provider messages. User and assistant
/// text travel; system and tool messages are transport-owned and never reach
/// the model here.
pub(crate) fn history_to_messages(history: &[ChatMessage]) -> Vec<Message> {
    history
        .iter()
        .filter_map(|message| match message {
            ChatMessage::User(text) => Some(Message::user(text.clone())),
            ChatMessage::Assistant { content, .. } => Some(Message::assistant(content.clone())),
            ChatMessage::System(_) | ChatMessage::Tool { .. } => None,
        })
        .collect()
}

/// Builds the runner for one prompt: history, turn cap, host tool scope,
/// single-flight tool execution and invalid-call recovery. Live turns and
/// cassette replays share it, so a budget change cannot affect only one path.
pub(crate) fn assistant_runner(
    agent: &Agent,
    user_message: String,
    history: Vec<Message>,
    scope: &ToolHostScope,
    config: AgentLoopConfig,
) -> AgentRunner {
    let tool_scope: Arc<dyn std::any::Any + Send + Sync> = Arc::new(scope.clone());
    agent
        .prompt(Message::user(user_message))
        .history(history)
        .max_turns(config.max_steps)
        .tool_context(ToolContext::new().with_scope(tool_scope))
        .tool_concurrency(1)
        .max_invalid_tool_call_retries(config.max_steps)
}
