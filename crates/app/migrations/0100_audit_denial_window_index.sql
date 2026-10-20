-- no-transaction
-- phase: expand
-- SQLx 0.9.0 detects the directive with starts_with, so it must stay first.
-- Bound authorization-denial auditing in the database. The audit table is
-- append-only, so a denial cannot be counted into an existing row; instead
-- the table holds at most one denial row per user, permission, resource type
-- and one-minute window, and the writer inserts with ON CONFLICT DO NOTHING.
-- Built CONCURRENTLY so N-1 writers are not blocked during the rollout; the
-- partial predicate means it indexes no row that exists today.
--
-- Deliberately no IF NOT EXISTS: an interrupted build leaves an INVALID index
-- of this name, and a retry must fail loudly rather than skip it. Recovery is in
-- docs/architecture.md ("Migration recovery").
CREATE UNIQUE INDEX CONCURRENTLY admin_audit_denial_window_idx
  ON admin_audit_events (
    principal_user_id,
    (metadata->>'permission'),
    (metadata->>'resource_type'),
    (metadata->>'window')
  )
  WHERE event_type = 'authorization.denied';
