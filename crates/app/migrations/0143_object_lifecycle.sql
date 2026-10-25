-- phase: expand
-- Registry survives tenant deletion. Tombstones prevent late attachment/reuse
-- after external deletion, including a worker crash before its DB commit.
CREATE TABLE object_lifecycle (
  storage_key text PRIMARY KEY,
  user_id uuid,
  workspace_id uuid,
  project_id uuid,
  cleanup_after timestamptz,
  retired_at timestamptz,
  deleted_at timestamptz,
  cleanup_attempts bigint NOT NULL DEFAULT 0,
  last_error text,
  created_at timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX object_lifecycle_due_idx ON object_lifecycle(cleanup_after)
  WHERE deleted_at IS NULL AND cleanup_after IS NOT NULL;
CREATE TABLE object_retained_references (
  reference_id uuid NOT NULL,
  storage_key text NOT NULL REFERENCES object_lifecycle(storage_key),
  expires_at timestamptz NOT NULL,
  PRIMARY KEY (reference_id, storage_key)
);
CREATE INDEX object_retained_references_expiry_idx ON object_retained_references(expires_at);

INSERT INTO object_lifecycle(storage_key, workspace_id, project_id)
SELECT object_key, min(p.workspace_id::text)::uuid, min(d.project_id::text)::uuid
FROM documents d JOIN projects p ON p.id=d.project_id WHERE object_key IS NOT NULL
GROUP BY object_key;
INSERT INTO object_lifecycle(storage_key, workspace_id, project_id)
SELECT storage_key, min(p.workspace_id::text)::uuid, min(f.project_id::text)::uuid
FROM workflow_files f JOIN projects p ON p.id=f.project_id WHERE storage_key IS NOT NULL
GROUP BY storage_key ON CONFLICT DO NOTHING;

CREATE FUNCTION object_reference_changed() RETURNS trigger LANGUAGE plpgsql AS $$
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
    IF entry.retired_at IS NOT NULL OR entry.deleted_at IS NOT NULL THEN
      RAISE EXCEPTION 'object reference has been retired';
    END IF;
    IF entry.workspace_id IS DISTINCT FROM tenant THEN
      RAISE EXCEPTION 'object reference belongs to another tenant';
    END IF;
    UPDATE object_lifecycle SET cleanup_after=NULL WHERE storage_key=next_key;
  END IF;
  IF previous_key IS NOT NULL THEN
    UPDATE object_lifecycle SET cleanup_after=now() + interval '24 hours'
      WHERE storage_key=previous_key AND deleted_at IS NULL;
  END IF;
  RETURN NULL;
END;
$$;
CREATE TRIGGER documents_object_lifecycle AFTER INSERT OR UPDATE OR DELETE ON documents
  FOR EACH ROW EXECUTE FUNCTION object_reference_changed();
CREATE TRIGGER workflow_files_object_lifecycle AFTER INSERT OR UPDATE OR DELETE ON workflow_files
  FOR EACH ROW EXECUTE FUNCTION object_reference_changed();

-- New kind never appears in N-1's queued-before-LIMIT candidate set or in
-- UPDATE RETURNING recovery. Capable binaries declare kinds transactionally.
CREATE FUNCTION object_cleanup_jobs_fence() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
  IF NEW.kind <> 'cleanup_objects' THEN RETURN NEW; END IF;
  IF NOT coalesce(NEW.kind = ANY(string_to_array(current_setting('deepref.job_kinds', true), ',')), false) THEN
    IF NEW.state = 'running' AND (TG_OP = 'INSERT' OR OLD.state <> 'running') THEN RETURN NULL; END IF;
    IF TG_OP = 'UPDATE' AND OLD.state = 'running' AND OLD.leased_until < now() THEN RETURN NULL; END IF;
  END IF;
  IF NEW.state = 'queued' THEN NEW.state := 'capability_wait'; END IF;
  RETURN NEW;
END;
$$;
CREATE TRIGGER object_cleanup_jobs_fence_trigger BEFORE INSERT OR UPDATE ON jobs
  FOR EACH ROW EXECUTE FUNCTION object_cleanup_jobs_fence();

CREATE OR REPLACE FUNCTION jobs_workload_class_for_kind(kind text)
RETURNS text LANGUAGE sql IMMUTABLE AS $$
  SELECT CASE $1
    WHEN 'assistant_agent_run' THEN 'ai'
    WHEN 'ai_usage_reconcile' THEN 'ai'
    WHEN 'ai_usage_export_replay' THEN 'ai'
    WHEN 'retrieve_document' THEN 'documents'
    WHEN 'parse_document' THEN 'documents'
    WHEN 'run_deduplication' THEN 'documents'
    WHEN 'cleanup_objects' THEN 'documents'
    WHEN 'durable_run_step' THEN 'workflow'
    WHEN 'backfill_workflow_files' THEN 'workflow'
    WHEN 'work_fetch_requested' THEN 'integration'
    WHEN 'pmid_import' THEN 'integration'
    WHEN 'recompute_metrics' THEN 'projection'
    WHEN 'recompute_prisma' THEN 'projection'
    ELSE 'workflow'
  END
$$;
