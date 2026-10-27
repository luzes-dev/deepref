//! Scientific output only. Keep credentials, executable workflow graphs,
//! connector/provider envelopes and operational errors outside this allowlist.
//! Every section shares the collector's account-wide budgets and RR snapshot.

pub(super) const SECTIONS: &[(&str, &str)] = &[
    (
        "document_pages.json",
        section_query!(
            "SELECT s.document_id,s.parser_version,s.page_number,s.width,s.height,s.ocr_required,s.active FROM document_pages s JOIN documents d ON d.id=s.document_id JOIN projects p ON p.id=d.project_id WHERE p.workspace_id=ANY($1) ORDER BY s.document_id,s.parser_version,s.page_number"
        ),
    ),
    (
        "document_blocks.json",
        section_query!(
            "SELECT s.id,s.document_id,s.parser_version,s.page_number,s.kind,s.section_path,s.ordinal,s.text,s.bbox,s.content_hash,s.active,s.page_width,s.page_height FROM document_blocks s JOIN documents d ON d.id=s.document_id JOIN projects p ON p.id=d.project_id WHERE p.workspace_id=ANY($1) ORDER BY s.id"
        ),
    ),
    (
        "study_reports.json",
        section_query!(
            "SELECT s.project_id,s.study_id,s.report_id,s.relationship,s.created_at FROM study_reports s JOIN projects p ON p.id=s.project_id WHERE p.workspace_id=ANY($1) ORDER BY s.project_id,s.study_id,s.report_id"
        ),
    ),
    (
        "study_events.json",
        section_query!(
            "SELECT s.id,s.project_id,s.study_id,s.report_id,s.event_type,s.before_study_id,s.result_study_id,s.before_revision,s.result_revision,s.before_snapshot,s.result_snapshot,s.payload,s.actor_kind,s.actor_id,s.created_at FROM study_events s JOIN projects p ON p.id=s.project_id WHERE p.workspace_id=ANY($1) ORDER BY s.id"
        ),
    ),
    (
        "appraisal_assessments.json",
        section_query!(
            "SELECT s.id,s.project_id,s.report_id,s.definition_id,s.definition_version,s.responses,s.judgments,s.actor_kind,s.actor_id,s.completed_at,s.created_at FROM appraisal_assessments s JOIN projects p ON p.id=s.project_id WHERE p.workspace_id=ANY($1) ORDER BY s.id"
        ),
    ),
    (
        "appraisal_evidence.json",
        section_query!(
            "SELECT s.id,s.assessment_id,s.project_id,s.report_id,s.question_id,s.document_id,s.block_id,s.created_at FROM appraisal_assessment_evidence s JOIN projects p ON p.id=s.project_id WHERE p.workspace_id=ANY($1) ORDER BY s.id"
        ),
    ),
    (
        "appraisal_events.json",
        section_query!(
            "SELECT s.id,s.assessment_id,s.project_id,s.report_id,s.event_type,s.payload,s.actor_kind,s.actor_id,s.created_at FROM appraisal_events s JOIN projects p ON p.id=s.project_id WHERE p.workspace_id=ANY($1) ORDER BY s.id"
        ),
    ),
    (
        "extraction_fields.json",
        section_query!(
            "SELECT s.id,s.project_id,s.version,s.field_key,s.label,s.value_type,s.required,s.created_at FROM extraction_field_definitions s JOIN projects p ON p.id=s.project_id WHERE p.workspace_id=ANY($1) ORDER BY s.project_id,s.id,s.version"
        ),
    ),
    (
        "extraction_values.json",
        section_query!(
            "SELECT s.id,s.project_id,s.study_id,s.report_id,s.field_definition_id,s.field_definition_version,s.value_type,s.text_value,s.number_value,s.boolean_value,s.date_value,s.rationale,s.source_document_id,s.source_block_id,s.source_page,s.source_parser_version,s.source_content_hash,s.approved_by_actor_kind,s.approved_by_actor_id,s.approved_at,s.superseded_at,s.superseded_by_actor_kind,s.superseded_by_actor_id,s.needs_verification,s.verified_at,s.verified_by_actor_kind,s.verified_by_actor_id,s.activity_id,s.created_at FROM extraction_values s JOIN projects p ON p.id=s.project_id WHERE p.workspace_id=ANY($1) ORDER BY s.id"
        ),
    ),
    (
        "extraction_events.json",
        section_query!(
            "SELECT s.id,s.project_id,s.study_id,s.proposal_id,s.event_type,s.payload,s.actor_kind,s.actor_id,s.created_at FROM extraction_events s JOIN projects p ON p.id=s.project_id WHERE p.workspace_id=ANY($1) ORDER BY s.id"
        ),
    ),
    (
        "project_reports.json",
        section_query!(
            "SELECT s.project_id,s.report_id,s.first_seen_record_id,s.lifecycle_status,s.created_at FROM project_reports s JOIN projects p ON p.id=s.project_id WHERE p.workspace_id=ANY($1) ORDER BY s.project_id,s.report_id"
        ),
    ),
    (
        "report_identifiers.json",
        section_query!(
            "SELECT s.report_id,s.scheme,s.value FROM report_identifiers s WHERE EXISTS(SELECT 1 FROM project_reports pr JOIN projects p ON p.id=pr.project_id WHERE pr.report_id=s.report_id AND p.workspace_id=ANY($1)) ORDER BY s.report_id,s.scheme,s.value"
        ),
    ),
    (
        "acquisition_runs.json",
        section_query!(
            "SELECT s.id,s.project_id,s.source,s.strategy,s.status,s.max_depth,s.historical_max_depth,s.seed_count,s.queued_count,s.fetched_count,s.failed_count,s.metadata_provider,s.citation_provider,s.created_at,s.started_at,s.completed_at FROM acquisition_runs s JOIN projects p ON p.id=s.project_id WHERE p.workspace_id=ANY($1) ORDER BY s.id"
        ),
    ),
    (
        "acquisition_items.json",
        section_query!(
            "SELECT s.acquisition_id,s.project_id,s.report_id,s.canonical_doi,s.depth,s.parent_doi,s.status,s.attempts,s.queued_at,s.fetched_at,s.work_event_id FROM acquisition_items s JOIN projects p ON p.id=s.project_id WHERE p.workspace_id=ANY($1) ORDER BY s.acquisition_id,s.canonical_doi"
        ),
    ),
    (
        "exclusion_reasons.json",
        section_query!(
            "SELECT s.id,s.project_id,s.code,s.label,s.stage FROM exclusion_reasons s JOIN projects p ON p.id=s.project_id WHERE p.workspace_id=ANY($1) ORDER BY s.id"
        ),
    ),
    (
        "screening_state.json",
        section_query!(
            "SELECT s.project_id,s.report_id,s.title_abstract_status,s.full_text_status,s.full_text_exclusion_reason_id,s.final_status,s.revision,s.last_event_id,s.updated_at FROM screening_state s JOIN projects p ON p.id=s.project_id WHERE p.workspace_id=ANY($1) ORDER BY s.project_id,s.report_id"
        ),
    ),
    (
        "review_events.json",
        section_query!(
            "SELECT s.id,s.project_id,s.event_type,s.aggregate_type,s.aggregate_id,s.payload,s.actor_kind,s.actor_id,s.created_at FROM review_events s JOIN projects p ON p.id=s.project_id WHERE p.workspace_id=ANY($1) ORDER BY s.id"
        ),
    ),
    (
        "ai_runs.json",
        section_query!(
            "SELECT s.id,s.project_id,s.task_kind,s.provider,s.model,s.model_version,s.prompt_version,s.schema_version,s.prompt_hash,s.schema_hash,s.input_hash,s.output,s.status,s.parent_automation_run_id,s.created_at,s.completed_at FROM ai_runs s JOIN projects p ON p.id=s.project_id WHERE p.workspace_id=ANY($1) ORDER BY s.id"
        ),
    ),
    (
        "ai_proposals.json",
        section_query!(
            "SELECT s.id,s.project_id,s.model_run_id,s.proposal_type,s.task_kind,s.entity_type,s.entity_id,s.operation,s.authority_tier,s.target_report_id,s.target_record_id,s.target_study_id,s.protocol_version_id,s.expected_revision,s.payload,s.status,s.resolved_at,s.resolved_by_actor_kind,s.resolved_by_actor_id,s.resolution_reason,s.created_at FROM ai_proposals s JOIN projects p ON p.id=s.project_id WHERE p.workspace_id=ANY($1) ORDER BY s.id"
        ),
    ),
    (
        "ai_run_evidence.json",
        section_query!(
            "SELECT s.ai_run_id,s.project_id,s.document_id,s.document_block_id,s.rank,s.retrieval_score,s.content_hash,s.created_at FROM ai_run_evidence s JOIN projects p ON p.id=s.project_id WHERE p.workspace_id=ANY($1) ORDER BY s.ai_run_id,s.rank"
        ),
    ),
    (
        "ai_proposal_evidence.json",
        section_query!(
            "SELECT s.proposal_id,s.project_id,s.ordinal,s.evidence_kind,s.report_id,s.document_id,s.document_block_id,s.page,s.source_field,s.content_hash,s.created_at FROM ai_proposal_evidence s JOIN projects p ON p.id=s.project_id WHERE p.workspace_id=ANY($1) ORDER BY s.proposal_id,s.ordinal"
        ),
    ),
    (
        "ai_criterion_judgments.json",
        section_query!(
            "SELECT s.proposal_id,s.project_id,s.criterion_id,s.protocol_version_id,s.ordinal,s.judgment,s.rationale,s.evidence,s.created_at FROM ai_proposal_criterion_judgments s JOIN projects p ON p.id=s.project_id WHERE p.workspace_id=ANY($1) ORDER BY s.proposal_id,s.ordinal"
        ),
    ),
    (
        "automation_definitions.json",
        section_query!(
            "SELECT s.id,s.project_id,s.name,s.trigger_kind,s.recipe_id,s.recipe_version,s.status,s.actor_kind,s.actor_id,s.created_at,s.updated_at FROM automation_definitions s JOIN projects p ON p.id=s.project_id WHERE p.workspace_id=ANY($1) ORDER BY s.id"
        ),
    ),
    (
        "automation_runs.json",
        section_query!(
            "SELECT s.id,s.project_id,s.actor_kind,s.actor_id,s.status,s.created_at,s.started_at,s.finished_at,d.definition_id,d.recipe_id,d.recipe_version,d.trigger_kind FROM durable_runs s LEFT JOIN automation_run_details d ON d.id=s.id JOIN projects p ON p.id=s.project_id WHERE p.workspace_id=ANY($1) AND s.kind='automation' ORDER BY s.id"
        ),
    ),
    (
        "automation_steps.json",
        section_query!(
            "SELECT s.id,s.project_id,s.run_id,s.handler_kind,s.step_key,s.ordinal,s.status,s.attempts,s.output,s.created_at,s.started_at,s.finished_at FROM durable_steps s JOIN durable_runs r ON r.id=s.run_id JOIN projects p ON p.id=s.project_id WHERE p.workspace_id=ANY($1) AND r.kind='automation' ORDER BY s.id"
        ),
    ),
    (
        "automation_attempts.json",
        section_query!(
            "SELECT s.id,s.project_id,s.run_id,s.step_id,s.attempt_number,s.status,s.started_at,s.finished_at FROM durable_attempts s JOIN durable_runs r ON r.id=s.run_id JOIN projects p ON p.id=s.project_id WHERE p.workspace_id=ANY($1) AND r.kind='automation' ORDER BY s.id"
        ),
    ),
    (
        "review_manifests.json",
        section_query!(
            "SELECT s.project_id,s.automation_run_id,s.definition_key,s.definition_id,s.definition_version,s.manifest_hash,s.semantic_bundle_hash,s.manifest,s.subject,s.origin,s.candidate_hash,s.proposal_id,s.created_at FROM review_run_manifests s JOIN projects p ON p.id=s.project_id WHERE p.workspace_id=ANY($1) ORDER BY s.automation_run_id"
        ),
    ),
    (
        "review_artifacts.json",
        section_query!(
            "SELECT s.id,s.project_id,s.content_hash,s.media_type,s.payload,s.created_at FROM review_artifacts s JOIN projects p ON p.id=s.project_id WHERE p.workspace_id=ANY($1) ORDER BY s.id"
        ),
    ),
    (
        "review_artifact_lineage.json",
        section_query!(
            "SELECT s.project_id,s.artifact_id,s.predecessor_artifact_id FROM review_artifact_lineage s JOIN projects p ON p.id=s.project_id WHERE p.workspace_id=ANY($1) ORDER BY s.artifact_id,s.predecessor_artifact_id"
        ),
    ),
    (
        "review_attempts.json",
        section_query!(
            "SELECT s.id,s.project_id,s.run_id,s.node_id,s.node_version,s.input_fingerprint,s.artifact_id,s.model_run_id,s.accepted_at,s.accepted_attempt_id FROM review_attempt_details s JOIN projects p ON p.id=s.project_id WHERE p.workspace_id=ANY($1) ORDER BY s.id"
        ),
    ),
    (
        "review_finalizations.json",
        section_query!(
            "SELECT s.project_id,s.automation_run_id,s.candidate_hash,s.proposal_id,s.model_run_id,s.created_at FROM review_proposal_finalizations s JOIN projects p ON p.id=s.project_id WHERE p.workspace_id=ANY($1) ORDER BY s.automation_run_id,s.candidate_hash"
        ),
    ),
];
