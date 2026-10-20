-- no-transaction
-- phase: expand
-- SQLx 0.9.0 detects the directive with starts_with, so it must stay first.
-- Build the `projects (workspace_id)` lookup index without blocking N-1
-- reads and writes during the rollout. It was removed from 0087, whose
-- ALTER TABLE lock it would have extended for the whole scan.
CREATE INDEX CONCURRENTLY IF NOT EXISTS projects_workspace_idx
  ON projects (workspace_id);
