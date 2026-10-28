-- phase: expand
-- No blob is erased before the existing thirty-day cancellation window ends.
-- A due privacy target skips operational grace only after metadata/references
-- are withdrawn. Failed physical deletion remains an observable obligation.
ALTER TABLE object_lifecycle ADD COLUMN privacy_due_at timestamptz;

-- Only the blocking staging owner acknowledges removal. Process crashes or
-- cleanup failure leave a pending obligation; elapsed time is not proof.
CREATE TABLE account_export_writers (
  attempt_id uuid PRIMARY KEY,
  user_id uuid NOT NULL,
  export_id uuid NOT NULL REFERENCES account_data_exports(id),
  archive_key text NOT NULL,
  captured_workspaces uuid[] NOT NULL DEFAULT '{}',
  staging_directory text CHECK(octet_length(staging_directory)<=4096),
  upload_outcome text NOT NULL DEFAULT 'not_started'
    CHECK(upload_outcome IN ('not_started','dispatched','acknowledged','unresolved')),
  upload_intent_at timestamptz,
  upload_acknowledged_at timestamptz,
  CHECK(upload_outcome='not_started' OR upload_intent_at IS NOT NULL),
  CHECK((upload_outcome='acknowledged')=(upload_acknowledged_at IS NOT NULL)),
  writer_owner text NOT NULL,
  claim_seq bigint NOT NULL CHECK(claim_seq>0),
  started_at timestamptz NOT NULL DEFAULT now(),
  stopped_at timestamptz,
  privacy_due_at timestamptz
);
CREATE INDEX account_export_writers_privacy_pending
  ON account_export_writers(privacy_due_at, attempt_id)
  WHERE stopped_at IS NULL AND privacy_due_at IS NOT NULL;

-- Pseudonymous progress is not a fulfillment proof. Record it before profile
-- anonymization so a retry can scrub restored invitation addresses honestly.
CREATE TABLE account_erasure_progress (
  user_id uuid PRIMARY KEY REFERENCES users(id),
  due_at timestamptz NOT NULL,
  started_at timestamptz NOT NULL DEFAULT clock_timestamp(),
  email_hmac_sha256 text[] NOT NULL,
  logical_scrubbed_at timestamptz
);
CREATE TABLE account_erasure_objects (
  user_id uuid NOT NULL REFERENCES account_erasure_progress(user_id) ON DELETE CASCADE,
  storage_key text NOT NULL REFERENCES object_lifecycle(storage_key),
  PRIMARY KEY(user_id, storage_key)
);
CREATE TABLE account_erasure_writers (
  user_id uuid NOT NULL REFERENCES account_erasure_progress(user_id) ON DELETE CASCADE,
  attempt_id uuid NOT NULL REFERENCES account_export_writers(attempt_id),
  PRIMARY KEY(user_id, attempt_id)
);

-- NULL on historical proofs: a migration cannot manufacture physical evidence.
ALTER TABLE fulfilled_account_deletions
  ADD COLUMN physical_verified_at timestamptz,
  ADD COLUMN logical_scrubbed_at timestamptz;
COMMENT ON COLUMN fulfilled_account_deletions.physical_verified_at IS
  'Set only by new completion after every targeted object deletion and writer-owned staging cleanup acknowledgement. Historical NULL is unverified, not proof of remaining bytes.';

-- Untracked staging from older/crashed binaries cannot be declared absent by
-- a timestamp. Deployment must stop old writers and actually remove their
-- private staging before enabling new physical proofs or automatic inactivity.
CREATE TABLE account_erasure_rollout (
  singleton boolean PRIMARY KEY DEFAULT true CHECK(singleton),
  activated boolean NOT NULL DEFAULT false,
  activated_at timestamptz,
  attestation text,
  CHECK(NOT activated OR (activated_at IS NOT NULL AND attestation IS NOT NULL
    AND attestation='all-legacy-export-writers-stopped-and-untracked-staging-removed-no-restart'))
);
INSERT INTO account_erasure_rollout(singleton) VALUES(true);

