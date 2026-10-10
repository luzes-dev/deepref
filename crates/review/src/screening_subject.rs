//! The semantic mapping from a persisted screening subject to the task the
//! model judges.
//!
//! The PostgreSQL adapter reads the rows. Everything that decides what the
//! model is shown and what it may cite lives here, inside the boundary that
//! calibration identity fingerprints: which criteria a stage judges, the
//! criterion prompts, the metadata that may be cited, the retrieval query and
//! the exclusion reasons a stage may use. The golden fixtures in
//! [`crate::golden`] render this mapping, so a change to it is a semantic
//! change that makes calibration evidence stale.

use std::collections::BTreeSet;

use deepref_ai::{
    CriterionPrompt, ScreeningEvidence, ScreeningEvidenceField, ScreeningInput, ScreeningStage,
    ScreeningTask, ScreeningTaskConfig, criteria_for_stage, sha256_bytes,
};
use deepref_domain::{
    CriterionKind, CriterionStage, EligibilityCriterion, ProjectId, ProtocolVersionId, ReportId,
    ScreeningStage as DomainScreeningStage,
};
use uuid::Uuid;

use crate::{ReviewError, worker::PreparedReviewTask};

/// The rows that define one persisted screening subject: a report at a stage,
/// judged against the published protocol.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScreeningSubjectSource {
    pub project_id: ProjectId,
    pub report_id: ReportId,
    pub stage: DomainScreeningStage,
    pub protocol_version_id: ProtocolVersionId,
    pub expected_revision: i64,
    /// Every criterion of the published protocol, in stored order.
    pub criteria: Vec<EligibilityCriterion>,
    pub title: Option<String>,
    pub abstract_text: Option<String>,
    /// Every exclusion reason the project defines. The stage decides which of
    /// them a screen may cite.
    pub exclusion_reasons: Vec<ScreeningExclusionReason>,
}

/// An exclusion reason and the screening stage it applies to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScreeningExclusionReason {
    pub id: Uuid,
    pub stage: DomainScreeningStage,
}

/// Maps a persisted screening subject to the prepared task that the worker
/// executes and that the run manifest fingerprints.
///
/// The input is the rows as stored. Nothing here reads the database, so the
/// same rows always produce the same task.
pub fn prepare_screening_task(
    source: ScreeningSubjectSource,
) -> Result<PreparedReviewTask, ReviewError> {
    if source.expected_revision < 0 {
        return Err(ReviewError::InvalidExpectedRevision);
    }
    let stage = match source.stage {
        DomainScreeningStage::TitleAbstract => ScreeningStage::TitleAbstract,
        DomainScreeningStage::FullText => ScreeningStage::FullText,
    };
    let report_id = source.report_id.as_uuid();
    let allowed_exclusion_reasons = source
        .exclusion_reasons
        .iter()
        .filter(|reason| reason.stage == source.stage)
        .map(|reason| reason.id)
        .collect::<BTreeSet<_>>();
    let criteria = source.criteria;
    // Report metadata is citable only at title/abstract. Full-text citations come
    // from the retrieved passages, so no metadata is offered at full text.
    let allowed_evidence = match stage {
        ScreeningStage::TitleAbstract => metadata_evidence(
            report_id,
            source.title.as_deref(),
            source.abstract_text.as_deref(),
        ),
        ScreeningStage::FullText => Vec::new(),
    };
    let input = ScreeningInput {
        project_id: source.project_id,
        report_id: source.report_id,
        stage,
        protocol_version_id: source.protocol_version_id,
        expected_revision: source.expected_revision,
        title: source.title.clone(),
        abstract_text: source.abstract_text.clone(),
        document_hash: None,
        retrieval_query: (stage == ScreeningStage::FullText).then(|| {
            screening_retrieval_query(
                source.title.as_deref(),
                source.abstract_text.as_deref(),
                &criteria,
            )
        }),
        // Only the criteria this stage judges reach the model, in the order the
        // validator requires. The full protocol stays on the task for validation.
        criteria: criteria_for_stage(&criteria, stage)
            .into_iter()
            .map(criterion_prompt)
            .collect(),
    };
    Ok(PreparedReviewTask::Screening {
        input,
        criteria,
        allowed_evidence,
        allowed_exclusion_reasons,
    })
}

/// The task that judges a prepared screening subject. The executor and the
/// golden fixtures build it here, so they cannot disagree about its
/// configuration.
pub(crate) fn screening_task_for(
    input: &ScreeningInput,
    criteria: &[EligibilityCriterion],
    allowed_evidence: &[ScreeningEvidence],
    allowed_exclusion_reasons: &BTreeSet<Uuid>,
) -> ScreeningTask {
    ScreeningTask::new(ScreeningTaskConfig {
        project_id: input.project_id,
        report_id: input.report_id,
        stage: input.stage,
        protocol_version_id: input.protocol_version_id,
        expected_revision: input.expected_revision,
        criteria: criteria.to_vec(),
        allowed_evidence: allowed_evidence.to_vec(),
        allowed_exclusion_reasons: allowed_exclusion_reasons.clone(),
    })
}

