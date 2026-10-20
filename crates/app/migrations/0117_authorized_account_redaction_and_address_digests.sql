-- phase: expand
-- Account deletion replay needs to recognize restored invitations without
-- retaining the original addresses. Store only lowercase hexadecimal SHA-256
-- digests of normalized email addresses in the five-year fulfillment log.
ALTER TABLE fulfilled_account_deletions
  ADD COLUMN email_sha256 text[] NOT NULL DEFAULT ARRAY[]::text[];

COMMENT ON COLUMN fulfilled_account_deletions.email_sha256 IS
  'Lowercase hex SHA-256 digests of normalized account addresses, used to scrub invitations after restore; never plaintext email.';

-- The append-only trigger still allows purge only for rows past retention.
-- Redaction is now authorized by one SECURITY DEFINER function, rather than
-- any application caller choosing a session GUC. The trigger independently
-- checks both the exact target user and that user's deletion state.
CREATE OR REPLACE FUNCTION reject_admin_audit_event_changes()
RETURNS trigger
LANGUAGE plpgsql
AS $$
BEGIN
  IF TG_OP = 'DELETE' AND current_setting('deepref.audit_purge', true) = 'on' THEN
    IF OLD.event_type = 'account.deletion_completed' THEN
      IF OLD.occurred_at < now() - interval '5 years' THEN
        RETURN OLD;
      END IF;
    ELSIF OLD.occurred_at < now() - interval '6 months' THEN
      RETURN OLD;
    END IF;
  END IF;
  IF TG_OP = 'UPDATE'
     AND current_setting('deepref.audit_redact_user', true) = OLD.target_user_id::text
     AND OLD.event_type IN ('legacy_actor.mapped', 'legacy_actor.non_identity_recorded')
     AND OLD.target_user_id IS NOT NULL
     AND EXISTS (
       SELECT 1 FROM public.users u
       WHERE u.id = OLD.target_user_id
         AND (u.deleted_at IS NOT NULL OR EXISTS (
           SELECT 1 FROM public.account_deletion_requests r
           WHERE r.user_id = u.id AND r.state = 'completing'
         ))
     )
     AND NEW.id = OLD.id
     AND NEW.occurred_at = OLD.occurred_at
     AND NEW.event_type = OLD.event_type
     AND NEW.principal_kind = OLD.principal_kind
     AND NEW.principal_user_id IS NOT DISTINCT FROM OLD.principal_user_id
     AND NEW.target_user_id IS NOT DISTINCT FROM OLD.target_user_id
     AND NEW.workspace_id IS NOT DISTINCT FROM OLD.workspace_id
     AND NEW.project_id IS NOT DISTINCT FROM OLD.project_id
     AND NEW.session_ref IS NOT DISTINCT FROM OLD.session_ref
     AND NEW.request_id IS NOT DISTINCT FROM OLD.request_id
     AND NEW.metadata - 'legacy_actor_id' - 'source'
         = OLD.metadata - 'legacy_actor_id' - 'source'
     AND NEW.metadata->>'legacy_actor_id' = 'redacted'
     AND NEW.metadata->>'source' = 'redacted'
  THEN
    RETURN NEW;
  END IF;
  RAISE EXCEPTION 'administrative audit events are append-only'
    USING ERRCODE = 'integrity_constraint_violation';
END;
$$;

CREATE FUNCTION redact_deleted_user_audit(p_user_id uuid)
RETURNS bigint
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = pg_catalog, public
AS $$
DECLARE
  changed bigint;
BEGIN
  IF NOT EXISTS (
    SELECT 1 FROM public.users u
    WHERE u.id = p_user_id
      AND (u.deleted_at IS NOT NULL OR EXISTS (
        SELECT 1 FROM public.account_deletion_requests r
        WHERE r.user_id = u.id AND r.state = 'completing'
      ))
  ) THEN
    RAISE EXCEPTION 'audit redaction requires a deleted user or a completing deletion request'
      USING ERRCODE = 'insufficient_privilege';
  END IF;

  -- Equivalent to SET LOCAL: this gate lasts only for the surrounding
  -- transaction and can name only the already-authorized user.
  PERFORM pg_catalog.set_config('deepref.audit_redact_user', p_user_id::text, true);

  UPDATE public.admin_audit_events
  SET metadata = metadata || pg_catalog.jsonb_build_object(
        'legacy_actor_id', 'redacted', 'source', 'redacted')
  WHERE target_user_id = p_user_id
    AND event_type IN ('legacy_actor.mapped', 'legacy_actor.non_identity_recorded')
    AND (metadata->>'legacy_actor_id' IS DISTINCT FROM 'redacted'
         OR metadata->>'source' IS DISTINCT FROM 'redacted');
  GET DIAGNOSTICS changed = ROW_COUNT;
  RETURN changed;
END;
$$;

REVOKE ALL ON FUNCTION redact_deleted_user_audit(uuid) FROM PUBLIC;
-- The repository uses the migration/runtime role for PostgreSQL access. If a
-- deployment provisions the conventional separate application role, grant
-- only this constrained operation to it.
DO $$
BEGIN
  IF EXISTS (SELECT 1 FROM pg_catalog.pg_roles WHERE rolname = 'deepref_app') THEN
    EXECUTE 'GRANT EXECUTE ON FUNCTION public.redact_deleted_user_audit(uuid) TO deepref_app';
  END IF;
END
$$;

COMMENT ON FUNCTION redact_deleted_user_audit(uuid) IS
  'Redacts named legacy-actor PII keys only for a deleted user or a user whose deletion request is completing.';