-- A timestamp chosen by a caller is not evidence. New verified proofs must
-- agree with durable per-account targets and actual writer/storage ACKs.
-- Historical NULL proofs stay unverified; UPDATE cannot relabel them.
CREATE FUNCTION enforce_account_physical_proof() RETURNS trigger
LANGUAGE plpgsql SET search_path=pg_catalog,public AS $$
BEGIN
  IF TG_OP='UPDATE' THEN
    IF NEW.physical_verified_at IS DISTINCT FROM OLD.physical_verified_at
       OR NEW.logical_scrubbed_at IS DISTINCT FROM OLD.logical_scrubbed_at THEN
      RAISE EXCEPTION 'physical verification cannot be retroactively assigned';
    END IF;
    RETURN NEW;
  END IF;
  IF NEW.physical_verified_at IS NULL THEN
    IF EXISTS(SELECT 1 FROM public.account_erasure_rollout WHERE singleton AND activated) THEN
      RAISE EXCEPTION 'new completions require physical deletion acknowledgements';
    END IF;
    RETURN NEW;
  END IF;
  IF NEW.logical_scrubbed_at IS DISTINCT FROM (SELECT logical_scrubbed_at FROM public.account_erasure_progress WHERE user_id=NEW.user_id) THEN
    RAISE EXCEPTION 'physical proof must preserve its original logical scrub cutoff';
  END IF;
  IF NOT EXISTS(SELECT 1 FROM public.account_erasure_rollout WHERE singleton AND activated)
     OR NOT EXISTS(SELECT 1 FROM public.account_erasure_progress p
      WHERE p.user_id=NEW.user_id AND p.due_at<=clock_timestamp()
        AND p.logical_scrubbed_at IS NOT NULL)
     OR EXISTS(SELECT 1 FROM public.account_erasure_objects t
       JOIN public.object_lifecycle o USING(storage_key)
       WHERE t.user_id=NEW.user_id AND o.deleted_at IS NULL)
     OR EXISTS(SELECT 1 FROM public.account_erasure_writers t
       JOIN public.account_export_writers w USING(attempt_id)
       WHERE t.user_id=NEW.user_id AND (w.stopped_at IS NULL OR w.upload_outcome IN ('dispatched','unresolved')))
  THEN
    RAISE EXCEPTION 'physical deletion acknowledgements are incomplete';
  END IF;
  NEW.physical_verified_at:=clock_timestamp();
  RETURN NEW;
END $$;
REVOKE ALL ON FUNCTION enforce_account_physical_proof() FROM PUBLIC;
CREATE TRIGGER fulfilled_account_physical_proof
  BEFORE INSERT OR UPDATE ON fulfilled_account_deletions
  FOR EACH ROW EXECUTE FUNCTION enforce_account_physical_proof();


-- Existing append-only object identities remain authoritative across binaries.
CREATE OR REPLACE FUNCTION object_reference_changed() RETURNS trigger LANGUAGE plpgsql AS $$
DECLARE
  previous_key text;
  next_key text;
  tenant uuid;
  owner_project uuid;
  entry object_lifecycle;