fn metadata_evidence(
    report_id: Uuid,
    title: Option<&str>,
    abstract_text: Option<&str>,
) -> Vec<ScreeningEvidence> {
    let mut evidence = Vec::new();
    if let Some(title) = title {
        evidence.push(ScreeningEvidence::ReportMetadata {
            report_id,
            field: ScreeningEvidenceField::Title,
            content_hash: sha256_bytes(title.as_bytes()),
        });
    }
    if let Some(abstract_text) = abstract_text {
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
            CriterionKind::Inclusion => "inclusion",
            CriterionKind::Exclusion => "exclusion",
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
    title: Option<&str>,
    abstract_text: Option<&str>,
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
    if let Some(title) = title {
        add_terms(title);
    }
    if let Some(abstract_text) = abstract_text {
        add_terms(abstract_text);
    }
    if terms.is_empty() {
        "full-text eligibility evidence".to_owned()
    } else {
        terms.join(" OR ")
    }
}

#[cfg(test)]
mod tests {
    use deepref_domain::{CriterionDimension, CriterionKind, CriterionStage};

    use super::*;

    fn criterion(
        id: u128,
        kind: CriterionKind,
        stage: CriterionStage,
        dimension: CriterionDimension,
        label: &str,
        description: &str,
        ordinal: i32,
    ) -> EligibilityCriterion {
        EligibilityCriterion {
            id: Uuid::from_u128(id),
            kind,
            stage,
            dimension,
            label: label.to_owned(),
            description: description.to_owned(),
            ordinal,
        }
    }

    fn subject(
        name: &'static str,
        stage: DomainScreeningStage,
        revision: i64,
        title: Option<&str>,
        abstract_text: Option<String>,
        criteria: Vec<EligibilityCriterion>,
        reasons: &[(u128, DomainScreeningStage)],
    ) -> (&'static str, ScreeningSubjectSource) {
        (
            name,
            ScreeningSubjectSource {
                project_id: ProjectId::new(Uuid::from_u128(0x1000)),
                report_id: ReportId::new(Uuid::from_u128(0x2000)),
                stage,
                protocol_version_id: ProtocolVersionId::new(Uuid::from_u128(0x3000)),
                expected_revision: revision,
                criteria,
                title: title.map(str::to_owned),
                abstract_text,
                exclusion_reasons: reasons
                    .iter()
                    .map(|(id, stage)| ScreeningExclusionReason {
                        id: Uuid::from_u128(*id),
                        stage: *stage,
                    })
                    .collect(),
            },
        )
    }

    /// Six subjects covering both stages, every criterion stage, missing title
    /// or abstract, unicode and very long text, and many retrieval terms.
    fn samples() -> Vec<(&'static str, ScreeningSubjectSource)> {
        use CriterionDimension as D;
        use CriterionKind as K;
        use CriterionStage as C;
        use DomainScreeningStage as S;
        let a = criterion(
            0xa1,
            K::Inclusion,
            C::Both,
            D::Population,
            "Adults",
            "Adults aged 18 or older",
            2,
        );
        let b = criterion(
            0xb2,
            K::Exclusion,
            C::TitleAbstract,
            D::Design,
            "Animal study",
            "Non-human animal study",
            1,
        );
        let c = criterion(
            0xc3,
            K::Inclusion,
            C::FullText,
            D::Outcome,
            "Outcome",
            "Reports mortality at one year",
            3,
        );
        let d = criterion(
            0xd4,
            K::Exclusion,
            C::FullText,
            D::Other,
            "Duplicate",
            "Duplicate publication of the same cohort",
            3,
        );
        let e = criterion(
            0xe5,
            K::Inclusion,
            C::Both,
            D::Language,
            "English",
            "Full text in English",
            0,
        );
        let long_abstract = "Unicode: café, naïve, 研究, ✓ and tabs\tand\nnewlines. ".repeat(400);
        let long_terms = (0..90)
            .map(|index| {
                format!(
                    "term{index:03}alphabetagammadeltaepsilonzetaetathetaiotakappalambdamu{index}"
                )
            })
            .collect::<Vec<_>>()
            .join(" ");
        let title = "Efficacy of drug X in adults";
        let abstract_text = "We randomised 200 adults to drug X or placebo.";
        vec![
            subject(
                "ta_full",
                S::TitleAbstract,
                3,
                Some(title),
                Some(abstract_text.to_owned()),
                vec![a.clone(), b.clone(), c.clone(), d.clone(), e.clone()],
                &[(0x4001, S::TitleAbstract), (0x4002, S::FullText)],
            ),
            subject(
                "ta_missing_abstract",
                S::TitleAbstract,
                3,
                Some("Short title"),
                None,
                vec![a.clone(), b.clone()],
                &[],
            ),
            subject(
                "ta_unicode_long",
                S::TitleAbstract,
                3,
                Some("Efeitos — 研究 ✓\tTab"),
                Some(long_abstract),
                vec![b.clone(), a.clone()],
                &[],
            ),
            subject(
                "ft_full",
                S::FullText,
                7,
                Some(title),
                Some(abstract_text.to_owned()),
                vec![a.clone(), b.clone(), c.clone(), d.clone(), e.clone()],
                &[
                    (0x4001, S::TitleAbstract),
                    (0x4002, S::FullText),
                    (0x4003, S::FullText),
                ],
            ),
            subject(
                "ft_none",
                S::FullText,
                3,
                None,
                None,
                vec![c.clone(), d.clone()],
                &[],
            ),
            subject(
                "ft_long_terms",
                S::FullText,
                3,
                Some(&long_terms),
                Some("Short abstract with Mixed CASE words and 12345 digits ab".to_owned()),
                vec![c, a],
                &[(0x4002, S::FullText)],
            ),
        ]
    }

    /// `(protocol_hash, source_content_hash)` recorded from the adapter before
    /// the subject mapping moved into this module. The mapping is unchanged iff
    /// these digests are.
    const PRE_MOVE_DIGESTS: [(&str, &str, &str); 6] = [
        (
            "ta_full",
            "5b30b2c1bbf3882ff8f1ed782ac9a6186ad9559ec33d19fd06ad230a26c35571",
            "6fac5707334235c21791b33e40b2d49362d44249ae8998d374cb2841e8499f3f",
        ),
        (
            "ta_missing_abstract",
            "9be1791458766e811ede3fad1839bc6f102ed5045710cf9d0e5c89daf4c6beca",
            "af43ea8e0e8e0875ea421722e3829286804074e6f5cdd24da60ea89a4f586652",
        ),
        (
            "ta_unicode_long",
            "9be1791458766e811ede3fad1839bc6f102ed5045710cf9d0e5c89daf4c6beca",
            "0a58140b60ac9d995c6bb5ec8b58b11b949807168fcc2c0c497a34e82fcc5765",
        ),
        (
            "ft_full",
            "ca4cba7d46012ce6600323df36f4a1b10f14b9f1a25ecd71a521b55562cf3e55",
            "30da9b7c7c52cbaec59612763cfb617e6673e3501d622d097d3ab7a7a746004d",
        ),
        (
            "ft_none",
            "05b811a67940bf0ecca2f6b6b76a85e8f15a68d68626d6357620641576536cba",
            "fa2167f1e143f52bc92fd5bc642faac1b98bc1d46234f863fc6790a9f319cd42",
        ),
        (
            "ft_long_terms",
            "6b407a91a095d882dbf8745085c22f07dc5b5ec0fafaa4167cb68af9e51be52b",
            "e3b9628c1ef4334822a657f078e60435f1e3cedcd978e9a17bd6e373fec8850e",
        ),
    ];

    #[test]
    fn prepared_tasks_match_the_pre_move_adapter() {
        for (name, source) in samples() {
            let task = prepare_screening_task(source).expect("subject prepares");
            let (_, protocol, content) = PRE_MOVE_DIGESTS
                .iter()
                .find(|(pinned, _, _)| *pinned == name)
                .expect("every sample is pinned");
            assert_eq!(
                task.protocol_hash().expect("hash").as_str(),
                *protocol,
                "{name}"
            );
            assert_eq!(
                task.source_content_hash().expect("hash").as_str(),
                *content,
                "{name}"
            );
        }
    }

    #[test]
    fn a_stage_only_cites_the_exclusion_reasons_defined_for_it() {
        let (_, full_text) = samples()
            .into_iter()
            .find(|(name, _)| *name == "ft_full")
            .expect("sample exists");
        let PreparedReviewTask::Screening {
            allowed_exclusion_reasons,
            ..
        } = prepare_screening_task(full_text).expect("subject prepares")
        else {
            panic!("screening subject");
        };
        assert_eq!(
            allowed_exclusion_reasons,
            BTreeSet::from([Uuid::from_u128(0x4002), Uuid::from_u128(0x4003)])
        );
    }

    #[test]
    fn a_negative_revision_is_refused() {
        let (_, mut source) = samples().remove(0);
        source.expected_revision = -1;
        assert_eq!(
            prepare_screening_task(source),
            Err(ReviewError::InvalidExpectedRevision)
        );
    }
}
