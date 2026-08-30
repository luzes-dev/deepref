use super::*;

pub(super) fn value_uuid(value: &serde_json::Value, field: &str) -> Option<Uuid> {
    value
        .get(field)
        .and_then(serde_json::Value::as_str)
        .and_then(|value| Uuid::parse_str(value).ok())
}

pub(super) fn reviewed_payload_value(
    proposal: &AiProposalRecord,
    reviewed: Option<&ReviewedAiProposalPayload>,
) -> Result<serde_json::Value, AiProposalError> {
    let Some(reviewed) = reviewed else {
        return Ok(proposal.payload.clone());
    };
    match (proposal.operation.as_str(), reviewed) {
        ("appraisal_prefill", ReviewedAiProposalPayload::AppraisalPrefill(reviewed)) => {
            let original: deepref_ai::AppraisalPrefill =
                serde_json::from_value(proposal.payload.clone()).map_err(|error| {
                    AiProposalError::InvalidPayload(format!(
                        "stored appraisal prefill payload is invalid: {error}"
                    ))
                })?;
            if original.report_id != reviewed.report_id
                || original.definition_id != reviewed.definition_id
                || original.definition_version != reviewed.definition_version
            {
                return Err(AiProposalError::InvalidPayload(
                    "reviewed appraisal must retain the original report and definition version"
                        .to_owned(),
                ));
            }
            serde_json::to_value(reviewed).map_err(|error| {
                AiProposalError::InvalidPayload(format!(
                    "reviewed appraisal is not serializable: {error}"
                ))
            })
        }
        ("data_extraction", ReviewedAiProposalPayload::DataExtraction(reviewed)) => {
            let original: deepref_ai::DataExtraction =
                serde_json::from_value(proposal.payload.clone()).map_err(|error| {
                    AiProposalError::InvalidPayload(format!(
                        "stored extraction payload is invalid: {error}"
                    ))
                })?;
            if original.study_id != reviewed.study_id
                || extraction_field_set(&original) != extraction_field_set(reviewed)
            {
                return Err(AiProposalError::InvalidPayload(
                    "reviewed extraction must retain the original study and field versions"
                        .to_owned(),
                ));
            }
            serde_json::to_value(reviewed).map_err(|error| {
                AiProposalError::InvalidPayload(format!(
                    "reviewed extraction is not serializable: {error}"
                ))
            })
        }
        _ => Err(AiProposalError::InvalidPayload(
            "reviewed payload variant does not match this proposal operation".to_owned(),
        )),
    }
}

pub(super) fn extraction_field_set(
    extraction: &deepref_ai::DataExtraction,
) -> std::collections::BTreeSet<(Uuid, u32)> {
    extraction
        .fields
        .iter()
        .map(|field| match field {
            deepref_ai::ExtractedField::Value {
                field_id,
                field_version,
                ..
            }
            | deepref_ai::ExtractedField::InsufficientEvidence {
                field_id,
                field_version,
                ..
            } => (*field_id, *field_version),
        })
        .collect()
}

#[derive(Debug, Clone)]
pub(super) struct ClassificationReportMetadata {
    report_id: Uuid,
    title: Option<String>,
    abstract_text: Option<String>,
    publication_year: Option<i32>,
}

