use std::collections::BTreeSet;

use deepref_ai::{
    AppraisalAnswerSchema, AppraisalPrefillDomain, AppraisalPrefillEvidence, AppraisalPrefillInput,
    AppraisalPrefillQuestion, ClassificationReportField, CriterionPrompt, DataExtractionInput,
    DedupeInput, ExtractionEvidence, ExtractionField, ExtractionValueType, IdentityProvenance,
    ScreeningEvidence, ScreeningEvidenceField, ScreeningInput, ScreeningStage,
    StudyDesignClassificationInput, StudyDesignEvidence, StudyDesignLabel, StudyDesignReport,
    StudyGroupingCandidate, StudyGroupingEvidence, StudyGroupingField, StudyGroupingInput,
    StudyMetadataField, sha256_bytes,
};
use deepref_application::{
    AnswerSchema, DedupeCandidate, ExtractionFieldType, FUZZY_PROPOSAL_THRESHOLD, score_candidate,
};
use deepref_domain::{Actor, CriterionStage, EligibilityCriterion, StudyDesign};
use deepref_review::{
    ReviewFuture, ReviewOrigin, ReviewRunId, ReviewRunSnapshot, ReviewScheduler, ReviewSubject,
    ScheduleReviewRun, execution::PreparedReviewTask,
};
use thiserror::Error;
use uuid::Uuid;

use crate::{
    AiDedupeTarget, AiGroupingReport, AiProposalError, AiScreeningTarget, AiStudyGroupingTarget,
    ExtractionError, PostgresReviewError, PreparedReviewRun, ProtocolError, StudyError,
    get_ai_dedupe_target, get_ai_screening_target, get_ai_study_grouping_target,
    get_published_protocol, get_study, list_ai_exclusion_reasons, list_ai_extraction_evidence,
    list_ai_grounding_blocks, list_field_definitions, schedule_prepared_review_run,
};

