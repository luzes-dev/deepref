use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Durable queue kinds. Spellings are the persisted database and JSON contract.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JobKind {
    WorkFetchRequested,
    RecomputeMetrics,
    #[serde(rename = "run_deduplication")]
    ImportDeduplication,
    PmidImport,
    DurableRunStep,
    AssistantAgentRun,
    RecomputePrisma,
    RetrieveDocument,
    ParseDocument,
    AiUsageReconcile,
    AiUsageExportReplay,
    BackfillWorkflowFiles,
}

/// Workload class of a durable job: the pool a worker serves and the unit of
/// independent concurrency, backpressure and failure isolation. The class is
/// derived from the [`JobKind`] at enqueue; callers never choose it.
///
/// The class set contains only classes real job kinds need. `review` is absent
/// today because review work rides `durable_run_step` (its handler kind is a
/// dispatch detail, not a queue kind); add the class when a review-owned kind
/// exists. The normative kind-to-class mapping is documented in
/// `docs/architecture.md`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkloadClass {
    Ai,
    Documents,
    Workflow,
    Integration,
    Projection,
}

impl WorkloadClass {
    /// Every workload class, in declaration order. A worker without a pool
    /// selection serves all of them.
    pub const ALL: [Self; 5] = [
        Self::Ai,
        Self::Documents,
        Self::Workflow,
        Self::Integration,
        Self::Projection,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Ai => "ai",
            Self::Documents => "documents",
            Self::Workflow => "workflow",
            Self::Integration => "integration",
            Self::Projection => "projection",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("unknown workload class: {0}")]
pub struct InvalidWorkloadClass(pub String);

impl std::str::FromStr for WorkloadClass {
    type Err = InvalidWorkloadClass;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "ai" => Ok(Self::Ai),
            "documents" => Ok(Self::Documents),
            "workflow" => Ok(Self::Workflow),
            "integration" => Ok(Self::Integration),
            "projection" => Ok(Self::Projection),
            _ => Err(InvalidWorkloadClass(value.to_owned())),
        }
    }
}

impl std::fmt::Display for WorkloadClass {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl JobKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::WorkFetchRequested => "work_fetch_requested",
            Self::RecomputeMetrics => "recompute_metrics",
            Self::ImportDeduplication => "run_deduplication",
            Self::PmidImport => "pmid_import",
            Self::DurableRunStep => "durable_run_step",
            Self::AssistantAgentRun => "assistant_agent_run",
            Self::RecomputePrisma => "recompute_prisma",
            Self::RetrieveDocument => "retrieve_document",
            Self::ParseDocument => "parse_document",
            Self::AiUsageReconcile => "ai_usage_reconcile",
            Self::AiUsageExportReplay => "ai_usage_export_replay",
            Self::BackfillWorkflowFiles => "backfill_workflow_files",
        }
    }

    /// The workload class this kind runs in. Derived from the kind alone and
    /// stored on the job row at enqueue so claims can filter by pool.
    pub const fn workload_class(self) -> WorkloadClass {
        match self {
            // Model calls, workspace-scoped usage recovery and the platform
            // usage export replay.
            Self::AssistantAgentRun | Self::AiUsageReconcile | Self::AiUsageExportReplay => {
                WorkloadClass::Ai
            }
            // The document pipeline: full-text retrieval feeds parsing, and
            // corpus deduplication is CPU/memory-bound local processing.
            Self::RetrieveDocument | Self::ParseDocument | Self::ImportDeduplication => {
                WorkloadClass::Documents
            }
            // Durable automation, workflow and review step dispatch.
            Self::DurableRunStep | Self::BackfillWorkflowFiles => WorkloadClass::Workflow,
            // External provider traffic, bounded by provider rate limits.
            Self::WorkFetchRequested | Self::PmidImport => WorkloadClass::Integration,
            // Derived read-model recomputes.
            Self::RecomputeMetrics | Self::RecomputePrisma => WorkloadClass::Projection,
        }
    }
}

/// The sole step dispatcher routes to typed domain handlers, not queue kinds
/// with independent lifecycles.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DurableHandlerKind {
    Automation,
    Workflow,
    Review,
}

impl DurableHandlerKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Automation => "automation",
            Self::Workflow => "workflow",
            Self::Review => "review",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("unknown durable job kind: {0}")]
pub struct InvalidJobKind(pub String);

