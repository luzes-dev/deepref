use std::collections::{BTreeMap, BTreeSet};

use deepref_domain::{Actor, ProjectId, ReportId, StudyDesign};
use schemars::{JsonSchema, schema_for};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use thiserror::Error;
use uuid::Uuid;

mod suggestion;

pub use suggestion::{DomainJudgmentSuggestion, JudgmentSuggestion, suggest_judgments};

/// Longest accepted reason for overriding a suggested judgment.
pub const OVERRIDE_REASON_MAX_CHARS: usize = 1_000;
/// Key used in `override_reasons` for the overall judgment.
pub const OVERALL_JUDGMENT_TARGET: &str = "overall";
/// Answer value for a conditional question whose condition does not hold.
const NOT_APPLICABLE: &str = "not_applicable";

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
    /// Present when the question is asked only for some earlier answers. The
    /// answer is `not_applicable` whenever the condition does not hold.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub applies_when: Option<AppliesWhen>,
}

/// Condition under which a question is asked, built from earlier answers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AppliesWhen {
    /// `all`: every clause must hold. `any`: at least one clause must hold.
    #[serde(rename = "match")]
    pub match_mode: ConditionMatch,
    pub clauses: Vec<ConditionClause>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ConditionMatch {
    All,
    Any,
}

/// Holds when the earlier question `question` was answered with one of `answers`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ConditionClause {
    pub question: String,
    pub answers: Vec<String>,
}

