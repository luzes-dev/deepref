-- 05a: typed job scope and workload pools.
--
-- The jobs table gains a typed scope (project | workspace | platform) and a
-- workload class derived from the job kind at enqueue. Claiming filters on
-- workload class, not on `project_id IS NOT NULL`, so workspace- and
-- platform-scoped work runs without a fabricated project. `project_id` stays
-- the project reference for project-scoped work only.
--
-- Scope columns are `{scope, project_id, workspace_id}` and the consistency
-- CHECK pins them together. `workspace_id` is a typed tenant reference without
-- a foreign key: the workspace tables arrive with 03a.

ALTER TABLE jobs
  ADD COLUMN IF NOT EXISTS workload_class text,
  ADD COLUMN IF NOT EXISTS scope text NOT NULL DEFAULT 'project',
  ADD COLUMN IF NOT EXISTS workspace_id uuid;

-- Backfill the class every existing kind maps to (the same mapping as
-- `JobKind::workload_class` in crates/domain/src/jobs.rs; docs/architecture.md
-- carries the normative table). A kind outside that mapping cannot be
-- classified; it lands in the generic `workflow` pool so the row stays
-- claimable and fails visibly at dispatch instead of becoming invisible.
UPDATE jobs
SET workload_class = CASE kind
    WHEN 'assistant_agent_run' THEN 'ai'
    WHEN 'retrieve_document' THEN 'documents'
    WHEN 'parse_document' THEN 'documents'
    WHEN 'run_deduplication' THEN 'documents'
    WHEN 'durable_run_step' THEN 'workflow'
    WHEN 'work_fetch_requested' THEN 'integration'
    WHEN 'pmid_import' THEN 'integration'
    WHEN 'recompute_metrics' THEN 'projection'
    WHEN 'recompute_prisma' THEN 'projection'
    ELSE 'workflow'
  END
WHERE workload_class IS NULL;

ALTER TABLE jobs
  ALTER COLUMN workload_class SET NOT NULL;

ALTER TABLE jobs
  DROP CONSTRAINT IF EXISTS jobs_workload_class_check;
ALTER TABLE jobs
  ADD CONSTRAINT jobs_workload_class_check
  CHECK (workload_class IN ('ai', 'documents', 'workflow', 'integration', 'projection'));

-- Scope consistency. `project` rows carry their project (the historical
-- `jobs_active_project_id_check` rule, folded in here) and no workspace;
-- `workspace` rows carry their workspace and no project; `platform` rows
-- carry neither. NOT VALID mirrors the 0019 precedent for this table: the
-- constraint governs every new write while tolerating historical rows that
-- predate explicit scope.
ALTER TABLE jobs
  DROP CONSTRAINT IF EXISTS jobs_active_project_id_check;
ALTER TABLE jobs
  DROP CONSTRAINT IF EXISTS jobs_scope_check;
ALTER TABLE jobs
  ADD CONSTRAINT jobs_scope_check
  CHECK (
    (scope = 'project'
      AND workspace_id IS NULL
      AND (project_id IS NOT NULL OR state IN ('completed', 'failed', 'dead')))
    OR (scope = 'workspace'
      AND project_id IS NULL
      AND workspace_id IS NOT NULL)
    OR (scope = 'platform'
      AND project_id IS NULL
      AND workspace_id IS NULL)
  ) NOT VALID;

-- Pool-scoped claim scans: eligible work per workload class in due order.
CREATE INDEX IF NOT EXISTS jobs_claim_pool_idx
  ON jobs (workload_class, priority DESC, available_at ASC)
  WHERE state = 'queued';
