use serde::{Deserialize, Serialize};

/// A methodology assignment within one project. Account roles grant product
/// access; these roles describe a person's work in the research process.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScientificAssignmentRole {
    PrimaryScreener,
    SecondaryScreener,
    ConsensusReviewer,
    Extractor,
    AppraisalReviewer,
    FreshAiAuditor,
}

impl ScientificAssignmentRole {
    pub const ALL: [Self; 6] = [
        Self::PrimaryScreener,
        Self::SecondaryScreener,
        Self::ConsensusReviewer,
        Self::Extractor,
        Self::AppraisalReviewer,
        Self::FreshAiAuditor,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::PrimaryScreener => "primary_screener",
            Self::SecondaryScreener => "secondary_screener",
            Self::ConsensusReviewer => "consensus_reviewer",
            Self::Extractor => "extractor",
            Self::AppraisalReviewer => "appraisal_reviewer",
            Self::FreshAiAuditor => "fresh_ai_auditor",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|role| role.as_str() == value)
    }
}
