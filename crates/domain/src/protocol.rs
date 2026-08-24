use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use thiserror::Error;
use uuid::Uuid;

const MAX_FRAMEWORK_FIELDS: usize = 16;
const MAX_FIELD_NAME_LENGTH: usize = 64;
const MAX_FIELD_VALUE_LENGTH: usize = 2_000;
const MAX_CRITERIA: usize = 64;
const MAX_CRITERION_LABEL_LENGTH: usize = 200;
const MAX_CRITERION_DESCRIPTION_LENGTH: usize = 4_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FrameworkKind {
    Pico,
    Picos,
    Peco,
    Peo,
    Pcc,
    Spider,
    Custom,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProtocolStatus {
    Draft,
    Published,
    Superseded,
}

impl FrameworkKind {
    pub const fn allowed_fields(self) -> &'static [&'static str] {
        match self {
            Self::Pico => &["population", "intervention", "comparator", "outcome"],
            Self::Picos => &[
                "population",
                "intervention",
                "comparator",
                "outcome",
                "study_design",
            ],
            Self::Peco => &["population", "exposure", "comparator", "outcome"],
            Self::Peo => &["population", "exposure", "outcome"],
            Self::Pcc => &["population", "concept", "context"],
            Self::Spider => &[
                "sample",
                "phenomenon",
                "design",
                "evaluation",
                "research_type",
            ],
            Self::Custom => &[],
        }
    }

    pub const fn required_fields(self) -> &'static [&'static str] {
        match self {
            Self::Pico | Self::Picos => &["population", "intervention", "outcome"],
            Self::Peco => &["population", "exposure", "outcome"],
            Self::Peo => &["population", "exposure", "outcome"],
            Self::Pcc => &["population", "concept", "context"],
            Self::Spider => &[
                "sample",
                "phenomenon",
                "design",
                "evaluation",
                "research_type",
            ],
            Self::Custom => &[],
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProtocolFramework {
    pub kind: FrameworkKind,
    pub fields: BTreeMap<String, String>,
}

impl ProtocolFramework {
    pub fn new(
        kind: FrameworkKind,
        fields: BTreeMap<String, String>,
    ) -> Result<Self, ProtocolValidationError> {
        validate_framework_fields(kind, &fields)?;
        Ok(Self { kind, fields })
    }
}

fn validate_framework_fields(
    kind: FrameworkKind,
    fields: &BTreeMap<String, String>,
) -> Result<(), ProtocolValidationError> {
    if fields.len() > MAX_FRAMEWORK_FIELDS {
        return Err(ProtocolValidationError::TooManyFrameworkFields);
    }
    let allowed = kind.allowed_fields();
    let required = kind.required_fields();
    for (name, value) in fields {
        if name.trim().is_empty() {
            return Err(ProtocolValidationError::BlankFrameworkFieldName);
        }
        if name.len() > MAX_FIELD_NAME_LENGTH {
            return Err(ProtocolValidationError::FrameworkFieldNameTooLong);
        }
        if value.trim().is_empty() {
            return Err(ProtocolValidationError::BlankFrameworkFieldValue {
                field: name.clone(),
            });
        }
        if value.len() > MAX_FIELD_VALUE_LENGTH {
            return Err(ProtocolValidationError::FrameworkFieldValueTooLong {
                field: name.clone(),
            });
        }
        if kind != FrameworkKind::Custom && !allowed.contains(&name.as_str()) {
            return Err(ProtocolValidationError::UnknownFrameworkField {
                kind,
                field: name.clone(),
            });
        }
    }
    for field in required {
        if !fields.contains_key(*field) {
            return Err(ProtocolValidationError::MissingFrameworkField {
                kind,
                field: (*field).to_owned(),
            });
        }
    }
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CriterionKind {
    Inclusion,
    Exclusion,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CriterionStage {
    TitleAbstract,
    FullText,
    Both,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CriterionDimension {
    Population,
    Intervention,
    Comparator,
    Outcome,
    Design,
    Setting,
    Language,
    Date,
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EligibilityCriterion {
    pub id: Uuid,
    pub kind: CriterionKind,
    pub stage: CriterionStage,
    pub dimension: CriterionDimension,
    pub label: String,
    pub description: String,
    pub ordinal: i32,
}

impl EligibilityCriterion {
    pub fn new(
        id: Uuid,
        kind: CriterionKind,
        stage: CriterionStage,
        dimension: CriterionDimension,
        label: String,
        description: String,
        ordinal: i32,
    ) -> Result<Self, ProtocolValidationError> {
        if label.trim().is_empty() {
            return Err(ProtocolValidationError::BlankCriterionLabel);
        }
        if label.len() > MAX_CRITERION_LABEL_LENGTH {
            return Err(ProtocolValidationError::CriterionLabelTooLong);
        }
        if description.trim().is_empty() {
            return Err(ProtocolValidationError::BlankCriterionDescription);
        }
        if description.len() > MAX_CRITERION_DESCRIPTION_LENGTH {
            return Err(ProtocolValidationError::CriterionDescriptionTooLong);
        }
        Ok(Self {
            id,
            kind,
            stage,
            dimension,
            label,
            description,
            ordinal,
        })
    }
}

pub fn validate_criteria(
    criteria: &[EligibilityCriterion],
) -> Result<Vec<EligibilityCriterion>, ProtocolValidationError> {
    if criteria.len() > MAX_CRITERIA {
        return Err(ProtocolValidationError::TooManyCriteria);
    }
    let mut ids = BTreeSet::new();
    criteria
        .iter()
        .enumerate()
        .map(|(ordinal, criterion)| {
            if !ids.insert(criterion.id) {
                return Err(ProtocolValidationError::DuplicateCriterionId);
            }
            let mut normalized = EligibilityCriterion::new(
                criterion.id,
                criterion.kind,
                criterion.stage,
                criterion.dimension,
                criterion.label.clone(),
                criterion.description.clone(),
                i32::try_from(ordinal).map_err(|_| ProtocolValidationError::TooManyCriteria)?,
            )?;
            normalized.ordinal =
                i32::try_from(ordinal).map_err(|_| ProtocolValidationError::TooManyCriteria)?;
            Ok(normalized)
        })
        .collect()
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ProtocolValidationError {
    #[error("protocol framework has too many fields")]
    TooManyFrameworkFields,
    #[error("protocol framework field name must not be blank")]
    BlankFrameworkFieldName,
    #[error("protocol framework field name is too long")]
    FrameworkFieldNameTooLong,
    #[error("protocol framework field value for {field} must not be blank")]
    BlankFrameworkFieldValue { field: String },
    #[error("protocol framework field value for {field} is too long")]
    FrameworkFieldValueTooLong { field: String },
    #[error("framework {kind:?} does not allow field {field}")]
    UnknownFrameworkField { kind: FrameworkKind, field: String },
    #[error("framework {kind:?} requires field {field}")]
    MissingFrameworkField { kind: FrameworkKind, field: String },
    #[error("too many eligibility criteria")]
    TooManyCriteria,
    #[error("eligibility criterion ids must be unique")]
    DuplicateCriterionId,
    #[error("eligibility criterion label must not be blank")]
    BlankCriterionLabel,
    #[error("eligibility criterion label is too long")]
    CriterionLabelTooLong,
    #[error("eligibility criterion description must not be blank")]
    BlankCriterionDescription,
    #[error("eligibility criterion description is too long")]
    CriterionDescriptionTooLong,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fields(values: &[(&str, &str)]) -> BTreeMap<String, String> {
        values
            .iter()
            .map(|(name, value)| ((*name).to_owned(), (*value).to_owned()))
            .collect()
    }

    #[test]
    fn known_frameworks_reject_unknown_and_missing_fields() {
        assert!(
            ProtocolFramework::new(
                FrameworkKind::Pico,
                fields(&[
                    ("population", "people"),
                    ("intervention", "test"),
                    ("outcome", "health")
                ])
            )
            .is_ok()
        );
        assert!(matches!(
            ProtocolFramework::new(FrameworkKind::Pico, fields(&[("population", "people")])),
            Err(ProtocolValidationError::MissingFrameworkField { .. })
        ));
        assert!(matches!(
            ProtocolFramework::new(
                FrameworkKind::Pico,
                fields(&[
                    ("population", "people"),
                    ("intervention", "test"),
                    ("outcome", "health"),
                    ("unexpected", "value")
                ])
            ),
            Err(ProtocolValidationError::UnknownFrameworkField { .. })
        ));
    }

    #[test]
    fn criteria_are_reordered_deterministically() {
        let result = validate_criteria(&[
            EligibilityCriterion::new(
                Uuid::new_v4(),
                CriterionKind::Inclusion,
                CriterionStage::Both,
                CriterionDimension::Population,
                "Population".to_owned(),
                "People in scope".to_owned(),
                42,
            )
            .unwrap(),
            EligibilityCriterion::new(
                Uuid::new_v4(),
                CriterionKind::Exclusion,
                CriterionStage::FullText,
                CriterionDimension::Date,
                "Date".to_owned(),
                "Date in scope".to_owned(),
                -1,
            )
            .unwrap(),
        ])
        .unwrap();
        assert_eq!(result[0].ordinal, 0);
        assert_eq!(result[1].ordinal, 1);
    }
}
