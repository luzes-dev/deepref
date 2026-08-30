use super::*;

pub(crate) async fn run_task<T>(
    state: &AppState,
    task: &T,
    input: T::Input,
) -> Result<AiProposalRecord, ApiError>
where
    T: deepref_ai::AiTask,
{
    let store = deepref_postgres::PostgresAiStore::new(&state.pool);
    let runner = AiTaskRunner::new(
        state.ai_gateway.as_ref(),
        &store,
        &store,
        &store,
        &store,
        &SystemClock,
        &UuidProvider,
    );
    let result = runner.run(task, input).await.map_err(map_ai_error)?;
    let proposal = result
        .proposal
        .ok_or_else(|| ApiError::Internal(anyhow::anyhow!("AI task did not produce a proposal")))?;
    deepref_postgres::get_ai_proposal(
        &state.pool,
        proposal.draft.project_id.as_uuid(),
        proposal.id,
    )
    .await
    .map_err(map_ai_proposal_error)
}

pub(super) fn metadata_evidence(
    report_id: Uuid,
    target: &deepref_postgres::AiScreeningTarget,
) -> Vec<ScreeningEvidence> {
    let mut evidence = Vec::new();
    if let Some(title) = &target.title {
        evidence.push(ScreeningEvidence::ReportMetadata {
            report_id,
            field: ScreeningEvidenceField::Title,
            content_hash: deepref_ai::sha256_bytes(title.as_bytes()),
        });
    }
    if let Some(abstract_text) = &target.abstract_text {
        evidence.push(ScreeningEvidence::ReportMetadata {
            report_id,
            field: ScreeningEvidenceField::Abstract,
            content_hash: deepref_ai::sha256_bytes(abstract_text.as_bytes()),
        });
    }
    evidence
}

pub(super) fn grouping_evidence(
    target: &deepref_postgres::AiStudyGroupingTarget,
) -> Vec<StudyGroupingEvidence> {
    let mut evidence = Vec::new();
    let mut add_report = |report: &deepref_postgres::AiGroupingReport| {
        if let Some(title) = &report.title {
            evidence.push(StudyGroupingEvidence::ReportMetadata {
                report_id: report.report_id,
                field: StudyGroupingField::Title,
                content_hash: deepref_ai::sha256_bytes(title.as_bytes()),
            });
        }
        if let Some(abstract_text) = &report.abstract_text {
            evidence.push(StudyGroupingEvidence::ReportMetadata {
                report_id: report.report_id,
                field: StudyGroupingField::Abstract,
                content_hash: deepref_ai::sha256_bytes(abstract_text.as_bytes()),
            });
        }
        if let Some(year) = report.publication_year {
            evidence.push(StudyGroupingEvidence::ReportMetadata {
                report_id: report.report_id,
                field: StudyGroupingField::PublicationYear,
                content_hash: deepref_ai::sha256_bytes(year.to_string().as_bytes()),
            });
        }
        if let Some(author) = &report.first_author {
            evidence.push(StudyGroupingEvidence::ReportMetadata {
                report_id: report.report_id,
                field: StudyGroupingField::FirstAuthor,
                content_hash: deepref_ai::sha256_bytes(author.as_bytes()),
            });
        }
    };
    add_report(&target.report);
    for study in &target.studies {
        evidence.push(StudyGroupingEvidence::StudyMetadata {
            study_id: study.study_id,
            field: StudyGroupingField::Title,
            content_hash: deepref_ai::sha256_bytes(study.title.as_bytes()),
        });
        for report in &study.reports {
            if let Some(title) = &report.title {
                evidence.push(StudyGroupingEvidence::StudyReportMetadata {
                    study_id: study.study_id,
                    report_id: report.report_id,
                    field: StudyGroupingField::Title,
                    content_hash: deepref_ai::sha256_bytes(title.as_bytes()),
                });
            }
            if let Some(abstract_text) = &report.abstract_text {
                evidence.push(StudyGroupingEvidence::StudyReportMetadata {
                    study_id: study.study_id,
                    report_id: report.report_id,
                    field: StudyGroupingField::Abstract,
                    content_hash: deepref_ai::sha256_bytes(abstract_text.as_bytes()),
                });
            }
            if let Some(year) = report.publication_year {
                evidence.push(StudyGroupingEvidence::StudyReportMetadata {
                    study_id: study.study_id,
                    report_id: report.report_id,
                    field: StudyGroupingField::PublicationYear,
                    content_hash: deepref_ai::sha256_bytes(year.to_string().as_bytes()),
                });
            }
            if let Some(author) = &report.first_author {
                evidence.push(StudyGroupingEvidence::StudyReportMetadata {
                    study_id: study.study_id,
                    report_id: report.report_id,
                    field: StudyGroupingField::FirstAuthor,
                    content_hash: deepref_ai::sha256_bytes(author.as_bytes()),
                });
            }
        }
    }
    evidence
}

pub(super) fn appraisal_answer_schema(
    schema: &deepref_application::AnswerSchema,
) -> AppraisalAnswerSchema {
    match schema {
        deepref_application::AnswerSchema::Enum { options } => AppraisalAnswerSchema::Enum {
            options: options.iter().map(|option| option.value.clone()).collect(),
        },
        deepref_application::AnswerSchema::Boolean => AppraisalAnswerSchema::Boolean,
        deepref_application::AnswerSchema::Scale { min, max, .. } => AppraisalAnswerSchema::Scale {
            min: *min,
            max: *max,
        },
        deepref_application::AnswerSchema::Text { max_length } => AppraisalAnswerSchema::Text {
            max_length: *max_length,
        },
    }
}

pub(super) fn criterion_prompt(criterion: &EligibilityCriterion) -> CriterionPrompt {
    CriterionPrompt {
        id: criterion.id,
        label: criterion.label.clone(),
        description: criterion.description.clone(),
        ordinal: criterion.ordinal,
        kind: match criterion.kind {
            deepref_domain::CriterionKind::Inclusion => "inclusion".to_owned(),
            deepref_domain::CriterionKind::Exclusion => "exclusion".to_owned(),
        },
        stage: match criterion.stage {
            CriterionStage::TitleAbstract => "title_abstract",
            CriterionStage::FullText => "full_text",
            CriterionStage::Both => "both",
        }
        .to_owned(),
    }
}

pub(super) fn screening_retrieval_query(
    target: &deepref_postgres::AiScreeningTarget,
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
            let char_count = normalized.chars().count();
            if (char_count >= 3
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
        terms.into_iter().collect::<Vec<_>>().join(" OR ")
    }
}

pub(super) fn dedupe_provenance(
    source_record_id: Uuid,
    candidate_report_id: Uuid,
    target: &deepref_postgres::AiDedupeTarget,
) -> Vec<IdentityProvenance> {
    let mut provenance = Vec::new();
    let mut push = |entity_type: &str, entity_id: Uuid, field: &str, value: &str| {
        provenance.push(IdentityProvenance {
            entity_type: entity_type.to_owned(),
            entity_id: entity_id.to_string(),
            field: field.to_owned(),
            content_hash: deepref_ai::sha256_bytes(value.as_bytes()),
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

pub(super) fn dedupe_signals(
    candidate_report_id: Uuid,
    target: &deepref_postgres::AiDedupeTarget,
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
