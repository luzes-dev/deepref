//! DeepRef tools as Rig dynamic tools.
//!
//! The catalog is NOT rebuilt: every Rig tool maps one-to-one onto
//! [`agent_tool_declarations`][crate::agent_tool_declarations], the same
//! declarations the custom loop sends the model. Rig owns dispatch
//! mechanics; this adapter owns DeepRef semantics:
//!
//! ```text
//! Rig tool call
//!     |
//!     v
//! DeepRef Rig adapter (this module)
//!     |
//!     v
//! AgentTool::from_name_and_args
//!     |
//!     v
//! validate
//!     |
//!     v
//! PolicyEngine (writes) / direct execution (reads)
//!     |
//!     +---- authorized read ----> AssistantToolHost (scoped data)
//!     |
//!     +---- proposal -----------> PlanAction in PlanCollector
//! ```
//!
//! Project scope is host-controlled: the adapter overwrites any
//! model-supplied `project_id` with the host context before validation.
//! Proposal tools never mutate scientific state; they append [`PlanAction`]
//! data to the turn's [`PlanCollector`], which the host drains after the run
//! for persistence and human confirmation.

use std::sync::{Arc, Mutex};

use rig_core::message::ToolName;
use rig_core::tool::{DynamicTool, ToolContext, ToolExecutionError, ToolOutput};
use serde_json::{Value, json};

use crate::{
    AiError, ChatToolCall, PlanAction, agent_tool_declarations, is_read_tool,
    runtime::context::{AssistantToolHost, DeepRefAgentContext},
};

/// Model-visible error envelope, mirroring the custom loop's tool errors.
fn error_output(message: &str) -> Value {
    json!({"error": message})
}

/// Plan actions collected by one turn's proposal tools.
///
/// Shared between tool callbacks (which push) and the turn driver (which
/// drains after the run for plan persistence). Proposal tools may be
/// re-executed by worker retries or cassette replay; recreating `PlanAction`
/// data is safe because applying a plan still requires human confirmation
/// outside the agent run.
#[derive(Debug, Clone, Default)]
pub struct PlanCollector {
    inner: Arc<Mutex<Vec<PlanAction>>>,
}

impl PlanCollector {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push(&self, action: PlanAction) {
        if let Ok(mut actions) = self.inner.lock() {
            actions.push(action);
        }
    }

    pub fn actions(&self) -> Vec<PlanAction> {
        self.inner
            .lock()
            .map(|actions| actions.clone())
            .unwrap_or_default()
    }

