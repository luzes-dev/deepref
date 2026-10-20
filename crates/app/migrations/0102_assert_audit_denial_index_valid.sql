-- phase: expand
-- The denial audit bound rests on admin_audit_denial_window_idx (0101). Refuse
-- to continue unless it exists and is valid: a missing or INVALID unique index
-- enforces nothing.
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
      'admin_audit_denial_window_idx is missing or invalid; run DROP INDEX CONCURRENTLY IF EXISTS admin_audit_denial_window_idx and re-run the migrations';
  END IF;
END
$$;
