-- phase: expand
-- The denial audit bound rests on admin_audit_denial_window_idx (0100). Refuse
-- to continue unless it exists and is valid, ready and unique: an INVALID
-- unique index enforces nothing.
DO $$
BEGIN
  IF NOT EXISTS (
    SELECT 1
    FROM pg_index i
    JOIN pg_class c ON c.oid = i.indexrelid
    WHERE c.relname = 'admin_audit_denial_window_idx'
      AND i.indisvalid AND i.indisready AND i.indisunique
  ) THEN
    RAISE EXCEPTION
      'admin_audit_denial_window_index is missing or invalid; see "Migration recovery" in docs/architecture.md';
  END IF;
END
$$;
