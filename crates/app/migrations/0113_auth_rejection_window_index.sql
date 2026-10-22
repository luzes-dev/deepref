-- no-transaction
-- phase: expand
-- SQLx 0.9.0 detects the directive with starts_with, so it must stay first.
-- Bound authentication-rejection auditing in the database, the same way 0100
-- bounds authorization denials. Authentication rejections are
-- attacker-controlled, so the table holds at most one row per
-- (stage, reason, target user, one-minute window) and the writer inserts with
-- ON CONFLICT DO NOTHING. The target is NULL for floods with no stored
-- session behind them, so an attacker's random cookies cannot create new
-- keys; a target is only ever a user id read from a stored session row.
--
-- Built CONCURRENTLY so N-1 writers are not blocked during the rollout; the
-- partial predicate means it indexes no row that exists today.
--
-- Deliberately no IF NOT EXISTS: an interrupted build leaves an INVALID index
-- of this name, and a retry must fail loudly rather than skip it. Recovery is in
-- docs/architecture.md ("Migration recovery").
CREATE UNIQUE INDEX CONCURRENTLY admin_audit_auth_rejection_window_idx
  ON admin_audit_events (
    (metadata->>'stage'),
    (metadata->>'reason'),
    (COALESCE(target_user_id, '00000000-0000-0000-0000-000000000000'::uuid)),
    (metadata->>'window')
  )
  WHERE event_type = 'auth.rejected';