pub(super) async fn apply_study_classification(
    tx: &mut Transaction<'_, Postgres>,
    proposal: &AiProposalRecord,
    payload: &serde_json::Value,
    actor: &Actor,
) -> Result<i64, AiProposalError> {
    let classification: StudyDesignClassification = serde_json::from_value(payload.clone())
        .map_err(|_| {
            AiProposalError::InvalidPayload("classification payload is invalid".to_owned())
        })?;
    let study_id = proposal.target_study_id.ok_or_else(|| {
        AiProposalError::InvalidTarget("classification study is missing".to_owned())
    })?;
    if proposal.entity_type != "study_classification"
        || proposal.entity_id != Some(study_id)
        || classification.study_id != study_id
    {
        return Err(AiProposalError::InvalidTarget(
            "classification target is invalid".to_owned(),
        ));
    }
    let expected_revision = proposal.expected_revision.ok_or_else(|| {
        AiProposalError::InvalidTarget("classification revision is missing".to_owned())
    })?;
    if expected_revision < 0
        || payload
            .get("expected_revision")
            .and_then(serde_json::Value::as_i64)
            != Some(expected_revision)
    {
        return Err(AiProposalError::InvalidPayload(
            "classification revision is invalid".to_owned(),
        ));
    }
    let expected_revision_u64 = u64::try_from(expected_revision).map_err(|_| {
        AiProposalError::InvalidPayload("classification revision is invalid".to_owned())
    })?;
    if classification.suggested_design.is_none() {
        return Err(AiProposalError::InvalidPayload(
            "classification abstention cannot be accepted".to_owned(),
        ));
    }

    let current = crate::study::lock_study(tx, proposal.project_id, study_id).await?;
    if current.revision != expected_revision {
        return Err(AiProposalError::InvalidTarget(
            "classification study revision is stale".to_owned(),
        ));
    }
    let report_rows = sqlx::query(
        "SELECT sr.report_id,r.title,r.abstract_text,r.publication_year
         FROM study_reports sr
         JOIN reports r ON r.id=sr.report_id
         WHERE sr.project_id=$1 AND sr.study_id=$2
         ORDER BY sr.created_at,sr.report_id
         LIMIT 100
         FOR UPDATE OF r",
    )
    .bind(proposal.project_id)
    .bind(study_id)
    .fetch_all(&mut **tx)
    .await?;
    let reports = report_rows
        .into_iter()
        .map(|row| ClassificationReportMetadata {
            report_id: row.get("report_id"),
            title: row.get("title"),
            abstract_text: row.get("abstract_text"),
            publication_year: row.get("publication_year"),
        })
        .collect::<Vec<_>>();
    let grounded_evidence = classification_grounding(&current, &reports);
    let input = StudyDesignClassificationInput {
        project_id: proposal.project_id.into(),
        study_id: study_id.into(),
        expected_revision: expected_revision_u64,
        study_title: current.title.clone(),
        current_design: current.design.map(classification_label),
        reports: reports
            .iter()
            .map(|report| deepref_ai::StudyDesignReport {
                report_id: report.report_id,
                title: report
                    .title
                    .as_deref()
                    .map(|value| bounded_classification_text(value, 4_000)),
                abstract_text: report
                    .abstract_text
                    .as_deref()
                    .map(|value| bounded_classification_text(value, 16_000)),
                publication_year: report.publication_year,
            })
            .collect(),
        allowed_designs: StudyDesignLabel::ALL.to_vec(),
        grounded_evidence,
    };
    let task = deepref_ai::StudyDesignClassificationTask::new(&input).map_err(|_| {
        AiProposalError::InvalidPayload("classification payload is invalid".to_owned())
    })?;
    task.semantic_validate(&classification).map_err(|_| {
        AiProposalError::InvalidPayload("classification payload is invalid".to_owned())
    })?;
    validate_classification_evidence(&current, &reports, &classification.evidence)?;

    let design = classification
        .suggested_design
        .map(classification_design)
        .ok_or_else(|| {
            AiProposalError::InvalidPayload(
                "classification abstention cannot be accepted".to_owned(),
            )
        })?;
    crate::study::apply_classification_in_transaction(
        tx,
        ClassifyStudy {
            project_id: proposal.project_id.into(),
            study_id: study_id.into(),
            design,
            context: current.design_context,
            expected_revision: expected_revision_u64,
            actor: actor.clone(),
        },
        current,
    )
    .await
    .map_err(AiProposalError::Study)
}

pub(super) fn classification_grounding(
    study: &crate::study::LockedStudy,
    reports: &[ClassificationReportMetadata],
) -> Vec<StudyDesignEvidence> {
    let mut evidence = vec![StudyDesignEvidence::StudyMetadata {
        study_id: study.id.into(),
        field: StudyMetadataField::Title,
        content_hash: deepref_ai::sha256_bytes(study.title.as_bytes()),
    }];
    for report in reports {
        if let Some(title) = &report.title {
            evidence.push(StudyDesignEvidence::ReportMetadata {
                report_id: report.report_id,
                field: ClassificationReportField::Title,
                content_hash: deepref_ai::sha256_bytes(title.as_bytes()),
            });
        }
        if let Some(abstract_text) = &report.abstract_text {
            evidence.push(StudyDesignEvidence::ReportMetadata {
                report_id: report.report_id,
                field: ClassificationReportField::Abstract,
                content_hash: deepref_ai::sha256_bytes(abstract_text.as_bytes()),
            });
        }
        if let Some(publication_year) = report.publication_year {
            evidence.push(StudyDesignEvidence::ReportMetadata {
                report_id: report.report_id,
                field: ClassificationReportField::PublicationYear,
                content_hash: deepref_ai::sha256_bytes(publication_year.to_string().as_bytes()),
            });
        }
    }
    evidence
}