impl std::str::FromStr for JobKind {
    type Err = InvalidJobKind;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "work_fetch_requested" => Ok(Self::WorkFetchRequested),
            "recompute_metrics" => Ok(Self::RecomputeMetrics),
            "run_deduplication" => Ok(Self::ImportDeduplication),
            "pmid_import" => Ok(Self::PmidImport),
            "durable_run_step" => Ok(Self::DurableRunStep),
            "assistant_agent_run" => Ok(Self::AssistantAgentRun),
            "recompute_prisma" => Ok(Self::RecomputePrisma),
            "retrieve_document" => Ok(Self::RetrieveDocument),
            "parse_document" => Ok(Self::ParseDocument),
            "ai_usage_reconcile" => Ok(Self::AiUsageReconcile),
            "ai_usage_export_replay" => Ok(Self::AiUsageExportReplay),
            "backfill_workflow_files" => Ok(Self::BackfillWorkflowFiles),
            _ => Err(InvalidJobKind(value.to_owned())),
        }
    }
}

impl std::fmt::Display for JobKind {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JobState {
    Queued,
    Running,
    Completed,
    Failed,
    Dead,
}

impl JobState {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Running => "running",
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Dead => "dead",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("unknown durable job state: {0}")]
pub struct InvalidJobState(pub String);

impl std::str::FromStr for JobState {
    type Err = InvalidJobState;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "queued" => Ok(Self::Queued),
            "running" => Ok(Self::Running),
            "completed" => Ok(Self::Completed),
            "failed" => Ok(Self::Failed),
            "dead" => Ok(Self::Dead),
            _ => Err(InvalidJobState(value.to_owned())),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{InvalidWorkloadClass, JobKind, JobState, WorkloadClass};

    #[test]
    fn queued_wire_kinds_round_trip() {
        for kind in [
            JobKind::WorkFetchRequested,
            JobKind::RecomputeMetrics,
            JobKind::ImportDeduplication,
            JobKind::PmidImport,
            JobKind::DurableRunStep,
            JobKind::AssistantAgentRun,
            JobKind::RecomputePrisma,
            JobKind::RetrieveDocument,
            JobKind::ParseDocument,
            JobKind::AiUsageReconcile,
            JobKind::BackfillWorkflowFiles,
        ] {
            assert_eq!(kind.as_str().parse::<JobKind>().unwrap(), kind);
            assert_eq!(serde_json::to_value(kind).unwrap(), kind.as_str());
        }
    }

    #[test]
    fn invalid_persisted_kind_is_an_error() {
        assert!("not_a_job".parse::<JobKind>().is_err());
        assert!(serde_json::from_str::<JobKind>("\"not_a_job\"").is_err());
    }

    /// The mapping is normative: every kind maps to exactly one class, only
    /// classes real kinds need exist, and the persisted spellings round-trip.
    #[test]
    fn every_job_kind_maps_to_one_workload_class() {
        for (kind, class) in [
            (JobKind::AssistantAgentRun, WorkloadClass::Ai),
            (JobKind::AiUsageReconcile, WorkloadClass::Ai),
            (JobKind::RetrieveDocument, WorkloadClass::Documents),
            (JobKind::ParseDocument, WorkloadClass::Documents),
            (JobKind::ImportDeduplication, WorkloadClass::Documents),
            (JobKind::DurableRunStep, WorkloadClass::Workflow),
            (JobKind::BackfillWorkflowFiles, WorkloadClass::Workflow),
            (JobKind::WorkFetchRequested, WorkloadClass::Integration),
            (JobKind::PmidImport, WorkloadClass::Integration),
            (JobKind::RecomputeMetrics, WorkloadClass::Projection),
            (JobKind::RecomputePrisma, WorkloadClass::Projection),
        ] {
            assert_eq!(kind.workload_class(), class);
        }
    }

    #[test]
    fn workload_classes_round_trip_and_reject_unknown_names() {
        for class in WorkloadClass::ALL {
            assert_eq!(class.as_str().parse::<WorkloadClass>().unwrap(), class);
            assert_eq!(class.to_string(), class.as_str());
            assert_eq!(serde_json::to_value(class).unwrap(), class.as_str());
        }
        assert_eq!(WorkloadClass::ALL.len(), 5);
        assert!(
            matches!(
                "review".parse::<WorkloadClass>(),
                Err(InvalidWorkloadClass(name)) if name == "review"
            ),
            "classes without real job kinds are not part of the pool set"
        );
        assert!(matches!(
            "not_a_class".parse::<WorkloadClass>(),
            Err(InvalidWorkloadClass(_))
        ));
    }

    #[test]
    fn durable_job_states_round_trip_and_reject_invalid_values() {
        for state in [
            JobState::Queued,
            JobState::Running,
            JobState::Completed,
            JobState::Failed,
            JobState::Dead,
        ] {
            assert_eq!(state.as_str().parse::<JobState>().unwrap(), state);
        }
        for invalid in ["", "complete", "RUNNING", " queued"] {
            assert!(invalid.parse::<JobState>().is_err());
        }
    }
}
