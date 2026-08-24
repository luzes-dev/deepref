use std::collections::{BTreeMap, BTreeSet};

use deepref_domain::{Actor, ProjectId, ReportId, StudyDesign};
use schemars::{JsonSchema, schema_for};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use thiserror::Error;
use uuid::Uuid;

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(transparent)]
pub struct DefinitionId(String);

impl DefinitionId {
    pub fn new(value: impl Into<String>) -> Result<Self, AppraisalDefinitionError> {
        let value = value.into();
        if value.trim().is_empty() || value.len() > 100 {
            return Err(AppraisalDefinitionError::InvalidIdentity);
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(transparent)]
pub struct DefinitionVersion(u32);

impl DefinitionVersion {
    pub const fn new(value: u32) -> Option<Self> {
        if value == 0 { None } else { Some(Self(value)) }
    }

    pub const fn get(self) -> u32 {
        self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AnswerOption {
    pub value: String,
    pub label: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AnswerSchema {
    Enum {
        options: Vec<AnswerOption>,
    },
    Boolean,
    Scale {
        min: i64,
        max: i64,
        labels: BTreeMap<String, String>,
    },
    Text {
        max_length: u32,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct JudgmentSchema {
    pub options: Vec<AnswerOption>,
    pub allow_custom: bool,
    pub required: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AppraisalQuestion {
    pub id: String,
    pub label: String,
    pub help: Option<String>,
    pub answer_schema: AnswerSchema,
    pub required: bool,
    pub requires_evidence: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AppraisalDomain {
    pub id: String,
    pub label: String,
    pub description: Option<String>,
    pub questions: Vec<AppraisalQuestion>,
    pub judgment: JudgmentSchema,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AppraisalApplicability {
    #[schemars(with = "Vec<String>")]
    pub designs: Vec<StudyDesign>,
    pub note: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AppraisalDefinition {
    pub id: DefinitionId,
    pub version: DefinitionVersion,
    pub name: String,
    pub description: String,
    pub applicability: AppraisalApplicability,
    pub domains: Vec<AppraisalDomain>,
    pub overall_judgment: JudgmentSchema,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct EvidenceReferenceInput {
    pub question_id: String,
    pub document_id: Uuid,
    pub block_id: Uuid,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AppraisalAssessmentInput {
    pub definition_id: DefinitionId,
    pub definition_version: DefinitionVersion,
    pub responses: Value,
    pub evidence: Vec<EvidenceReferenceInput>,
    pub domain_judgments: BTreeMap<String, String>,
    pub overall_judgment: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AppraisalCompleted {
    pub assessment_id: Uuid,
    pub project_id: ProjectId,
    pub report_id: ReportId,
    pub definition_id: DefinitionId,
    pub definition_version: DefinitionVersion,
    pub actor: Actor,
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum AppraisalDefinitionError {
    #[error("definition identity is invalid")]
    InvalidIdentity,
    #[error("definition version must be greater than zero")]
    InvalidVersion,
    #[error("definition must have a name, description, and at least one domain")]
    MissingMetadata,
    #[error("definition contains a duplicate domain or question id: {0}")]
    DuplicateId(String),
    #[error("definition contains an invalid answer schema: {0}")]
    InvalidAnswerSchema(String),
    #[error("definition JSON schema is invalid: {0}")]
    InvalidJsonSchema(String),
    #[error("definition resource could not be parsed: {0}")]
    InvalidResource(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum AppraisalValidationError {
    #[error("assessment definition does not match the selected definition")]
    DefinitionMismatch,
    #[error("assessment responses are invalid: {0}")]
    InvalidResponses(String),
    #[error("assessment has an unknown question id: {0}")]
    UnknownQuestion(String),
    #[error("assessment has duplicate evidence for question: {0}")]
    DuplicateEvidence(String),
    #[error("question requires at least one evidence block: {0}")]
    MissingEvidence(String),
    #[error("assessment has an unknown domain id: {0}")]
    UnknownDomain(String),
    #[error("a required domain judgment is missing: {0}")]
    MissingDomainJudgment(String),
    #[error("domain judgment is not allowed: {0}")]
    InvalidDomainJudgment(String),
    #[error("overall judgment is required")]
    MissingOverallJudgment,
    #[error("overall judgment is not allowed: {0}")]
    InvalidOverallJudgment(String),
}

pub fn validate_definition_resource(
    definition: &AppraisalDefinition,
) -> Result<(), AppraisalDefinitionError> {
    validate_definition_semantics(definition)
}

pub fn parse_appraisal_definition_resource(
    raw: &Value,
) -> Result<AppraisalDefinition, AppraisalDefinitionError> {
    let schema = serde_json::to_value(schema_for!(AppraisalDefinition))
        .map_err(|error| AppraisalDefinitionError::InvalidJsonSchema(error.to_string()))?;
    let validator = jsonschema::validator_for(&schema)
        .map_err(|error| AppraisalDefinitionError::InvalidJsonSchema(error.to_string()))?;
    validator
        .validate(raw)
        .map_err(|error| AppraisalDefinitionError::InvalidJsonSchema(error.to_string()))?;
    let definition = serde_json::from_value(raw.clone())
        .map_err(|error| AppraisalDefinitionError::InvalidResource(error.to_string()))?;
    validate_definition_semantics(&definition)?;
    Ok(definition)
}

fn validate_definition_semantics(
    definition: &AppraisalDefinition,
) -> Result<(), AppraisalDefinitionError> {
    if definition.id.as_str().trim().is_empty() || definition.id.as_str().len() > 100 {
        return Err(AppraisalDefinitionError::InvalidIdentity);
    }
    if definition.name.trim().is_empty()
        || definition.description.trim().is_empty()
        || definition.domains.is_empty()
        || definition.applicability.designs.is_empty()
    {
        return Err(AppraisalDefinitionError::MissingMetadata);
    }
    if definition.version.get() == 0 {
        return Err(AppraisalDefinitionError::InvalidVersion);
    }

    let mut ids = BTreeSet::new();
    for domain in &definition.domains {
        if !ids.insert(domain.id.clone()) {
            return Err(AppraisalDefinitionError::DuplicateId(domain.id.clone()));
        }
        if domain.id.trim().is_empty()
            || domain.label.trim().is_empty()
            || domain.questions.is_empty()
        {
            return Err(AppraisalDefinitionError::MissingMetadata);
        }
        validate_judgment(&domain.judgment)?;
        for question in &domain.questions {
            if !ids.insert(question.id.clone()) {
                return Err(AppraisalDefinitionError::DuplicateId(question.id.clone()));
            }
            if question.id.trim().is_empty() || question.label.trim().is_empty() {
                return Err(AppraisalDefinitionError::MissingMetadata);
            }
            validate_answer_schema(&question.answer_schema)?;
        }
    }
    validate_judgment(&definition.overall_judgment)?;
    Ok(())
}

fn validate_judgment(judgment: &JudgmentSchema) -> Result<(), AppraisalDefinitionError> {
    if judgment.options.is_empty() {
        return Err(AppraisalDefinitionError::InvalidAnswerSchema(
            "judgment options must not be empty".to_owned(),
        ));
    }
    let mut values = BTreeSet::new();
    for option in &judgment.options {
        if option.value.trim().is_empty()
            || option.label.trim().is_empty()
            || !values.insert(option.value.clone())
        {
            return Err(AppraisalDefinitionError::InvalidAnswerSchema(
                "judgment options must have unique non-empty values".to_owned(),
            ));
        }
    }
    Ok(())
}

fn validate_answer_schema(schema: &AnswerSchema) -> Result<(), AppraisalDefinitionError> {
    match schema {
        AnswerSchema::Enum { options } => {
            if options.is_empty() {
                return Err(AppraisalDefinitionError::InvalidAnswerSchema(
                    "enum options must not be empty".to_owned(),
                ));
            }
            let mut values = BTreeSet::new();
            for option in options {
                if option.value.trim().is_empty()
                    || option.label.trim().is_empty()
                    || !values.insert(option.value.clone())
                {
                    return Err(AppraisalDefinitionError::InvalidAnswerSchema(
                        "enum options must have unique non-empty values".to_owned(),
                    ));
                }
            }
        }
        AnswerSchema::Boolean => {}
        AnswerSchema::Scale { min, max, labels } => {
            if min > max
                || labels.keys().any(|key| {
                    key.parse::<i64>()
                        .map_or(true, |key| key < *min || key > *max)
                })
            {
                return Err(AppraisalDefinitionError::InvalidAnswerSchema(
                    "scale bounds or labels are invalid".to_owned(),
                ));
            }
        }
        AnswerSchema::Text { max_length } if *max_length == 0 => {
            return Err(AppraisalDefinitionError::InvalidAnswerSchema(
                "text max_length must be positive".to_owned(),
            ));
        }
        AnswerSchema::Text { .. } => {}
    }
    Ok(())
}

pub fn validate_assessment_input(
    definition: &AppraisalDefinition,
    input: &AppraisalAssessmentInput,
) -> Result<(), AppraisalValidationError> {
    if input.definition_id != definition.id || input.definition_version != definition.version {
        return Err(AppraisalValidationError::DefinitionMismatch);
    }
    let response_schema = responses_schema(definition);
    let validator = jsonschema::validator_for(&response_schema)
        .map_err(|error| AppraisalValidationError::InvalidResponses(error.to_string()))?;
    validator
        .validate(&input.responses)
        .map_err(|error| AppraisalValidationError::InvalidResponses(error.to_string()))?;

    let questions = definition
        .domains
        .iter()
        .flat_map(|domain| domain.questions.iter())
        .map(|question| (question.id.as_str(), question))
        .collect::<BTreeMap<_, _>>();
    let mut evidence_questions = BTreeSet::new();
    let mut evidence_identities = BTreeSet::new();
    for evidence in &input.evidence {
        let Some(question) = questions.get(evidence.question_id.as_str()) else {
            return Err(AppraisalValidationError::UnknownQuestion(
                evidence.question_id.clone(),
            ));
        };
        evidence_questions.insert(evidence.question_id.clone());
        if !evidence_identities.insert((
            evidence.question_id.clone(),
            evidence.document_id,
            evidence.block_id,
        )) {
            return Err(AppraisalValidationError::DuplicateEvidence(
                evidence.question_id.clone(),
            ));
        }
        if !question.requires_evidence {
            continue;
        }
    }
    for question in questions.values() {
        if question.requires_evidence && !evidence_questions.contains(&question.id) {
            return Err(AppraisalValidationError::MissingEvidence(
                question.id.clone(),
            ));
        }
    }

    for domain in &definition.domains {
        let Some(judgment) = input.domain_judgments.get(&domain.id) else {
            if domain.judgment.required {
                return Err(AppraisalValidationError::MissingDomainJudgment(
                    domain.id.clone(),
                ));
            }
            continue;
        };
        if !judgment_allowed(&domain.judgment, judgment) {
            return Err(AppraisalValidationError::InvalidDomainJudgment(
                domain.id.clone(),
            ));
        }
    }
    for domain_id in input.domain_judgments.keys() {
        if !definition
            .domains
            .iter()
            .any(|domain| domain.id == *domain_id)
        {
            return Err(AppraisalValidationError::UnknownDomain(domain_id.clone()));
        }
    }
    if definition.overall_judgment.required && input.overall_judgment.is_none() {
        return Err(AppraisalValidationError::MissingOverallJudgment);
    }
    if let Some(judgment) = &input.overall_judgment
        && !judgment_allowed(&definition.overall_judgment, judgment)
    {
        return Err(AppraisalValidationError::InvalidOverallJudgment(
            judgment.clone(),
        ));
    }
    Ok(())
}

fn judgment_allowed(schema: &JudgmentSchema, value: &str) -> bool {
    schema.allow_custom && !value.trim().is_empty()
        || schema.options.iter().any(|option| option.value == value)
}

fn responses_schema(definition: &AppraisalDefinition) -> Value {
    let mut properties = Map::new();
    let mut required = Vec::new();
    for question in definition
        .domains
        .iter()
        .flat_map(|domain| domain.questions.iter())
    {
        properties.insert(
            question.id.clone(),
            answer_schema_json(&question.answer_schema),
        );
        if question.required {
            required.push(Value::String(question.id.clone()));
        }
    }
    let mut object = Map::from_iter([
        ("type".to_owned(), Value::String("object".to_owned())),
        ("properties".to_owned(), Value::Object(properties)),
        ("additionalProperties".to_owned(), Value::Bool(false)),
    ]);
    if !required.is_empty() {
        object.insert("required".to_owned(), Value::Array(required));
    }
    Value::Object(object)
}

fn answer_schema_json(schema: &AnswerSchema) -> Value {
    match schema {
        AnswerSchema::Enum { options } => json!({
            "type": "string",
            "enum": options.iter().map(|option| option.value.clone()).collect::<Vec<_>>()
        }),
        AnswerSchema::Boolean => json!({ "type": "boolean" }),
        AnswerSchema::Scale { min, max, .. } => json!({
            "type": "integer",
            "minimum": min,
            "maximum": max
        }),
        AnswerSchema::Text { max_length } => json!({
            "type": "string",
            "maxLength": max_length
        }),
    }
}

const DEFINITION_RESOURCES: [&str; 2] = [
    include_str!("../appraisal-definitions/deepref-rct-generic/v1.json"),
    include_str!("../appraisal-definitions/deepref-qualitative-generic/v1.json"),
];

pub fn all_appraisal_definitions() -> Result<Vec<AppraisalDefinition>, AppraisalDefinitionError> {
    DEFINITION_RESOURCES
        .into_iter()
        .map(|raw| {
            let value: Value = serde_json::from_str(raw)
                .map_err(|error| AppraisalDefinitionError::InvalidResource(error.to_string()))?;
            parse_appraisal_definition_resource(&value)
        })
        .collect()
}

pub fn validate_shipped_appraisal_definitions() -> Result<(), AppraisalDefinitionError> {
    all_appraisal_definitions().map(|_| ())
}

pub fn get_appraisal_definition(
    id: &str,
    version: u32,
) -> Result<AppraisalDefinition, AppraisalDefinitionError> {
    let definition = all_appraisal_definitions()?
        .into_iter()
        .find(|definition| definition.id.as_str() == id && definition.version.get() == version)
        .ok_or(AppraisalDefinitionError::InvalidIdentity)?;
    Ok(definition)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shipped_definitions_validate_and_have_different_answer_shapes() {
        let definitions = all_appraisal_definitions().unwrap();
        assert_eq!(definitions.len(), 2);
        for definition in &definitions {
            validate_definition_resource(definition).unwrap();
        }
        assert!(matches!(
            definitions[0].domains[0].questions[0].answer_schema,
            AnswerSchema::Enum { .. }
        ));
        assert!(matches!(
            definitions[1].domains[0].questions[0].answer_schema,
            AnswerSchema::Scale { .. }
        ));
    }

    #[test]
    fn shipped_definition_startup_validation_is_fallible() -> Result<(), AppraisalDefinitionError> {
        validate_shipped_appraisal_definitions()
    }

    #[test]
    fn oversized_definition_id_is_rejected_after_raw_deserialization() {
        let mut raw: Value = serde_json::from_str(DEFINITION_RESOURCES[0]).unwrap();
        raw["id"] = Value::String("x".repeat(101));

        assert_eq!(
            parse_appraisal_definition_resource(&raw),
            Err(AppraisalDefinitionError::InvalidIdentity)
        );
    }

    #[test]
    fn assessment_requires_known_answers_and_evidence() {
        let definition = get_appraisal_definition("deepref-rct-generic", 1).unwrap();
        let input = AppraisalAssessmentInput {
            definition_id: definition.id.clone(),
            definition_version: definition.version,
            responses: json!({
                "allocation_description": "yes",
                "outcome_measure_prespecified": true
            }),
            evidence: vec![EvidenceReferenceInput {
                question_id: "allocation_description".to_owned(),
                document_id: Uuid::new_v4(),
                block_id: Uuid::new_v4(),
            }],
            domain_judgments: BTreeMap::from([
                ("allocation".to_owned(), "low_concern".to_owned()),
                ("outcome_reporting".to_owned(), "some_concern".to_owned()),
            ]),
            overall_judgment: Some("some_concern".to_owned()),
        };
        validate_assessment_input(&definition, &input).unwrap();
        let missing = AppraisalAssessmentInput {
            evidence: Vec::new(),
            ..input
        };
        assert!(matches!(
            validate_assessment_input(&definition, &missing),
            Err(AppraisalValidationError::MissingEvidence(_))
        ));
    }
}
