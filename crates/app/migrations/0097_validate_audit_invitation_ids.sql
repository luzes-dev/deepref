-- phase: expand
-- Validates the check 0096 added NOT VALID, under a lock that lets reads and
-- writes continue.
ALTER TABLE admin_audit_events VALIDATE CONSTRAINT admin_audit_events_check;
