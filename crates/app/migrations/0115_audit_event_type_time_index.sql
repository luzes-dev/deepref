-- no-transaction
-- phase: expand
-- SQLx 0.9.0 detects the directive with starts_with, so it must stay first.
-- Incident-response query support (11 Part B): filter the administrative
-- audit by event type and time range. Built CONCURRENTLY, and with no
-- IF NOT EXISTS so an interrupted build fails loudly (0101's reasoning;
-- recovery is in docs/architecture.md "Migration recovery").
CREATE INDEX CONCURRENTLY admin_audit_events_event_type_time_idx
  ON admin_audit_events (event_type, occurred_at DESC, id DESC);
