-- phase: expand
-- Validates the audit constraints 0091 added NOT VALID, under a lock that lets
-- reads and writes continue.

ALTER TABLE admin_audit_events VALIDATE CONSTRAINT admin_audit_events_event_type_check;
ALTER TABLE admin_audit_events VALIDATE CONSTRAINT admin_audit_events_check;
