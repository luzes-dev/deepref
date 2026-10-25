-- no-transaction
-- phase: expand
CREATE INDEX CONCURRENTLY IF NOT EXISTS jobs_unemitted_wakeup_idx
  ON jobs (available_at,id)
  WHERE state IN ('queued','capability_wait') AND wakeup_emitted_generation<wakeup_generation;
