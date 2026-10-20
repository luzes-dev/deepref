-- no-transaction
-- phase: expand
-- SQLx 0.9.0 detects the directive with starts_with, so it must stay first.
-- An interrupted CREATE INDEX CONCURRENTLY leaves an INVALID index behind, and
-- a retry that skips an existing name would accept a uniqueness guarantee the
-- database is not enforcing. Clear any such leftover of the denial-window index
-- before 0101 builds it. The index does not exist on a clean database, so this
-- is a no-op there.
DROP INDEX CONCURRENTLY IF EXISTS admin_audit_denial_window_idx;
