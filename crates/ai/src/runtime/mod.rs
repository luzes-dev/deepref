//! DeepRef's interactive assistant on the Rig 0.44 agent runtime.
//!
//! Rig owns generic agent execution (the model → tool → model state machine,
//! tool dispatch mechanics, hooks, streaming, lifecycle events). DeepRef owns
//! scientific semantics, authorization, project isolation, model routing,
//! budgets, plans, persistence, human confirmation and calibration:
//!
//! * [`context`] — host-owned turn context and the [`AssistantToolHost`]
//!   read port.
//! * [`model`] — the [`AgentModelFactory`] boundary from `ResolvedModel` to
//!   Rig completion models.
//! * [`tools`] — the DeepRef tool catalog as Rig dynamic tools, with plan
//!   collection for proposal tools.
//! * [`hooks`] — budget, usage, telemetry and plan-consistency hooks.
//! * [`agent`] — the turn driver mapping Rig events onto DeepRef's assistant
//!   contract. Cassette recording attaches through [`RigTurn`][agent::RigTurn]'s
//!   recorder; replay fixtures arrive with the cassette module (PR4).
//! * [`builder`] — the single shared agent construction (preamble, tools,
//!   hooks, history, runner limits) for live runs and cassette replays.

mod agent;
mod builder;
mod cassette;
mod context;
mod hooks;
mod model;
mod plan;
mod tools;

#[cfg(test)]
mod test_fixtures;
#[cfg(test)]
mod tests_cassette;
#[cfg(test)]
mod tests_live;

pub use agent::{AgentProgress, RigTurn, RigTurnOutcome, run_rig_turn, run_rig_turn_channel};
pub use cassette::{AssistantCassette, ReplayTurn, replay_rig_turn};
pub use context::{AssistantToolHost, DeepRefAgentContext};
pub use hooks::{BUDGET_STOP_REASON, DeepRefHooks, TOKEN_STOP_REASON};
pub use model::{AgentModelFactory, OpenAiCompatModelFactory, StaticModelFactory};
pub use plan::{
    ASSISTANT_PROMPT_VERSION, AgentLoopConfig, ChatMessage, ChatToolCall, MAX_REPORTS_PER_ACTION,
    ManualStep, PlanAction, TOOL_FINAL_EXCLUSION, TOOL_LIST_REPORTS, TOOL_PROJECT_OVERVIEW,
    TOOL_PUBLISH_PROTOCOL, TOOL_SCREEN_REPORTS, ToolTraceEntry, agent_tool_declarations,
    assistant_system_prompt, claims_pending_change, is_read_tool, no_plan_notice,
    plan_action_is_executable, truncate_chars, validate_screen_reports,
};
pub use tools::{
    PlanCollector, RecordedToolOutput, ToolHostScope, TraceCollector, deepref_dynamic_tools,
};
