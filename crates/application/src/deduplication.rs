use deepref_domain::{ProjectId, RecordId, ReportId, normalize_bibliography_title};
use rapidfuzz::fuzz::ratio;
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Fuzzy matches above this score become human proposals. They are never
/// linked automatically; exact durable identifiers are the only automatic
/// resolution path.
pub const FUZZY_PROPOSAL_THRESHOLD: f64 = 0.82;
pub const FUZZY_SHORTLIST_LIMIT: i64 = 50;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DedupeCandidate {
    pub report_id: ReportId,
    pub title: Option<String>,
    pub first_author: Option<String>,
    pub publication_year: Option<i32>,
    pub exact_identifier_match: bool,
    pub conflicting_identifier: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DedupeScore {
    pub title_similarity: f64,
    pub year_match: Option<bool>,
    pub first_author_similarity: Option<f64>,
    pub exact_identifier_match: bool,
    pub conflicting_identifier: bool,
    pub total: f64,
}

impl DedupeScore {
    pub fn is_credible_fuzzy_match(&self) -> bool {
        !self.exact_identifier_match
            && !self.conflicting_identifier
            && self.total >= FUZZY_PROPOSAL_THRESHOLD
    }
}

/// Score only after the database has produced a bounded pg_trgm shortlist.
/// Weights are renormalized over fields that are present so missing metadata
/// never silently penalizes a record.
pub fn score_candidate(
    source_title: Option<&str>,
    source_first_author: Option<&str>,
    source_year: Option<i32>,
    candidate: &DedupeCandidate,
) -> DedupeScore {
    let title_similarity = match (source_title, candidate.title.as_deref()) {
        (Some(source), Some(target)) => ratio(
            normalize_bibliography_title(source).chars(),
            normalize_bibliography_title(target).chars(),
        ),
        _ => 0.0,
    };
    let year_match = source_year
        .zip(candidate.publication_year)
        .map(|(left, right)| left == right);
    let first_author_similarity = source_first_author
        .zip(candidate.first_author.as_deref())
        .map(|(left, right)| ratio(left.to_lowercase().chars(), right.to_lowercase().chars()));

    let mut weighted_total = 0.0;
    let mut total_weight = 0.0;
    weighted_total += title_similarity * 0.70;
    total_weight += 0.70;
    if let Some(year_match) = year_match {
        weighted_total += (if year_match { 1.0 } else { 0.0 }) * 0.15;
        total_weight += 0.15;
    }
    if let Some(first_author_similarity) = first_author_similarity {
        weighted_total += first_author_similarity * 0.15;
        total_weight += 0.15;
    }

    let total = if candidate.conflicting_identifier {
        0.0
    } else if candidate.exact_identifier_match {
        1.0
    } else if total_weight > 0.0 {
        weighted_total / total_weight
    } else {
        0.0
    };

    DedupeScore {
        title_similarity,
        year_match,
        first_author_similarity,
        exact_identifier_match: candidate.exact_identifier_match,
        conflicting_identifier: candidate.conflicting_identifier,
        total,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DedupeDisposition {
    AutoLink,
    Proposal,
    CreateReport,
}

pub fn disposition(score: &DedupeScore, _has_candidate: bool) -> DedupeDisposition {
    if score.exact_identifier_match && !score.conflicting_identifier {
        DedupeDisposition::AutoLink
    } else if score.conflicting_identifier || score.is_credible_fuzzy_match() {
        DedupeDisposition::Proposal
    } else {
        DedupeDisposition::CreateReport
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProposalKind {
    Fuzzy,
    Conflict,
}

impl ProposalKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Fuzzy => "fuzzy",
            Self::Conflict => "conflict",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProposalDecision {
    Accept,
    Reject,
    CreateNew,
}

impl ProposalDecision {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Accept => "accept",
            Self::Reject => "reject",
            Self::CreateNew => "create_new",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecordResolutionAction {
    Create,
    Link,
    Reassign,
    Revert,
}

impl RecordResolutionAction {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Create => "create",
            Self::Link => "link",
            Self::Reassign => "reassign",
            Self::Revert => "revert",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DedupeProposalCommand {
    pub project_id: ProjectId,
    pub record_id: RecordId,
    pub candidate_report_id: Option<ReportId>,
    pub kind: ProposalKind,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecideProposalCommand {
    pub project_id: ProjectId,
    pub proposal_id: uuid::Uuid,
    pub decision: ProposalDecision,
    pub reason: String,
    pub actor_kind: String,
    pub actor_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolveRecordCommand {
    pub project_id: ProjectId,
    pub record_id: RecordId,
    pub action: RecordResolutionAction,
    pub report_id: Option<ReportId>,
    pub proposal_id: Option<uuid::Uuid>,
    pub reason: String,
    pub actor_kind: String,
    pub actor_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ResolutionCommandError {
    #[error("resolution reason must not be empty")]
    EmptyReason,
    #[error("{0:?} requires a target report")]
    MissingTarget(RecordResolutionAction),
    #[error("create and revert resolutions cannot carry a target report")]
    UnexpectedTarget,
}

impl ResolveRecordCommand {
    pub fn validate(&self) -> Result<(), ResolutionCommandError> {
        if self.reason.trim().is_empty() {
            return Err(ResolutionCommandError::EmptyReason);
        }
        match (self.action, self.report_id) {
            (RecordResolutionAction::Link | RecordResolutionAction::Reassign, None) => {
                Err(ResolutionCommandError::MissingTarget(self.action))
            }
            (RecordResolutionAction::Create | RecordResolutionAction::Revert, Some(_)) => {
                Err(ResolutionCommandError::UnexpectedTarget)
            }
            _ => Ok(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    fn candidate() -> DedupeCandidate {
        DedupeCandidate {
            report_id: Uuid::new_v4().into(),
            title: Some("A study of β-catenin in cancer".to_owned()),
            first_author: Some("Müller".to_owned()),
            publication_year: Some(2024),
            exact_identifier_match: false,
            conflicting_identifier: false,
        }
    }

    #[test]
    fn fuzzy_score_is_explainable_and_credible() {
        let score = score_candidate(
            Some("A study of β-catenin in cancer"),
            Some("Muller"),
            Some(2024),
            &candidate(),
        );
        assert!(score.title_similarity > 0.8);
        assert_eq!(score.year_match, Some(true));
        assert!(score.first_author_similarity.is_some());
        assert!(score.is_credible_fuzzy_match());
    }

    #[test]
    fn exact_identifier_is_the_only_auto_link_signal() {
        let mut exact = candidate();
        exact.exact_identifier_match = true;
        let score = score_candidate(None, None, None, &exact);
        assert_eq!(disposition(&score, true), DedupeDisposition::AutoLink);

        let mut conflicting = exact;
        conflicting.conflicting_identifier = true;
        let conflict_score = score_candidate(None, None, None, &conflicting);
        assert_eq!(
            disposition(&conflict_score, true),
            DedupeDisposition::Proposal
        );
    }

    #[test]
    fn resolution_commands_enforce_target_invariants() {
        let command = ResolveRecordCommand {
            project_id: Uuid::new_v4().into(),
            record_id: Uuid::new_v4().into(),
            action: RecordResolutionAction::Link,
            report_id: None,
            proposal_id: None,
            reason: "reviewed".to_owned(),
            actor_kind: "user".to_owned(),
            actor_id: "test".to_owned(),
        };
        assert_eq!(
            command.validate(),
            Err(ResolutionCommandError::MissingTarget(
                RecordResolutionAction::Link
            ))
        );
    }
}