pub(super) fn validate_classification_evidence(
    study: &crate::study::LockedStudy,
    reports: &[ClassificationReportMetadata],
    evidence: &[StudyDesignEvidence],
) -> Result<(), AiProposalError> {
    let reports_by_id = reports
        .iter()
        .map(|report| (report.report_id, report))
        .collect::<HashMap<_, _>>();
    for item in evidence {
        let (content, expected_hash) = match item {
            StudyDesignEvidence::StudyMetadata {
                study_id,
                field: StudyMetadataField::Title,
                content_hash,
            } if *study_id == study.id.as_uuid() => (study.title.clone(), content_hash),
            StudyDesignEvidence::StudyMetadata { .. } => {
                return Err(AiProposalError::InvalidPayload(
                    "classification evidence is invalid".to_owned(),
                ));
            }
            StudyDesignEvidence::ReportMetadata {
                report_id,
                field,
                content_hash,
            } => {
                let report = reports_by_id.get(report_id).ok_or_else(|| {
                    AiProposalError::InvalidPayload("classification evidence is invalid".to_owned())
                })?;
                let content = match field {
                    ClassificationReportField::Title => report.title.clone(),
                    ClassificationReportField::Abstract => report.abstract_text.clone(),
                    ClassificationReportField::PublicationYear => {
                        report.publication_year.map(|value| value.to_string())
                    }
                }
                .ok_or_else(|| {
                    AiProposalError::InvalidPayload("classification evidence is invalid".to_owned())
                })?;
                (content, content_hash)
            }
        };
        if deepref_ai::sha256_bytes(content.as_bytes()) != *expected_hash {
            return Err(AiProposalError::InvalidPayload(
                "classification evidence is stale".to_owned(),
            ));
        }
    }
    Ok(())
}

pub(super) fn bounded_classification_text(value: &str, max_chars: usize) -> String {
    value.chars().take(max_chars).collect()
}

pub(super) fn classification_label(design: deepref_domain::StudyDesign) -> StudyDesignLabel {
    match design {
        deepref_domain::StudyDesign::Rct => StudyDesignLabel::Rct,
        deepref_domain::StudyDesign::NonRandomizedIntervention => {
            StudyDesignLabel::NonRandomizedIntervention
        }
        deepref_domain::StudyDesign::Cohort => StudyDesignLabel::Cohort,
        deepref_domain::StudyDesign::CaseControl => StudyDesignLabel::CaseControl,
        deepref_domain::StudyDesign::CrossSectional => StudyDesignLabel::CrossSectional,
        deepref_domain::StudyDesign::DiagnosticAccuracy => StudyDesignLabel::DiagnosticAccuracy,
        deepref_domain::StudyDesign::PredictionModel => StudyDesignLabel::PredictionModel,
        deepref_domain::StudyDesign::Qualitative => StudyDesignLabel::Qualitative,
        deepref_domain::StudyDesign::SystematicReview => StudyDesignLabel::SystematicReview,
        deepref_domain::StudyDesign::CaseSeries => StudyDesignLabel::CaseSeries,
    }
}

pub(super) fn classification_design(label: StudyDesignLabel) -> deepref_domain::StudyDesign {
    match label {
        StudyDesignLabel::Rct => deepref_domain::StudyDesign::Rct,
        StudyDesignLabel::NonRandomizedIntervention => {
            deepref_domain::StudyDesign::NonRandomizedIntervention
        }
        StudyDesignLabel::Cohort => deepref_domain::StudyDesign::Cohort,
        StudyDesignLabel::CaseControl => deepref_domain::StudyDesign::CaseControl,
        StudyDesignLabel::CrossSectional => deepref_domain::StudyDesign::CrossSectional,
        StudyDesignLabel::DiagnosticAccuracy => deepref_domain::StudyDesign::DiagnosticAccuracy,
        StudyDesignLabel::PredictionModel => deepref_domain::StudyDesign::PredictionModel,
        StudyDesignLabel::Qualitative => deepref_domain::StudyDesign::Qualitative,
        StudyDesignLabel::SystematicReview => deepref_domain::StudyDesign::SystematicReview,
        StudyDesignLabel::CaseSeries => deepref_domain::StudyDesign::CaseSeries,
    }
}

