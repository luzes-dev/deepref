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

mod agent;
mod context;
mod hooks;
mod model;
mod tools;

#[cfg(test)]
mod tests;

pub use agent::{RigTurn, RigTurnOutcome, run_rig_turn, run_rig_turn_channel};
pub use context::{AssistantToolHost, DeepRefAgentContext};
pub use hooks::{BUDGET_STOP_REASON, DeepRefHooks, TOKEN_STOP_REASON};
pub use model::{AgentModelFactory, OpenAiCompatModelFactory, StaticModelFactory};
pub use tools::{
    PlanCollector, RecordedToolOutput, ToolHostScope, TraceCollector, deepref_dynamic_tools,
};
