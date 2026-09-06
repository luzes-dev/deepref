use std::collections::BTreeSet;

use deepref_domain::{ProjectId, RecordId, ReportId};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{AiContext, AiError, AiTask, AuthorityTier, ModelProfile};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum DuplicateDecision {
    Match,
    NoMatch,
    InsufficientEvidence,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct DuplicateCandidate {
    pub source_record_id: Uuid,
    pub candidate_report_id: Uuid,
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum DuplicateSignalKind {
    TitleSimilarity,
    PublicationYear,
    FirstAuthor,
    DurableIdentifier,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum DuplicateSignal {
    TitleSimilarity {
        similarity: f64,
        supports_match: bool,
    },
    PublicationYear {
        source_year: i32,
        candidate_year: i32,
        supports_match: bool,
    },
    FirstAuthor {
        source_author: String,
        candidate_author: String,
        similarity: f64,
        supports_match: bool,
    },
    DurableIdentifier {
        scheme: String,
        source_value: String,
        candidate_value: String,
        supports_match: bool,
    },
}

impl DuplicateSignal {
    fn kind(&self) -> DuplicateSignalKind {
        match self {
            Self::TitleSimilarity { .. } => DuplicateSignalKind::TitleSimilarity,
            Self::PublicationYear { .. } => DuplicateSignalKind::PublicationYear,
            Self::FirstAuthor { .. } => DuplicateSignalKind::FirstAuthor,
            Self::DurableIdentifier { .. } => DuplicateSignalKind::DurableIdentifier,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct DuplicateRationale {
    pub code: String,
    pub explanation: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct IdentityProvenance {
    pub entity_type: String,
    pub entity_id: String,
    pub field: String,
    pub content_hash: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct DuplicateAssistance {
    pub candidate: DuplicateCandidate,
    pub decision: DuplicateDecision,
    pub rationale: Vec<DuplicateRationale>,
    pub signals: Vec<DuplicateSignal>,
    pub provenance: Vec<IdentityProvenance>,
    #[serde(default)]
    pub uncertainties: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DedupeInput {
    pub project_id: ProjectId,
    pub source_record_id: RecordId,
    pub candidate_report_id: ReportId,
    pub source_title: Option<String>,
    pub candidate_title: Option<String>,
    pub source_year: Option<i32>,
    pub candidate_year: Option<i32>,
    pub source_author: Option<String>,
    pub candidate_author: Option<String>,
    pub source_title_hash: String,
    pub candidate_title_hash: String,
    /// Deterministic application evidence that the model may copy into its
    /// explanation, but must not recalculate or extend.
    pub grounded_signals: Vec<DuplicateSignal>,
    pub grounded_provenance: Vec<IdentityProvenance>,
}

pub struct DedupeTask {
    project_id: ProjectId,
    source_record_id: RecordId,
    candidate_report_id: ReportId,
    allowed_provenance: Vec<IdentityProvenance>,
    allowed_signals: Vec<DuplicateSignal>,
}

impl DedupeTask {
    pub fn new(
        project_id: ProjectId,
        source_record_id: RecordId,
        candidate_report_id: ReportId,
        allowed_provenance: impl IntoIterator<Item = IdentityProvenance>,
        allowed_signals: impl IntoIterator<Item = DuplicateSignal>,
    ) -> Self {
        Self {
            project_id,
            source_record_id,
            candidate_report_id,
            allowed_provenance: allowed_provenance.into_iter().collect(),
            allowed_signals: allowed_signals.into_iter().collect(),
        }
    }

    fn validate_output(&self, output: &DuplicateAssistance) -> Result<(), AiError> {
        let allowed_provenance = self
            .allowed_provenance
            .iter()
            .map(provenance_key)
            .collect::<BTreeSet<_>>();
        if output.candidate.source_record_id != self.source_record_id.as_uuid()
            || output.candidate.candidate_report_id != self.candidate_report_id.as_uuid()
            || output.rationale.is_empty()
            || (output.signals.is_empty()
                && !matches!(output.decision, DuplicateDecision::InsufficientEvidence))
        {
            return Err(AiError::SemanticValidation(
                "duplicate assistance is incomplete or targets a different pair".to_owned(),
            ));
        }
        let mut signal_kinds = BTreeSet::new();
        for signal in &output.signals {
            if !self.allowed_signals.contains(signal) || !signal_kinds.insert(signal.kind()) {
                return Err(AiError::SemanticValidation(
                    "duplicate signal is not grounded in the candidate pair".to_owned(),
                ));
            }
        }
        let mut keys = BTreeSet::new();
        for provenance in &output.provenance {
            let key = provenance_key(provenance);
            if provenance.entity_type.trim().is_empty()
                || provenance.field.trim().is_empty()
                || !crate::is_sha256(&provenance.content_hash)
                || !allowed_provenance.contains(&key)
                || !keys.insert(key)
            {
                return Err(AiError::SemanticValidation(
                    "duplicate provenance is invalid or outside the allowed identity context"
                        .to_owned(),
                ));
            }
        }
        if matches!(
            output.decision,
            DuplicateDecision::Match | DuplicateDecision::NoMatch
        ) {
            if output.signals.len() != self.allowed_signals.len()
                || self
                    .allowed_signals
                    .iter()
                    .any(|signal| !output.signals.contains(signal))
            {
                return Err(AiError::SemanticValidation(
                    "a consequential duplicate decision must cite every available signal"
                        .to_owned(),
                ));
            }
            if self.allowed_provenance.is_empty()
                || output.provenance.len() != self.allowed_provenance.len()
                || self
                    .allowed_provenance
                    .iter()
                    .map(provenance_key)
                    .any(|key| !keys.contains(&key))
                || !output
                    .provenance
                    .iter()
                    .any(|item| item.entity_type == "record")
                || !output
                    .provenance
                    .iter()
                    .any(|item| item.entity_type == "report")
            {
                return Err(AiError::SemanticValidation(
                    "a consequential duplicate decision must cite both candidate sides".to_owned(),
                ));
            }
        } else if !output.provenance.is_empty()
            && output
                .provenance
                .iter()
                .any(|item| !allowed_provenance.contains(&provenance_key(item)))
        {
            return Err(AiError::SemanticValidation(
                "duplicate abstention cites unavailable provenance".to_owned(),
            ));
        }
        if matches!(output.decision, DuplicateDecision::InsufficientEvidence)
            && output.uncertainties.is_empty()
        {
            return Err(AiError::SemanticValidation(
                "duplicate abstention requires an uncertainty".to_owned(),
            ));
        }
        if output
            .rationale
            .iter()
            .any(|item| item.code.trim().is_empty() || item.explanation.trim().is_empty())
        {
            return Err(AiError::SemanticValidation(
                "duplicate rationale is invalid".to_owned(),
            ));
        }
        Ok(())
    }

    fn validate_grounding(&self, input: &DedupeInput) -> Result<(), AiError> {
        let input_provenance = input
            .grounded_provenance
            .iter()
            .map(provenance_key)
            .collect::<BTreeSet<_>>();
        let allowed_provenance = self
            .allowed_provenance
            .iter()
            .map(provenance_key)
            .collect::<BTreeSet<_>>();
        let provenance_matches = input.grounded_provenance.len() == self.allowed_provenance.len()
            && input_provenance.len() == input.grounded_provenance.len()
            && input_provenance == allowed_provenance;
        let signals_match = input.grounded_signals.len() == self.allowed_signals.len()
            && input
                .grounded_signals
                .iter()
                .all(|signal| self.allowed_signals.contains(signal))
            && self
                .allowed_signals
                .iter()
                .all(|signal| input.grounded_signals.contains(signal));
        if !provenance_matches || !signals_match {
            return Err(AiError::InvalidContext(
                "dedupe grounding disagrees with the deterministic candidate evidence".to_owned(),
            ));
        }
        Ok(())
    }
}

fn provenance_key(item: &IdentityProvenance) -> String {
    format!(
        "{}:{}:{}:{}",
        item.entity_type, item.entity_id, item.field, item.content_hash
    )
}

impl AiTask for DedupeTask {
    type Input = DedupeInput;
    type Output = DuplicateAssistance;

    const KIND: crate::AiTaskKind = crate::AiTaskKind::DuplicateCandidateDetection;
    const PROMPT_VERSION: &'static str = "dedupe.assistance.v1";
    const SCHEMA_VERSION: &'static str = "dedupe.assistance.v1";

    fn model_profile(&self) -> ModelProfile {
        ModelProfile::FastClassifier
    }

    fn build_context(&self, input: &Self::Input) -> Result<AiContext, AiError> {
        if input.project_id != self.project_id
            || input.source_record_id != self.source_record_id
            || input.candidate_report_id != self.candidate_report_id
        {
            return Err(AiError::InvalidContext(
                "dedupe task and input identities disagree".to_owned(),
            ));
        }
        if !crate::is_sha256(&input.source_title_hash)
            || !crate::is_sha256(&input.candidate_title_hash)
        {
            return Err(AiError::InvalidContext(
                "dedupe identity hashes are invalid".to_owned(),
            ));
        }
        self.validate_grounding(input)?;
        Ok(AiContext {
            project_id: Some(self.project_id),
            system_prompt: "Return only duplicate assistance JSON. The grounded_signals and grounded_provenance in the input are authoritative deterministic application evidence: copy exact values from them when citing evidence, explain them, and never recalculate, alter, or invent values. A consequential match or no_match must include every grounded signal and provenance for both candidate sides. Exact identifier linking remains a deterministic application command.".to_owned(),
            user_prompt: serde_json::to_string(input)
                .map_err(|_| AiError::InputSerialization("dedupe input".to_owned()))?,
            retrieval: None,
            protocol_hash: None,
            document_hash: None,
        })
    }

    fn semantic_validate(&self, output: &Self::Output) -> Result<(), AiError> {
        self.validate_output(output)
    }

    fn authority(&self) -> AuthorityTier {
        AuthorityTier::WorkflowSuggestion
    }

    fn proposal(&self, output: &Self::Output) -> Option<crate::ProposalDraft> {
        Some(crate::ProposalDraft {
            project_id: self.project_id,
            entity_type: "dedupe_record".to_owned(),
            entity_id: Some(self.source_record_id.into()),
            operation: "dedupe_suggestion".to_owned(),
            payload: serde_json::to_value(output).ok()?,
            authority: self.authority(),
        })
    }
}