pub(super) async fn validate_study_grouping_provenance(
    tx: &mut Transaction<'_, Postgres>,
    project_id: Uuid,
    target_report_id: Uuid,
    grouping: &deepref_ai::StudyGroupingProposal,
) -> Result<(), AiProposalError> {
    let mut seen = std::collections::BTreeSet::new();
    if grouping.provenance.len() > 256 {
        return Err(AiProposalError::InvalidPayload(
            "grouping provenance exceeds the acceptance bound".to_owned(),
        ));
    }
    for evidence in &grouping.provenance {
        let key = serde_json::to_string(evidence).map_err(|error| {
            AiProposalError::InvalidPayload(format!("grouping provenance is invalid: {error}"))
        })?;
        if !seen.insert(key) {
            return Err(AiProposalError::InvalidPayload(
                "grouping provenance contains a duplicate entry".to_owned(),
            ));
        }
        let (current_hash, scope_is_valid) = match evidence {
            deepref_ai::StudyGroupingEvidence::ReportMetadata {
                report_id, field, ..
            } => {
                let row = sqlx::query(
                    "SELECT r.title,r.abstract_text,r.publication_year,r.authors
                     FROM project_reports pr JOIN reports r ON r.id=pr.report_id
                     WHERE pr.project_id=$1 AND pr.report_id=$2",
                )
                .bind(project_id)
                .bind(report_id)
                .fetch_optional(&mut **tx)
                .await?;
                let current_hash = row.and_then(|row| {
                    grouping_metadata_hash(
                        *field,
                        row.get("title"),
                        row.get("abstract_text"),
                        row.get("publication_year"),
                        first_author(row.get("authors")),
                    )
                });
                (current_hash, *report_id == target_report_id)
            }
            deepref_ai::StudyGroupingEvidence::StudyMetadata {
                study_id, field, ..
            } => {
                let row = sqlx::query("SELECT title FROM studies WHERE project_id=$1 AND id=$2")
                    .bind(project_id)
                    .bind(study_id)
                    .fetch_optional(&mut **tx)
                    .await?;
                let current_hash = row.and_then(|row| {
                    grouping_metadata_hash(
                        *field,
                        Some(row.get::<String, _>("title")),
                        None,
                        None,
                        None,
                    )
                });
                (current_hash, true)
            }
            deepref_ai::StudyGroupingEvidence::StudyReportMetadata {
                study_id,
                report_id,
                field,
                ..
            } => {
                let row = sqlx::query(
                    "SELECT r.title,r.abstract_text,r.publication_year,r.authors
                     FROM study_reports sr
                     JOIN reports r ON r.id=sr.report_id
                     WHERE sr.project_id=$1 AND sr.study_id=$2 AND sr.report_id=$3",
                )
                .bind(project_id)
                .bind(study_id)
                .bind(report_id)
                .fetch_optional(&mut **tx)
                .await?;
                let current_hash = row.and_then(|row| {
                    grouping_metadata_hash(
                        *field,
                        row.get("title"),
                        row.get("abstract_text"),
                        row.get("publication_year"),
                        first_author(row.get("authors")),
                    )
                });
                (current_hash, true)
            }
        };
        let expected_hash = match evidence {
            deepref_ai::StudyGroupingEvidence::ReportMetadata { content_hash, .. }
            | deepref_ai::StudyGroupingEvidence::StudyMetadata { content_hash, .. }
            | deepref_ai::StudyGroupingEvidence::StudyReportMetadata { content_hash, .. } => {
                content_hash
            }
        };
        if !scope_is_valid
            || !deepref_ai::is_sha256(expected_hash)
            || current_hash.as_deref() != Some(expected_hash)
        {
            return Err(AiProposalError::InvalidPayload(
                "grouping provenance is stale or outside the target project".to_owned(),
            ));
        }
    }
    Ok(())
}

