-- phase: expand
-- Provider-neutral AI usage fact export (handoff 09 slice B). Export emits by
-- replaying retained usage facts: each fact's immutable id is the outbox event
-- identity, and the durable replay job runs beside `ai_usage_reconcile` in the
-- AI workload pool. No export state table exists here on purpose; the outbox
-- row keyed by the fact id is the emission record.
--
-- The N-1 classifier defaults this unrecognized kind to workflow; N claims it
-- from the AI pool using this matching durable kind mapping.
CREATE OR REPLACE FUNCTION jobs_workload_class_for_kind(kind text)
RETURNS text
LANGUAGE sql IMMUTABLE
AS $$
  SELECT CASE $1
    WHEN 'assistant_agent_run' THEN 'ai'
    WHEN 'ai_usage_reconcile' THEN 'ai'
    WHEN 'ai_usage_export_replay' THEN 'ai'
    WHEN 'retrieve_document' THEN 'documents'
    WHEN 'parse_document' THEN 'documents'
    WHEN 'run_deduplication' THEN 'documents'
    WHEN 'durable_run_step' THEN 'workflow'
    WHEN 'backfill_workflow_files' THEN 'workflow'
    WHEN 'work_fetch_requested' THEN 'integration'
    WHEN 'pmid_import' THEN 'integration'
    WHEN 'recompute_metrics' THEN 'projection'
    WHEN 'recompute_prisma' THEN 'projection'
    ELSE 'workflow'
  END
$$;

UPDATE jobs
SET workload_class = 'ai'
WHERE kind = 'ai_usage_export_replay' AND workload_class IS DISTINCT FROM 'ai';

-- Correction records export as their own immutable facts referencing the
-- original usage fact. The replay keysets corrections by (created_at, id); the
-- table is new and empty, so an ordinary build is safe.
CREATE INDEX ai_usage_corrections_created_idx
  ON ai_usage_corrections (created_at, id);

COMMENT ON COLUMN ai_usage_corrections.id IS
  'Immutable correction fact identity; also its export event identity.';
COMMENT ON COLUMN ai_usage_corrections.ledger_id IS
  'Original usage fact this correction references. The original row is never edited.';
