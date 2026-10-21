-- phase: expand
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
--
-- Rolling deploys (docs/saas-program/12_OPERATIONS_ACCEPTANCE.md, "Migrations
-- and rolling deploys"): binaries N and N-1 share this schema in both
-- directions, so this migration is expand-phase only. `workload_class` stays
-- nullable because an N-1 enqueue inserts without it; the fill trigger below
-- classifies those rows exactly like the N writers do. A later contract
-- migration batches terminal history before adding NOT NULL, once no N-1
-- writer remains. This migration classifies active rows only because the
-- queued/running set is small; terminal history is never claimed and remains
-- NULL until that contract migration. N-1 workers claim with
-- `project_id IS NOT NULL` and know nothing of pools, so workspace- and
-- platform-scoped jobs created by N (whose `project_id` is NULL) are invisible
-- to them and are served only by N workers, exactly as intended.

-- The old 0019 CHECK is NOT VALID, so historical active orphans can exist even
-- though new writes cannot create them. Terminalize these before touching
-- workload_class: updating them while active would violate that CHECK.
UPDATE jobs
SET state = 'dead',
    last_error = 'quarantined by 0086: active job without project',
    lease_owner = NULL,
    leased_until = NULL,
    lease_renewed_at = NULL
WHERE state IN ('queued', 'running')
  AND project_id IS NULL;

ALTER TABLE jobs
  ADD COLUMN IF NOT EXISTS workload_class text,
  ADD COLUMN IF NOT EXISTS scope text NOT NULL DEFAULT 'project',
  ADD COLUMN IF NOT EXISTS workspace_id uuid,
  ADD COLUMN IF NOT EXISTS claim_seq bigint NOT NULL DEFAULT 0;

-- Claim identity is independent of the refundable business attempt counter.
-- This trigger also advances identity for N-1 claims made during the overlap
-- window because those binaries only update state to `running`.
CREATE OR REPLACE FUNCTION jobs_increment_claim_seq()
RETURNS trigger
LANGUAGE plpgsql
AS $$
BEGIN
  NEW.claim_seq := OLD.claim_seq + 1;
  RETURN NEW;
END;
$$;

DROP TRIGGER IF EXISTS jobs_claim_identity_increment ON jobs;
CREATE TRIGGER jobs_claim_identity_increment
  BEFORE UPDATE OF state ON jobs
  FOR EACH ROW
  WHEN (OLD.state IS DISTINCT FROM 'running' AND NEW.state = 'running')
  EXECUTE FUNCTION jobs_increment_claim_seq();

-- Worker health needs low-cardinality capacity per serving pool. N-1
-- heartbeats keep the empty default and remain compatible with this column.
ALTER TABLE worker_heartbeats
  ADD COLUMN IF NOT EXISTS pool_capacities jsonb NOT NULL DEFAULT '{}'::jsonb;

-- One kind-to-class mapping shared by the backfill and the fill trigger below
-- (the same mapping as `JobKind::workload_class` in crates/domain/src/jobs.rs;
-- docs/architecture.md carries the normative table). A kind outside that
-- mapping cannot be classified; it lands in the generic `workflow` pool so the
-- row stays claimable and fails visibly at dispatch instead of becoming
-- invisible.
CREATE OR REPLACE FUNCTION jobs_workload_class_for_kind(kind text)
RETURNS text
LANGUAGE sql IMMUTABLE
AS $$
  SELECT CASE $1
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
$$;

UPDATE jobs
SET workload_class = jobs_workload_class_for_kind(kind)
WHERE workload_class IS NULL
  AND state IN ('queued', 'running');

-- Fill `workload_class` from `kind` for every writer that omits it: N writers
-- set it at enqueue, an N-1 (previous release) enqueue leaves it NULL and is
-- classified here, so both binaries insert cleanly during the rollout
-- overlap. `UPDATE OF kind` keeps a kind swap re-classified without firing on
-- unrelated updates. NOT NULL is deferred to a later contract migration once
-- no N-1 writer remains; the CHECK below therefore tolerates NULL, and this
-- trigger is what guarantees non-null in practice.
CREATE OR REPLACE FUNCTION jobs_fill_workload_class()
RETURNS trigger
LANGUAGE plpgsql
AS $$
BEGIN
  IF NEW.workload_class IS NULL THEN
    NEW.workload_class := jobs_workload_class_for_kind(NEW.kind);
  END IF;
  RETURN NEW;
END;
$$;

DROP TRIGGER IF EXISTS jobs_workload_class_fill ON jobs;
CREATE TRIGGER jobs_workload_class_fill
  BEFORE INSERT OR UPDATE OF kind ON jobs
  FOR EACH ROW
  WHEN (NEW.workload_class IS NULL)
  EXECUTE FUNCTION jobs_fill_workload_class();

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

