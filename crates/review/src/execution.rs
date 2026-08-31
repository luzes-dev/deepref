use std::collections::BTreeSet;

use deepref_ai::{
    AiError, AiExecutionContext, AiGateway, AiRunStore, AiTask, AiTaskRunner,
    AppraisalPrefillInput, AppraisalPrefillTask, Clock, DataExtractionInput, DataExtractionTask,
    DedupeInput, DedupeTask, EvidenceRetriever, IdProvider, ModelProfile, ModelRouter,
    ProposalDraft, ProposalStore, ScreeningEvidence, ScreeningInput, ScreeningTask,
    ScreeningTaskConfig, StudyDesignClassificationInput, StudyDesignClassificationTask,
    StudyGroupingInput, StudyGroupingTask,
};
use deepref_domain::{EligibilityCriterion, ScreeningStage};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use crate::{
    DefinedAiTask, ReviewCatalog, ReviewDefinitionKey, ReviewError, ReviewHash, ReviewSubject,
};

/// Canonical, serializable task input persisted with a durable review run.
///
/// This is an adapter contract for the PostgreSQL scheduler and worker. HTTP
/// callers only select a typed review subject; they do not submit this value.
#[doc(hidden)]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PreparedReviewTask {
    Screening {
        input: ScreeningInput,
        criteria: Vec<EligibilityCriterion>,
        allowed_evidence: Vec<ScreeningEvidence>,
        allowed_exclusion_reasons: BTreeSet<Uuid>,
    },
    DuplicateDetection {
        input: DedupeInput,
    },
    StudyClassification {
        input: StudyDesignClassificationInput,
    },
    StudyGrouping {
        input: StudyGroupingInput,
    },
    AppraisalPrefill {
        input: AppraisalPrefillInput,
    },
    DataExtraction {
        input: DataExtractionInput,
    },
}

#[doc(hidden)]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExecutedReviewTask {
    pub output: Value,
    pub model_run_id: Uuid,
    pub proposal: ProposalDraft,
}

impl PreparedReviewTask {
    pub const fn project_id(&self) -> deepref_domain::ProjectId {
        match self {
            Self::Screening { input, .. } => input.project_id,
            Self::DuplicateDetection { input } => input.project_id,
            Self::StudyClassification { input } => input.project_id,
            Self::StudyGrouping { input } => input.project_id,
            Self::AppraisalPrefill { input } => input.project_id,
            Self::DataExtraction { input } => input.project_id,
        }
    }

    pub const fn definition_key(&self) -> ReviewDefinitionKey {
        match self {
            Self::Screening { .. } => ReviewDefinitionKey::Screening,
            Self::DuplicateDetection { .. } => ReviewDefinitionKey::DuplicateDetection,
            Self::StudyClassification { .. } => ReviewDefinitionKey::StudyClassification,
            Self::StudyGrouping { .. } => ReviewDefinitionKey::StudyGrouping,
            Self::AppraisalPrefill { .. } => ReviewDefinitionKey::AppraisalPrefill,
            Self::DataExtraction { .. } => ReviewDefinitionKey::DataExtraction,
        }
    }

    pub fn subject(&self) -> ReviewSubject {
        match self {
            Self::Screening { input, .. } => ReviewSubject::Screening {
                report_id: input.report_id,
                stage: match input.stage {
                    deepref_ai::ScreeningStage::TitleAbstract => ScreeningStage::TitleAbstract,
                    deepref_ai::ScreeningStage::FullText => ScreeningStage::FullText,
                },
                protocol_version_id: input.protocol_version_id,
                expected_revision: input.expected_revision,
            },
            Self::DuplicateDetection { input } => ReviewSubject::DuplicateDetection {
                record_id: input.source_record_id,
                candidate_report_id: input.candidate_report_id,
            },
            Self::StudyClassification { input } => ReviewSubject::StudyClassification {
                study_id: input.study_id,
                expected_revision: i64::try_from(input.expected_revision).unwrap_or(i64::MAX),
            },
            Self::StudyGrouping { input } => ReviewSubject::StudyGrouping {
                report_id: input.report_id,
                expected_previous_study_id: input.current_study_id,
                expected_previous_study_revision: input.current_study_revision,
            },
            Self::AppraisalPrefill { input } => ReviewSubject::AppraisalPrefill {
                report_id: input.report_id,
                definition_id: input.definition_id.clone(),
                definition_version: input.definition_version,
            },
            Self::DataExtraction { input } => ReviewSubject::DataExtraction {
                study_id: input.study_id,
                field_set_version: input
                    .fields
                    .iter()
                    .map(|field| field.version)
                    .max()
                    .unwrap_or(0),
            },
        }
    }

