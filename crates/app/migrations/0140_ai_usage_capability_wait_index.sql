-- no-transaction
-- phase: expand
-- N claims both ordinary queued and capability-protected pending work.
CREATE INDEX CONCURRENTLY IF NOT EXISTS jobs_claim_capability_wait_idx
  ON jobs (workload_class, priority DESC, available_at ASC)
  WHERE state = 'capability_wait';
