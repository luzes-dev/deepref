-- phase: expand
-- Validates the event and metadata checks 0141 added NOT VALID, under a lock
-- that allows existing audit rows to remain readable and writable.
ALTER TABLE admin_audit_events VALIDATE CONSTRAINT admin_audit_events_event_type_check;
ALTER TABLE admin_audit_events VALIDATE CONSTRAINT admin_audit_events_check;
