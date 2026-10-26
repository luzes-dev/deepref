-- phase: expand
-- 11: Self-service account data export ("Download my data").
-- Durable export request, attempt tracking, status, hash, and retained lifetime.
CREATE TABLE account_data_exports (
  id uuid PRIMARY KEY,
  user_id uuid NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  status text NOT NULL CHECK (status IN ('requested', 'processing', 'completed', 'failed', 'cancelled', 'expired')),
  attempt_id uuid,
  job_id uuid NOT NULL,
  claim_owner text,
  claim_seq bigint,
  captured_workspaces uuid[] NOT NULL DEFAULT '{}',
  CHECK ((status = 'processing') = (claim_owner IS NOT NULL AND claim_seq IS NOT NULL)),
  CHECK (status <> 'processing' OR attempt_id IS NOT NULL),
  storage_key text,
  size_bytes bigint CHECK (size_bytes >= 0),
  sha256 text CHECK (sha256 ~ '^[0-9a-f]{64}$'),
  retained_until timestamptz,
  requested_at timestamptz NOT NULL DEFAULT now(),
  processing_started_at timestamptz,
  completed_at timestamptz,
  failed_at timestamptz,
  cancelled_at timestamptz,
  error_message text,
  scope_manifest jsonb,
  CHECK (status <> 'completed' OR (storage_key IS NOT NULL AND size_bytes IS NOT NULL AND sha256 IS NOT NULL AND retained_until IS NOT NULL AND completed_at IS NOT NULL)),
  CHECK ((status = 'failed') = (failed_at IS NOT NULL AND error_message IS NOT NULL)),
  CHECK ((status = 'cancelled') = (cancelled_at IS NOT NULL))
);

CREATE UNIQUE INDEX account_data_exports_one_active ON account_data_exports(user_id) WHERE status IN ('requested','processing');

CREATE INDEX account_data_exports_user_status
  ON account_data_exports (user_id, status, requested_at DESC);

CREATE INDEX account_data_exports_retained_until
  ON account_data_exports (retained_until)
  WHERE status = 'completed';

-- Operator activation gate for retained object references.
-- Defaults to false until exact operator attestation confirms all
-- legacy eager-deleting API/worker binaries have been drained.
CREATE TABLE account_export_rollout (
  singleton boolean PRIMARY KEY DEFAULT true CHECK (singleton),
  retention_activated boolean NOT NULL DEFAULT false,
  activated_at timestamptz,
  operator_attestation text,
  CHECK (
    (NOT retention_activated AND activated_at IS NULL AND operator_attestation IS NULL)
    OR
    (retention_activated AND activated_at IS NOT NULL AND operator_attestation IS NOT NULL AND operator_attestation = 'all-eager-deleting-binaries-drained-no-restart')
  )
);
INSERT INTO account_export_rollout (singleton) VALUES (true);

-- Workload class mapping update for account_data_export (documents class).
CREATE OR REPLACE FUNCTION jobs_workload_class_for_kind(kind text)
RETURNS text LANGUAGE sql IMMUTABLE AS $$
  SELECT CASE $1
    WHEN 'assistant_agent_run' THEN 'ai'
    WHEN 'ai_usage_reconcile' THEN 'ai'
    WHEN 'ai_usage_export_replay' THEN 'ai'
    WHEN 'retrieve_document' THEN 'documents'
    WHEN 'parse_document' THEN 'documents'
    WHEN 'run_deduplication' THEN 'documents'
    WHEN 'cleanup_objects' THEN 'documents'
    WHEN 'account_data_export' THEN 'documents'
    WHEN 'durable_run_step' THEN 'workflow'
    WHEN 'backfill_workflow_files' THEN 'workflow'
    WHEN 'work_fetch_requested' THEN 'integration'
    WHEN 'pmid_import' THEN 'integration'
    WHEN 'recompute_metrics' THEN 'projection'
    WHEN 'recompute_prisma' THEN 'projection'
    ELSE 'workflow'
  END
$$;

-- Capability fencing for account_data_export. Unknown workers without
-- 'account_data_export' in deepref.job_kinds cannot steal leases, and
-- newly queued rows are transitioned to capability_wait to avoid LIMIT starvation.
CREATE FUNCTION account_export_jobs_fence() RETURNS trigger LANGUAGE plpgsql AS $$
DECLARE
  declared_kinds text[];
BEGIN
  IF NEW.kind <> 'account_data_export' THEN
    RETURN NEW;
  END IF;
  declared_kinds := string_to_array(current_setting('deepref.job_kinds', true), ',');
  IF NOT coalesce(NEW.kind = ANY(declared_kinds), false) THEN
    IF NEW.state = 'running' AND (TG_OP = 'INSERT' OR OLD.state <> 'running') THEN
      RETURN NULL;
    END IF;
    IF TG_OP = 'UPDATE' AND OLD.state = 'running' AND OLD.leased_until < now() THEN
      RETURN NULL;
    END IF;
  END IF;
  IF NEW.state = 'queued' THEN
    NEW.state := 'capability_wait';
  END IF;
  RETURN NEW;
END;
$$;

CREATE TRIGGER account_export_jobs_fence_trigger
BEFORE INSERT OR UPDATE ON jobs
FOR EACH ROW EXECUTE FUNCTION account_export_jobs_fence();

UPDATE jobs SET state = 'queued'
WHERE kind = 'account_data_export' AND state = 'queued';

-- Completed exports can only be released, never re-enter processing. Every
-- replacement processing claim uses a new immutable storage attempt identity.
CREATE FUNCTION account_export_transition_guard() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
  IF NEW.user_id <> OLD.user_id OR NEW.job_id <> OLD.job_id THEN
    RAISE EXCEPTION 'export owner and durable job are immutable';
  END IF;
  IF OLD.status IN ('failed','cancelled','expired') AND NEW.status <> OLD.status THEN
    RAISE EXCEPTION 'released export cannot be republished';
  END IF;
  IF OLD.status = 'completed' AND NEW.status NOT IN ('completed','failed','cancelled','expired') THEN
    RAISE EXCEPTION 'completed export cannot restart';
  END IF;
  IF OLD.status = 'processing' AND NEW.status = 'processing'
     AND (NEW.claim_owner,NEW.claim_seq) IS DISTINCT FROM (OLD.claim_owner,OLD.claim_seq)
     AND NEW.attempt_id IS NOT DISTINCT FROM OLD.attempt_id THEN
    RAISE EXCEPTION 'replacement claim requires a fresh attempt';
  END IF;
  RETURN NEW;
END;
$$;
CREATE TRIGGER account_export_transition_guard_trigger BEFORE UPDATE ON account_data_exports
FOR EACH ROW EXECUTE FUNCTION account_export_transition_guard();