#[derive(Debug, Error)]
pub enum ReviewPreparationError {
    #[error(transparent)]
    Review(#[from] PostgresReviewError),
    #[error(transparent)]
    Protocol(#[from] ProtocolError),
    #[error(transparent)]
    AiProposal(#[from] AiProposalError),
    #[error(transparent)]
    Extraction(#[from] ExtractionError),
    #[error(transparent)]
    Study(#[from] StudyError),
    #[error("review request is invalid: {0}")]
    InvalidInput(String),
}

#[derive(Clone)]
pub struct PostgresReviewScheduler {
    pool: sqlx::PgPool,
}

impl PostgresReviewScheduler {
    pub fn new(pool: &sqlx::PgPool) -> Self {
        Self { pool: pool.clone() }
    }
}

struct ReviewScheduleContext {
    origin: ReviewOrigin,
    actor: Actor,
    expected_subject: Option<ReviewSubject>,
}

impl ReviewScheduleContext {
    fn reviewer_requested(actor: Actor) -> Self {
        Self {
            origin: ReviewOrigin::ReviewerRequested,
            actor,
            expected_subject: None,
        }
    }
}

pub async fn schedule_screening_review(
    pool: &sqlx::PgPool,
    project_id: Uuid,
    report_id: Uuid,
    stage: ScreeningStage,
    requested_protocol_version_id: Option<Uuid>,
    requested_revision: Option<i64>,
    actor: Actor,
) -> Result<ReviewRunSnapshot, ReviewPreparationError> {
    schedule_screening_review_with_origin(
        pool,
        project_id,
        report_id,
        stage,
        requested_protocol_version_id,
        requested_revision,
        ReviewScheduleContext::reviewer_requested(actor),
    )
    .await
}

async fn schedule_screening_review_with_origin(
    pool: &sqlx::PgPool,
    project_id: Uuid,
    report_id: Uuid,
    stage: ScreeningStage,
    requested_protocol_version_id: Option<Uuid>,
    requested_revision: Option<i64>,
    context: ReviewScheduleContext,
) -> Result<ReviewRunSnapshot, ReviewPreparationError> {
    let protocol = get_published_protocol(pool, project_id).await?;
    if requested_protocol_version_id.is_some_and(|id| id != protocol.id) {
        return Err(ReviewPreparationError::InvalidInput(format!(
            "published protocol changed to {}",
            protocol.id
        )));
    }
    let target = get_ai_screening_target(pool, project_id, report_id).await?;
    let expected_revision = requested_revision.unwrap_or(target.expected_revision);
    if expected_revision != target.expected_revision {
        return Err(ReviewPreparationError::InvalidInput(format!(
            "screening revision changed to {}",
            target.expected_revision
        )));
    }
    let domain_stage = match stage {
        ScreeningStage::TitleAbstract => deepref_domain::ScreeningStage::TitleAbstract,
        ScreeningStage::FullText => deepref_domain::ScreeningStage::FullText,
    };
    let allowed_exclusion_reasons = list_ai_exclusion_reasons(pool, project_id, domain_stage)
        .await?
        .into_iter()
        .collect();
    let criteria = protocol.criteria.clone();
    let allowed_evidence = metadata_evidence(report_id, &target);
    let input = ScreeningInput {
        project_id: project_id.into(),
        report_id: report_id.into(),
        stage,
        protocol_version_id: protocol.id.into(),
        expected_revision,
        title: target.title.clone(),
        abstract_text: target.abstract_text.clone(),
        document_hash: None,
        retrieval_query: (stage == ScreeningStage::FullText)
            .then(|| screening_retrieval_query(&target, &criteria)),
        criteria: criteria.iter().map(criterion_prompt).collect(),
    };
    schedule(
        pool,
        PreparedReviewTask::Screening {
            input,
            criteria,
            allowed_evidence,
            allowed_exclusion_reasons,
        },
        context,
    )
    .await
}

pub async fn schedule_duplicate_detection_review(
    pool: &sqlx::PgPool,
    project_id: Uuid,
    record_id: Uuid,
    candidate_report_id: Uuid,
    actor: Actor,
) -> Result<ReviewRunSnapshot, ReviewPreparationError> {
    schedule_duplicate_detection_review_with_origin(
        pool,
        project_id,
        record_id,
        candidate_report_id,
        ReviewScheduleContext::reviewer_requested(actor),
    )
    .await
}

async fn schedule_duplicate_detection_review_with_origin(
    pool: &sqlx::PgPool,
    project_id: Uuid,
    record_id: Uuid,
    candidate_report_id: Uuid,
    context: ReviewScheduleContext,
) -> Result<ReviewRunSnapshot, ReviewPreparationError> {
    let target = get_ai_dedupe_target(pool, project_id, record_id, candidate_report_id).await?;
    let grounded_provenance = dedupe_provenance(record_id, candidate_report_id, &target);
    let grounded_signals = dedupe_signals(candidate_report_id, &target);
    let input = DedupeInput {
        project_id: project_id.into(),
        source_record_id: record_id.into(),
        candidate_report_id: candidate_report_id.into(),
        source_title: target.source_title,
        candidate_title: target.candidate_title,
        source_year: target.source_year,
        candidate_year: target.candidate_year,
        source_author: target.source_author,
        candidate_author: target.candidate_author,
        source_title_hash: target.source_title_hash,
        candidate_title_hash: target.candidate_title_hash,
        grounded_signals,
        grounded_provenance,
    };
    schedule(
        pool,
        PreparedReviewTask::DuplicateDetection { input },
        context,
    )
    .await
}

pub async fn schedule_study_grouping_review(
    pool: &sqlx::PgPool,
    project_id: Uuid,
    report_id: Uuid,
    actor: Actor,
) -> Result<ReviewRunSnapshot, ReviewPreparationError> {
    schedule_study_grouping_review_with_origin(
        pool,
        project_id,
        report_id,
        ReviewScheduleContext::reviewer_requested(actor),
    )
    .await
}

async fn schedule_study_grouping_review_with_origin(
    pool: &sqlx::PgPool,
    project_id: Uuid,
    report_id: Uuid,
    context: ReviewScheduleContext,
) -> Result<ReviewRunSnapshot, ReviewPreparationError> {
    let target = get_ai_study_grouping_target(pool, project_id, report_id).await?;
    let input = StudyGroupingInput {
        project_id: project_id.into(),
        report_id: report_id.into(),
        report_title: target.report.title.clone(),
        report_abstract: target.report.abstract_text.clone(),
        publication_year: target.report.publication_year,
        first_author: target.report.first_author.clone(),
        current_study_id: target.current_study_id.map(Into::into),
        current_study_revision: target.current_study_revision,
        candidates: target
            .studies
            .iter()
            .map(|study| StudyGroupingCandidate {
                study_id: study.study_id,
                title: study.title.clone(),
                revision: study.revision,
                report_ids: study
                    .reports
                    .iter()
                    .map(|report| report.report_id)
                    .collect(),
            })
            .collect(),
        grounded_evidence: grouping_evidence(&target),
    };
    schedule(pool, PreparedReviewTask::StudyGrouping { input }, context).await
}

pub async fn schedule_study_classification_review(
    pool: &sqlx::PgPool,
    project_id: Uuid,
    study_id: Uuid,
    actor: Actor,
) -> Result<ReviewRunSnapshot, ReviewPreparationError> {
    schedule_study_classification_review_with_origin(
        pool,
        project_id,
        study_id,
        ReviewScheduleContext::reviewer_requested(actor),
    )
    .await
}

async fn schedule_study_classification_review_with_origin(
    pool: &sqlx::PgPool,
    project_id: Uuid,
    study_id: Uuid,
    context: ReviewScheduleContext,
) -> Result<ReviewRunSnapshot, ReviewPreparationError> {
    let target = get_study(pool, project_id, study_id).await?;
    let expected_revision = u64::try_from(target.study.revision).map_err(|_| {
        ReviewPreparationError::InvalidInput("study revision is invalid".to_owned())
    })?;
    let reports = target
        .reports
        .iter()
        .take(100)
        .map(|report| StudyDesignReport {
            report_id: report.report_id.as_uuid(),
            title: report
                .title
                .as_deref()
                .map(|value| bounded_text(value, 4_000)),
            abstract_text: report
                .abstract_text
                .as_deref()
                .map(|value| bounded_text(value, 16_000)),
            publication_year: report.publication_year,
        })
        .collect();
    let mut grounded_evidence = vec![StudyDesignEvidence::StudyMetadata {
        study_id,
        field: StudyMetadataField::Title,
        content_hash: sha256_bytes(target.study.title.as_bytes()),
    }];
    for report in target.reports.iter().take(100) {
        if let Some(title) = &report.title {
            grounded_evidence.push(StudyDesignEvidence::ReportMetadata {
                report_id: report.report_id.as_uuid(),
                field: ClassificationReportField::Title,
                content_hash: sha256_bytes(title.as_bytes()),
            });
        }
        if let Some(abstract_text) = &report.abstract_text {
            grounded_evidence.push(StudyDesignEvidence::ReportMetadata {
                report_id: report.report_id.as_uuid(),
                field: ClassificationReportField::Abstract,
                content_hash: sha256_bytes(abstract_text.as_bytes()),
            });
        }
        if let Some(year) = report.publication_year {
            grounded_evidence.push(StudyDesignEvidence::ReportMetadata {
                report_id: report.report_id.as_uuid(),
                field: ClassificationReportField::PublicationYear,
                content_hash: sha256_bytes(year.to_string().as_bytes()),
            });
        }
    }
    let input = StudyDesignClassificationInput {
        project_id: project_id.into(),
        study_id: study_id.into(),
        expected_revision,
        study_title: target.study.title,
        current_design: target.study.design.map(study_design_label),
        reports,
        allowed_designs: StudyDesignLabel::ALL.to_vec(),
        grounded_evidence,
    };
    schedule(
        pool,
        PreparedReviewTask::StudyClassification { input },
        context,
    )
    .await
}

pub async fn schedule_appraisal_prefill_review(
    pool: &sqlx::PgPool,
    project_id: Uuid,
    report_id: Uuid,
    definition_id: &str,
    definition_version: u32,
    actor: Actor,
) -> Result<ReviewRunSnapshot, ReviewPreparationError> {
    schedule_appraisal_prefill_review_with_origin(
        pool,
        project_id,
        report_id,
        definition_id,
        definition_version,
        ReviewScheduleContext::reviewer_requested(actor),
    )
    .await
}

async fn schedule_appraisal_prefill_review_with_origin(
    pool: &sqlx::PgPool,
    project_id: Uuid,
    report_id: Uuid,
    definition_id: &str,
    definition_version: u32,
    context: ReviewScheduleContext,
) -> Result<ReviewRunSnapshot, ReviewPreparationError> {
    let definition =
        deepref_application::get_appraisal_definition(definition_id, definition_version)
            .map_err(|error| ReviewPreparationError::InvalidInput(error.to_string()))?;
    let target = get_ai_screening_target(pool, project_id, report_id).await?;
    let query = definition
        .domains
        .iter()
        .flat_map(|domain| {
            domain.questions.iter().map(|question| {
                format!(
                    "{} {}",
                    question.label,
                    question.help.as_deref().unwrap_or("")
                )
            })
        })
        .collect::<Vec<_>>()
        .join(" ");
    let blocks = list_ai_grounding_blocks(pool, project_id, report_id, &query).await?;
    let input = AppraisalPrefillInput {
        project_id: project_id.into(),
        report_id: report_id.into(),
        definition_id: definition.id.as_str().to_owned(),
        definition_version: definition.version.get(),
        questions: definition
            .domains
            .iter()
            .flat_map(|domain| domain.questions.iter())
            .map(|question| AppraisalPrefillQuestion {
                id: question.id.clone(),
                answer_schema: appraisal_answer_schema(&question.answer_schema),
                required: question.required,
                requires_evidence: question.requires_evidence,
            })
            .collect(),
        domains: definition
            .domains
            .iter()
            .map(|domain| AppraisalPrefillDomain {
                id: domain.id.clone(),
                allowed_judgments: domain
                    .judgment
                    .options
                    .iter()
                    .map(|option| option.value.clone())
                    .collect(),
                required: domain.judgment.required,
            })
            .collect(),
        overall_allowed_judgments: definition
            .overall_judgment
            .options
            .iter()
            .map(|option| option.value.clone())
            .collect(),
        report_title: target.title,
        report_abstract: target.abstract_text,
        grounded_evidence: blocks
            .into_iter()
            .map(|block| AppraisalPrefillEvidence {
                document_id: block.document_id,
                document_block_id: block.document_block_id,
                page: block.page,
                parser_version: block.parser_version,
                content_hash: block.content_hash,
            })
            .collect(),
    };
    schedule(
        pool,
        PreparedReviewTask::AppraisalPrefill { input },
        context,
    )
    .await
}

pub async fn schedule_data_extraction_review(
    pool: &sqlx::PgPool,
    project_id: Uuid,
    study_id: Uuid,
    actor: Actor,
) -> Result<ReviewRunSnapshot, ReviewPreparationError> {
    schedule_data_extraction_review_with_origin(
        pool,
        project_id,
        study_id,
        ReviewScheduleContext::reviewer_requested(actor),
    )
    .await
}

async fn schedule_data_extraction_review_with_origin(
    pool: &sqlx::PgPool,
    project_id: Uuid,
    study_id: Uuid,
    context: ReviewScheduleContext,
) -> Result<ReviewRunSnapshot, ReviewPreparationError> {
    let definitions = list_field_definitions(pool, project_id).await?;
    if definitions.is_empty() {
        return Err(ReviewPreparationError::InvalidInput(
            "create at least one extraction field before generating a proposal".to_owned(),
        ));
    }
    let query = definitions
        .iter()
        .map(|definition| format!("{} {}", definition.field_key, definition.label))
        .collect::<Vec<_>>()
        .join(" ");
    let blocks = list_ai_extraction_evidence(pool, project_id, study_id, &query).await?;
    let input = DataExtractionInput {
        project_id: project_id.into(),
        study_id: study_id.into(),
        fields: definitions
            .into_iter()
            .map(|definition| ExtractionField {
                id: definition.id,
                version: definition.version,
                field_key: definition.field_key,
                label: definition.label,
                value_type: extraction_value_type(definition.value_type),
                required: definition.required,
            })
            .collect(),
        grounded_evidence: blocks
            .into_iter()
            .map(|block| ExtractionEvidence {
                report_id: block.report_id,
                document_id: block.document_id,
                document_block_id: block.document_block_id,
                page: block.page,
                parser_version: block.parser_version,
                content_hash: block.content_hash,
            })
            .collect(),
    };
    schedule(pool, PreparedReviewTask::DataExtraction { input }, context).await
}

async fn schedule(
    pool: &sqlx::PgPool,
    task: PreparedReviewTask,
    context: ReviewScheduleContext,
) -> Result<ReviewRunSnapshot, ReviewPreparationError> {
    let ReviewScheduleContext {
        origin,
        actor,
        expected_subject,
    } = context;
    let project_id = task.project_id();
    let definition = task.definition_key();
    let subject = task.subject();
    if expected_subject
        .as_ref()
        .is_some_and(|expected| expected != &subject)
    {
        return Err(ReviewPreparationError::InvalidInput(
            "review subject changed while the run was being prepared".to_owned(),
        ));
    }
    Ok(schedule_prepared_review_run(
        pool,
        PreparedReviewRun {
            command: ScheduleReviewRun {
                project_id,
                definition,
                subject,
                origin,
                actor,
            },
            task,
        },
    )
    .await?)
}

impl ReviewScheduler for PostgresReviewScheduler {
    type Error = ReviewPreparationError;

    fn schedule<'a>(
        &'a self,
        command: ScheduleReviewRun,
    ) -> ReviewFuture<'a, ReviewRunSnapshot, Self::Error> {
        Box::pin(async move {
            command
                .validate()
                .map_err(PostgresReviewError::from)
                .map_err(ReviewPreparationError::from)?;
            let project_id = command.project_id.as_uuid();
            let origin = command.origin;
            let actor = command.actor.clone();
            let expected_subject = Some(command.subject.clone());
            match command.subject {
                ReviewSubject::Screening {
                    report_id,
                    stage,
                    protocol_version_id,
                    expected_revision,
                } => {
                    let stage = match stage {
                        deepref_domain::ScreeningStage::TitleAbstract => {
                            ScreeningStage::TitleAbstract
                        }
                        deepref_domain::ScreeningStage::FullText => ScreeningStage::FullText,
                    };
                    schedule_screening_review_with_origin(
                        &self.pool,
                        project_id,
                        report_id.as_uuid(),
                        stage,
                        Some(protocol_version_id.as_uuid()),
                        Some(expected_revision),
                        ReviewScheduleContext {
                            origin,
                            actor,
                            expected_subject,
                        },
                    )
                    .await
                }
                ReviewSubject::DuplicateDetection {
                    record_id,
                    candidate_report_id,
                } => {
                    schedule_duplicate_detection_review_with_origin(
                        &self.pool,
                        project_id,
                        record_id.as_uuid(),
                        candidate_report_id.as_uuid(),
                        ReviewScheduleContext {
                            origin,
                            actor,
                            expected_subject,
                        },
                    )
                    .await
                }
                ReviewSubject::StudyClassification {
                    study_id,
                    expected_revision: _,
                } => {
                    schedule_study_classification_review_with_origin(
                        &self.pool,
                        project_id,
                        study_id.as_uuid(),
                        ReviewScheduleContext {
                            origin,
                            actor,
                            expected_subject,
                        },
                    )
                    .await
                }
                ReviewSubject::StudyGrouping { report_id, .. } => {
                    schedule_study_grouping_review_with_origin(
                        &self.pool,
                        project_id,
                        report_id.as_uuid(),
                        ReviewScheduleContext {
                            origin,
                            actor,
                            expected_subject,
                        },
                    )
                    .await
                }
                ReviewSubject::AppraisalPrefill {
                    report_id,
                    definition_id,
                    definition_version,
                } => {
                    schedule_appraisal_prefill_review_with_origin(
                        &self.pool,
                        project_id,
                        report_id.as_uuid(),
                        &definition_id,
                        definition_version,
                        ReviewScheduleContext {
                            origin,
                            actor,
                            expected_subject,
                        },
                    )
                    .await
                }
                ReviewSubject::DataExtraction {
                    study_id,
                    field_set_version: _,
                } => {
                    schedule_data_extraction_review_with_origin(
                        &self.pool,
                        project_id,
                        study_id.as_uuid(),
                        ReviewScheduleContext {
                            origin,
                            actor,
                            expected_subject,
                        },
                    )
                    .await
                }
            }
        })
    }

    fn get<'a>(
        &'a self,
        project_id: deepref_domain::ProjectId,
        run_id: ReviewRunId,
    ) -> ReviewFuture<'a, ReviewRunSnapshot, Self::Error> {
        Box::pin(async move {
            crate::get_review_run(&self.pool, project_id, run_id)
                .await
                .map_err(ReviewPreparationError::from)
        })
    }
}

fn metadata_evidence(report_id: Uuid, target: &AiScreeningTarget) -> Vec<ScreeningEvidence> {
    let mut evidence = Vec::new();
    if let Some(title) = &target.title {
        evidence.push(ScreeningEvidence::ReportMetadata {
            report_id,
            field: ScreeningEvidenceField::Title,
            content_hash: sha256_bytes(title.as_bytes()),
        });
    }
    if let Some(abstract_text) = &target.abstract_text {
        evidence.push(ScreeningEvidence::ReportMetadata {
            report_id,
            field: ScreeningEvidenceField::Abstract,
            content_hash: sha256_bytes(abstract_text.as_bytes()),
        });
    }
    evidence
}

fn criterion_prompt(criterion: &EligibilityCriterion) -> CriterionPrompt {
    CriterionPrompt {
        id: criterion.id,
        label: criterion.label.clone(),
        description: criterion.description.clone(),
        ordinal: criterion.ordinal,
        kind: match criterion.kind {
            deepref_domain::CriterionKind::Inclusion => "inclusion",
            deepref_domain::CriterionKind::Exclusion => "exclusion",
        }
        .to_owned(),
        stage: match criterion.stage {
            CriterionStage::TitleAbstract => "title_abstract",
            CriterionStage::FullText => "full_text",
            CriterionStage::Both => "both",
        }
        .to_owned(),
    }
}

fn screening_retrieval_query(
    target: &AiScreeningTarget,
    criteria: &[EligibilityCriterion],
) -> String {
    const MAX_TERMS: usize = 64;
    const MAX_TERM_CHARS: usize = 48;
    let mut terms = Vec::new();
    let mut seen = BTreeSet::new();
    let mut add_terms = |text: &str| {
        let mut token = String::new();
        let mut flush = |token: &mut String| {
            if token.is_empty() {
                return;
            }
            let normalized: String = token
                .chars()
                .flat_map(char::to_lowercase)
                .take(MAX_TERM_CHARS)
                .collect();
            if (normalized.chars().count() >= 3
                || normalized
                    .chars()
                    .all(|character| character.is_ascii_digit()))
                && seen.insert(normalized.clone())
                && terms.len() < MAX_TERMS
            {
                terms.push(normalized);
            }
            token.clear();
        };
        for character in text.chars() {
            if character.is_alphanumeric() {
                token.push(character);
            } else {
                flush(&mut token);
            }
        }
        flush(&mut token);
    };
    for criterion in criteria {
        add_terms(&criterion.label);
        add_terms(&criterion.description);
    }
    if let Some(title) = &target.title {
        add_terms(title);
    }
    if let Some(abstract_text) = &target.abstract_text {
        add_terms(abstract_text);
    }
    if terms.is_empty() {
        "full-text eligibility evidence".to_owned()
    } else {
        terms.join(" OR ")
    }
}

fn grouping_evidence(target: &AiStudyGroupingTarget) -> Vec<StudyGroupingEvidence> {
    let mut evidence = Vec::new();
    add_grouping_report_evidence(&mut evidence, &target.report);
    for study in &target.studies {
        evidence.push(StudyGroupingEvidence::StudyMetadata {
            study_id: study.study_id,
            field: StudyGroupingField::Title,
            content_hash: sha256_bytes(study.title.as_bytes()),
        });
        for report in &study.reports {
            if let Some(title) = &report.title {
                evidence.push(StudyGroupingEvidence::StudyReportMetadata {
                    study_id: study.study_id,
                    report_id: report.report_id,
                    field: StudyGroupingField::Title,
                    content_hash: sha256_bytes(title.as_bytes()),
                });
            }
            if let Some(abstract_text) = &report.abstract_text {
                evidence.push(StudyGroupingEvidence::StudyReportMetadata {
                    study_id: study.study_id,
                    report_id: report.report_id,
                    field: StudyGroupingField::Abstract,
                    content_hash: sha256_bytes(abstract_text.as_bytes()),
                });
            }
            if let Some(year) = report.publication_year {
                evidence.push(StudyGroupingEvidence::StudyReportMetadata {
                    study_id: study.study_id,
                    report_id: report.report_id,
                    field: StudyGroupingField::PublicationYear,
                    content_hash: sha256_bytes(year.to_string().as_bytes()),
                });
            }
            if let Some(author) = &report.first_author {
                evidence.push(StudyGroupingEvidence::StudyReportMetadata {
                    study_id: study.study_id,
                    report_id: report.report_id,
                    field: StudyGroupingField::FirstAuthor,
                    content_hash: sha256_bytes(author.as_bytes()),
                });
            }
        }
    }
    evidence
}

fn add_grouping_report_evidence(
    evidence: &mut Vec<StudyGroupingEvidence>,
    report: &AiGroupingReport,
) {
    if let Some(title) = &report.title {
        evidence.push(StudyGroupingEvidence::ReportMetadata {
            report_id: report.report_id,
            field: StudyGroupingField::Title,
            content_hash: sha256_bytes(title.as_bytes()),
        });
    }
    if let Some(abstract_text) = &report.abstract_text {
        evidence.push(StudyGroupingEvidence::ReportMetadata {
            report_id: report.report_id,
            field: StudyGroupingField::Abstract,
            content_hash: sha256_bytes(abstract_text.as_bytes()),
        });
    }
    if let Some(year) = report.publication_year {
        evidence.push(StudyGroupingEvidence::ReportMetadata {
            report_id: report.report_id,
            field: StudyGroupingField::PublicationYear,
            content_hash: sha256_bytes(year.to_string().as_bytes()),
        });
    }
    if let Some(author) = &report.first_author {
        evidence.push(StudyGroupingEvidence::ReportMetadata {
            report_id: report.report_id,
            field: StudyGroupingField::FirstAuthor,
            content_hash: sha256_bytes(author.as_bytes()),
        });
    }
}

fn appraisal_answer_schema(schema: &AnswerSchema) -> AppraisalAnswerSchema {
    match schema {
        AnswerSchema::Enum { options } => AppraisalAnswerSchema::Enum {
            options: options.iter().map(|option| option.value.clone()).collect(),
        },
        AnswerSchema::Boolean => AppraisalAnswerSchema::Boolean,
        AnswerSchema::Scale { min, max, .. } => AppraisalAnswerSchema::Scale {
            min: *min,
            max: *max,
        },
        AnswerSchema::Text { max_length } => AppraisalAnswerSchema::Text {
            max_length: *max_length,
        },
    }
}

fn dedupe_provenance(
    source_record_id: Uuid,
    candidate_report_id: Uuid,
    target: &AiDedupeTarget,
) -> Vec<IdentityProvenance> {
    let mut provenance = Vec::new();
    let mut push = |entity_type: &str, entity_id: Uuid, field: &str, value: &str| {
        provenance.push(IdentityProvenance {
            entity_type: entity_type.to_owned(),
            entity_id: entity_id.to_string(),
            field: field.to_owned(),
            content_hash: sha256_bytes(value.as_bytes()),
        });
    };
    if let Some(title) = target.source_title.as_deref() {
        push("record", source_record_id, "title", title);
    }
    if let Some(title) = target.candidate_title.as_deref() {
        push("report", candidate_report_id, "title", title);
    }
    if let Some(year) = target.source_year {
        push(
            "record",
            source_record_id,
            "publication_year",
            &year.to_string(),
        );
    }
    if let Some(year) = target.candidate_year {
        push(
            "report",
            candidate_report_id,
            "publication_year",
            &year.to_string(),
        );
    }
    if let Some(author) = target.source_author.as_deref() {
        push("record", source_record_id, "first_author", author);
    }
    if let Some(author) = target.candidate_author.as_deref() {
        push("report", candidate_report_id, "first_author", author);
    }
    provenance
}

fn dedupe_signals(
    candidate_report_id: Uuid,
    target: &AiDedupeTarget,
) -> Vec<deepref_ai::DuplicateSignal> {
    let candidate = DedupeCandidate {
        report_id: candidate_report_id.into(),
        title: target.candidate_title.clone(),
        first_author: target.candidate_author.clone(),
        publication_year: target.candidate_year,
        exact_identifier_match: false,
        conflicting_identifier: false,
    };
    let score = score_candidate(
        target.source_title.as_deref(),
        target.source_author.as_deref(),
        target.source_year,
        &candidate,
    );
    let mut signals = Vec::new();
    if target.source_title.is_some() && target.candidate_title.is_some() {
        signals.push(deepref_ai::DuplicateSignal::TitleSimilarity {
            similarity: score.title_similarity,
            supports_match: score.title_similarity >= FUZZY_PROPOSAL_THRESHOLD,
        });
    }
    if let Some((source_year, candidate_year)) = target.source_year.zip(target.candidate_year) {
        signals.push(deepref_ai::DuplicateSignal::PublicationYear {
            source_year,
            candidate_year,
            supports_match: score.year_match == Some(true),
        });
    }
    if let Some((source_author, candidate_author)) = target
        .source_author
        .as_ref()
        .zip(target.candidate_author.as_ref())
    {
        signals.push(deepref_ai::DuplicateSignal::FirstAuthor {
            source_author: source_author.clone(),
            candidate_author: candidate_author.clone(),
            similarity: score.first_author_similarity.unwrap_or_default(),
            supports_match: score
                .first_author_similarity
                .is_some_and(|similarity| similarity >= FUZZY_PROPOSAL_THRESHOLD),
        });
    }
    signals
}

const fn extraction_value_type(value_type: ExtractionFieldType) -> ExtractionValueType {
    match value_type {
        ExtractionFieldType::Text => ExtractionValueType::Text,
        ExtractionFieldType::Number => ExtractionValueType::Number,
        ExtractionFieldType::Boolean => ExtractionValueType::Boolean,
        ExtractionFieldType::Date => ExtractionValueType::Date,
    }
}

fn bounded_text(value: &str, max_chars: usize) -> String {
    value.chars().take(max_chars).collect()
}

const fn study_design_label(design: StudyDesign) -> StudyDesignLabel {
    match design {
        StudyDesign::Rct => StudyDesignLabel::Rct,
        StudyDesign::NonRandomizedIntervention => StudyDesignLabel::NonRandomizedIntervention,
        StudyDesign::Cohort => StudyDesignLabel::Cohort,
        StudyDesign::CaseControl => StudyDesignLabel::CaseControl,
        StudyDesign::CrossSectional => StudyDesignLabel::CrossSectional,
        StudyDesign::DiagnosticAccuracy => StudyDesignLabel::DiagnosticAccuracy,
        StudyDesign::PredictionModel => StudyDesignLabel::PredictionModel,
        StudyDesign::Qualitative => StudyDesignLabel::Qualitative,
        StudyDesign::SystematicReview => StudyDesignLabel::SystematicReview,
        StudyDesign::CaseSeries => StudyDesignLabel::CaseSeries,
    }
}
