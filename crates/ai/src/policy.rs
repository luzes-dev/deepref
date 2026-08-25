use crate::AuthorityTier;
use deepref_domain::{Actor, ActorKind, ProjectId};
use serde_json::Value;
use std::collections::BTreeSet;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProjectAiPolicy {
    allowed_reversible_tools: BTreeSet<String>,
}
impl ProjectAiPolicy {
    pub fn allow_reversible(mut self, tool: impl Into<String>) -> Self {
        self.allowed_reversible_tools.insert(tool.into());
        self
    }
    fn allows(&self, tool: &str) -> bool {
        self.allowed_reversible_tools.contains(tool)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct PolicyInput {
    pub actor: Actor,
    pub project_id: ProjectId,
    pub declared_project_id: ProjectId,
    pub tool: String,
    pub action: RequestedAction,
    pub authority: AuthorityTier,
    pub args: Value,
    pub project_policy: ProjectAiPolicy,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RequestedAction {
    Read,
    ReversibleMetadataWrite,
    WorkflowSuggestion,
    ScientificConclusion,
    ArbitrarySql,
    FinalExclusion,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PolicyDecision {
    ExecuteRead,
    ExecuteReversibleWrite,
    CreateProposal,
    Forbidden,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct PolicyEngine;
impl PolicyEngine {
    pub fn authorize(&self, input: &PolicyInput) -> PolicyDecision {
        if !matches!(
            input.actor.kind(),
            ActorKind::User | ActorKind::Automation | ActorKind::System
        ) || input.actor.id().trim().is_empty()
            || input.project_id != input.declared_project_id
            || input.tool.trim().is_empty()
        {
            return PolicyDecision::Forbidden;
        }
        match input.action {
            RequestedAction::ArbitrarySql | RequestedAction::FinalExclusion => {
                PolicyDecision::Forbidden
            }
            RequestedAction::Read => PolicyDecision::ExecuteRead,
            RequestedAction::ReversibleMetadataWrite => {
                if input.authority == AuthorityTier::ReversibleMetadata
                    && input.project_policy.allows(&input.tool)
                {
                    PolicyDecision::ExecuteReversibleWrite
                } else {
                    PolicyDecision::Forbidden
                }
            }
            RequestedAction::WorkflowSuggestion | RequestedAction::ScientificConclusion => {
                PolicyDecision::CreateProposal
            }
        }
    }
}
