-- no-transaction
-- phase: expand
CREATE INDEX CONCURRENTLY object_privacy_pending
  ON object_lifecycle(privacy_due_at, storage_key)
  WHERE privacy_due_at IS NOT NULL AND deleted_at IS NULL;
