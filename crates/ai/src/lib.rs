//! DeepRef's provider-neutral AI foundation.
//!
//! The crate is split by seam: domain-safe types and hashes, provider
//! gateways, prompt definitions, grounding, policy, and task orchestration.
//! SQLx and pgvector stay in `deepref-postgres`.

mod agent_loop;
mod agents;
mod assistant;
mod chat;
mod classification;
mod dedupe;
mod endpoint;
mod evals;
mod gateway;
mod grounding;
mod openai_compat;
mod policy;
mod pricing;
mod prompts;
mod review_assistance;
mod runner;
pub mod runtime;
mod screening;
mod structured;
mod types;
mod usage;

pub use agent_loop::*;
pub use agents::*;
pub use assistant::*;
pub use chat::*;
pub use classification::{
    ClassificationReportField, StudyDesignClassification, StudyDesignClassificationInput,
    StudyDesignClassificationTask, StudyDesignEvidence, StudyDesignLabel, StudyDesignReport,
    StudyMetadataField,
};
pub use dedupe::{
    DedupeInput, DedupeTask, DuplicateAssistance, DuplicateCandidate, DuplicateDecision,
    DuplicateRationale, DuplicateSignal, DuplicateSignalKind, IdentityProvenance,
};
pub use deepref_domain::{Actor, ActorKind};
pub use endpoint::{
    ProviderEndpoint, ProviderEndpointError, provider_endpoint, register_provider_endpoint,
};
pub use evals::*;
pub use gateway::{
    ANY_MODEL, AiGateway, EmbeddingGateway, RigEmbeddingGateway, RigGateway, RoutedGateway,
    build_metered_provider,
};
pub use grounding::GroundingContextBuilder;
pub use openai_compat::{
    OpenAiCompatGateway, ProviderDialect, strip_code_fence, structured_request_body,
};
pub use policy::{PolicyDecision, PolicyEngine, PolicyInput, ProjectAiPolicy, RequestedAction};
pub use pricing::{
    GLM_5_3_FLASH_PRICE, ModelPrice, PriceBook, UNKNOWN_MODEL_PRICE, estimate_cost_micros,
    price_for_model,
};
pub use prompts::{PromptDefinition, PromptRegistry, PromptVersion};
pub use review_assistance::*;
pub use runner::{
    AiExecutionContext, AiRunStore, AiTask, AiTaskResult, AiTaskRunner, Clock, EvidenceRetriever,
    IdProvider, ModelRouter, ProposalPersistence, ProposalStore, SystemClock, UuidProvider,
    interpret_structured_response, safe_error_metadata, structured_output_schema,
};
pub use screening::{
    CriterionJudgment, CriterionPrompt, CriterionResult, ScreeningAnalysis, ScreeningEvidence,
    ScreeningEvidenceField, ScreeningInput, ScreeningStage, ScreeningTask, ScreeningTaskConfig,
    SuggestedDecision, criteria_for_stage,
};
pub use types::*;
pub use usage::{BudgetSnapshot, MeteredGateway, UsageEntry, UsageLedger};

#[cfg(test)]
mod tests;

#[cfg(test)]
mod agent_tests;
