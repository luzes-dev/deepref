-- phase: expand
-- Validate the admin_audit_events CHECKs widened NOT VALID in 0030.
ALTER TABLE public.admin_audit_events VALIDATE CONSTRAINT admin_audit_events_event_type_check;
ALTER TABLE public.admin_audit_events VALIDATE CONSTRAINT admin_audit_events_check;
