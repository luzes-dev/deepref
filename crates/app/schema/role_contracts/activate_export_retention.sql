-- Canonical protected function. This resource is also the trusted drift-repair fixture.
CREATE OR REPLACE FUNCTION public.activate_export_retention(p_attestation text)
RETURNS boolean LANGUAGE plpgsql SECURITY DEFINER
SET search_path=pg_catalog SET lock_timeout='250ms' SET statement_timeout='1500ms' AS $$
BEGIN
  IF p_attestation IS DISTINCT FROM 'all-eager-deleting-binaries-drained-no-restart' THEN
    RAISE EXCEPTION 'exact eager-deleter drain and no-restart attestation required'
      USING ERRCODE='invalid_parameter_value';
  END IF;
  UPDATE public.account_export_rollout
    SET retention_activated=true,activated_at=coalesce(activated_at,clock_timestamp()),
        operator_attestation=p_attestation WHERE singleton;
  IF NOT FOUND THEN
    RAISE EXCEPTION 'export retention rollout control is missing'
      USING ERRCODE='integrity_constraint_violation';
  END IF;
  RETURN true;
END;
$$;
REVOKE ALL ON FUNCTION public.activate_export_retention(text) FROM PUBLIC;
