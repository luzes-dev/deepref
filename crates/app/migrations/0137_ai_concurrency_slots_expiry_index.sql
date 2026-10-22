-- no-transaction
-- phase: expand
-- The periodic sweep deletes rows whose lease expired long ago; it finds them
-- by expiry alone. Built concurrently per the index convention, so rolling
-- deploys never take a write lock on the admission path.
CREATE INDEX CONCURRENTLY IF NOT EXISTS ai_concurrency_slots_expiry_idx
  ON ai_concurrency_slots (lease_expires_at);
