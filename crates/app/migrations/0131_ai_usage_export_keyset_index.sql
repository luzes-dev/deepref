-- no-transaction
-- phase: expand
-- The export replay keysets retained usage facts by (settled_at, id) to emit
-- missing outbox events. This relation carries the five-year billing history,
-- so the keyset index is built concurrently.
CREATE INDEX CONCURRENTLY IF NOT EXISTS ai_usage_ledger_settle_keyset_idx
  ON ai_usage_ledger (created_at, id);
