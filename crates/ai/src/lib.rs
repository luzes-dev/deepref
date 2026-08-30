//! DeepRef's provider-neutral AI foundation.
//!
//! The crate is split by seam: domain-safe types and hashes, provider
//! gateways, prompt definitions, grounding, policy, and task orchestration.
//! SQLx and pgvector stay in `deepref-postgres`.

mod dedupe;
mod gateway;
mod grounding;
mod policy;
mod prompts;
mod review_assistance;
mod runner;
mod screening;
mod types;

pub use dedupe::{
    DedupeInput, DedupeTask, DuplicateAssistance, DuplicateCandidate, DuplicateDecision,
    DuplicateRationale, DuplicateSignal, DuplicateSignalKind, IdentityProvenance,
};
pub use deepref_domain::{Actor, ActorKind};
pub use gateway::{AiGateway, EmbeddingGateway, RigEmbeddingGateway, RigGateway, RoutedGateway};
pub use grounding::GroundingContextBuilder;
pub use policy::{PolicyDecision, PolicyEngine, PolicyInput, ProjectAiPolicy, RequestedAction};
pub use prompts::{PromptDefinition, PromptRegistry, PromptVersion};
pub use review_assistance::*;
pub use runner::{
    AiRunStore, AiTask, AiTaskResult, AiTaskRunner, Clock, EvidenceRetriever, IdProvider,
    ModelRouter, ProposalStore, SystemClock, UuidProvider, safe_error_metadata,
};
pub use screening::{
    CriterionJudgment, CriterionPrompt, CriterionResult, ScreeningAnalysis, ScreeningEvidence,
    ScreeningEvidenceField, ScreeningInput, ScreeningStage, ScreeningTask, ScreeningTaskConfig,
    SuggestedDecision,
};
pub use types::*;

#[cfg(test)]
mod tests;
