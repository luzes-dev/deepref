use deepref_domain::{Actor, ProjectId};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use uuid::Uuid;

pub const MAX_AUTOMATION_NAME_LENGTH: usize = 200;
pub const MAX_AUTOMATION_IDEMPOTENCY_KEY_LENGTH: usize = 200;
pub const MAX_AUTOMATION_REFERENCE_LENGTH: usize = 500;
pub const MAX_AUTOMATION_ACTOR_ID_LENGTH: usize = 200;
pub const MAX_AUTOMATION_WORKER_ID_LENGTH: usize = 200;
pub const MAX_AUTOMATION_ERROR_LENGTH: usize = 4_096;
pub const MAX_AUTOMATION_RUN_LIST_LIMIT: i64 = 100;

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum AutomationValidationError {
    #[error("automation project id must not be nil")]
    NilProjectId,
    #[error("automation id must not be nil")]
    NilId,
    #[error("automation name must not be blank")]
    BlankName,
    #[error("automation name exceeds {MAX_AUTOMATION_NAME_LENGTH} bytes")]
    NameTooLong,
    #[error("automation idempotency key must not be blank")]
    BlankIdempotencyKey,
    #[error("automation idempotency key exceeds {MAX_AUTOMATION_IDEMPOTENCY_KEY_LENGTH} bytes")]
    IdempotencyKeyTooLong,
    #[error("automation trigger reference must not be blank")]
    BlankTriggerReference,
    #[error("automation trigger reference exceeds {MAX_AUTOMATION_REFERENCE_LENGTH} bytes")]
    TriggerReferenceTooLong,
    #[error("automation actor id must not be blank")]
    BlankActorId,
    #[error("automation actor id exceeds {MAX_AUTOMATION_ACTOR_ID_LENGTH} bytes")]
    ActorIdTooLong,
    #[error("automation worker id must not be blank")]
    BlankWorkerId,
    #[error("automation worker id exceeds {MAX_AUTOMATION_WORKER_ID_LENGTH} bytes")]
    WorkerIdTooLong,
    #[error("automation error must not be blank")]
    BlankError,
    #[error("automation error exceeds {MAX_AUTOMATION_ERROR_LENGTH} bytes")]
    ErrorTooLong,
    #[error("automation run list limit must be between 1 and {MAX_AUTOMATION_RUN_LIST_LIMIT}")]
    InvalidRunListLimit,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct AutomationDefinitionId(Uuid);

impl AutomationDefinitionId {
    pub fn new(value: Uuid) -> Result<Self, AutomationValidationError> {
        if value.is_nil() {
            return Err(AutomationValidationError::NilId);
        }
        Ok(Self(value))
    }

    pub const fn as_uuid(&self) -> Uuid {
        self.0
    }
}

impl TryFrom<Uuid> for AutomationDefinitionId {
    type Error = AutomationValidationError;

    fn try_from(value: Uuid) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl From<AutomationDefinitionId> for Uuid {
    fn from(value: AutomationDefinitionId) -> Self {
        value.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct AutomationRunId(Uuid);

impl AutomationRunId {
    pub fn new(value: Uuid) -> Result<Self, AutomationValidationError> {
        if value.is_nil() {
            return Err(AutomationValidationError::NilId);
        }
        Ok(Self(value))
    }

    pub const fn as_uuid(&self) -> Uuid {
        self.0
    }
}

impl TryFrom<Uuid> for AutomationRunId {
    type Error = AutomationValidationError;

    fn try_from(value: Uuid) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl From<AutomationRunId> for Uuid {
    fn from(value: AutomationRunId) -> Self {
        value.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct AutomationStepRunId(Uuid);

impl AutomationStepRunId {
    pub fn new(value: Uuid) -> Result<Self, AutomationValidationError> {
        if value.is_nil() {
            return Err(AutomationValidationError::NilId);
        }
        Ok(Self(value))
    }

    pub const fn as_uuid(&self) -> Uuid {
        self.0
    }
}

impl TryFrom<Uuid> for AutomationStepRunId {
    type Error = AutomationValidationError;

    fn try_from(value: Uuid) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl From<AutomationStepRunId> for Uuid {
    fn from(value: AutomationStepRunId) -> Self {
        value.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct AutomationName(String);

impl AutomationName {
    pub fn new(value: impl Into<String>) -> Result<Self, AutomationValidationError> {
        let value = value.into();
        if value.trim().is_empty() {
            return Err(AutomationValidationError::BlankName);
        }
        if value.len() > MAX_AUTOMATION_NAME_LENGTH {
            return Err(AutomationValidationError::NameTooLong);
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct AutomationIdempotencyKey(String);

impl AutomationIdempotencyKey {
    pub fn new(value: impl Into<String>) -> Result<Self, AutomationValidationError> {
        let value = value.into();
        if value.trim().is_empty() {
            return Err(AutomationValidationError::BlankIdempotencyKey);
        }
        if value.len() > MAX_AUTOMATION_IDEMPOTENCY_KEY_LENGTH {
            return Err(AutomationValidationError::IdempotencyKeyTooLong);
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct AutomationTriggerReference(String);

impl AutomationTriggerReference {
    pub fn new(value: impl Into<String>) -> Result<Self, AutomationValidationError> {
        let value = value.into();
        if value.trim().is_empty() {
            return Err(AutomationValidationError::BlankTriggerReference);
        }
        if value.len() > MAX_AUTOMATION_REFERENCE_LENGTH {
            return Err(AutomationValidationError::TriggerReferenceTooLong);
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AutomationTriggerKind {
    ReportAdded,
    AcquisitionCompleted,
    FullTextAttached,
    ReportIncluded,
    StudyCreated,
    AppraisalCompleted,
    Manual,
}

impl AutomationTriggerKind {
    pub const ALL: [Self; 7] = [
        Self::ReportAdded,
        Self::AcquisitionCompleted,
        Self::FullTextAttached,
        Self::ReportIncluded,
        Self::StudyCreated,
        Self::AppraisalCompleted,
        Self::Manual,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ReportAdded => "report_added",
            Self::AcquisitionCompleted => "acquisition_completed",
            Self::FullTextAttached => "full_text_attached",
            Self::ReportIncluded => "report_included",
            Self::StudyCreated => "study_created",
            Self::AppraisalCompleted => "appraisal_completed",
            Self::Manual => "manual",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|kind| kind.as_str() == value)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BuiltInAutomationRecipe {
    ProjectMaintenanceV1,
}

impl BuiltInAutomationRecipe {
    pub const ALL: [Self; 1] = [Self::ProjectMaintenanceV1];

    pub const fn id(self) -> &'static str {
        match self {
            Self::ProjectMaintenanceV1 => "project_maintenance",
        }
    }

    pub const fn version(self) -> i32 {
        match self {
            Self::ProjectMaintenanceV1 => 1,
        }
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ProjectMaintenanceV1 => "project_maintenance.v1",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|recipe| recipe.as_str() == value)
    }

    pub const fn steps(self) -> &'static [BuiltInAutomationStep] {
        match self {
            Self::ProjectMaintenanceV1 => &[BuiltInAutomationStep {
                ordinal: 0,
                key: "recompute_project_metrics",
                kind: AutomationStepKind::DeterministicAction,
            }],
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct BuiltInAutomationStep {
    pub ordinal: i32,
    pub key: &'static str,
    pub kind: AutomationStepKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AutomationStepKind {
    DeterministicAction,
    AiTask,
    Agent,
    Notification,
    DomainCommand,
    DomainProposal,
}

impl AutomationStepKind {
    pub const ALL: [Self; 6] = [
        Self::DeterministicAction,
        Self::AiTask,
        Self::Agent,
        Self::Notification,
        Self::DomainCommand,
        Self::DomainProposal,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::DeterministicAction => "deterministic_action",
            Self::AiTask => "ai_task",
            Self::Agent => "agent",
            Self::Notification => "notification",
            Self::DomainCommand => "domain_command",
            Self::DomainProposal => "domain_proposal",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|kind| kind.as_str() == value)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AutomationDefinitionStatus {
    Active,
    Paused,
}

impl AutomationDefinitionStatus {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Paused => "paused",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "active" => Some(Self::Active),
            "paused" => Some(Self::Paused),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AutomationRunStatus {
    Queued,
    Running,
    Completed,
    Failed,
}

impl AutomationRunStatus {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Running => "running",
            Self::Completed => "completed",
            Self::Failed => "failed",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "queued" => Some(Self::Queued),
            "running" => Some(Self::Running),
            "completed" => Some(Self::Completed),
            "failed" => Some(Self::Failed),
            _ => None,
        }
    }

    pub fn transition_to(self, next: Self) -> Result<Self, AutomationTransitionError> {
        let allowed = match self {
            Self::Queued => matches!(next, Self::Running),
            Self::Running => matches!(next, Self::Completed | Self::Failed),
            Self::Completed => false,
            Self::Failed => matches!(next, Self::Queued),
        };
        if allowed {
            Ok(next)
        } else {
            Err(AutomationTransitionError::Run {
                from: self,
                to: next,
            })
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AutomationStepRunStatus {
    Pending,
    Running,
    Completed,
    Failed,
}

impl AutomationStepRunStatus {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Running => "running",
            Self::Completed => "completed",
            Self::Failed => "failed",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "pending" => Some(Self::Pending),
            "running" => Some(Self::Running),
            "completed" => Some(Self::Completed),
            "failed" => Some(Self::Failed),
            _ => None,
        }
    }

    pub fn transition_to(self, next: Self) -> Result<Self, AutomationTransitionError> {
        let allowed = match self {
            Self::Pending => matches!(next, Self::Running),
            Self::Running => matches!(next, Self::Completed | Self::Failed),
            Self::Completed => false,
            Self::Failed => matches!(next, Self::Running),
        };
        if allowed {
            Ok(next)
        } else {
            Err(AutomationTransitionError::Step {
                from: self,
                to: next,
            })
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AutomationJobStatus {
    Queued,
    Running,
    Completed,
    Failed,
    Dead,
}

impl AutomationJobStatus {
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "queued" => Some(Self::Queued),
            "running" => Some(Self::Running),
            "completed" => Some(Self::Completed),
            "failed" => Some(Self::Failed),
            "dead" => Some(Self::Dead),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum AutomationTransitionError {
    #[error("invalid automation run transition from {from:?} to {to:?}")]
    Run {
        from: AutomationRunStatus,
        to: AutomationRunStatus,
    },
    #[error("invalid automation step transition from {from:?} to {to:?}")]
    Step {
        from: AutomationStepRunStatus,
        to: AutomationStepRunStatus,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigureAutomationDefinition {
    pub project_id: ProjectId,
    pub name: AutomationName,
    pub trigger: AutomationTriggerKind,
    pub recipe: BuiltInAutomationRecipe,
    pub status: AutomationDefinitionStatus,
    pub actor: Actor,
}

impl ConfigureAutomationDefinition {
    pub fn new(
        project_id: ProjectId,
        name: impl Into<String>,
        trigger: AutomationTriggerKind,
        recipe: BuiltInAutomationRecipe,
        status: AutomationDefinitionStatus,
        actor: Actor,
    ) -> Result<Self, AutomationValidationError> {
        let request = Self {
            project_id,
            name: AutomationName::new(name)?,
            trigger,
            recipe,
            status,
            actor,
        };
        request.validate()?;
        Ok(request)
    }

    pub fn validate(&self) -> Result<(), AutomationValidationError> {
        validate_project_id(self.project_id)?;
        validate_actor(&self.actor)?;
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DispatchAutomationTrigger {
    pub project_id: ProjectId,
    pub definition_id: AutomationDefinitionId,
    pub trigger: AutomationTriggerKind,
    pub trigger_reference: Option<AutomationTriggerReference>,
    pub idempotency_key: AutomationIdempotencyKey,
    pub actor: Actor,
}

impl DispatchAutomationTrigger {
    pub fn new(
        project_id: ProjectId,
        definition_id: Uuid,
        trigger: AutomationTriggerKind,
        trigger_reference: Option<String>,
        idempotency_key: impl Into<String>,
        actor: Actor,
    ) -> Result<Self, AutomationValidationError> {
        let request = Self {
            project_id,
            definition_id: AutomationDefinitionId::new(definition_id)?,
            trigger,
            trigger_reference: trigger_reference
                .map(AutomationTriggerReference::new)
                .transpose()?,
            idempotency_key: AutomationIdempotencyKey::new(idempotency_key)?,
            actor,
        };
        request.validate()?;
        Ok(request)
    }

    pub fn validate(&self) -> Result<(), AutomationValidationError> {
        validate_project_id(self.project_id)?;
        validate_actor(&self.actor)?;
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StartAutomationManually {
    pub project_id: ProjectId,
    pub definition_id: AutomationDefinitionId,
    pub idempotency_key: AutomationIdempotencyKey,
    pub actor: Actor,
}

impl StartAutomationManually {
    pub fn new(
        project_id: ProjectId,
        definition_id: Uuid,
        idempotency_key: impl Into<String>,
        actor: Actor,
    ) -> Result<Self, AutomationValidationError> {
        let request = Self {
            project_id,
            definition_id: AutomationDefinitionId::new(definition_id)?,
            idempotency_key: AutomationIdempotencyKey::new(idempotency_key)?,
            actor,
        };
        request.validate()?;
        Ok(request)
    }

    pub fn validate(&self) -> Result<(), AutomationValidationError> {
        validate_project_id(self.project_id)?;
        validate_actor(&self.actor)?;
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AutomationDefinition {
    pub id: AutomationDefinitionId,
    pub project_id: ProjectId,
    pub name: AutomationName,
    pub trigger: AutomationTriggerKind,
    pub recipe: BuiltInAutomationRecipe,
    pub status: AutomationDefinitionStatus,
    pub actor: Actor,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
    pub steps: Vec<AutomationStepSnapshot>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AutomationStepSnapshot {
    pub ordinal: i32,
    pub key: String,
    pub kind: AutomationStepKind,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AutomationJobVisibility {
    pub id: Uuid,
    pub status: AutomationJobStatus,
    pub attempts: i32,
    pub max_attempts: i32,
    pub available_at: chrono::DateTime<chrono::Utc>,
    pub leased_until: Option<chrono::DateTime<chrono::Utc>>,
    pub last_error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AutomationUsage {
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cost_micros: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AutomationRun {
    pub id: AutomationRunId,
    pub project_id: ProjectId,
    pub definition_id: AutomationDefinitionId,
    pub recipe: BuiltInAutomationRecipe,
    pub trigger: AutomationTriggerKind,
    pub trigger_reference: Option<AutomationTriggerReference>,
    pub idempotency_key: AutomationIdempotencyKey,
    pub actor: Actor,
    pub status: AutomationRunStatus,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub started_at: Option<chrono::DateTime<chrono::Utc>>,
    pub finished_at: Option<chrono::DateTime<chrono::Utc>>,
    pub error: Option<String>,
    pub job: AutomationJobVisibility,
    pub steps: Vec<AutomationStepRun>,
    pub usage: AutomationUsage,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AutomationStepRun {
    pub id: AutomationStepRunId,
    pub project_id: ProjectId,
    pub run_id: AutomationRunId,
    pub ordinal: i32,
    pub key: String,
    pub kind: AutomationStepKind,
    pub status: AutomationStepRunStatus,
    pub attempts: i32,
    pub claimed_by: Option<String>,
    pub started_at: Option<chrono::DateTime<chrono::Utc>>,
    pub finished_at: Option<chrono::DateTime<chrono::Utc>>,
    pub error: Option<String>,
}

pub fn validate_worker_id(value: &str) -> Result<(), AutomationValidationError> {
    if value.trim().is_empty() {
        return Err(AutomationValidationError::BlankWorkerId);
    }
    if value.len() > MAX_AUTOMATION_WORKER_ID_LENGTH {
        return Err(AutomationValidationError::WorkerIdTooLong);
    }
    Ok(())
}

pub fn validate_error(value: &str) -> Result<(), AutomationValidationError> {
    if value.trim().is_empty() {
        return Err(AutomationValidationError::BlankError);
    }
    if value.len() > MAX_AUTOMATION_ERROR_LENGTH {
        return Err(AutomationValidationError::ErrorTooLong);
    }
    Ok(())
}

pub fn validate_run_list_limit(value: i64) -> Result<(), AutomationValidationError> {
    if !(1..=MAX_AUTOMATION_RUN_LIST_LIMIT).contains(&value) {
        return Err(AutomationValidationError::InvalidRunListLimit);
    }
    Ok(())
}

fn validate_project_id(project_id: ProjectId) -> Result<(), AutomationValidationError> {
    if project_id.as_uuid().is_nil() {
        Err(AutomationValidationError::NilProjectId)
    } else {
        Ok(())
    }
}

fn validate_actor(actor: &Actor) -> Result<(), AutomationValidationError> {
    if actor.id().trim().is_empty() {
        return Err(AutomationValidationError::BlankActorId);
    }
    if actor.id().len() > MAX_AUTOMATION_ACTOR_ID_LENGTH {
        return Err(AutomationValidationError::ActorIdTooLong);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use deepref_domain::{ActorKind, ProjectId};

    fn actor() -> Actor {
        Actor::new(ActorKind::User, "user-1").expect("valid actor")
    }

    #[test]
    fn trigger_and_step_catalogs_are_closed_and_stable() {
        assert_eq!(AutomationTriggerKind::ALL.len(), 7);
        assert_eq!(AutomationStepKind::ALL.len(), 6);
        assert_eq!(
            BuiltInAutomationRecipe::ProjectMaintenanceV1.steps()[0].key,
            "recompute_project_metrics"
        );
    }

    #[test]
    fn run_transitions_are_exhaustive_and_terminal_runs_cannot_restart() {
        assert_eq!(
            AutomationRunStatus::Queued
                .transition_to(AutomationRunStatus::Running)
                .expect("queued can run"),
            AutomationRunStatus::Running
        );
        assert!(
            AutomationRunStatus::Completed
                .transition_to(AutomationRunStatus::Queued)
                .is_err()
        );
        assert_eq!(
            AutomationRunStatus::Failed
                .transition_to(AutomationRunStatus::Queued)
                .expect("failed can retry"),
            AutomationRunStatus::Queued
        );
        assert!(
            AutomationStepRunStatus::Completed
                .transition_to(AutomationStepRunStatus::Running)
                .is_err()
        );
    }

    #[test]
    fn public_inputs_validate_ids_names_and_bounds() {
        assert!(AutomationName::new(" ").is_err());
        assert!(AutomationIdempotencyKey::new(" ").is_err());
        assert!(AutomationDefinitionId::new(Uuid::nil()).is_err());
        assert!(validate_worker_id(" ").is_err());
        assert!(validate_run_list_limit(101).is_err());

        let request = ConfigureAutomationDefinition::new(
            ProjectId::new(Uuid::new_v4()),
            "Maintenance",
            AutomationTriggerKind::Manual,
            BuiltInAutomationRecipe::ProjectMaintenanceV1,
            AutomationDefinitionStatus::Active,
            actor(),
        )
        .expect("valid configuration");
        assert_eq!(request.name.as_str(), "Maintenance");
    }
}
