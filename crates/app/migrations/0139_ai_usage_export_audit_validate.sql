-- phase: expand
-- Audit widening has no historical backfill dependency.
ALTER TABLE admin_audit_events VALIDATE CONSTRAINT admin_audit_events_event_type_check;
ALTER TABLE admin_audit_events VALIDATE CONSTRAINT admin_audit_events_check;
ALTER TABLE jobs VALIDATE CONSTRAINT jobs_state_check;
