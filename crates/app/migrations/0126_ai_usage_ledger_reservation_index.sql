-- no-transaction
-- phase: expand
-- A reservation produces at most one immutable usage fact, including retries
-- of settlement after a failed ledger/outbox transaction.
CREATE UNIQUE INDEX CONCURRENTLY ai_usage_ledger_reservation_unique_idx
  ON ai_usage_ledger (reservation_id)
  WHERE reservation_id IS NOT NULL;
