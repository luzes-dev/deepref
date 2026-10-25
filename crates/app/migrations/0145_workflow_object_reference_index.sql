-- no-transaction
-- phase: expand
-- Cleanup probes global references after project metadata has disappeared.
CREATE INDEX CONCURRENTLY IF NOT EXISTS workflow_files_storage_key_idx
  ON workflow_files(storage_key) WHERE storage_key IS NOT NULL;