pub(super) fn grouping_metadata_hash(
    field: deepref_ai::StudyGroupingField,
    title: Option<String>,
    abstract_text: Option<String>,
    publication_year: Option<i32>,
    first_author: Option<String>,
) -> Option<String> {
    let value = match field {
        deepref_ai::StudyGroupingField::Title => title,
        deepref_ai::StudyGroupingField::Abstract => abstract_text,
        deepref_ai::StudyGroupingField::PublicationYear => {
            publication_year.map(|year| year.to_string())
        }
        deepref_ai::StudyGroupingField::FirstAuthor => first_author,
    }?;
    Some(deepref_ai::sha256_bytes(value.as_bytes()))
}

pub(super) async fn apply_study_grouping(
    tx: &mut Transaction<'_, Postgres>,
    proposal: &AiProposalRecord,
    payload: &serde_json::Value,
    actor: &Actor,
) -> Result<(), AiProposalError> {
    let grouping: deepref_ai::StudyGroupingProposal = serde_json::from_value(payload.clone())
        .map_err(|error| {
            AiProposalError::InvalidPayload(format!("study grouping payload is invalid: {error}"))
        })?;
    let report_id = proposal
        .target_report_id
        .ok_or_else(|| AiProposalError::InvalidTarget("grouping report is missing".to_owned()))?;
    if grouping.report_id != report_id
        || grouping.rationale.trim().is_empty()
        || grouping.provenance.is_empty()
        || grouping.provenance.iter().any(|evidence| {
            !deepref_ai::is_sha256(match evidence {
                deepref_ai::StudyGroupingEvidence::ReportMetadata { content_hash, .. }
                | deepref_ai::StudyGroupingEvidence::StudyMetadata { content_hash, .. }
                | deepref_ai::StudyGroupingEvidence::StudyReportMetadata { content_hash, .. } => {
                    content_hash
                }
            })
        })
    {
        return Err(AiProposalError::InvalidPayload(
            "grouping rationale or provenance is invalid".to_owned(),
        ));
    }
    validate_study_grouping_provenance(tx, proposal.project_id, report_id, &grouping).await?;
    let previous_id = grouping.expected_previous_study_id.map(Into::into);
    let previous_revision = grouping
        .expected_previous_study_revision
        .map(|revision| {
            u64::try_from(revision).map_err(|_| {
                AiProposalError::InvalidPayload("previous study revision is invalid".to_owned())
            })
        })
        .transpose()?;
    match grouping.choice {
        deepref_ai::StudyGroupingChoice::ExistingStudy {
            study_id,
            expected_revision,
        } => {
            let expected_revision = u64::try_from(expected_revision).map_err(|_| {
                AiProposalError::InvalidPayload("target study revision is invalid".to_owned())
            })?;
            let study_id = study_id.into();
            if Some(study_id) == previous_id {
                return Err(AiProposalError::InvalidTarget(
                    "grouping proposal does not change study membership".to_owned(),
                ));
            }
            crate::study::assign_report_to_study_in_transaction(
                tx,
                deepref_application::AssignReportToStudy {
                    project_id: proposal.project_id.into(),
                    study_id,
                    report_id: report_id.into(),
                    role: StudyReportRole::ReportOfStudy,
                    expected_revision,
                    expected_previous_study_id: previous_id,
                    expected_previous_study_revision: previous_revision,
                    actor: actor.clone(),
                },
            )
            .await?;
        }
        deepref_ai::StudyGroupingChoice::NewStudy { title } => {
            let title = StudyTitle::new(title)
                .map_err(|error| AiProposalError::InvalidPayload(error.to_string()))?;
            crate::study::create_study_and_assign_report_in_transaction(
                tx,
                deepref_application::CreateStudy {
                    project_id: proposal.project_id.into(),
                    study_id: Uuid::new_v4().into(),
                    title,
                    actor: actor.clone(),
                },
                report_id.into(),
                StudyReportRole::ReportOfStudy,
                previous_id,
                previous_revision,
            )
            .await?;
        }
    }
    Ok(())
}

