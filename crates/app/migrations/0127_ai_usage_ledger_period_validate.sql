-- phase: contract
-- The bounded ai-usage backfill must complete before this migration is run.
-- Validation scans existing rows without rewriting the ledger; setting the
-- column NOT NULL is deferred to a later contract migration.
ALTER TABLE ai_usage_ledger
  VALIDATE CONSTRAINT ai_usage_ledger_period_start_present_check;
