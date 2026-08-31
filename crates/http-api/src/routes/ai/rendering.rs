use super::*;

pub(crate) fn proposal_dto(proposal: AiProposalRecord) -> Result<AiProposalDto, ApiError> {
    let payload = typed_payload(&proposal)?;
    Ok(AiProposalDto {
        id: proposal.id,
        project_id: proposal.project_id,
        task_kind: proposal.task_kind,
        entity_type: proposal.entity_type,
        entity_id: proposal.entity_id,
        operation: proposal.operation,
        payload,
        authority_tier: proposal.authority_tier,
        model_run_id: proposal.model_run_id,
        provider: proposal.provider,
        model: proposal.model,
        model_version: proposal.model_version,
        prompt_version: proposal.prompt_version,
        schema_version: proposal.schema_version,
        prompt_hash: proposal.prompt_hash,
        schema_hash: proposal.schema_hash,
        input_hash: proposal.input_hash,
        evidence_hash: proposal.evidence_hash,
        status: proposal.status,
        protocol_version_id: proposal.protocol_version_id,
        expected_revision: proposal.expected_revision,
        target_report_id: proposal.target_report_id,
        target_record_id: proposal.target_record_id,
        target_study_id: proposal.target_study_id,
        resolved_at: proposal.resolved_at,
        resolved_by_actor_kind: proposal.resolved_by_actor_kind,
        resolved_by_actor_id: proposal.resolved_by_actor_id,
        resolution_reason: proposal.resolution_reason,
        created_at: proposal.created_at,
    })
}

pub(super) fn typed_payload(proposal: &AiProposalRecord) -> Result<AiProposalPayload, ApiError> {
    let mut payload = proposal.payload.clone();
    let object = payload.as_object_mut().ok_or_else(|| {
        ApiError::Internal(anyhow::anyhow!(
            "stored AI proposal payload is not an object"
        ))
    })?;
    let kind = match proposal.task_kind.as_str() {
        "title_abstract_screening" | "full_text_screening" => "screening",
        "duplicate_candidate_detection" => "duplicate",
        "study_grouping" => "study_grouping",
        "study_design_classification" => "classification",
        "appraisal_prefill" => "appraisal_prefill",
        "data_extraction" => "data_extraction",
        task_kind => {
            return Err(ApiError::Internal(anyhow::anyhow!(
                "unsupported AI proposal task kind: {task_kind}"
            )));
        }
    };
    object.insert("kind".to_owned(), Value::String(kind.to_owned()));
    serde_json::from_value(payload).map_err(|error| {
        ApiError::Internal(anyhow::anyhow!(
            "stored AI proposal payload is invalid: {error}"
        ))
    })
}

pub(crate) fn map_ai_error(error: AiError) -> ApiError {
    match error {
        AiError::InvalidContext(message)
        | AiError::SemanticValidation(message)
        | AiError::SchemaValidation(message)
        | AiError::MalformedOutput(message)
        | AiError::InputSerialization(message) => ApiError::BadRequest(message),
        AiError::Route(message) => ApiError::Configuration(message),
        AiError::Gateway(message) => {
            ApiError::Configuration(format!("AI provider is unavailable: {message}"))
        }
        AiError::Persistence(message) | AiError::Proposal(message) => {
            ApiError::Internal(anyhow::anyhow!(message))
        }
        AiError::PromptRegistry(message) | AiError::InvalidEmbedding(message) => {
            ApiError::BadRequest(message)
        }
    }
}

pub(super) fn map_ai_proposal_error(error: AiProposalError) -> ApiError {
    match error {
        AiProposalError::Database(error) => ApiError::Database(error),
        AiProposalError::NotFound => ApiError::NotFound("AI proposal target not found".to_owned()),
        AiProposalError::NotPending => ApiError::Conflict {
            code: "ai_proposal_not_pending".to_owned(),
            message: "AI proposal is no longer pending".to_owned(),
            details: Value::Null,
        },
        AiProposalError::InvalidPayload(message) | AiProposalError::InvalidTarget(message) => {
            ApiError::BadRequest(message)
        }
        AiProposalError::InvalidActor => ApiError::BadRequest("actor is invalid".to_owned()),
        AiProposalError::Screening(error) => super::super::review::map_screening_error(error),
        AiProposalError::Dedupe(error) => super::super::deduplication::map_dedupe_error(error),
        AiProposalError::Study(error) => super::super::study::map_study_error(error),
        AiProposalError::Appraisal(error) => super::super::study::map_appraisal_error(error),
        AiProposalError::Extraction(
            error @ (deepref_postgres::ExtractionError::EvidenceNotInStudy
            | deepref_postgres::ExtractionError::RequiredFieldInsufficient
            | deepref_postgres::ExtractionError::StaleDefinitionVersion
            | deepref_postgres::ExtractionError::ValueAlreadyApproved),
        ) => ApiError::Conflict {
            code: "extraction_conflict".to_owned(),
            message: error.to_string(),
            details: Value::Null,
        },
        AiProposalError::Extraction(
            error @ (deepref_postgres::ExtractionError::DefinitionNotFound
            | deepref_postgres::ExtractionError::StudyNotFound),
        ) => ApiError::NotFound(error.to_string()),
        AiProposalError::Extraction(error) => ApiError::BadRequest(error.to_string()),
    }
}

pub(super) fn map_protocol_error(error: deepref_postgres::ProtocolError) -> ApiError {
    match error {
        deepref_postgres::ProtocolError::ProjectNotFound
        | deepref_postgres::ProtocolError::NotFound => ApiError::NotFound(error.to_string()),
        deepref_postgres::ProtocolError::Database(error) => ApiError::Database(error),
        deepref_postgres::ProtocolError::Serialization(error) => ApiError::Internal(error.into()),
        deepref_postgres::ProtocolError::Invalid(message)
        | deepref_postgres::ProtocolError::DataIntegrity(message) => ApiError::BadRequest(message),
        deepref_postgres::ProtocolError::DraftAlreadyExists
        | deepref_postgres::ProtocolError::NotEditable => ApiError::Conflict {
            code: "protocol_conflict".to_owned(),
            message: error.to_string(),
            details: Value::Null,
        },
        deepref_postgres::ProtocolError::Conflict { message, .. } => ApiError::Conflict {
            code: "protocol_conflict".to_owned(),
            message: message.to_owned(),
            details: Value::Null,
        },
    }
}
