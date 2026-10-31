-- Canonical protected function. This resource is also the trusted drift-repair fixture.
CREATE OR REPLACE FUNCTION public.reject_admin_audit_event_changes()
RETURNS trigger LANGUAGE plpgsql SET search_path = pg_catalog AS $$
BEGIN
  IF TG_OP='DELETE'
     AND current_user::regrole=(SELECT proowner FROM pg_catalog.pg_proc WHERE oid='public.purge_retention_records(integer)'::regprocedure)
     AND current_setting('deepref.audit_purge',true)='on'
     AND OLD.occurred_at < now()-(CASE WHEN OLD.event_type='account.deletion_completed' THEN interval '5 years' ELSE interval '6 months' END)
  THEN RETURN OLD; END IF;
  IF TG_OP='UPDATE'
     AND current_user::regrole=(SELECT proowner FROM pg_catalog.pg_proc WHERE oid='public.redact_deleted_user_audit(uuid)'::regprocedure)
     AND current_setting('deepref.audit_redact_user',true)=OLD.target_user_id::text
     AND OLD.event_type IN ('legacy_actor.mapped','legacy_actor.non_identity_recorded')
     AND OLD.target_user_id IS NOT NULL
     AND EXISTS(SELECT 1 FROM public.users u WHERE u.id=OLD.target_user_id AND (u.deleted_at IS NOT NULL OR EXISTS(
       SELECT 1 FROM public.account_deletion_requests r WHERE r.user_id=u.id AND r.state='completing')))
     AND NEW.id=OLD.id AND NEW.occurred_at=OLD.occurred_at AND NEW.event_type=OLD.event_type
     AND NEW.principal_kind=OLD.principal_kind
     AND NEW.principal_user_id IS NOT DISTINCT FROM OLD.principal_user_id
     AND NEW.target_user_id IS NOT DISTINCT FROM OLD.target_user_id
     AND NEW.workspace_id IS NOT DISTINCT FROM OLD.workspace_id
     AND NEW.project_id IS NOT DISTINCT FROM OLD.project_id
     AND NEW.session_ref IS NOT DISTINCT FROM OLD.session_ref
     AND NEW.request_id IS NOT DISTINCT FROM OLD.request_id
     AND NEW.metadata-'legacy_actor_id'-'source'=OLD.metadata-'legacy_actor_id'-'source'
     AND NEW.metadata->>'legacy_actor_id'='redacted' AND NEW.metadata->>'source'='redacted'
  THEN RETURN NEW; END IF;
  RAISE EXCEPTION 'administrative audit events are append-only' USING ERRCODE='integrity_constraint_violation';
END;
$$;
REVOKE ALL ON FUNCTION public.reject_admin_audit_event_changes() FROM PUBLIC;
