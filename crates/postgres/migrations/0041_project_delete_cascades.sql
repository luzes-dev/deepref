-- Deleting a project has to remove everything that belongs to it, in one
-- transaction. Before this migration `DELETE FROM projects` failed with a 500
-- as soon as a project had extraction values, study events or AI proposals,
-- because the project-owned foreign keys to project_reports, studies,
-- documents, ai_proposals and ai_runs were RESTRICT or NO ACTION.
--
-- Rows that belong to a project now cascade with it. The constraints that
-- protect data during normal operation are deliberately NOT changed: evidence
-- and extraction values that cite a document block, criterion judgments that
-- cite an eligibility criterion, extraction values that cite a field
-- definition, screening rows that cite an exclusion reason, and automation runs
-- that cite their definition or job keep rejecting deletes of the parent. The
-- automation-run keys become DEFERRABLE INITIALLY DEFERRED: a single delete is
-- still refused at once, while a project delete can remove the runs first and
-- the check then passes at commit. delete_project_in_transaction (crates/postgres/src/
-- project_deletion.rs) removes the other protected children first.
--
-- Each change only applies where the constraint exists, so the migration also
-- runs on partial schemas that earlier migrations build conditionally.

-- Extraction rows always belong to a project, but extraction_values.report_id
-- is nullable, so those rows would not cascade through their composite keys.
ALTER TABLE extraction_values ADD CONSTRAINT extraction_values_project_id_fkey
  FOREIGN KEY (project_id) REFERENCES projects(id) ON DELETE CASCADE;
ALTER TABLE extraction_events ADD CONSTRAINT extraction_events_project_id_fkey
  FOREIGN KEY (project_id) REFERENCES projects(id) ON DELETE CASCADE;

-- Project-owned references. No code path removes these referenced rows (ai_runs,
-- ai_proposals, protocol_versions, records, project_reports, studies, documents,
-- dedupe_proposals, project_reports) except a project delete, so cascading
-- changes nothing for normal operations.
DO $$
DECLARE
  flip record;