    pub const fn model_profile(&self) -> ModelProfile {
        match self {
            Self::Screening { input, .. } => match input.stage {
                deepref_ai::ScreeningStage::TitleAbstract => ModelProfile::Reasoning,
                deepref_ai::ScreeningStage::FullText => ModelProfile::LongContextReasoning,
            },
            Self::DuplicateDetection { .. } | Self::StudyClassification { .. } => {
                ModelProfile::FastClassifier
            }
            Self::StudyGrouping { .. } => ModelProfile::Reasoning,
            Self::AppraisalPrefill { .. } | Self::DataExtraction { .. } => {
                ModelProfile::LongContextReasoning
            }
        }
    }

    pub fn source_content_hash(&self) -> Result<ReviewHash, ReviewError> {
        ReviewHash::digest_json(self)
    }

    pub fn protocol_hash(&self) -> Result<ReviewHash, ReviewError> {
        let hash = match self {
            Self::Screening {
                input,
                criteria,
                allowed_evidence,
                allowed_exclusion_reasons,
            } => {
                ScreeningTask::new(ScreeningTaskConfig {
                    project_id: input.project_id,
                    report_id: input.report_id,
                    stage: input.stage,
                    protocol_version_id: input.protocol_version_id,
                    expected_revision: input.expected_revision,
                    criteria: criteria.clone(),
                    allowed_evidence: allowed_evidence.clone(),
                    allowed_exclusion_reasons: allowed_exclusion_reasons.clone(),
                })
                .build_context(input)
                .map_err(review_ai_error)?
                .protocol_hash
            }
            Self::AppraisalPrefill { input } => {
                AppraisalPrefillTask::new(input)
                    .and_then(|task| task.build_context(input))
                    .map_err(review_ai_error)?
                    .protocol_hash
            }
            Self::StudyClassification { input } => {
                StudyDesignClassificationTask::new(input)
                    .and_then(|task| task.build_context(input))
                    .map_err(review_ai_error)?
                    .protocol_hash
            }
            Self::DuplicateDetection { .. }
            | Self::StudyGrouping { .. }
            | Self::DataExtraction { .. } => None,
        };
        match hash {
            Some(hash) => ReviewHash::parse(hash),
            None => Ok(ReviewHash::digest_bytes(b"no-protocol")),
        }
    }

    pub fn validate(&self) -> Result<(), ReviewError> {
        if self.subject().definition_key() != self.definition_key() {
            return Err(ReviewError::InvalidDefinition(
                "prepared task subject does not match its definition".to_owned(),
            ));
        }
        match self {
            Self::Screening {
                input,
                criteria,
                allowed_evidence,
                allowed_exclusion_reasons,
            } => ScreeningTask::new(ScreeningTaskConfig {
                project_id: input.project_id,
                report_id: input.report_id,
                stage: input.stage,
                protocol_version_id: input.protocol_version_id,
                expected_revision: input.expected_revision,
                criteria: criteria.clone(),
                allowed_evidence: allowed_evidence.clone(),
                allowed_exclusion_reasons: allowed_exclusion_reasons.clone(),
            })
            .build_context(input)
            .map(|_| ()),
            Self::DuplicateDetection { input } => DedupeTask::new(
                input.project_id,
                input.source_record_id,
                input.candidate_report_id,
                input.grounded_provenance.clone(),
                input.grounded_signals.clone(),
            )
            .build_context(input)
            .map(|_| ()),
            Self::StudyClassification { input } => {
                if input.expected_revision > i64::MAX as u64 {
                    return Err(ReviewError::InvalidDefinition(
                        "study revision exceeds the supported range".to_owned(),
                    ));
                }
                StudyDesignClassificationTask::new(input)
                    .and_then(|task| task.build_context(input))
                    .map(|_| ())
            }
            Self::StudyGrouping { input } => StudyGroupingTask::new(input)
                .and_then(|task| task.build_context(input))
                .map(|_| ()),
            Self::AppraisalPrefill { input } => AppraisalPrefillTask::new(input)
                .and_then(|task| task.build_context(input))
                .map(|_| ()),
            Self::DataExtraction { input } => DataExtractionTask::new(input)
                .and_then(|task| task.build_context(input))
                .map(|_| ()),
        }
        .map_err(review_ai_error)
    }

    pub async fn execute<G, R, E, S, P, C, I>(
        &self,
        runner: &AiTaskRunner<'_, G, R, E, S, P, C, I>,
        execution: AiExecutionContext,
    ) -> Result<ExecutedReviewTask, ReviewError>
    where
        G: AiGateway + ?Sized,
        R: ModelRouter,
        E: EvidenceRetriever,
        S: AiRunStore,
        P: ProposalStore,
        C: Clock,
        I: IdProvider,
    {
        let node_id = if self.definition_key() == ReviewDefinitionKey::Screening {
            "primary_screen"
        } else {
            "generate"
        };
        self.execute_for_node(runner, execution, node_id, None)
            .await
    }

