use serde::{Deserialize, Serialize};
use thiserror::Error;
use uuid::Uuid;

use super::study::{StudyDesign, StudyTitle};

macro_rules! typed_id {
    ($name:ident) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(Uuid);

        impl $name {
            pub const fn new(value: Uuid) -> Self {
                Self(value)
            }

            pub const fn as_uuid(self) -> Uuid {
                self.0
            }
        }

        impl From<Uuid> for $name {
            fn from(value: Uuid) -> Self {
                Self::new(value)
            }
        }

        impl From<$name> for Uuid {
            fn from(value: $name) -> Self {
                value.as_uuid()
            }
        }
    };
}

typed_id!(RecordId);
typed_id!(ReportId);
typed_id!(StudyId);

#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("title must not be empty")]
pub struct TitleError;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Title(String);

impl Title {
    pub fn new(value: impl Into<String>) -> Result<Self, TitleError> {
        let value = value.into();
        if value.trim().is_empty() {
            return Err(TitleError);
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Record {
    pub id: RecordId,
    pub report_id: Option<ReportId>,
    pub source: String,
    pub source_key: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Report {
    pub id: ReportId,
    pub title: Option<Title>,
    pub identifiers: Vec<super::ReportIdentifier>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Study {
    pub id: StudyId,
    pub title: StudyTitle,
    pub design: Option<StudyDesign>,
    pub revision: u64,
    pub report_ids: Vec<ReportId>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Citation {
    pub source_report_id: ReportId,
    pub target_report_id: ReportId,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bibliography::{IdentifierScheme, ReportIdentifier};

    #[test]
    fn report_can_have_no_identifiers() {
        let report = Report {
            id: Uuid::new_v4().into(),
            title: Some(Title::new("Identifier-free report").unwrap()),
            identifiers: Vec::new(),
        };
        assert!(report.identifiers.is_empty());
    }

    #[test]
    fn report_can_have_multiple_identifiers() {
        let report = Report {
            id: Uuid::new_v4().into(),
            title: None,
            identifiers: vec![
                ReportIdentifier::new(IdentifierScheme::Doi, "10.1000/example").unwrap(),
                ReportIdentifier::new(IdentifierScheme::Pmid, "12345").unwrap(),
            ],
        };
        assert_eq!(report.identifiers.len(), 2);
    }

    #[test]
    fn one_study_can_have_multiple_reports() {
        let first: ReportId = Uuid::new_v4().into();
        let second: ReportId = Uuid::new_v4().into();
        let study = Study {
            id: Uuid::new_v4().into(),
            title: StudyTitle::new("One investigation").unwrap(),
            design: None,
            revision: 0,
            report_ids: vec![first, second],
        };
        assert_eq!(study.report_ids, vec![first, second]);
    }

    #[test]
    fn title_rejects_empty_values() {
        assert!(Title::new(" \n ").is_err());
    }
}
