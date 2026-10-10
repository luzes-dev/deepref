//! Plan policy: validation and authorization of write tool calls.
//!
//! Every write-capable tool is intercepted, authorized by the shared
//! [`PolicyEngine`][crate::PolicyEngine] and, if allowed, recorded as a
//! [`PlanAction`] instead of being executed. The turn ends with a plan that a
//! human must confirm. Tools the policy never allows (final exclusion,
//! protocol publishing) become manual steps with a link target.

use std::collections::BTreeSet;

use deepref_domain::{Actor, ProjectId};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use uuid::Uuid;

use super::ChatToolCall;
use super::catalog::{TOOL_FINAL_EXCLUSION, TOOL_PUBLISH_PROTOCOL, TOOL_SCREEN_REPORTS};
use crate::{
    AgentToolName, AuthorityTier, PolicyDecision, PolicyEngine, PolicyInput, ProjectAiPolicy,
    RequestedAction,
};

pub(crate) const MAX_ACTIONS_PER_PLAN: usize = 25;
pub const MAX_REPORTS_PER_ACTION: usize = 200;

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
