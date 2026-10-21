-- phase: expand
-- Validate the append-only AI quota audit shapes and the expanded usage
-- ledger constraints after 0120/0121 installed them without blocking writes.
ALTER TABLE admin_audit_events VALIDATE CONSTRAINT admin_audit_events_event_type_check;
ALTER TABLE admin_audit_events VALIDATE CONSTRAINT admin_audit_events_check;
ALTER TABLE ai_usage_ledger VALIDATE CONSTRAINT ai_usage_ledger_purpose_check;
ALTER TABLE ai_usage_ledger VALIDATE CONSTRAINT ai_usage_ledger_reasoning_tokens_check;
ALTER TABLE ai_usage_ledger VALIDATE CONSTRAINT ai_usage_ledger_commercial_units_check;
ALTER TABLE ai_usage_ledger VALIDATE CONSTRAINT ai_usage_ledger_principal_check;
ALTER TABLE ai_usage_ledger VALIDATE CONSTRAINT ai_usage_ledger_step_id_check;
ALTER TABLE ai_usage_ledger ALTER COLUMN period_start SET NOT NULL;
