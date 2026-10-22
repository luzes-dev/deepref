-- phase: expand
-- Workspace-scoped AI usage recovery runs in the 05a AI workload pool. An
-- N-1 classifier defaults this unrecognized kind to workflow, while N claims
-- it from the AI pool using this matching durable kind mapping.
CREATE OR REPLACE FUNCTION jobs_workload_class_for_kind(kind text)
RETURNS text
LANGUAGE sql IMMUTABLE
AS $$
  SELECT CASE $1
    WHEN 'assistant_agent_run' THEN 'ai'
    WHEN 'ai_usage_reconcile' THEN 'ai'
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
WHERE kind = 'ai_usage_reconcile' AND workload_class IS DISTINCT FROM 'ai';