pub(super) async fn apply_appraisal_prefill(
    tx: &mut Transaction<'_, Postgres>,
    proposal: &AiProposalRecord,
    payload: &serde_json::Value,
    actor: &Actor,
) -> Result<(), AiProposalError> {
    let prefill: deepref_ai::AppraisalPrefill =
        serde_json::from_value(payload.clone()).map_err(|error| {
            AiProposalError::InvalidPayload(format!(
                "appraisal prefill payload is invalid: {error}"
            ))
        })?;
    let report_id = proposal
        .target_report_id
        .ok_or_else(|| AiProposalError::InvalidTarget("appraisal report is missing".to_owned()))?;
    if prefill.report_id != report_id || prefill.answers.is_empty() {
        return Err(AiProposalError::InvalidTarget(
            "appraisal payload targets another report".to_owned(),
        ));
    }
    let definition_id = DefinitionId::new(prefill.definition_id.clone())
        .map_err(|error| AiProposalError::InvalidPayload(error.to_string()))?;
    let definition_version =
        DefinitionVersion::new(prefill.definition_version).ok_or_else(|| {
            AiProposalError::InvalidPayload("appraisal definition version is invalid".to_owned())
        })?;
    let definition = get_appraisal_definition(definition_id.as_str(), definition_version.get())
        .map_err(|error| AiProposalError::InvalidPayload(error.to_string()))?;
    let questions = definition
        .domains
        .iter()
        .flat_map(|domain| domain.questions.iter())
        .map(|question| deepref_ai::AppraisalPrefillQuestion {
            id: question.id.clone(),
            answer_schema: match &question.answer_schema {
                deepref_application::AnswerSchema::Enum { options } => {
                    deepref_ai::AppraisalAnswerSchema::Enum {
                        options: options.iter().map(|option| option.value.clone()).collect(),
                    }
                }
                deepref_application::AnswerSchema::Boolean => {
                    deepref_ai::AppraisalAnswerSchema::Boolean
                }
                deepref_application::AnswerSchema::Scale { min, max, .. } => {
                    deepref_ai::AppraisalAnswerSchema::Scale {
                        min: *min,
                        max: *max,
                    }
                }
                deepref_application::AnswerSchema::Text { max_length } => {
                    deepref_ai::AppraisalAnswerSchema::Text {
                        max_length: *max_length,
                    }
                }
            },
            required: question.required,
            requires_evidence: question.requires_evidence,
        })
        .collect::<Vec<_>>();
    let domains = definition
        .domains
        .iter()
        .map(|domain| deepref_ai::AppraisalPrefillDomain {
            id: domain.id.clone(),
            allowed_judgments: domain
                .judgment
                .options
                .iter()
                .map(|option| option.value.clone())
                .collect(),
            required: domain.judgment.required,
        })
        .collect::<Vec<_>>();
    let overall_allowed_judgments = definition
        .overall_judgment
        .options
        .iter()
        .map(|option| option.value.clone())
        .collect::<Vec<_>>();
    let grounded_evidence = prefill
        .answers
        .iter()
        .flat_map(|answer| answer.evidence.iter().cloned())
        .collect::<Vec<_>>();
    let task_input = deepref_ai::AppraisalPrefillInput {
        project_id: proposal.project_id.into(),
        report_id: report_id.into(),
        definition_id: definition_id.as_str().to_owned(),
        definition_version: definition_version.get(),
        questions,
        domains,
        overall_allowed_judgments,
        report_title: None,
        report_abstract: None,
        grounded_evidence,
    };
    let task = deepref_ai::AppraisalPrefillTask::new(&task_input)
        .map_err(|error| AiProposalError::InvalidPayload(error.to_string()))?;
    <deepref_ai::AppraisalPrefillTask as deepref_ai::AiTask>::semantic_validate(&task, &prefill)
        .map_err(|error| AiProposalError::InvalidPayload(error.to_string()))?;
    let mut responses = serde_json::Map::new();
    let mut evidence = Vec::new();
    for answer in &prefill.answers {
        let response = match &answer.answer {
            deepref_ai::AppraisalAnswerValue::Enum { value }
            | deepref_ai::AppraisalAnswerValue::Text { value } => {
                serde_json::Value::String(value.clone())
            }
            deepref_ai::AppraisalAnswerValue::Boolean { value } => serde_json::Value::Bool(*value),
            deepref_ai::AppraisalAnswerValue::Scale { value } => {
                serde_json::json!(value)
            }
        };
        if responses
            .insert(answer.question_id.clone(), response)
            .is_some()
        {
            return Err(AiProposalError::InvalidPayload(
                "appraisal payload contains a duplicate question".to_owned(),
            ));
        }
        for source in &answer.evidence {
            let page = source.page;
            if !deepref_ai::is_sha256(&source.content_hash)
                || source.parser_version.trim().is_empty()
            {
                return Err(AiProposalError::InvalidPayload(
                    "appraisal evidence provenance is invalid".to_owned(),
                ));
            }
            evidence.push(EvidenceReferenceInput {
                question_id: answer.question_id.clone(),
                document_id: source.document_id,
                block_id: source.document_block_id,
                page: Some(page),
                parser_version: Some(source.parser_version.clone()),
                content_hash: Some(source.content_hash.clone()),
            });
        }
    }
    let input = AppraisalAssessmentInput {
        definition_id,
        definition_version,
        responses: serde_json::Value::Object(responses),
        evidence,
        domain_judgments: prefill.domain_judgments,
        overall_judgment: Some(prefill.overall_judgment),
    };
    crate::appraisal::complete_appraisal_in_transaction(
        tx,
        proposal.project_id.into(),
        report_id.into(),
        input,
        actor.clone(),
    )
    .await?;
    Ok(())
}

