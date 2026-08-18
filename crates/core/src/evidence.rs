use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

macro_rules! typed_id {
    ($name:ident) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(pub Uuid);

        impl $name {
            #[must_use]
            pub fn new() -> Self {
                Self(Uuid::new_v4())
            }
        }

        impl Default for $name {
            fn default() -> Self {
                Self::new()
            }
        }

        impl From<Uuid> for $name {
            fn from(value: Uuid) -> Self {
                Self(value)
            }
        }

        impl From<$name> for Uuid {
            fn from(value: $name) -> Self {
                value.0
            }
        }
    };
}

typed_id!(RecordId);
typed_id!(ReportId);
typed_id!(StudyId);
typed_id!(AcquisitionRunId);
typed_id!(ProtocolVersionId);
typed_id!(EligibilityCriterionId);
typed_id!(ExclusionReasonId);
typed_id!(ScreeningEventId);
typed_id!(DocumentId);
typed_id!(DocumentBlockId);
typed_id!(AiRunId);
typed_id!(AiProposalId);
typed_id!(AutomationRunId);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IdentifierScheme {
    Doi,
    Pmid,
    Pmcid,
    Arxiv,
    Isbn,
    ClinicalTrialRegistry,
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReportIdentifier {
    pub scheme: IdentifierScheme,
    pub value: String,
    pub normalized_value: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Record {
    pub id: RecordId,
    pub acquisition_run_id: AcquisitionRunId,
    pub source: String,
    pub source_record_id: Option<String>,
    pub raw: serde_json::Value,
    pub resolved_report_id: Option<ReportId>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Report {
    pub id: ReportId,
    pub title: Option<String>,
    pub abstract_text: Option<String>,
    pub publication_year: Option<i32>,
    pub journal: Option<String>,
    pub identifiers: Vec<ReportIdentifier>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Study {
    pub id: StudyId,
    pub project_id: Uuid,
    pub name: String,
    pub description: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn typed_ids_do_not_share_identity_even_when_created_together() {
        let report = ReportId::new();
        let study = StudyId::new();
        assert_ne!(Uuid::from(report), Uuid::from(study));
    }

    #[test]
    fn report_can_exist_without_a_doi() {
        let now = Utc::now();
        let report = Report {
            id: ReportId::new(),
            title: Some("Report without durable identifier".to_owned()),
            abstract_text: None,
            publication_year: None,
            journal: None,
            identifiers: Vec::new(),
            created_at: now,
            updated_at: now,
        };

        assert!(report.identifiers.is_empty());
    }
}
