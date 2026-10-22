-- phase: expand
-- Capability fencing belongs at the claim authority, not in a sample of
-- recent heartbeats. N declares its decoder set transaction-locally; N-1
-- declares nothing, even if its heartbeat is stale, absent, or still writing.
-- A skipped UPDATE returns no job to its decoder and spends no attempt.
-- Keep this fence installed through application rollback.
--
-- Shared statement locks precede all job row locks. Rollout transitions take
-- the exclusive side BEFORE the rollout row lock, so enqueue, fail/retry,
-- shutdown release, and expired-lease recovery cannot republish queued work
-- after the parking pass. Shared locks do not serialize workers against one
-- another. The exclusive side is never taken while a writer holds the shared
-- side and never waits for the rollout row: transitions acquire it with
-- pg_try_advisory_xact_lock and FOR UPDATE NOWAIT, and return a typed busy
-- error after releasing their locks on contention. A rollout transition that
-- holds the exclusive side therefore never waits for anything (it must keep
-- its transactions short and must not hold job row locks before writing
-- jobs), so ordinary project/ledger/job transactions cannot deadlock on it.
ALTER TABLE jobs DROP CONSTRAINT jobs_state_check;
ALTER TABLE jobs ADD CONSTRAINT jobs_state_check CHECK (
  state IN ('queued', 'capability_wait', 'running', 'completed', 'failed', 'dead')
) NOT VALID;

CREATE FUNCTION ai_usage_jobs_write_lock() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
  PERFORM pg_advisory_xact_lock_shared(913, 9);
  RETURN NULL;
END;
$$;
CREATE TRIGGER ai_usage_jobs_write_lock_trigger
BEFORE INSERT OR UPDATE ON jobs
FOR EACH STATEMENT EXECUTE FUNCTION ai_usage_jobs_write_lock();

CREATE FUNCTION ai_usage_jobs_fence() RETURNS trigger LANGUAGE plpgsql AS $$
DECLARE
  declared_kinds text[];
BEGIN
  IF NEW.kind NOT IN ('ai_usage_reconcile', 'ai_usage_export_replay') THEN
    RETURN NEW;
  END IF;
  declared_kinds := string_to_array(current_setting('deepref.job_kinds', true), ',');
  IF NOT coalesce(NEW.kind = ANY(declared_kinds), false) THEN
    IF NEW.state = 'running' AND (TG_OP = 'INSERT' OR OLD.state <> 'running') THEN
      RETURN NULL;
    END IF;
    -- N-1 lease recovery decodes every UPDATE RETURNING kind too. Leave an
    -- expired protected lease for capable recovery, returning no unknown row.
    IF TG_OP = 'UPDATE' AND OLD.state = 'running' AND OLD.leased_until < now() THEN
      RETURN NULL;
    END IF;
  END IF;
  -- N-1 selects queued rows BEFORE LIMIT. Keep protected pending work outside
  -- that candidate set altogether, so a protected row cannot starve ordinary
  -- work in an all-N-1 fleet. Only N also selects capability_wait.
  IF NEW.state = 'queued' THEN
    NEW.state := 'capability_wait';
  END IF;
  IF NEW.kind = 'ai_usage_export_replay' AND NEW.state = 'capability_wait'
     AND NOT coalesce((SELECT ai_usage_export_activated
                       FROM ai_usage_rollout_state WHERE singleton), false) THEN
    NEW.state := 'failed';
    NEW.last_error := 'parked by ai-usage disable-export (database fence)';
    NEW.lease_owner := NULL;
    NEW.leased_until := NULL;
    NEW.lease_renewed_at := NULL;
  END IF;
  -- Recovery remains available when admission caps are suspended. N-1 cannot
  -- claim it, and a capable N recovery worker can drain retained liabilities.
  RETURN NEW;
END;
$$;
CREATE TRIGGER ai_usage_jobs_fence_trigger
BEFORE INSERT OR UPDATE ON jobs
FOR EACH ROW EXECUTE FUNCTION ai_usage_jobs_fence();

-- Harden existing queued rows too, including upgrades from a previous 09b
-- build whose application-side scheduler had already read an open gate.
UPDATE jobs SET state = 'queued'
WHERE kind IN ('ai_usage_reconcile', 'ai_usage_export_replay') AND state = 'queued';
