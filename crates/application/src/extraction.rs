use chrono::NaiveDate;
use deepref_domain::ProjectId;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExtractionFieldType {
    Text,
    Number,
    Boolean,
    Date,
}

impl ExtractionFieldType {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Text => "text",
            Self::Number => "number",
            Self::Boolean => "boolean",
            Self::Date => "date",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "text" => Some(Self::Text),
            "number" => Some(Self::Number),
            "boolean" => Some(Self::Boolean),
            "date" => Some(Self::Date),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExtractionFieldDefinition {
    pub id: Uuid,
    pub project_id: ProjectId,
    pub version: u32,
    pub field_key: String,
    pub label: String,
    pub value_type: ExtractionFieldType,
    pub required: bool,
}

impl ExtractionFieldDefinition {
    pub fn validate(&self) -> Result<(), ExtractionValidationError> {
        if self.version == 0 {
            return Err(ExtractionValidationError::InvalidDefinition(
                "field definition version must be positive".to_owned(),
            ));
        }
        if self.field_key.trim().is_empty() || self.field_key.chars().count() > 100 {
            return Err(ExtractionValidationError::InvalidDefinition(
                "field key must contain 1 through 100 characters".to_owned(),
            ));
        }
        if self.label.trim().is_empty() || self.label.chars().count() > 200 {
            return Err(ExtractionValidationError::InvalidDefinition(
                "field label must contain 1 through 200 characters".to_owned(),
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ExtractionValue {
    Text { value: String },
    Number { value: f64 },
    Boolean { value: bool },
    Date { value: NaiveDate },
}

impl ExtractionValue {
    pub fn field_type(&self) -> ExtractionFieldType {
        match self {
            Self::Text { .. } => ExtractionFieldType::Text,
            Self::Number { .. } => ExtractionFieldType::Number,
            Self::Boolean { .. } => ExtractionFieldType::Boolean,
            Self::Date { .. } => ExtractionFieldType::Date,
        }
    }

    pub fn validate_for(
        &self,
        definition: &ExtractionFieldDefinition,
    ) -> Result<(), ExtractionValidationError> {
        definition.validate()?;
        if self.field_type() != definition.value_type {
            return Err(ExtractionValidationError::TypeMismatch {
                field: definition.field_key.clone(),
            });
        }
        if let Self::Text { value } = self
            && value.trim().is_empty()
        {
            return Err(ExtractionValidationError::InvalidValue {
                field: definition.field_key.clone(),
                message: "text value must not be blank".to_owned(),
            });
        }
        if let Self::Number { value } = self
            && !value.is_finite()
        {
            return Err(ExtractionValidationError::InvalidValue {
                field: definition.field_key.clone(),
                message: "number value must be finite".to_owned(),
            });
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ExtractionValidationError {
    #[error("extraction field definition is invalid: {0}")]
    InvalidDefinition(String),
    #[error("extraction value type does not match field {field}")]
    TypeMismatch { field: String },
    #[error("extraction value for {field} is invalid: {message}")]
    InvalidValue { field: String, message: String },
}
