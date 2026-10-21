-- no-transaction
-- phase: expand
-- Recovery correlates each reservation with its owning attempt fence.
CREATE INDEX CONCURRENTLY ai_usage_reservations_job_fence_idx
  ON ai_usage_reservations (job_id, attempt_fence)
  WHERE job_id IS NOT NULL AND state IN ('open', 'observed', 'unresolved');