BEGIN
  IF TG_TABLE_NAME = 'documents' THEN
    IF TG_OP <> 'INSERT' THEN previous_key := OLD.object_key; END IF;
    IF TG_OP <> 'DELETE' THEN next_key := NEW.object_key; END IF;
  ELSE
    IF TG_OP <> 'INSERT' THEN previous_key := OLD.storage_key; END IF;
    IF TG_OP <> 'DELETE' THEN next_key := NEW.storage_key; END IF;
  END IF;
  IF previous_key IS NOT DISTINCT FROM next_key THEN RETURN NULL; END IF;
  -- Lock keys in sorted order even on replacement, avoiding inverted key locks.
  PERFORM 1 FROM object_lifecycle WHERE storage_key IN (previous_key, next_key)
    ORDER BY storage_key FOR UPDATE;
  IF next_key IS NOT NULL THEN
    owner_project := NEW.project_id;
    SELECT workspace_id INTO STRICT tenant FROM projects WHERE id=owner_project;
    IF next_key LIKE 'projects/%' AND next_key NOT LIKE 'projects/' || owner_project::text || '/workflow-runs/%' THEN
      RAISE EXCEPTION 'object reference belongs to another project';
    END IF;
    IF next_key LIKE 'workspaces/%' AND
       next_key NOT LIKE 'workspaces/' || tenant::text || '/projects/' || owner_project::text || '/%' THEN
      RAISE EXCEPTION 'object reference belongs to another tenant/project';
    END IF;
    INSERT INTO object_lifecycle(storage_key, workspace_id, project_id)
      VALUES(next_key, tenant, owner_project) ON CONFLICT DO NOTHING;
    SELECT * INTO STRICT entry FROM object_lifecycle WHERE storage_key=next_key FOR UPDATE;
    IF entry.retired_at IS NOT NULL OR entry.deleted_at IS NOT NULL OR entry.privacy_due_at<=clock_timestamp() THEN
      RAISE EXCEPTION 'object reference has been retired';
    END IF;
    IF entry.workspace_id IS DISTINCT FROM tenant THEN
      RAISE EXCEPTION 'object reference belongs to another tenant';
    END IF;
    UPDATE object_lifecycle SET cleanup_after=NULL WHERE storage_key=next_key;
  END IF;
  IF previous_key IS NOT NULL THEN
    UPDATE object_lifecycle SET cleanup_after=CASE WHEN privacy_due_at<=clock_timestamp() THEN clock_timestamp() ELSE now()+interval '24 hours' END
      WHERE storage_key=previous_key AND deleted_at IS NULL;
  END IF;
  RETURN NULL;
END;
$$;

CREATE FUNCTION enforce_object_retention_target() RETURNS trigger
LANGUAGE plpgsql SET search_path=pg_catalog,public AS $$
DECLARE live boolean;
BEGIN
  SELECT retired_at IS NULL AND deleted_at IS NULL
      AND (privacy_due_at IS NULL OR privacy_due_at>clock_timestamp())
    INTO live FROM public.object_lifecycle WHERE storage_key=NEW.storage_key FOR UPDATE;
  IF NOT coalesce(live,false) THEN
    RAISE EXCEPTION 'retention target is retired or due for privacy erasure';
  END IF;
  RETURN NEW;
END $$;
REVOKE ALL ON FUNCTION enforce_object_retention_target() FROM PUBLIC;
CREATE TRIGGER object_retention_target_guard BEFORE INSERT OR UPDATE ON object_retained_references
  FOR EACH ROW EXECUTE FUNCTION enforce_object_retention_target();

CREATE FUNCTION activate_account_erasure(p_attestation text) RETURNS boolean
LANGUAGE plpgsql SECURITY DEFINER SET search_path=pg_catalog,public AS $$
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
REVOKE ALL ON FUNCTION activate_account_erasure(text) FROM PUBLIC;

-- Business progress/maps and writer journals participate in ordinary runtime
-- deletion transactions. Only rollout activation is a maintenance capability.
REVOKE ALL ON account_erasure_rollout FROM PUBLIC;
DO $$
BEGIN
  IF EXISTS(SELECT 1 FROM pg_roles WHERE rolname='deepref_runtime') THEN
    REVOKE ALL ON account_erasure_rollout FROM deepref_runtime;
    GRANT SELECT ON account_erasure_rollout TO deepref_runtime;
    REVOKE ALL ON FUNCTION activate_account_erasure(text) FROM deepref_runtime;
  END IF;
  IF EXISTS(SELECT 1 FROM pg_roles WHERE rolname='deepref_maintenance') THEN
    GRANT EXECUTE ON FUNCTION activate_account_erasure(text) TO deepref_maintenance;
  END IF;
END $$;
