//! Shared plan mechanics for the project assistant: tool declarations,
//! proposal validation, policy-checked plan actions, and the plan-consistency
//! helpers. Reads run immediately; every write-capable tool is intercepted,
//! authorized by the shared [`PolicyEngine`][crate::PolicyEngine] and, if
//! allowed, recorded as a [`PlanAction`] instead of being executed. The turn
//! ends with a plan that a human must confirm. Tools the policy never allows
//! (final exclusion, protocol publishing) become manual steps with a link
//! target.
//!
//! The Rig runtime owns these exact semantics; the removed custom loop once
//! shared them. Parity by construction, not by parallel implementation.
//!
//! Layout: [`catalog`] (tool declarations, prompt), [`policy`] (validation,
//! authorization, plan actions), [`consistency`] (NL claim detection). This
//! root keeps the shared turn vocabulary and the model-facing output bound.

mod catalog;
mod consistency;
mod policy;

pub(crate) use catalog::is_write_tool;
pub use catalog::{
    ASSISTANT_PROMPT_VERSION, TOOL_FINAL_EXCLUSION, TOOL_LIST_REPORTS, TOOL_PROJECT_OVERVIEW,
    TOOL_PUBLISH_PROTOCOL, TOOL_SCREEN_REPORTS, agent_tool_declarations, assistant_system_prompt,
    is_read_tool,
};
pub(crate) use consistency::NO_PLAN_CORRECTION;
pub use consistency::{claims_pending_change, no_plan_notice};
pub(crate) use policy::{MAX_ACTIONS_PER_PLAN, plan_write_call};
pub use policy::{
    MAX_REPORTS_PER_ACTION, ManualStep, PlanAction, plan_action_is_executable,
    validate_screen_reports,
};

use serde_json::Value;

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

#[derive(Debug, Clone, PartialEq)]
pub struct ToolTraceEntry {
    pub call: ChatToolCall,
    pub output: Value,
}

const MAX_TOOL_OUTPUT_CHARS: usize = 12_000;

/// Takes the first `limit` characters of `text`. The single shared core for
/// model-facing truncation: [`truncate_for_model`] (tool outputs for the Rig
/// loop, with a marker) and the Postgres assistant host's read bounds (plain
/// truncation) both funnel through here.
///
/// Deliberately NOT shared with the durable worker's `stored_output` limit
/// (`services/worker`, owned by another agent): that one persists a JSON
/// `{truncated, preview}` envelope for run events, a different semantic from
/// these model-facing plain-text bounds.
pub fn truncate_chars(text: &str, limit: usize) -> String {
    text.chars().take(limit).collect()
}

pub(crate) fn truncate_for_model(value: &Value) -> String {
    let text = value.to_string();
    if text.chars().count() <= MAX_TOOL_OUTPUT_CHARS {
        return text;
    }
    let cut = truncate_chars(&text, MAX_TOOL_OUTPUT_CHARS);
    format!("{cut}... [truncated]")
}