impl AppliesWhen {
    /// Whether the conditional question is asked for these responses.
    pub fn holds(&self, responses: &Value) -> bool {
        let clause_holds = |clause: &ConditionClause| {
            responses
                .get(&clause.question)
                .and_then(Value::as_str)
                .is_some_and(|answer| clause.answers.iter().any(|expected| expected == answer))
        };
        match self.match_mode {
            ConditionMatch::All => self.clauses.iter().all(clause_holds),
            ConditionMatch::Any => self.clauses.iter().any(clause_holds),
        }
    }
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
    pub page: Option<u32>,
    pub parser_version: Option<String>,
    pub content_hash: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AppraisalAssessmentInput {
    pub definition_id: DefinitionId,
    pub definition_version: DefinitionVersion,
    pub responses: Value,
    pub evidence: Vec<EvidenceReferenceInput>,
    pub domain_judgments: BTreeMap<String, String>,
    pub overall_judgment: Option<String>,
    /// Reasons for judgments that differ from the suggestion, keyed by domain id
    /// or `overall`. A reason is required wherever the chosen judgment differs.
    #[serde(default)]
    pub override_reasons: BTreeMap<String, String>,
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
    #[error("question {0} applies to the recorded answers and cannot be not applicable")]
    ConditionalQuestionRequiresAnswer(String),
    #[error("question {0} does not apply to the recorded answers and must be not applicable")]
    ConditionalQuestionNotApplicable(String),
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
    #[error("a reason is required to override the suggested judgment for {0}")]
    MissingOverrideReason(String),
    #[error("override reason for {0} must be non-empty and at most 1000 characters")]
    InvalidOverrideReason(String),
    #[error("override reason targets an unknown judgment: {0}")]
    UnknownOverrideTarget(String),
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
    validate_conditions(definition)?;
    Ok(())
}

/// A conditional question needs a `not_applicable` option, and each clause must
/// name an earlier enum question and only answers that question can give.
fn validate_conditions(definition: &AppraisalDefinition) -> Result<(), AppraisalDefinitionError> {
    let questions = definition
        .domains
        .iter()
        .flat_map(|domain| domain.questions.iter())
        .collect::<Vec<_>>();
    let invalid = |message: String| Err(AppraisalDefinitionError::InvalidAnswerSchema(message));
    for (index, question) in questions.iter().enumerate() {
        let Some(condition) = &question.applies_when else {
            continue;
        };
        let AnswerSchema::Enum { options } = &question.answer_schema else {
            return invalid(format!(
                "conditional question {} must be an enum",
                question.id
            ));
        };
        if !options.iter().any(|option| option.value == NOT_APPLICABLE) {
            return invalid(format!(
                "conditional question {} needs a not_applicable option",
                question.id
            ));
        }
        if condition.clauses.is_empty() {
            return invalid(format!(
                "conditional question {} needs at least one clause",
                question.id
            ));
        }
        for clause in &condition.clauses {
            let Some(position) = questions
                .iter()
                .position(|earlier| earlier.id == clause.question)
            else {
                return invalid(format!(
                    "condition names an unknown question {}",
                    clause.question
                ));
            };
            if position >= index {
                return invalid(format!(
                    "condition for {} must name an earlier question",
                    question.id
                ));
            }
            let AnswerSchema::Enum {
                options: earlier_options,
            } = &questions[position].answer_schema
            else {
                return invalid(format!(
                    "condition names a non-enum question {}",
                    clause.question
                ));
            };
            if clause.answers.is_empty()
                || clause.answers.iter().any(|answer| {
                    answer == NOT_APPLICABLE
                        || !earlier_options.iter().any(|option| option.value == *answer)
                })
            {
                return invalid(format!(
                    "condition for {} has an answer that {} cannot give",
                    question.id, clause.question
                ));
            }
        }
    }
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
    validate_conditional_responses(definition, &input.responses)?;

    let questions = definition
        .domains
        .iter()
        .flat_map(|domain| domain.questions.iter())
        .map(|question| (question.id.as_str(), question))
        .collect::<BTreeMap<_, _>>();
    let mut evidence_questions = BTreeSet::new();
    let mut evidence_identities = BTreeSet::new();
    for evidence in &input.evidence {
        if !questions.contains_key(evidence.question_id.as_str()) {
            return Err(AppraisalValidationError::UnknownQuestion(
                evidence.question_id.clone(),
            ));
        }
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
    validate_override_reasons(definition, input)
}

/// A conditional question is answered only when its condition holds. Otherwise
/// its answer must be `not_applicable`, so the stored answers say what was asked.
fn validate_conditional_responses(
    definition: &AppraisalDefinition,
    responses: &Value,
) -> Result<(), AppraisalValidationError> {
    for question in definition
        .domains
        .iter()
        .flat_map(|domain| domain.questions.iter())
    {
        let Some(condition) = &question.applies_when else {
            continue;
        };
        let not_applicable =
            responses.get(&question.id).and_then(Value::as_str) == Some(NOT_APPLICABLE);
        let applies = condition.holds(responses);
        if applies && not_applicable {
            return Err(AppraisalValidationError::ConditionalQuestionRequiresAnswer(
                question.id.clone(),
            ));
        }
        if !applies && !not_applicable {
            return Err(AppraisalValidationError::ConditionalQuestionNotApplicable(
                question.id.clone(),
            ));
        }
    }
    Ok(())
}

/// A judgment that differs from the rule suggestion needs a reason, and every
/// reason must target a real domain or the overall judgment.
fn validate_override_reasons(
    definition: &AppraisalDefinition,
    input: &AppraisalAssessmentInput,
) -> Result<(), AppraisalValidationError> {
    for (target, reason) in &input.override_reasons {
        let known = target == OVERALL_JUDGMENT_TARGET
            || definition.domains.iter().any(|domain| domain.id == *target);
        if !known {
            return Err(AppraisalValidationError::UnknownOverrideTarget(
                target.clone(),
            ));
        }
        if reason.trim().is_empty() || reason.chars().count() > OVERRIDE_REASON_MAX_CHARS {
            return Err(AppraisalValidationError::InvalidOverrideReason(
                target.clone(),
            ));
        }
    }
    let Some(suggestion) = suggest_judgments(definition, &input.responses) else {
        return Ok(());
    };
    for domain in &suggestion.domains {
        let (Some(suggested), Some(chosen)) = (
            &domain.judgment,
            input.domain_judgments.get(&domain.domain_id),
        ) else {
            continue;
        };
        if chosen != suggested && !input.override_reasons.contains_key(&domain.domain_id) {
            return Err(AppraisalValidationError::MissingOverrideReason(
                domain.domain_id.clone(),
            ));
        }
    }
    if let (Some(suggested), Some(chosen)) = (&suggestion.overall_judgment, &input.overall_judgment)
        && chosen != suggested
        && !input.override_reasons.contains_key(OVERALL_JUDGMENT_TARGET)
    {
        return Err(AppraisalValidationError::MissingOverrideReason(
            OVERALL_JUDGMENT_TARGET.to_owned(),
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

/// Shipped definitions in listing order. The first RoB 2 entry is the default
/// for new assessments; v1 stays loadable for assessments already recorded.
const DEFINITION_RESOURCES: [&str; 4] = [
    include_str!("../appraisal-definitions/deepref-rct-rob2/v2.json"),
    include_str!("../appraisal-definitions/deepref-rct-rob2/v1.json"),
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

/// Shipped definition (id and version) that implements a tool suggested for a
/// study design. Tools without a shipped definition are never suggested.
pub fn appraisal_definition_for_tool(tool: &str) -> Option<(&'static str, u32)> {
    match tool {
        "RoB 2 (Cochrane risk-of-bias tool for randomized trials)" => {
            Some((suggestion::RCT_ROB2_ID, suggestion::RCT_ROB2_VERSION))
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shipped_definitions_validate_and_have_different_answer_shapes() {
        let definitions = all_appraisal_definitions().unwrap();
        assert_eq!(definitions.len(), 4);
        for definition in &definitions {
            validate_definition_resource(definition).unwrap();
        }
        let by_id = |id: &str| {
            definitions
                .iter()
                .find(|definition| definition.id.as_str() == id)
                .unwrap()
        };
        assert!(matches!(
            by_id("deepref-rct-generic").domains[0].questions[0].answer_schema,
            AnswerSchema::Enum { .. }
        ));
        assert!(matches!(
            by_id("deepref-qualitative-generic").domains[0].questions[0].answer_schema,
            AnswerSchema::Scale { .. }
        ));
    }

    #[test]
    fn rob2_definition_has_the_five_domains_and_attribution() {
        let definition = get_appraisal_definition("deepref-rct-rob2", 1).unwrap();
        assert_eq!(
            definition.name,
            "Randomized trial risk of bias (RoB 2 structure)"
        );
        assert_eq!(definition.applicability.designs, vec![StudyDesign::Rct]);
        assert!(
            definition
                .applicability
                .note
                .as_deref()
                .is_some_and(|note| note.contains("Sterne et al., BMJ 2019"))
        );
        let domain_ids = definition
            .domains
            .iter()
            .map(|domain| domain.id.as_str())
            .collect::<Vec<_>>();
        assert_eq!(
            domain_ids,
            vec![
                "randomization_process",
                "deviations_from_intervention",
                "missing_outcome_data",
                "outcome_measurement",
                "reported_result",
            ]
        );
        assert!(
            definition
                .domains
                .iter()
                .all(|domain| !domain.questions.is_empty()
                    && domain.judgment.options.len() == 3
                    && !domain.judgment.allow_custom)
        );
        let not_applicable = definition
            .domains
            .iter()
            .flat_map(|domain| domain.questions.iter())
            .filter(|question| match &question.answer_schema {
                AnswerSchema::Enum { options } => options
                    .iter()
                    .any(|option| option.value == "not_applicable"),
                _ => false,
            })
            .map(|question| question.id.as_str())
            .collect::<Vec<_>>();
        assert_eq!(
            not_applicable,
            vec![
                "adherence_analysis_appropriate",
                "missing_data_method",
                "assessors_unaware"
            ]
        );
    }

    #[test]
    fn rob2_definition_validation_rejects_malformed_resources() {
        let raw: Value = serde_json::from_str(include_str!(
            "../appraisal-definitions/deepref-rct-rob2/v1.json"
        ))
        .unwrap();

        let mut duplicate_question = raw.clone();
        duplicate_question["domains"][1]["questions"][0]["id"] =
            Value::String("sequence_unpredictable".to_owned());
        assert!(matches!(
            parse_appraisal_definition_resource(&duplicate_question),
            Err(AppraisalDefinitionError::DuplicateId(_))
        ));

        let mut no_designs = raw.clone();
        no_designs["applicability"]["designs"] = json!([]);
        assert_eq!(
            parse_appraisal_definition_resource(&no_designs),
            Err(AppraisalDefinitionError::MissingMetadata)
        );

        let mut duplicate_option = raw.clone();
        duplicate_option["overall_judgment"]["options"][1]["value"] =
            Value::String("low_risk".to_owned());
        assert!(matches!(
            parse_appraisal_definition_resource(&duplicate_option),
            Err(AppraisalDefinitionError::InvalidAnswerSchema(_))
        ));

        let mut unknown_design = raw;
        unknown_design["applicability"]["designs"] = json!(["cohort_study"]);
        assert!(matches!(
            parse_appraisal_definition_resource(&unknown_design),
            Err(AppraisalDefinitionError::InvalidJsonSchema(_)
                | AppraisalDefinitionError::InvalidResource(_))
        ));
    }

    #[test]
    fn tool_suggestions_resolve_only_to_shipped_definitions() {
        let tool = "RoB 2 (Cochrane risk-of-bias tool for randomized trials)";
        let (id, version) = appraisal_definition_for_tool(tool).expect("RoB 2 is available");
        assert_eq!((id, version), ("deepref-rct-rob2", 2));
        get_appraisal_definition(id, version).unwrap();
        assert_eq!(appraisal_definition_for_tool("RoB 2"), None);
        assert_eq!(appraisal_definition_for_tool("ROBINS-I"), None);
        assert_eq!(appraisal_definition_for_tool("PEDro"), None);
        // Every tool the domain suggests for a randomized trial has a definition.
        for suggested in deepref_domain::suggest_appraisal_tools(
            StudyDesign::Rct,
            deepref_domain::StudyDesignContext::default(),
        ) {
            assert!(
                appraisal_definition_for_tool(&suggested.tool).is_some(),
                "{} has no shipped definition",
                suggested.tool
            );
        }
    }

    #[test]
    fn rob2_v2_is_the_default_and_v1_stays_loadable() {
        let definitions = all_appraisal_definitions().unwrap();
        let rob2_versions = definitions
            .iter()
            .filter(|definition| definition.id.as_str() == "deepref-rct-rob2")
            .map(|definition| definition.version.get())
            .collect::<Vec<_>>();
        assert_eq!(rob2_versions, vec![2, 1]);
        assert_eq!(definitions[0].version.get(), 2);
        assert_eq!(
            get_appraisal_definition("deepref-rct-rob2", 1)
                .unwrap()
                .name,
            "Randomized trial risk of bias (RoB 2 structure)"
        );
    }

    #[test]
    fn rob2_v2_uses_the_official_signalling_questions_and_attribution() {
        let definition = get_appraisal_definition("deepref-rct-rob2", 2).unwrap();
        assert_eq!(
            definition.name,
            "RoB 2 (Cochrane risk-of-bias tool for randomized trials)"
        );
        assert_eq!(
            definition.applicability.note.as_deref(),
            Some(
                "Sterne JAC, et al. RoB 2: a revised tool for assessing risk of bias in randomised trials. BMJ 2019;366:l4898. riskofbias.info"
            )
        );
        let codes = definition
            .domains
            .iter()
            .flat_map(|domain| domain.questions.iter())
            .filter_map(|question| question.label.split(' ').next())
            .collect::<Vec<_>>();
        assert_eq!(
            codes,
            [
                "1.1", "1.2", "1.3", "2.1", "2.2", "2.3", "2.4", "2.5", "2.6", "2.7", "3.1", "3.2",
                "3.3", "3.4", "4.1", "4.2", "4.3", "4.4", "4.5", "5.1", "5.2", "5.3",
            ]
        );
        let conditional = definition
            .domains
            .iter()
            .flat_map(|domain| domain.questions.iter())
            .filter(|question| question.applies_when.is_some())
            .count();
        assert_eq!(conditional, 10);
    }

    #[test]
    fn rob2_v2_conditional_questions_must_be_not_applicable_exactly_when_their_condition_fails() {
        let definition = get_appraisal_definition("deepref-rct-rob2", 2).unwrap();
        // A consistent all-low answer set: 2.1 and 2.2 are "no", so 2.3 onward is not asked.
        let answers = [
            ("sequence_random", "yes"),
            ("allocation_concealed", "yes"),
            ("baseline_imbalance", "no"),
            ("participants_aware", "no"),
            ("personnel_aware", "probably_no"),
            ("trial_context_deviations", "not_applicable"),
            ("deviations_affected_outcome", "not_applicable"),
            ("deviations_balanced", "not_applicable"),
            ("analysis_appropriate", "yes"),
            ("switching_substantial_impact", "not_applicable"),
            ("outcome_data_complete", "yes"),
            ("missing_data_not_biased", "not_applicable"),
            ("missingness_could_depend", "not_applicable"),
            ("missingness_likely_depends", "not_applicable"),
            ("measurement_inappropriate", "no"),
            ("measurement_differed", "no"),
            ("assessors_aware", "no"),
            ("assessment_could_be_influenced", "not_applicable"),
            ("assessment_likely_influenced", "not_applicable"),
            ("analysis_prespecified", "yes"),
            ("outcome_measurement_selected", "no"),
            ("analysis_selected", "no"),
        ];
        let responses = Value::Object(
            answers
                .iter()
                .map(|(id, value)| ((*id).to_owned(), json!(value)))
                .collect(),
        );
        assert_eq!(
            answers.len(),
            definition
                .domains
                .iter()
                .map(|domain| domain.questions.len())
                .sum::<usize>(),
            "every question has an answer"
        );
        // Judgments follow the suggestion, so only the applicability rules are under test.
        let suggestion = suggest_judgments(&definition, &responses).unwrap();
        let input = AppraisalAssessmentInput {
            definition_id: definition.id.clone(),
            definition_version: definition.version,
            responses: responses.clone(),
            evidence: Vec::new(),
            domain_judgments: suggestion
                .domains
                .iter()
                .map(|domain| (domain.domain_id.clone(), domain.judgment.clone().unwrap()))
                .collect(),
            overall_judgment: suggestion.overall_judgment.clone(),
            override_reasons: BTreeMap::new(),
        };
        assert_eq!(validate_assessment_input(&definition, &input), Ok(()));

        // Asking 2.3 although neither 2.1 nor 2.2 calls for it is rejected.
        let mut asked_anyway = input.clone();
        asked_anyway.responses["trial_context_deviations"] = json!("no");
        assert_eq!(
            validate_assessment_input(&definition, &asked_anyway),
            Err(AppraisalValidationError::ConditionalQuestionNotApplicable(
                "trial_context_deviations".to_owned()
            ))
        );

        // Marking 2.3 not applicable while 2.1 is "yes" is rejected too.
        let mut skipped = input;
        skipped.responses["participants_aware"] = json!("yes");
        assert_eq!(
            validate_assessment_input(&definition, &skipped),
            Err(AppraisalValidationError::ConditionalQuestionRequiresAnswer(
                "trial_context_deviations".to_owned()
            ))
        );
    }

    #[test]
    fn rob2_v2_definition_validation_rejects_bad_conditions() {
        let raw: Value = serde_json::from_str(include_str!(
            "../appraisal-definitions/deepref-rct-rob2/v2.json"
        ))
        .unwrap();
        // Index 2 of domain 1 is 2.3 (trial_context_deviations), which has a condition.
        let condition = |raw: &Value| raw["domains"][1]["questions"][2]["applies_when"].clone();
        assert!(!condition(&raw).is_null());

        let mut unknown_question = raw.clone();
        unknown_question["domains"][1]["questions"][2]["applies_when"]["clauses"][0]["question"] =
            json!("no_such_question");
        assert!(matches!(
            parse_appraisal_definition_resource(&unknown_question),
            Err(AppraisalDefinitionError::InvalidAnswerSchema(_))
        ));

        let mut later_question = raw.clone();
        later_question["domains"][1]["questions"][2]["applies_when"]["clauses"][0]["question"] =
            json!("switching_substantial_impact");
        assert!(matches!(
            parse_appraisal_definition_resource(&later_question),
            Err(AppraisalDefinitionError::InvalidAnswerSchema(_))
        ));

        let mut foreign_answer = raw.clone();
        foreign_answer["domains"][1]["questions"][2]["applies_when"]["clauses"][0]["answers"] =
            json!(["maybe"]);
        assert!(matches!(
            parse_appraisal_definition_resource(&foreign_answer),
            Err(AppraisalDefinitionError::InvalidAnswerSchema(_))
        ));

        let mut no_not_applicable = raw;
        let options = no_not_applicable["domains"][1]["questions"][2]["answer_schema"]["options"]
            .as_array_mut()
            .unwrap();
        options.retain(|option| option["value"] != "not_applicable");
        assert!(matches!(
            parse_appraisal_definition_resource(&no_not_applicable),
            Err(AppraisalDefinitionError::InvalidAnswerSchema(_))
        ));
    }

    #[test]
    fn rob2_overrides_need_a_reason_when_judgments_differ_from_the_suggestion() {
        let definition = get_appraisal_definition("deepref-rct-rob2", 1).unwrap();
        let mut responses = serde_json::Map::new();
        for domain in &definition.domains {
            for question in &domain.questions {
                responses.insert(question.id.clone(), json!("yes"));
            }
        }
        responses.insert("sequence_unpredictable".to_owned(), json!("no"));
        responses.insert(
            "adherence_analysis_appropriate".to_owned(),
            json!("not_applicable"),
        );
        responses.insert("missing_data_method".to_owned(), json!("not_applicable"));
        responses.insert("assessors_unaware".to_owned(), json!("not_applicable"));
        responses.insert("selected_by_significance".to_owned(), json!("no"));
        responses.insert("deviations_unbalanced".to_owned(), json!("no"));
        responses.insert("missingness_related_to_outcome".to_owned(), json!("no"));
        responses.insert("alternatives_available".to_owned(), json!("no"));
        let responses = Value::Object(responses);
        let suggestion = suggest_judgments(&definition, &responses).unwrap();
        assert_eq!(suggestion.overall_judgment.as_deref(), Some("high_risk"));

        let mut domain_judgments = BTreeMap::new();
        for domain in &definition.domains {
            domain_judgments.insert(domain.id.clone(), "low_risk".to_owned());
        }
        let input = |override_reasons: BTreeMap<String, String>| AppraisalAssessmentInput {
            definition_id: definition.id.clone(),
            definition_version: definition.version,
            responses: responses.clone(),
            evidence: Vec::new(),
            domain_judgments: domain_judgments.clone(),
            overall_judgment: Some("low_risk".to_owned()),
            override_reasons,
        };

        assert_eq!(
            validate_assessment_input(&definition, &input(BTreeMap::new())),
            Err(AppraisalValidationError::MissingOverrideReason(
                "randomization_process".to_owned()
            ))
        );
        let reason = BTreeMap::from([(
            "randomization_process".to_owned(),
            "Sequence described as chance-based in the methods.".to_owned(),
        )]);
        assert_eq!(
            validate_assessment_input(&definition, &input(reason.clone())),
            Err(AppraisalValidationError::MissingOverrideReason(
                "overall".to_owned()
            ))
        );
        let with_overall = BTreeMap::from([
            reason.into_iter().next().unwrap(),
            (
                "overall".to_owned(),
                "Overall risk is driven by the sequence check.".to_owned(),
            ),
        ]);
        assert_eq!(
            validate_assessment_input(&definition, &input(with_overall.clone())),
            Ok(())
        );

        let blank = BTreeMap::from([("overall".to_owned(), "   ".to_owned())]);
        assert!(matches!(
            validate_assessment_input(&definition, &input(blank)),
            Err(AppraisalValidationError::InvalidOverrideReason(_))
        ));
        let too_long = BTreeMap::from([(
            "overall".to_owned(),
            "x".repeat(OVERRIDE_REASON_MAX_CHARS + 1),
        )]);
        assert!(matches!(
            validate_assessment_input(&definition, &input(too_long)),
            Err(AppraisalValidationError::InvalidOverrideReason(_))
        ));
        let unknown = BTreeMap::from([("not_a_domain".to_owned(), "reason".to_owned())]);
        assert!(matches!(
            validate_assessment_input(&definition, &input(unknown)),
            Err(AppraisalValidationError::UnknownOverrideTarget(_))
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
                page: None,
                parser_version: None,
                content_hash: None,
            }],
            domain_judgments: BTreeMap::from([
                ("allocation".to_owned(), "low_concern".to_owned()),
                ("outcome_reporting".to_owned(), "some_concern".to_owned()),
            ]),
            overall_judgment: Some("some_concern".to_owned()),
            override_reasons: BTreeMap::new(),
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
