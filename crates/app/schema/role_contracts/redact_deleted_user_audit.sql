-- Canonical protected function. This resource is also the trusted drift-repair fixture.
CREATE OR REPLACE FUNCTION public.redact_deleted_user_audit(p_user_id uuid)
RETURNS bigint
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = pg_catalog
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
REVOKE ALL ON FUNCTION public.redact_deleted_user_audit(uuid) FROM PUBLIC;
