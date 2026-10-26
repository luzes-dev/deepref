-- phase: expand
ALTER TABLE admin_audit_events VALIDATE CONSTRAINT admin_audit_events_event_type_check;
ALTER TABLE admin_audit_events VALIDATE CONSTRAINT admin_audit_events_check;
