use chrono::{DateTime, Utc};
use serde_json::Value;
use sqlx::PgPool;
use uuid::Uuid;

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct AuditExportRow {
    pub id: Uuid,
    pub created_at: DateTime<Utc>,
    pub event_type: String,
    pub aggregate_type: String,
    pub aggregate_id: Uuid,
    pub actor_kind: Option<String>,
    pub actor_id: Option<String>,
    pub protocol_version_id: Option<Uuid>,
    pub stage: Option<String>,
    pub decision: Option<String>,
    pub reason_id: Option<Uuid>,
    pub event_kind: String,
    pub supersedes_event_id: Option<Uuid>,
    pub undoes_event_id: Option<Uuid>,
    pub previous_snapshot: Value,
    pub result_snapshot: Value,
    pub notes: Option<String>,
    pub payload: Value,
    pub provenance: Value,
}

pub async fn load_audit_export_rows(
    pool: &PgPool,
    project_id: Uuid,
    limit: i64,
) -> Result<Vec<AuditExportRow>, sqlx::Error> {
    sqlx::query_as::<_, AuditExportRow>(
        r#"SELECT id, created_at, event_type, aggregate_type, aggregate_id,
                  actor_kind, actor_id, protocol_version_id, stage, decision,
                  reason_id, event_kind, supersedes_event_id, undoes_event_id,
                  previous_snapshot, result_snapshot, notes, payload, provenance
           FROM (
             SELECT id, created_at, 'screening' AS event_type, 'screening' AS aggregate_type,
                    report_id AS aggregate_id, actor_kind, actor_id, protocol_version_id,
                    stage, decision, exclusion_reason_id AS reason_id, event_kind,
                    supersedes_event_id, undoes_event_id,
                    jsonb_build_object(
                      'title_abstract_status', previous_title_abstract_status,
                      'full_text_status', previous_full_text_status,
                      'full_text_exclusion_reason_id', previous_full_text_exclusion_reason_id,
                      'final_status', previous_final_status
                    ) AS previous_snapshot,
                    jsonb_build_object(
                      'title_abstract_status', result_title_abstract_status,
                      'full_text_status', result_full_text_status,
                      'full_text_exclusion_reason_id', result_full_text_exclusion_reason_id,
                      'final_status', result_final_status
                    ) AS result_snapshot,
                    notes,
                    jsonb_build_object(
                      'stage', stage, 'decision', decision,
                      'exclusion_reason_id', exclusion_reason_id,
                      'event_kind', event_kind, 'notes', notes
                    ) AS payload,
                    jsonb_build_object('actor_kind', actor_kind, 'actor_id', actor_id,
                                       'protocol_version_id', protocol_version_id) AS provenance
             FROM screening_events WHERE project_id = $1
             UNION ALL
             SELECT id, created_at, event_type, 'study' AS aggregate_type,
                    study_id AS aggregate_id, actor_kind, actor_id, NULL::uuid,
                    NULL::text, NULL::text, NULL::uuid, event_type,
                    NULL::uuid, NULL::uuid,
                    jsonb_build_object('study_id', before_study_id,
                                       'revision', before_revision,
                                       'snapshot', before_snapshot) AS previous_snapshot,
                    jsonb_build_object('study_id', result_study_id,
                                       'revision', result_revision,
                                       'snapshot', result_snapshot) AS result_snapshot,
                    NULL::text, payload,
                    jsonb_build_object('actor_kind', actor_kind, 'actor_id', actor_id,
                                       'report_id', report_id, 'study_id', study_id) AS provenance
             FROM study_events WHERE project_id = $1
             UNION ALL
             SELECT id, created_at, event_type, 'appraisal' AS aggregate_type,
                    assessment_id AS aggregate_id, actor_kind, actor_id, NULL::uuid,
                    NULL::text, NULL::text, NULL::uuid, event_type,
                    NULL::uuid, NULL::uuid, '{}'::jsonb, '{}'::jsonb,
                    NULL::text, payload,
                    jsonb_build_object('actor_kind', actor_kind, 'actor_id', actor_id,
                                       'report_id', report_id, 'assessment_id', assessment_id) AS provenance
             FROM appraisal_events WHERE project_id = $1
             UNION ALL
             SELECT id, created_at, 'dedupe_resolution' AS event_type, 'dedupe_record' AS aggregate_type,
                    record_id AS aggregate_id, actor_kind, actor_id, NULL::uuid,
                    'dedupe'::text AS stage, action AS decision, NULL::uuid AS reason_id,
                    action AS event_kind, NULL::uuid AS supersedes_event_id,
                    reverted_event_id AS undoes_event_id,
                    jsonb_build_object(
                      'prior_report_id', prior_report_id,
                      'proposal_id', proposal_id
                    ) AS previous_snapshot,
                    jsonb_build_object(
                      'resolved_report_id', resolved_report_id,
                      'action', action
                    ) AS result_snapshot,
                    reason AS notes,
                    jsonb_build_object(
                      'action', action, 'reason', reason,
                      'prior_report_id', prior_report_id,
                      'resolved_report_id', resolved_report_id,
                      'proposal_id', proposal_id,
                      'reverted_event_id', reverted_event_id
                    ) AS payload,
                    jsonb_build_object(
                      'actor_kind', actor_kind, 'actor_id', actor_id,
                      'record_id', record_id, 'proposal_id', proposal_id,
                      'prior_report_id', prior_report_id,
                      'resolved_report_id', resolved_report_id
                    ) AS provenance
             FROM dedupe_resolution_events WHERE project_id = $1
             UNION ALL
             SELECT a.id, a.created_at, 'ai_run_snapshot' AS event_type, 'ai_run' AS aggregate_type,
                    a.id AS aggregate_id, NULL::text AS actor_kind, NULL::text AS actor_id,
                    NULL::uuid AS protocol_version_id, 'ai_run'::text AS stage,
                    NULL::text AS decision, NULL::uuid AS reason_id,
                    'durable_snapshot'::text AS event_kind, NULL::uuid AS supersedes_event_id,
                    NULL::uuid AS undoes_event_id, '{}'::jsonb AS previous_snapshot,
                    jsonb_strip_nulls(jsonb_build_object(
                      'snapshot_kind', 'durable_current_record',
                      'task_kind', a.task_kind,
                      'status', a.status,
                      'profile', a.profile,
                      'provider', a.provider,
                      'model', a.model,
                      'model_version', a.model_version,
                      'prompt_version', a.prompt_version,
                      'schema_version', a.schema_version,
                      'prompt_hash', a.prompt_hash,
                      'schema_hash', a.schema_hash,
                      'input_hash', a.input_hash,
                      'reuse_hash', a.reuse_hash,
                      'protocol_hash', a.protocol_hash,
                      'document_hash', a.document_hash,
                      'evidence_hash', a.evidence_hash,
                      'evidence_ref_count', jsonb_array_length(a.evidence_refs),
                      'evidence_block_count', COALESCE((
                        SELECT count(*)
                        FROM ai_run_evidence AS evidence
                        WHERE evidence.project_id = a.project_id
                          AND evidence.ai_run_id = a.id
                      ), 0),
                      'input_tokens', a.input_tokens,
                      'output_tokens', a.output_tokens,
                      'cost_micros', a.cost_micros,
                      'error_code', a.error_code
                    )) AS result_snapshot,
                    NULL::text AS notes,
                    jsonb_strip_nulls(jsonb_build_object(
                      'snapshot_kind', 'durable_current_record',
                      'task_kind', a.task_kind,
                      'status', a.status,
                      'provider', a.provider,
                      'model', a.model,
                      'model_version', a.model_version,
                      'prompt_version', a.prompt_version,
                      'schema_version', a.schema_version,
                      'prompt_hash', a.prompt_hash,
                      'schema_hash', a.schema_hash,
                      'input_hash', a.input_hash,
                      'reuse_hash', a.reuse_hash,
                      'protocol_hash', a.protocol_hash,
                      'document_hash', a.document_hash,
                      'evidence_hash', a.evidence_hash,
                      'evidence_ref_count', jsonb_array_length(a.evidence_refs),
                      'evidence_block_count', COALESCE((
                        SELECT count(*)
                        FROM ai_run_evidence AS evidence
                        WHERE evidence.project_id = a.project_id
                          AND evidence.ai_run_id = a.id
                      ), 0),
                      'input_tokens', a.input_tokens,
                      'output_tokens', a.output_tokens,
                      'cost_micros', a.cost_micros,
                      'parent_automation_run_id', a.parent_automation_run_id
                    )) AS payload,
                    jsonb_strip_nulls(jsonb_build_object(
                      'project_id', a.project_id,
                      'ai_run_id', a.id,
                      'parent_automation_run_id', a.parent_automation_run_id,
                      'provenance_kind', 'ai_run_record'
                    )) AS provenance
             FROM ai_runs AS a
             WHERE a.project_id = $1
             UNION ALL
             SELECT p.id, p.created_at, 'ai_proposal_snapshot' AS event_type,
                    'ai_proposal' AS aggregate_type, p.id AS aggregate_id,
                    p.resolved_by_actor_kind AS actor_kind,
                    p.resolved_by_actor_id AS actor_id,
                    p.protocol_version_id, 'ai_proposal'::text AS stage,
                    NULL::text AS decision, NULL::uuid AS reason_id,
                    'durable_snapshot'::text AS event_kind, NULL::uuid AS supersedes_event_id,
                    NULL::uuid AS undoes_event_id, '{}'::jsonb AS previous_snapshot,
                    jsonb_strip_nulls(jsonb_build_object(
                      'snapshot_kind', 'durable_current_record',
                      'task_kind', p.task_kind,
                      'operation', p.operation,
                      'authority_tier', p.authority_tier,
                      'status', p.status,
                      'model_run_id', p.model_run_id,
                      'target_report_id', p.target_report_id,
                      'target_record_id', p.target_record_id,
                      'target_study_id', p.target_study_id,
                      'protocol_version_id', p.protocol_version_id,
                      'resolved_at', p.resolved_at,
                      'resolved_by_actor_kind', p.resolved_by_actor_kind,
                      'resolved_by_actor_id', p.resolved_by_actor_id,
                      'resolution_reason', p.resolution_reason,
                      'provider', a.provider,
                      'model', a.model,
                      'model_version', a.model_version,
                      'prompt_version', a.prompt_version,
                      'schema_version', a.schema_version,
                      'prompt_hash', a.prompt_hash,
                      'schema_hash', a.schema_hash,
                      'input_hash', a.input_hash,
                      'reuse_hash', a.reuse_hash,
                      'protocol_hash', a.protocol_hash,
                      'document_hash', a.document_hash,
                      'evidence_hash', a.evidence_hash,
                      'evidence_ref_count', jsonb_array_length(a.evidence_refs),
                      'input_tokens', a.input_tokens,
                      'output_tokens', a.output_tokens,
                      'cost_micros', a.cost_micros
                    )) AS result_snapshot,
                    p.resolution_reason AS notes,
                    jsonb_strip_nulls(jsonb_build_object(
                      'snapshot_kind', 'durable_current_record',
                      'project_id', p.project_id,
                      'ai_proposal_id', p.id,
                      'model_run_id', p.model_run_id,
                      'task_kind', p.task_kind,
                      'operation', p.operation,
                      'authority_tier', p.authority_tier,
                      'status', p.status,
                      'resolved_at', p.resolved_at,
                      'resolved_by_actor_kind', p.resolved_by_actor_kind,
                      'resolved_by_actor_id', p.resolved_by_actor_id,
                      'resolution_reason', p.resolution_reason
                    )) AS payload,
                    jsonb_strip_nulls(jsonb_build_object(
                      'project_id', p.project_id,
                      'ai_proposal_id', p.id,
                      'model_run_id', p.model_run_id,
                      'target_report_id', p.target_report_id,
                      'target_record_id', p.target_record_id,
                      'target_study_id', p.target_study_id,
                      'protocol_version_id', p.protocol_version_id,
                      'reviewer_actor_kind', p.resolved_by_actor_kind,
                      'reviewer_actor_id', p.resolved_by_actor_id,
                      'provenance_kind', 'ai_proposal_record'
                    )) AS provenance
             FROM ai_proposals AS p
             JOIN ai_runs AS a
               ON a.project_id = p.project_id AND a.id = p.model_run_id
             WHERE p.project_id = $1
             UNION ALL
             SELECT d.id, d.created_at, 'automation_definition_snapshot' AS event_type,
                    'automation_definition' AS aggregate_type, d.id AS aggregate_id,
                    d.actor_kind, d.actor_id, NULL::uuid AS protocol_version_id,
                    d.trigger_kind AS stage, NULL::text AS decision,
                    NULL::uuid AS reason_id, 'durable_snapshot'::text AS event_kind,
                    NULL::uuid AS supersedes_event_id, NULL::uuid AS undoes_event_id,
                    '{}'::jsonb AS previous_snapshot,
                    jsonb_build_object(
                      'snapshot_kind', 'durable_current_record',
                      'name', d.name,
                      'recipe_id', d.recipe_id,
                      'recipe_version', d.recipe_version,
                      'trigger_kind', d.trigger_kind,
                      'status', d.status,
                      'actor_kind', d.actor_kind,
                      'actor_id', d.actor_id
                    ) AS result_snapshot,
                    NULL::text AS notes,
                    jsonb_build_object(
                      'snapshot_kind', 'durable_current_record',
                      'project_id', d.project_id,
                      'definition_id', d.id,
                      'name', d.name,
                      'recipe_id', d.recipe_id,
                      'recipe_version', d.recipe_version,
                      'trigger_kind', d.trigger_kind,
                      'status', d.status,
                      'actor_kind', d.actor_kind,
                      'actor_id', d.actor_id,
                      'provenance_kind', 'automation_definition_record'
                    ) AS payload,
                    jsonb_build_object(
                      'project_id', d.project_id,
                      'definition_id', d.id,
                      'initiator_actor_kind', d.actor_kind,
                      'initiator_actor_id', d.actor_id,
                      'provenance_kind', 'automation_definition_record'
                    ) AS provenance
             FROM automation_definitions AS d
             WHERE d.project_id = $1
             UNION ALL
             SELECT r.id, r.created_at, 'automation_run_snapshot' AS event_type,
                    'automation_run' AS aggregate_type, r.id AS aggregate_id,
                    r.actor_kind, r.actor_id, NULL::uuid AS protocol_version_id,
                    r.trigger_kind AS stage, NULL::text AS decision,
                    NULL::uuid AS reason_id, 'durable_snapshot'::text AS event_kind,
                    NULL::uuid AS supersedes_event_id, NULL::uuid AS undoes_event_id,
                    '{}'::jsonb AS previous_snapshot,
                    jsonb_build_object(
                      'snapshot_kind', 'durable_current_record',
                      'definition_id', r.definition_id,
                      'recipe_id', r.recipe_id,
                      'recipe_version', r.recipe_version,
                      'trigger_kind', r.trigger_kind,
                      'trigger_reference', r.trigger_reference,
                      'status', r.status,
                      'job_id', r.job_id,
                      'job_state', j.state,
                      'job_attempts', j.attempts,
                      'job_max_attempts', j.max_attempts,
                      'ai_run_count', COALESCE(ai_usage.ai_run_count, 0),
                      'linked_ai_run_ids', COALESCE(ai_usage.ai_run_ids, '[]'::jsonb),
                      'ai_linkage_scope', 'automation_run_parent',
                      'input_tokens', COALESCE(ai_usage.input_tokens, 0),
                      'output_tokens', COALESCE(ai_usage.output_tokens, 0),
                      'cost_micros', COALESCE(ai_usage.cost_micros, 0)
                    ) AS result_snapshot,
                    NULL::text AS notes,
                    jsonb_build_object(
                      'snapshot_kind', 'durable_current_record',
                      'definition_id', r.definition_id,
                      'recipe_id', r.recipe_id,
                      'recipe_version', r.recipe_version,
                      'trigger_kind', r.trigger_kind,
                      'trigger_reference', r.trigger_reference,
                      'status', r.status,
                      'job_id', r.job_id,
                      'job_state', j.state,
                      'job_attempts', j.attempts,
                      'job_max_attempts', j.max_attempts,
                      'ai_run_count', COALESCE(ai_usage.ai_run_count, 0),
                      'linked_ai_run_ids', COALESCE(ai_usage.ai_run_ids, '[]'::jsonb),
                      'ai_linkage_scope', 'automation_run_parent',
                      'input_tokens', COALESCE(ai_usage.input_tokens, 0),
                      'output_tokens', COALESCE(ai_usage.output_tokens, 0),
                      'cost_micros', COALESCE(ai_usage.cost_micros, 0)
                    ) AS payload,
                    jsonb_build_object(
                      'project_id', r.project_id,
                      'automation_run_id', r.id,
                      'definition_id', r.definition_id,
                      'job_id', r.job_id,
                      'initiator_actor_kind', r.actor_kind,
                      'initiator_actor_id', r.actor_id,
                      'linked_ai_run_ids', COALESCE(ai_usage.ai_run_ids, '[]'::jsonb),
                      'ai_linkage_scope', 'automation_run_parent',
                      'provenance_kind', 'automation_run_record'
                    ) AS provenance
             FROM automation_runs AS r
             JOIN jobs AS j
               ON j.project_id = r.project_id AND j.id = r.job_id
             LEFT JOIN LATERAL (
               SELECT count(*) AS ai_run_count,
                      jsonb_agg(a.id ORDER BY a.created_at, a.id) AS ai_run_ids,
                      sum(a.input_tokens) AS input_tokens,
                      sum(a.output_tokens) AS output_tokens,
                      sum(a.cost_micros) AS cost_micros
               FROM ai_runs AS a
               WHERE a.project_id = r.project_id
                 AND a.parent_automation_run_id = r.id
             ) AS ai_usage ON true
             WHERE r.project_id = $1
             UNION ALL
             SELECT j.id, j.created_at, 'automation_job_snapshot' AS event_type,
                    'automation_job' AS aggregate_type, j.id AS aggregate_id,
                    NULL::text AS actor_kind, NULL::text AS actor_id,
                    NULL::uuid AS protocol_version_id, 'automation_job'::text AS stage,
                    NULL::text AS decision, NULL::uuid AS reason_id,
                    'durable_snapshot'::text AS event_kind,
                    NULL::uuid AS supersedes_event_id, NULL::uuid AS undoes_event_id,
                    '{}'::jsonb AS previous_snapshot,
                    jsonb_build_object(
                      'snapshot_kind', 'durable_current_record',
                      'kind', j.kind,
                      'state', j.state,
                      'attempts', j.attempts,
                      'max_attempts', j.max_attempts,
                      'completed_at', j.completed_at
                    ) AS result_snapshot,
                    NULL::text AS notes,
                    jsonb_build_object(
                      'snapshot_kind', 'durable_current_record',
                      'kind', j.kind,
                      'state', j.state,
                      'attempts', j.attempts,
                      'max_attempts', j.max_attempts,
                      'completed_at', j.completed_at
                    ) AS payload,
                    jsonb_build_object(
                      'project_id', j.project_id,
                      'job_id', j.id,
                      'provenance_kind', 'automation_job_record'
                    ) AS provenance
             FROM jobs AS j
             WHERE j.project_id = $1 AND j.kind = 'automation_run'
             UNION ALL
             SELECT s.id, r.created_at, 'automation_step_snapshot' AS event_type,
                    'automation_step_run' AS aggregate_type, s.id AS aggregate_id,
                    r.actor_kind, r.actor_id, NULL::uuid AS protocol_version_id,
                    s.step_kind AS stage, NULL::text AS decision,
                    NULL::uuid AS reason_id, 'durable_snapshot'::text AS event_kind,
                    NULL::uuid AS supersedes_event_id, NULL::uuid AS undoes_event_id,
                    '{}'::jsonb AS previous_snapshot,
                    jsonb_build_object(
                      'snapshot_kind', 'durable_current_record',
                      'automation_run_id', s.automation_run_id,
                      'ordinal', s.ordinal,
                      'step_key', s.step_key,
                      'step_kind', s.step_kind,
                      'status', s.status,
                      'attempts', s.attempts,
                      'linked_ai_run_ids', COALESCE(ai_usage.ai_run_ids, '[]'::jsonb),
                      'ai_linkage_scope', 'automation_run_parent'
                    ) AS result_snapshot,
                    NULL::text AS notes,
                    jsonb_build_object(
                      'snapshot_kind', 'durable_current_record',
                      'automation_run_id', s.automation_run_id,
                      'definition_id', r.definition_id,
                      'ordinal', s.ordinal,
                      'step_key', s.step_key,
                      'step_kind', s.step_kind,
                      'status', s.status,
                      'attempts', s.attempts,
                      'ai_run_count', COALESCE(ai_usage.ai_run_count, 0),
                      'linked_ai_run_ids', COALESCE(ai_usage.ai_run_ids, '[]'::jsonb),
                      'input_tokens', COALESCE(ai_usage.input_tokens, 0),
                      'output_tokens', COALESCE(ai_usage.output_tokens, 0),
                      'cost_micros', COALESCE(ai_usage.cost_micros, 0),
                      'ai_linkage_scope', 'automation_run_parent'
                    ) AS payload,
                    jsonb_build_object(
                      'project_id', s.project_id,
                      'automation_step_run_id', s.id,
                      'automation_run_id', s.automation_run_id,
                      'definition_id', r.definition_id,
                      'initiator_actor_kind', r.actor_kind,
                      'initiator_actor_id', r.actor_id,
                      'created_at_source', 'automation_run.created_at',
                      'linked_ai_run_ids', COALESCE(ai_usage.ai_run_ids, '[]'::jsonb),
                      'ai_linkage_scope', 'automation_run_parent',
                      'provenance_kind', 'automation_step_run_record'
                    ) AS provenance
             FROM automation_step_runs AS s
             JOIN automation_runs AS r
               ON r.project_id = s.project_id AND r.id = s.automation_run_id
             LEFT JOIN LATERAL (
               SELECT count(*) AS ai_run_count,
                      jsonb_agg(a.id ORDER BY a.created_at, a.id) AS ai_run_ids,
                      sum(a.input_tokens) AS input_tokens,
                      sum(a.output_tokens) AS output_tokens,
                      sum(a.cost_micros) AS cost_micros
               FROM ai_runs AS a
               WHERE a.project_id = r.project_id
                 AND a.parent_automation_run_id = r.id
             ) AS ai_usage ON true
             WHERE s.project_id = $1
             UNION ALL
             SELECT m.automation_run_id, m.created_at,
                    'review_run_manifest' AS event_type,
                    'review_run' AS aggregate_type,
                    m.automation_run_id AS aggregate_id,
                    r.actor_kind, r.actor_id, NULL::uuid AS protocol_version_id,
                    m.definition_key AS stage, NULL::text AS decision,
                    NULL::uuid AS reason_id, 'immutable_manifest'::text AS event_kind,
                    NULL::uuid AS supersedes_event_id, NULL::uuid AS undoes_event_id,
                    '{}'::jsonb AS previous_snapshot,
                    jsonb_strip_nulls(jsonb_build_object(
                      'state', m.state,
                      'state_code', m.state_code,
                      'state_message', m.state_message,
                      'candidate_hash', m.candidate_hash,
                      'proposal_id', m.proposal_id,
                      'started_at', m.started_at,
                      'finished_at', m.finished_at
                    )) AS result_snapshot,
                    m.state_message AS notes,
                    jsonb_build_object(
                      'definition_key', m.definition_key,
                      'definition_id', m.definition_id,
                      'definition_version', m.definition_version,
                      'manifest_hash', m.manifest_hash,
                      'semantic_bundle_hash', m.semantic_bundle_hash,
                      'manifest', m.manifest,
                      'subject', m.subject,
                      'origin', m.origin
                    ) AS payload,
                    jsonb_strip_nulls(jsonb_build_object(
                      'project_id', m.project_id,
                      'automation_run_id', m.automation_run_id,
                      'proposal_id', m.proposal_id,
                      'calibration_bundle_id', m.origin->>'calibration_bundle_id',
                      'provenance_kind', 'review_run_manifest'
                    )) AS provenance
             FROM review_run_manifests AS m
             JOIN automation_runs AS r
               ON r.project_id=m.project_id AND r.id=m.automation_run_id
             WHERE m.project_id=$1
             UNION ALL
             SELECT a.id, a.started_at, 'review_step_attempt' AS event_type,
                    'review_step_attempt' AS aggregate_type, a.id AS aggregate_id,
                    NULL::text AS actor_kind, a.worker_id AS actor_id,
                    NULL::uuid AS protocol_version_id, a.node_id AS stage,
                    NULL::text AS decision, NULL::uuid AS reason_id,
                    a.status AS event_kind, NULL::uuid AS supersedes_event_id,
                    NULL::uuid AS undoes_event_id, '{}'::jsonb AS previous_snapshot,
                    jsonb_strip_nulls(jsonb_build_object(
                      'status', a.status,
                      'artifact_id', a.artifact_id,
                      'model_run_id', a.model_run_id,
                      'error_code', a.error_code,
                      'error_message', a.error_message,
                      'finished_at', a.finished_at,
                      'accepted_at', a.accepted_at
                    )) AS result_snapshot,
                    a.error_message AS notes,
                    jsonb_strip_nulls(jsonb_build_object(
                      'automation_run_id', a.automation_run_id,
                      'node_id', a.node_id,
                      'node_version', a.node_version,
                      'attempt_number', a.attempt_number,
                      'input_fingerprint', a.input_fingerprint,
                      'status', a.status,
                      'worker_id', a.worker_id,
                      'artifact_id', a.artifact_id,
                      'model_run_id', a.model_run_id
                    )) AS payload,
                    jsonb_strip_nulls(jsonb_build_object(
                      'project_id', a.project_id,
                      'automation_run_id', a.automation_run_id,
                      'attempt_id', a.id,
                      'artifact_id', a.artifact_id,
                      'model_run_id', a.model_run_id,
                      'predecessor_artifact_ids', COALESCE(lineage.predecessors, '[]'::jsonb),
                      'provenance_kind', 'review_step_attempt'
                    )) AS provenance
             FROM review_step_attempts AS a
             LEFT JOIN LATERAL (
               SELECT jsonb_agg(l.predecessor_artifact_id ORDER BY l.predecessor_artifact_id)
                        AS predecessors
               FROM review_artifact_lineage AS l
               WHERE l.project_id=a.project_id AND l.artifact_id=a.artifact_id
             ) AS lineage ON true
             WHERE a.project_id=$1
             UNION ALL
             SELECT artifact.id, artifact.created_at, 'review_artifact' AS event_type,
                    'review_artifact' AS aggregate_type, artifact.id AS aggregate_id,
                    NULL::text AS actor_kind, NULL::text AS actor_id,
                    NULL::uuid AS protocol_version_id, artifact.media_type AS stage,
                    NULL::text AS decision, NULL::uuid AS reason_id,
                    'content_addressed'::text AS event_kind,
                    NULL::uuid AS supersedes_event_id, NULL::uuid AS undoes_event_id,
                    '{}'::jsonb AS previous_snapshot,
                    jsonb_build_object(
                      'content_hash', artifact.content_hash,
                      'media_type', artifact.media_type
                    ) AS result_snapshot,
                    NULL::text AS notes,
                    jsonb_build_object(
                      'content_hash', artifact.content_hash,
                      'media_type', artifact.media_type
                    ) AS payload,
                    jsonb_build_object(
                      'project_id', artifact.project_id,
                      'artifact_id', artifact.id,
                      'predecessor_artifact_ids', COALESCE(lineage.predecessors, '[]'::jsonb),
                      'provenance_kind', 'review_artifact_lineage'
                    ) AS provenance
             FROM review_artifacts AS artifact
             LEFT JOIN LATERAL (
               SELECT jsonb_agg(l.predecessor_artifact_id ORDER BY l.predecessor_artifact_id)
                        AS predecessors
               FROM review_artifact_lineage AS l
               WHERE l.project_id=artifact.project_id AND l.artifact_id=artifact.id
             ) AS lineage ON true
             WHERE artifact.project_id=$1
             UNION ALL
             SELECT c.id, c.created_at, 'review_calibration_bundle' AS event_type,
                    'review_calibration_bundle' AS aggregate_type, c.id AS aggregate_id,
                    NULL::text AS actor_kind,
                    c.reviewer_metadata->>'reviewer_id' AS actor_id,
                    NULL::uuid AS protocol_version_id, c.definition_key AS stage,
                    c.status AS decision, NULL::uuid AS reason_id,
                    'immutable_calibration'::text AS event_kind,
                    NULL::uuid AS supersedes_event_id, NULL::uuid AS undoes_event_id,
                    '{}'::jsonb AS previous_snapshot,
                    jsonb_build_object(
                      'status', c.status,
                      'metrics', c.metrics,
                      'evaluated_at', c.evaluated_at
                    ) AS result_snapshot,
                    NULL::text AS notes,
                    jsonb_build_object(
                      'definition_key', c.definition_key,
                      'semantic_bundle_hash', c.semantic_bundle_hash,
                      'evaluation_set_id', c.evaluation_set_id,
                      'thresholds', c.thresholds,
                      'metrics', c.metrics,
                      'reviewer_metadata', c.reviewer_metadata,
                      'status', c.status,
                      'evaluated_at', c.evaluated_at
                    ) AS payload,
                    jsonb_build_object(
                      'project_id', c.project_id,
                      'calibration_bundle_id', c.id,
                      'semantic_bundle_hash', c.semantic_bundle_hash,
                      'provenance_kind', 'review_calibration_bundle'
                    ) AS provenance
             FROM review_calibration_bundles AS c
             WHERE c.project_id=$1
             UNION ALL
             SELECT p.id, p.resolved_at, 'reviewer_proposal_decision' AS event_type,
                    'ai_proposal' AS aggregate_type, p.id AS aggregate_id,
                    p.resolved_by_actor_kind AS actor_kind,
                    p.resolved_by_actor_id AS actor_id,
                    p.protocol_version_id, p.task_kind AS stage,
                    p.status AS decision, NULL::uuid AS reason_id,
                    'reviewer_decision'::text AS event_kind,
                    NULL::uuid AS supersedes_event_id, NULL::uuid AS undoes_event_id,
                    jsonb_build_object('status', 'pending') AS previous_snapshot,
                    jsonb_build_object(
                      'status', p.status,
                      'resolved_at', p.resolved_at
                    ) AS result_snapshot,
                    p.resolution_reason AS notes,
                    jsonb_build_object(
                      'proposal_id', p.id,
                      'status', p.status,
                      'resolution_reason', p.resolution_reason
                    ) AS payload,
                    jsonb_build_object(
                      'project_id', p.project_id,
                      'proposal_id', p.id,
                      'model_run_id', p.model_run_id,
                      'reviewer_actor_kind', p.resolved_by_actor_kind,
                      'reviewer_actor_id', p.resolved_by_actor_id,
                      'provenance_kind', 'reviewer_proposal_decision'
                    ) AS provenance
             FROM ai_proposals AS p
             WHERE p.project_id=$1 AND p.resolved_at IS NOT NULL
           ) events
           ORDER BY created_at, id, event_type
           LIMIT $2"#,
    )
    .bind(project_id)
    .bind(limit)
    .fetch_all(pool)
    .await
}
