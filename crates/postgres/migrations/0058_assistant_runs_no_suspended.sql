-- Assistant agent runs drop the `suspended` status: it never had a producer
-- (no code path ever wrote it) and run observers treat only
-- completed/failed/cancelled as terminal, so a suspended run would poll
-- forever. The lifecycle is queued -> running ->
-- completed/failed/cancelled. The original CHECK from 0057 stays untouched
-- as a harmless superset; this additional constraint enforces the tightened
-- set without renaming any existing constraint.
ALTER TABLE assistant_agent_runs
  ADD CONSTRAINT assistant_agent_runs_status_no_suspended
  CHECK (status IN ('queued', 'running', 'completed', 'failed', 'cancelled'));