    pub async fn execute_for_node<G, R, E, S, P, C, I>(
        &self,
        runner: &AiTaskRunner<'_, G, R, E, S, P, C, I>,
        execution: AiExecutionContext,
        node_id: &str,
        semantic_context: Option<Value>,
    ) -> Result<ExecutedReviewTask, ReviewError>
    where
        G: AiGateway + ?Sized,
        R: ModelRouter,
        E: EvidenceRetriever,
        S: AiRunStore,
        P: ProposalStore,
        C: Clock,
        I: IdProvider,
    {
        let definition = ReviewCatalog.compile(self.definition_key())?;
        match self {
            Self::Screening {
                input,
                criteria,
                allowed_evidence,
                allowed_exclusion_reasons,
            } => {
                let task = ScreeningTask::new(ScreeningTaskConfig {
                    project_id: input.project_id,
                    report_id: input.report_id,
                    stage: input.stage,
                    protocol_version_id: input.protocol_version_id,
                    expected_revision: input.expected_revision,
                    criteria: criteria.clone(),
                    allowed_evidence: allowed_evidence.clone(),
                    allowed_exclusion_reasons: allowed_exclusion_reasons.clone(),
                });
                execute_task(
                    runner,
                    definition,
                    task,
                    input.clone(),
                    execution,
                    node_id,
                    semantic_context,
                )
                .await
            }
            Self::DuplicateDetection { input } => {
                let task = DedupeTask::new(
                    input.project_id,
                    input.source_record_id,
                    input.candidate_report_id,
                    input.grounded_provenance.clone(),
                    input.grounded_signals.clone(),
                );
                execute_task(
                    runner,
                    definition,
                    task,
                    input.clone(),
                    execution,
                    node_id,
                    semantic_context,
                )
                .await
            }
            Self::StudyClassification { input } => {
                let task = StudyDesignClassificationTask::new(input).map_err(review_ai_error)?;
                execute_task(
                    runner,
                    definition,
                    task,
                    input.clone(),
                    execution,
                    node_id,
                    semantic_context,
                )
                .await
            }
            Self::StudyGrouping { input } => {
                let task = StudyGroupingTask::new(input).map_err(review_ai_error)?;
                execute_task(
                    runner,
                    definition,
                    task,
                    input.clone(),
                    execution,
                    node_id,
                    semantic_context,
                )
                .await
            }
            Self::AppraisalPrefill { input } => {
                let task = AppraisalPrefillTask::new(input).map_err(review_ai_error)?;
                execute_task(
                    runner,
                    definition,
                    task,
                    input.clone(),
                    execution,
                    node_id,
                    semantic_context,
                )
                .await
            }
            Self::DataExtraction { input } => {
                let task = DataExtractionTask::new(input).map_err(review_ai_error)?;
                execute_task(
                    runner,
                    definition,
                    task,
                    input.clone(),
                    execution,
                    node_id,
                    semantic_context,
                )
                .await
            }
        }
    }
}

async fn execute_task<T, G, R, E, S, P, C, I>(
    runner: &AiTaskRunner<'_, G, R, E, S, P, C, I>,
    definition: crate::CompiledReviewDefinition,
    task: T,
    input: T::Input,
    execution: AiExecutionContext,
    node_id: &str,
    semantic_context: Option<Value>,
) -> Result<ExecutedReviewTask, ReviewError>
where
    T: AiTask,
    G: AiGateway + ?Sized,
    R: ModelRouter,
    E: EvidenceRetriever,
    S: AiRunStore,
    P: ProposalStore,
    C: Clock,
    I: IdProvider,
{
    let task = DefinedAiTask::bind_for_node(definition, task, node_id, semantic_context)?;
    let result = runner
        .run_with_context(&task, input, execution)
        .await
        .map_err(review_ai_error)?;
    let mut proposal = task.proposal(&result.output).ok_or_else(|| {
        ReviewError::Execution("compiled consequential task did not assemble a proposal".to_owned())
    })?;
    proposal
        .payload
        .as_object_mut()
        .ok_or_else(|| {
            ReviewError::Execution("compiled proposal payload must be an object".to_owned())
        })?
        .insert(
            "task_kind".to_owned(),
            Value::String(task.kind().as_str().to_owned()),
        );
    Ok(ExecutedReviewTask {
        output: serde_json::to_value(result.output)
            .map_err(|error| ReviewError::Execution(error.to_string()))?,
        model_run_id: result.run.id,
        proposal,
    })
}

fn review_ai_error(error: AiError) -> ReviewError {
    ReviewError::Execution(error.to_string())
}
