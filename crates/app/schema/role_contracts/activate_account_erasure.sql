-- Canonical protected function. This resource is also the trusted drift-repair fixture.
CREATE OR REPLACE FUNCTION public.activate_account_erasure(p_attestation text) RETURNS boolean
LANGUAGE plpgsql SECURITY DEFINER SET search_path=pg_catalog,public SET lock_timeout='250ms' SET statement_timeout='1500ms' AS $$
BEGIN
  IF p_attestation IS DISTINCT FROM 'all-legacy-export-writers-stopped-and-untracked-staging-removed-no-restart' THEN
    RAISE EXCEPTION 'exact legacy writer/staging cleanup attestation required';
  END IF;
  -- Older mutable request pointers cannot establish the contents of every
  -- historical archive. Require actual deletion ACKs for all unjournaled
  -- account objects, including replaced attempts, before enabling proofs.
  IF EXISTS(SELECT 1 FROM public.object_lifecycle o
      WHERE o.user_id IS NOT NULL AND o.deleted_at IS NULL
        AND NOT EXISTS(SELECT 1 FROM public.account_export_writers w
          WHERE w.archive_key=o.storage_key)) THEN
    RAISE EXCEPTION 'unjournaled legacy account objects still require deletion acknowledgements';
  END IF;
  UPDATE public.account_erasure_rollout SET activated=true,
    activated_at=coalesce(activated_at,clock_timestamp()),attestation=p_attestation
    WHERE singleton;
  RETURN FOUND;
END $$;
REVOKE ALL ON FUNCTION public.activate_account_erasure(text) FROM PUBLIC;
