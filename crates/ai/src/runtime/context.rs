//! Host-owned context for one DeepRef assistant turn on the Rig runtime.
//!
//! Everything the model may influence arrives as tool arguments. Everything
//! here is host-known and never model-controlled: the project scope, the
//! actor, the resolved model route and the conversation identity live in this
//! struct (and in [`ToolContext`][rig_core::tool::ToolContext] scopes), never
//! in tool schemas.

use deepref_domain::{Actor, ProjectId};
use serde_json::Value;
use uuid::Uuid;

use crate::{AiFuture, ResolvedModel};

/// Host-known authority values for one assistant turn.
///
/// Cloned into every Rig tool closure and hook state. The model never sees
/// these values except as already-decided facts (for example the project id
/// inside the system prompt); tool arguments carrying them are overwritten
/// with these values before validation.
#[derive(Debug, Clone)]
pub struct DeepRefAgentContext {
    pub project_id: ProjectId,
    pub conversation_id: Uuid,
    pub actor: Actor,
    pub route: ResolvedModel,
}

/// Project-scoped read execution for Rig assistant tools.
///
/// Reads run immediately, exactly like the custom loop's read tools. The
/// adapter injects the host project id into tool arguments before
/// validation, so implementations must treat the `project_id` parameter as
/// the enforced scope, never as a model choice.
///
/// Proposal tools do NOT go through this trait: they produce [`PlanAction`][crate::PlanAction]
/// data collected in the turn's [`PlanCollector`][crate::runtime::PlanCollector],
/// and only a confirmed plan may execute domain writes outside the agent run.
///
/// Returning [`AiError::BudgetExceeded`] aborts the turn; returning
/// [`AiError::InvalidContext`] with a plain-language message sends that
/// message back to the model as the tool result.
pub trait AssistantToolHost: Send + Sync {
    /// Executes one read tool. `tool` names a catalog read tool or one of
    /// the loop-level read tools (`get_project_overview`,
    /// `list_reports_by_screening_status`); `args` already carry the host
    /// project id.
    fn execute_read<'a>(
        &'a self,
        project_id: ProjectId,
        tool: &'a str,
        args: Value,
    ) -> AiFuture<'a, Value>;

    /// Completes a validated, policy-checked plan action with host-known
    /// facts before it joins the plan, or refuses it.
    ///
    /// This is where plan-time state is captured: for `screen_reports` the
    /// Postgres host stores each record's title, current status and screening
    /// revision in the action's `items`, so confirmation can reject records
    /// that changed after the plan was made, and gates the action on the
    /// project's AI settings. Returning [`AiError::InvalidContext`] sends the
    /// message back to the model and keeps the action out of the plan.
    ///
    /// The default keeps the action unchanged.
    ///
    /// [`AiError::InvalidContext`]: crate::AiError::InvalidContext
    fn prepare_plan_action<'a>(
        &'a self,
        project_id: ProjectId,
        action: crate::PlanAction,
    ) -> AiFuture<'a, crate::PlanAction> {
        let _ = project_id;
        Box::pin(async move { Ok(action) })
    }
}