BEGIN
  FOR flip IN
    SELECT * FROM (VALUES
    ('ai_proposal_criterion_judgments', 'ai_proposal_criterion_judgments_protocol_version_id_fkey', 'protocol_versions', 'FOREIGN KEY (project_id, protocol_version_id) REFERENCES protocol_versions(project_id, id) ON DELETE CASCADE'),
    ('ai_proposal_evidence', 'ai_proposal_evidence_document_project_report_fkey', 'documents', 'FOREIGN KEY (project_id, report_id, document_id) REFERENCES documents(project_id, report_id, id) ON DELETE CASCADE'),
    ('ai_proposal_evidence', 'ai_proposal_evidence_project_report_fkey', 'project_reports', 'FOREIGN KEY (project_id, report_id) REFERENCES project_reports(project_id, report_id) ON DELETE CASCADE'),
    ('ai_proposals', 'ai_proposals_ai_run_id_fkey', 'ai_runs', 'FOREIGN KEY (ai_run_id) REFERENCES ai_runs(id) ON DELETE CASCADE'),
    ('ai_proposals', 'ai_proposals_model_run_project_fk', 'ai_runs', 'FOREIGN KEY (model_run_id, project_id) REFERENCES ai_runs(id, project_id) ON DELETE CASCADE'),
    ('ai_proposals', 'ai_proposals_project_protocol_target_fkey', 'protocol_versions', 'FOREIGN KEY (project_id, protocol_version_id) REFERENCES protocol_versions(project_id, id) ON DELETE CASCADE'),
    ('ai_proposals', 'ai_proposals_project_record_target_fkey', 'records', 'FOREIGN KEY (project_id, target_record_id) REFERENCES records(project_id, id) ON DELETE CASCADE'),
    ('ai_proposals', 'ai_proposals_project_report_target_fkey', 'project_reports', 'FOREIGN KEY (project_id, target_report_id) REFERENCES project_reports(project_id, report_id) ON DELETE CASCADE'),
    ('ai_proposals', 'ai_proposals_project_study_target_fkey', 'studies', 'FOREIGN KEY (project_id, target_study_id) REFERENCES studies(project_id, id) ON DELETE CASCADE'),
    ('ai_run_evidence', 'ai_run_evidence_project_id_document_id_fkey', 'documents', 'FOREIGN KEY (project_id, document_id) REFERENCES documents(project_id, id) ON DELETE CASCADE'),
    ('extraction_events', 'extraction_events_project_id_proposal_id_fkey', 'ai_proposals', 'FOREIGN KEY (project_id, proposal_id) REFERENCES ai_proposals(project_id, id) ON DELETE CASCADE'),
    ('extraction_events', 'extraction_events_project_id_study_id_fkey', 'studies', 'FOREIGN KEY (project_id, study_id) REFERENCES studies(project_id, id) ON DELETE CASCADE'),
    ('automation_runs', 'automation_runs_project_id_definition_id_fkey', 'automation_definitions', 'FOREIGN KEY (project_id, definition_id) REFERENCES automation_definitions(project_id, id) DEFERRABLE INITIALLY DEFERRED'),
    ('automation_runs', 'automation_runs_project_id_job_id_fkey', 'jobs', 'FOREIGN KEY (project_id, job_id) REFERENCES jobs(project_id, id) DEFERRABLE INITIALLY DEFERRED'),
    ('dedupe_resolution_events', 'dedupe_resolution_events_proposal_project_fkey', 'dedupe_proposals', 'FOREIGN KEY (project_id, proposal_id) REFERENCES dedupe_proposals(project_id, id) ON DELETE CASCADE'),
    ('dedupe_resolution_events', 'dedupe_resolution_events_reverted_event_fkey', 'dedupe_resolution_events', 'FOREIGN KEY (project_id, record_id, reverted_event_id) REFERENCES dedupe_resolution_events(project_id, record_id, id) ON DELETE CASCADE'),
    ('dedupe_resolution_events', 'dedupe_resolution_events_prior_project_report_fkey', 'project_reports', 'FOREIGN KEY (project_id, prior_report_id) REFERENCES project_reports(project_id, report_id) ON DELETE CASCADE'),
    ('dedupe_resolution_events', 'dedupe_resolution_events_resolved_project_report_fkey', 'project_reports', 'FOREIGN KEY (project_id, resolved_report_id) REFERENCES project_reports(project_id, report_id) ON DELETE CASCADE'),
    ('extraction_values', 'extraction_values_project_id_report_id_fkey', 'project_reports', 'FOREIGN KEY (project_id, report_id) REFERENCES project_reports(project_id, report_id) ON DELETE CASCADE'),
    ('extraction_values', 'extraction_values_project_id_study_id_fkey', 'studies', 'FOREIGN KEY (project_id, study_id) REFERENCES studies(project_id, id) ON DELETE CASCADE'),
    ('extraction_values', 'extraction_values_project_id_report_id_source_document_id_fkey', 'documents', 'FOREIGN KEY (project_id, report_id, source_document_id) REFERENCES documents(project_id, report_id, id) ON DELETE CASCADE'),
    ('study_events', 'study_events_project_id_report_id_fkey', 'project_reports', 'FOREIGN KEY (project_id, report_id) REFERENCES project_reports(project_id, report_id) ON DELETE CASCADE'),
    ('study_events', 'study_events_project_id_before_study_id_fkey', 'studies', 'FOREIGN KEY (project_id, before_study_id) REFERENCES studies(project_id, id) ON DELETE CASCADE'),
    ('study_events', 'study_events_project_id_result_study_id_fkey', 'studies', 'FOREIGN KEY (project_id, result_study_id) REFERENCES studies(project_id, id) ON DELETE CASCADE')
    ) AS v(tbl, conname, ref_tbl, definition)
  LOOP
    IF to_regclass(flip.tbl) IS NOT NULL
       AND to_regclass(flip.ref_tbl) IS NOT NULL
       AND EXISTS (
         SELECT 1 FROM pg_constraint
         WHERE conname = flip.conname AND conrelid = to_regclass(flip.tbl)
       )
    THEN
      EXECUTE format('ALTER TABLE %I DROP CONSTRAINT %I', flip.tbl, flip.conname);
      EXECUTE format('ALTER TABLE %I ADD CONSTRAINT %I %s', flip.tbl, flip.conname, flip.definition);
    END IF;
  END LOOP;
END $$;
