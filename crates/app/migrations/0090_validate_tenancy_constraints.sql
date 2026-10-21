-- phase: expand
-- Validates the constraints 0088 and 0089 added NOT VALID. VALIDATE CONSTRAINT
-- scans under SHARE UPDATE EXCLUSIVE, which lets reads and writes continue, so
-- it is kept apart from the migrations whose DDL took the stronger locks.

ALTER TABLE projects VALIDATE CONSTRAINT projects_workspace_id_fkey;
ALTER TABLE admin_audit_events VALIDATE CONSTRAINT admin_audit_events_event_type_check;
ALTER TABLE admin_audit_events VALIDATE CONSTRAINT admin_audit_events_check;
