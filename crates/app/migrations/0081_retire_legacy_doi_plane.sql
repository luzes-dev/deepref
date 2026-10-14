-- Deploy with the canonical acquisition worker/API; no legacy producers may run.
ALTER TABLE citations DROP CONSTRAINT citations_first_seen_ingestion_id_fkey1;
ALTER TABLE citations RENAME COLUMN first_seen_ingestion_id TO acquisition_run_id;
ALTER TABLE citations ADD CONSTRAINT citations_acquisition_run_id_fkey
 FOREIGN KEY(acquisition_run_id) REFERENCES acquisition_runs(id) ON DELETE SET NULL;
ALTER TABLE citations DROP COLUMN legacy_source_doi, DROP COLUMN legacy_target_doi;
ALTER TABLE acquisition_runs DROP COLUMN legacy_ingestion_id;
ALTER TABLE acquisition_runs ALTER COLUMN strategy SET DEFAULT 'citation_traversal';

-- Preserve the established ranking algorithm; only its revision authority changes.
-- Lock the project before calculating so concurrent requests and recomputations
-- cannot commit an older completed watermark over a newer one.
DO $$
DECLARE definition text;
BEGIN
 SELECT pg_get_functiondef('recompute_project_report_metrics(uuid)'::regprocedure) INTO definition;
 definition := replace(definition,
 'BEGIN' || chr(10) || '  UPDATE project_reports pr',
 'BEGIN' || chr(10) || '  PERFORM 1 FROM projects WHERE id=target_project_id FOR UPDATE;' || chr(10) || '  UPDATE project_reports pr');
 definition := replace(definition,
 'SELECT COALESCE(MAX(revision), 0)' || chr(10) || '  INTO current_revision' || chr(10) || '  FROM domain_events' || chr(10) || '  WHERE entity_type = ''metric'' AND entity_key = target_project_id::text;',
 'SELECT metric_requested_revision INTO current_revision FROM projects WHERE id=target_project_id;');
 IF position('FROM domain_events' IN definition)>0 THEN
  RAISE EXCEPTION 'Unexpected metric function definition: revision authority not replaced';
 END IF;
 EXECUTE definition;
END $$;

DROP FUNCTION IF EXISTS recompute_project_metrics(uuid);
DROP TABLE record_provenance;
DROP TABLE project_works;
DROP TABLE legacy_citations;
DROP TABLE unresolved_references;
DROP TABLE ingestion_items;
DROP TABLE ingestions;
DROP TABLE doi_fetch_state;
ALTER TABLE works DROP COLUMN fetch_status;
DROP TABLE processed_events;
DROP TABLE dead_letter_records;
DROP TABLE domain_tombstones;
DROP TABLE domain_events;
DROP SEQUENCE graph_domain_revision_seq;
