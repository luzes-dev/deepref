-- no-transaction
-- phase: expand
-- SQLx 0.9.0 detects the directive with starts_with, so it must stay first.
-- Incident-response query support (11 Part B): filter the administrative
-- audit by workspace and time range. Principal and target already have
-- composite indexes (0085); event type and workspace get theirs here.
-- Built CONCURRENTLY so N-1 writers are not blocked during the rollout, and
-- with no IF NOT EXISTS so an interrupted build fails loudly (0101's
-- reasoning; recovery is in docs/architecture.md "Migration recovery").
CREATE INDEX CONCURRENTLY admin_audit_events_workspace_time_idx
  ON admin_audit_events (workspace_id, occurred_at DESC, id DESC)
  WHERE workspace_id IS NOT NULL;