    pub fn len(&self) -> usize {
        self.inner.lock().map(|actions| actions.len()).unwrap_or(0)
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// Full-fidelity tool outputs recorded by the adapter, in execution order.
///
/// The model sees truncated text (the custom loop's 12k bound is preserved);
/// the driver matches these records to Rig's tool-call events by tool name
/// and arguments, so persisted traces and evidence hashes keep full values.
/// Recording full outputs here is replay-safe: replay re-executes the tools
/// and re-records, exactly like plan actions.
#[derive(Debug, Clone, Default)]
pub struct TraceCollector {
    inner: Arc<Mutex<Vec<RecordedToolOutput>>>,
}

/// One executed tool call with its full output value.
#[derive(Debug, Clone, PartialEq)]
pub struct RecordedToolOutput {
    pub tool: String,
    pub args: Value,
    pub output: Value,
}

impl TraceCollector {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push(&self, record: RecordedToolOutput) {
        if let Ok(mut records) = self.inner.lock() {
            records.push(record);
        }
    }

    pub fn drain(&self) -> Vec<RecordedToolOutput> {
        self.inner
            .lock()
            .map(|mut records| std::mem::take(&mut *records))
            .unwrap_or_default()
    }

    /// Removes the first record matching a Rig tool-call event, so the turn
    /// driver can pair adapter outputs (full values) with Rig call ids in
    /// execution order. Duplicate identical calls match in order.
    pub fn take_matching(&self, tool: &str, args: &Value) -> Option<RecordedToolOutput> {
        self.inner.lock().ok().and_then(|mut records| {
            records
                .iter()
                .position(|record| record.tool == tool && &record.args == args)
                .map(|index| records.remove(index))
        })
    }
}

/// Live host state attached to every tool dispatch via [`ToolContext`]
/// scopes. Scopes are excluded from serialization, so effect logs never
/// capture credentials, pools or collectors.
#[derive(Clone)]
pub struct ToolHostScope {
    pub context: DeepRefAgentContext,
    pub host: Arc<dyn AssistantToolHost>,
    pub plans: PlanCollector,
    pub trace: TraceCollector,
    /// Set when a read hits budget exhaustion: the turn must abort with
    /// [`AiError::BudgetExceeded`], mirroring the custom loop.
    pub budget_exceeded: Arc<std::sync::atomic::AtomicBool>,
}

impl ToolHostScope {
    pub fn new(
        context: DeepRefAgentContext,
        host: Arc<dyn AssistantToolHost>,
        plans: PlanCollector,
    ) -> Self {
        Self {
            context,
            host,
            plans,
            trace: TraceCollector::new(),
            budget_exceeded: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        }
    }
}

/// Builds one Rig dynamic tool per assistant declaration, closing over a
/// clone of `scope`. Unknown model calls never reach a tool: Rig routes them
/// to invalid-call recovery, which the hooks answer with model feedback.
pub fn deepref_dynamic_tools(scope: &ToolHostScope) -> Vec<DynamicTool> {
    agent_tool_declarations()
        .into_iter()
        .filter_map(|declaration| {
            let name = declaration.name.clone();
            let scope = scope.clone();
            ToolName::new(declaration.name).ok().map(|tool_name| {
                DynamicTool::new_with_context(
                    tool_name,
                    declaration.description,
                    declaration.parameters,
                    move |_context: &mut ToolContext, args: Value| {
                        let scope = scope.clone();
                        let name = name.clone();
                        Box::pin(async move { execute_tool_call(&scope, &name, args).await })
                    },
                )
            })
        })
        .collect()
}

async fn execute_tool_call(
    scope: &ToolHostScope,
    name: &str,
    args: Value,
) -> Result<ToolOutput, ToolExecutionError> {
    if is_read_tool(name) {
        execute_read_tool(scope, name, args).await
    } else if crate::agent_loop::is_write_tool(name) {
        execute_write_tool(scope, name, args)
    } else {
        Ok(ToolOutput::json(error_output("unknown tool")))
    }
}

async fn execute_read_tool(
    scope: &ToolHostScope,
    name: &str,
    args: Value,
) -> Result<ToolOutput, ToolExecutionError> {
    // The host scope is injected into a clone: the trace keeps the model's
    // original arguments, exactly like the custom loop's trace entries.
    let mut scoped = args.clone();
    if let Some(object) = scoped.as_object_mut() {
        object.insert(
            "project_id".to_owned(),
            json!(scope.context.project_id.as_uuid()),
        );
    } else if scoped.is_null() {
        scoped = json!({"project_id": scope.context.project_id.as_uuid()});
    }
    match scope
        .host
        .execute_read(scope.context.project_id, name, scoped)
        .await
    {
        Ok(value) => {
            scope.trace.push(RecordedToolOutput {
                tool: name.to_owned(),
                args,
                output: value.clone(),
            });
            // The model sees truncated text, exactly like the custom loop;
            // the full value stays in the trace for persistence and evidence.
            Ok(ToolOutput::text(crate::agent_loop::truncate_for_model(
                &value,
            )))
        }
        Err(AiError::BudgetExceeded) => {
            scope
                .budget_exceeded
                .store(true, std::sync::atomic::Ordering::SeqCst);
            Err(ToolExecutionError::from_error(AiError::BudgetExceeded))
        }
        Err(AiError::InvalidContext(message)) => {
            let output = error_output(&message);
            scope.trace.push(RecordedToolOutput {
                tool: name.to_owned(),
                args,
                output: output.clone(),
            });
            Ok(ToolOutput::json(output))
        }
        Err(_) => {
            let output = error_output("the tool failed");
            scope.trace.push(RecordedToolOutput {
                tool: name.to_owned(),
                args,
                output: output.clone(),
            });
            Ok(ToolOutput::json(output))
        }
    }
}

fn execute_write_tool(
    scope: &ToolHostScope,
    name: &str,
    args: Value,
) -> Result<ToolOutput, ToolExecutionError> {
    if scope.plans.len() >= crate::agent_loop::MAX_ACTIONS_PER_PLAN {
        return Ok(ToolOutput::json(error_output(
            "the plan is full; ask the user to confirm it before adding more",
        )));
    }
    // The Rig call id is not visible to tool callbacks; plan actions carry
    // their own `a{index}` identity, so the id is unused here. Trace ids
    // come from Rig's tool-call events in the turn driver.
    let call = ChatToolCall {
        id: String::new(),
        name: name.to_owned(),
        arguments: args.clone(),
    };
    match crate::agent_loop::plan_write_call(
        scope.context.project_id,
        &scope.context.actor,
        &call,
        scope.plans.len(),
    ) {
        Ok(action) => {
            let output = if action.executable {
                json!({"status": "queued_in_plan", "note": "Not executed. It only runs if the user confirms the plan."})
            } else {
                json!({"status": "user_must_do_this", "note": "You may not do this. The user is shown a link to do it themselves."})
            };
            scope.trace.push(RecordedToolOutput {
                tool: name.to_owned(),
                args,
                output: output.clone(),
            });
            scope.plans.push(action);
            Ok(ToolOutput::json(output))
        }
        Err(message) => {
            let output = error_output(&message);
            scope.trace.push(RecordedToolOutput {
                tool: name.to_owned(),
                args,
                output: output.clone(),
            });
            Ok(ToolOutput::json(output))
        }
    }
}
