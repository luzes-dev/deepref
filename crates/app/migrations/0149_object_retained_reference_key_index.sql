-- no-transaction
-- phase: expand
-- Cleanup looks up retained references by object, independently of the
-- export-first primary key used when releasing an entire retained download.
CREATE INDEX CONCURRENTLY IF NOT EXISTS object_retained_references_storage_expiry_idx
  ON object_retained_references(storage_key, expires_at);
