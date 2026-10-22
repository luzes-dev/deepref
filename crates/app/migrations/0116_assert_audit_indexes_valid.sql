-- phase: expand
-- The authentication-rejection audit bound rests on
-- admin_audit_auth_rejection_window_idx (0113), exactly as the denial bound
-- rests on admin_audit_denial_window_idx (0100, asserted by 0101). Refuse to
-- continue unless the indexes built CONCURRENTLY in 0113-0115 exist and are
-- valid and ready: an INVALID unique index enforces nothing, and an INVALID
-- query index leaves the incident-response query sequential.
DO $$
DECLARE
  missing text;
BEGIN
  SELECT string_agg(c.relname, ', ') INTO missing
  FROM pg_index i
  JOIN pg_class c ON c.oid = i.indexrelid
  WHERE c.relname IN (
      'admin_audit_auth_rejection_window_idx',
      'admin_audit_events_workspace_time_idx',
      'admin_audit_events_event_type_time_idx')
    AND NOT (i.indisvalid AND i.indisready);
  IF missing IS NOT NULL THEN
    RAISE EXCEPTION 'audit indexes are missing or invalid: %; see "Migration recovery" in docs/architecture.md', missing;
  END IF;
  IF NOT EXISTS (
    SELECT 1
    FROM pg_index i
    JOIN pg_class c ON c.oid = i.indexrelid
    WHERE c.relname = 'admin_audit_auth_rejection_window_idx'
      AND i.indisvalid AND i.indisready AND i.indisunique
  ) THEN
    RAISE EXCEPTION
      'admin_audit_auth_rejection_window_idx is not a valid unique index; see "Migration recovery" in docs/architecture.md';
  END IF;
END
$$;
