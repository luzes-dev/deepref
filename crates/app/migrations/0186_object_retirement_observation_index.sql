-- no-transaction
-- phase: expand
-- Bound the periodic oldest-retirement/count scan to unfinished tombstones.
CREATE INDEX CONCURRENTLY IF NOT EXISTS object_lifecycle_unfinished_retirement_idx
  ON object_lifecycle(retired_at)
  WHERE retired_at IS NOT NULL AND deleted_at IS NULL;