pub(super) fn parse_screening_stage(
    value: Option<&str>,
) -> Result<ScreeningStage, AiProposalError> {
    match value {
        Some("title_abstract") => Ok(ScreeningStage::TitleAbstract),
        Some("full_text") => Ok(ScreeningStage::FullText),
        _ => Err(AiProposalError::InvalidPayload(
            "screening stage is invalid".to_owned(),
        )),
    }
}

pub(super) fn parse_screening_decision(
    payload: &serde_json::Value,
    stage: ScreeningStage,
) -> Result<ScreeningDecision, AiProposalError> {
    let kind = payload
        .get("suggested_decision")
        .and_then(|value| value.get("kind"))
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| {
            AiProposalError::InvalidPayload("suggested decision is missing".to_owned())
        })?;
    match (stage, kind) {
        (_, "include") => Ok(ScreeningDecision::Include),
        (_, "maybe") => Ok(ScreeningDecision::Maybe),
        (_, "exclude") => Ok(ScreeningDecision::Exclude),
        (_, "insufficient_evidence") => Err(AiProposalError::InvalidPayload(
            "insufficient evidence must be reviewed rather than accepted as a decision".to_owned(),
        )),
        _ => Err(AiProposalError::InvalidPayload(
            "suggested decision is invalid".to_owned(),
        )),
    }
}

pub(super) fn screening_reason_id(
    payload: &serde_json::Value,
) -> Result<Option<Uuid>, AiProposalError> {
    let Some(decision) = payload.get("suggested_decision") else {
        return Err(AiProposalError::InvalidPayload(
            "suggested decision is missing".to_owned(),
        ));
    };
    if decision.get("kind").and_then(serde_json::Value::as_str) != Some("exclude") {
        return Ok(None);
    }
    decision
        .get("exclusion_reason_id")
        .and_then(serde_json::Value::as_str)
        .map(|value| {
            Uuid::parse_str(value).map_err(|_| {
                AiProposalError::InvalidPayload("exclusion reason is invalid".to_owned())
            })
        })
        .transpose()
}

pub(super) fn proposal_record_from_row(
    row: sqlx::postgres::PgRow,
) -> Result<AiProposalRecord, String> {
    Ok(AiProposalRecord {
        id: row.get("id"),
        project_id: row.get("project_id"),
        task_kind: row.get("task_kind"),
        entity_type: row.get("entity_type"),
        entity_id: row.get("entity_id"),
        operation: row.get("operation"),
        payload: row.get("payload"),
        authority_tier: row.get("authority_tier"),
        model_run_id: row.get("model_run_id"),
        provider: row.get("provider"),
        model: row.get("model"),
        model_version: row.get("model_version"),
        prompt_version: row.get("prompt_version"),
        schema_version: row.get("schema_version"),
        status: row.get("status"),
        protocol_version_id: row.get("protocol_version_id"),
        expected_revision: row.get("expected_revision"),
        target_report_id: row.get("target_report_id"),
        target_record_id: row.get("target_record_id"),
        target_study_id: row.get("target_study_id"),
        prompt_hash: row.get("prompt_hash"),
        schema_hash: row.get("schema_hash"),
        input_hash: row.get("input_hash"),
        evidence_hash: row.get("evidence_hash"),
        resolved_at: row.get("resolved_at"),
        resolved_by_actor_kind: row.get("resolved_by_actor_kind"),
        resolved_by_actor_id: row.get("resolved_by_actor_id"),
        resolution_reason: row.get("resolution_reason"),
        created_at: row.get("created_at"),
    })
}
