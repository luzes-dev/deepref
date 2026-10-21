-- no-transaction
-- phase: expand
-- SQLx 0.9.0 detects the directive with starts_with, so it must stay first.
-- Build the pool claim index without blocking N-1 claims during rollout.
CREATE INDEX CONCURRENTLY IF NOT EXISTS jobs_claim_pool_idx
  ON jobs (workload_class, priority DESC, available_at ASC)
  WHERE state = 'queued';
