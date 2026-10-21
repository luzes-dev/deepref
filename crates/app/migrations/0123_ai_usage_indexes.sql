-- no-transaction
-- phase: expand
-- Reservation recovery scans are built concurrently because this relation
-- grows on the production hot path.
CREATE INDEX CONCURRENTLY ai_usage_reservations_workspace_open_idx
  ON ai_usage_reservations (workspace_id, opened_at)
  WHERE state IN ('open', 'observed', 'unresolved');
