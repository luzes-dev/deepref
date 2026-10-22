//! DeepRef's provider-neutral AI foundation.
//!
//! The crate is split by seam: domain-safe types and hashes, provider
//! gateways, prompt definitions, grounding, policy, and task orchestration.
//! SQLx and pgvector stay in `deepref-app`.

mod admission;
mod agents;
mod assistant;
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

pub use admission::{
    AdmissionError, AdmissionFuture, AiConcurrencyAdmission, AiConcurrencyLimits,
    AiConcurrencyMetrics, AiConcurrencyMetricsSnapshot, AiConcurrencyScope, AiConcurrencyScopeKind,
    HistogramSnapshot, SLOT_LEASE_RENEW_INTERVAL, SLOT_LEASE_TTL, ScopeCounters, SlotGrant,
    SlotHeartbeat, SlotLease, admission_metrics, renew_while,
};
pub use agents::*;
pub use assistant::*;
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
    ANY_MODEL, AiGateway, EmbeddingGateway, PreparedCompletionRequest, RigEmbeddingGateway,
    RigGateway, RoutedGateway, build_metered_provider,
};
pub use grounding::GroundingContextBuilder;
pub use openai_compat::{
    OpenAiCompatGateway, ProviderDialect, strip_code_fence, structured_request_body,
};
pub use policy::{PolicyDecision, PolicyEngine, PolicyInput, ProjectAiPolicy, RequestedAction};
pub use pricing::{BillableComponents, ModelPrice, PriceBook, RouteCapabilities};
pub use prompts::{PromptDefinition, PromptRegistry, PromptVersion};
pub use review_assistance::*;
pub use runner::{
    AiExecutionContext, AiRunStore, AiTask, AiTaskResult, AiTaskRunner, Clock, EvidenceRetriever,
    IdProvider, ModelRouter, ProposalPersistence, ProposalStore, SystemClock, UuidProvider,
    interpret_structured_response, safe_error_metadata, structured_output_schema,
};
pub use runtime::{
    ASSISTANT_PROMPT_VERSION, AgentLoopConfig, ChatMessage, ChatToolCall, MAX_REPORTS_PER_ACTION,
    ManualStep, PlanAction, PlannedScreeningItem, SCREEN_REPORT_ITEMS, TOOL_FINAL_EXCLUSION,
    TOOL_LIST_REPORTS, TOOL_PROJECT_OVERVIEW, TOOL_PUBLISH_PROTOCOL, TOOL_SCREEN_REPORTS,
    ToolTraceEntry, agent_tool_declarations, assistant_system_prompt, claims_pending_change,
    is_read_tool, no_plan_notice, plan_action_is_executable, truncate_chars,
    validate_screen_reports,
};
pub use screening::{
    CriterionJudgment, CriterionPrompt, CriterionResult, ScreeningAnalysis, ScreeningEvidence,
    ScreeningEvidenceField, ScreeningInput, ScreeningStage, ScreeningTask, ScreeningTaskConfig,
    SuggestedDecision, criteria_for_stage,
};
pub use types::*;
pub(crate) use usage::{
    AssistantRequestBounds, assistant_input_bound_tokens, assistant_reservation_request,
};
pub use usage::{
    MeteredGateway, ProviderUsageLookup, UsageLedger, UsageObservation, UsagePriceSnapshot,
    UsageReservation, UsageReservationRequest, UsageTopUpRequest, ZeroLiabilityProof,
    service_usage_context,
};

#[cfg(test)]
mod tests;

#[cfg(test)]
mod agent_tests;
