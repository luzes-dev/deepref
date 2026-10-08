-- Visual workflow engine.
--
-- A workflow has an editable draft graph and immutable published versions.
-- A run executes one published version (or, for test runs, the version being
-- tried). Every node execution is a durable row with its own job on the
-- existing queue. The fixed-recipe automation tables are left untouched.

CREATE TABLE workflows (
  id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
  project_id uuid NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
  name text NOT NULL CHECK (length(btrim(name)) BETWEEN 1 AND 200),
  description text NOT NULL DEFAULT '' CHECK (length(description) <= 2000),
  status text NOT NULL DEFAULT 'disabled' CHECK (status IN ('enabled', 'disabled')),
  draft_graph jsonb NOT NULL DEFAULT '{"nodes":[],"edges":[]}'::jsonb
    CHECK (jsonb_typeof(draft_graph) = 'object'),
  draft_revision bigint NOT NULL DEFAULT 1 CHECK (draft_revision >= 1),
  published_version_id uuid,
  trigger_kind text,
  webhook_token text NOT NULL UNIQUE,
  webhook_secret text NOT NULL,
  webhook_signature_required boolean NOT NULL DEFAULT true,
  email_token text NOT NULL UNIQUE,
  next_fire_at timestamptz,
  last_fired_at timestamptz,
  poll_state jsonb NOT NULL DEFAULT '{}'::jsonb CHECK (jsonb_typeof(poll_state) = 'object'),
  created_by_kind text NOT NULL CHECK (created_by_kind IN ('user', 'automation', 'system')),
  created_by_id text NOT NULL CHECK (length(btrim(created_by_id)) BETWEEN 1 AND 200),
  created_at timestamptz NOT NULL DEFAULT now(),
  updated_at timestamptz NOT NULL DEFAULT now(),
  CONSTRAINT workflows_project_id_key UNIQUE (project_id, id),
  CONSTRAINT workflows_project_name_key UNIQUE (project_id, name)
);

CREATE INDEX workflows_event_idx
  ON workflows(project_id, trigger_kind) WHERE status = 'enabled';
CREATE INDEX workflows_due_idx
  ON workflows(next_fire_at) WHERE status = 'enabled' AND next_fire_at IS NOT NULL;

CREATE TABLE workflow_versions (
  id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
  project_id uuid NOT NULL,
  workflow_id uuid NOT NULL,
  version integer NOT NULL CHECK (version >= 1),
  graph jsonb NOT NULL CHECK (jsonb_typeof(graph) = 'object'),
  trigger_kind text NOT NULL,
  note text CHECK (note IS NULL OR length(note) <= 500),
  published_by_kind text NOT NULL CHECK (published_by_kind IN ('user', 'automation', 'system')),
  published_by_id text NOT NULL CHECK (length(btrim(published_by_id)) BETWEEN 1 AND 200),
  published_at timestamptz NOT NULL DEFAULT now(),
  CONSTRAINT workflow_versions_number_key UNIQUE (workflow_id, version),
  CONSTRAINT workflow_versions_workflow_id_key UNIQUE (workflow_id, id),
  FOREIGN KEY (project_id, workflow_id)
    REFERENCES workflows(project_id, id) ON DELETE CASCADE
);

-- Published versions are immutable. Rows may only disappear together with
-- their workflow.
CREATE FUNCTION workflow_versions_immutable() RETURNS trigger
LANGUAGE plpgsql AS $$
BEGIN
  IF TG_OP = 'DELETE' THEN
    IF EXISTS (SELECT 1 FROM workflows WHERE id = OLD.workflow_id) THEN
      RAISE EXCEPTION 'published workflow versions cannot be deleted'
        USING ERRCODE = 'restrict_violation';
    END IF;
    RETURN OLD;
  END IF;
  RAISE EXCEPTION 'published workflow versions are immutable'
    USING ERRCODE = 'restrict_violation';
END;
$$;
CREATE TRIGGER workflow_versions_immutable_trigger
BEFORE UPDATE OR DELETE ON workflow_versions
FOR EACH ROW EXECUTE FUNCTION workflow_versions_immutable();

ALTER TABLE workflows
  ADD CONSTRAINT workflows_published_version_fk
  FOREIGN KEY (id, published_version_id)
  REFERENCES workflow_versions(workflow_id, id) DEFERRABLE INITIALLY DEFERRED;

CREATE TABLE workflow_runs (
  id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
  project_id uuid NOT NULL,
  workflow_id uuid NOT NULL,
  -- NULL for a test run of an unpublished draft.
  version_id uuid,
  -- Immutable copy of the graph this run executes.
  graph jsonb NOT NULL CHECK (jsonb_typeof(graph) = 'object'),
  trigger_kind text NOT NULL,
  trigger_data jsonb NOT NULL DEFAULT '{}'::jsonb,
  idempotency_key text NOT NULL CHECK (length(btrim(idempotency_key)) BETWEEN 1 AND 300),
  test_mode boolean NOT NULL DEFAULT false,
  status text NOT NULL DEFAULT 'queued'
    CHECK (status IN ('queued', 'running', 'completed', 'failed', 'cancelled')),
  actor_kind text NOT NULL CHECK (actor_kind IN ('user', 'automation', 'system')),
  actor_id text NOT NULL CHECK (length(btrim(actor_id)) BETWEEN 1 AND 200),
  error text CHECK (error IS NULL OR length(error) <= 4096),
  created_at timestamptz NOT NULL DEFAULT now(),
  started_at timestamptz,
  finished_at timestamptz,
  CONSTRAINT workflow_runs_project_id_key UNIQUE (project_id, id),
  CONSTRAINT workflow_runs_idempotency_key UNIQUE (workflow_id, idempotency_key),
  FOREIGN KEY (project_id, workflow_id)
    REFERENCES workflows(project_id, id) ON DELETE CASCADE,
  FOREIGN KEY (workflow_id, version_id)
    REFERENCES workflow_versions(workflow_id, id) ON DELETE CASCADE
);

CREATE INDEX workflow_runs_project_idx
  ON workflow_runs(project_id, created_at DESC, id DESC);
CREATE INDEX workflow_runs_workflow_idx
  ON workflow_runs(workflow_id, created_at DESC, id DESC);
CREATE INDEX workflow_runs_active_idx
  ON workflow_runs(status) WHERE status IN ('queued', 'running');

CREATE TABLE workflow_node_runs (
  id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
  project_id uuid NOT NULL,
  run_id uuid NOT NULL,
  node_id text NOT NULL CHECK (length(node_id) BETWEEN 1 AND 100),
  iteration text NOT NULL DEFAULT '',
  node_type text NOT NULL,
  status text NOT NULL DEFAULT 'queued'
    CHECK (status IN ('queued', 'running', 'completed', 'failed', 'skipped', 'cancelled')),
  attempts integer NOT NULL DEFAULT 0 CHECK (attempts >= 0),
  input jsonb,
  output jsonb,
  fired_ports text[] NOT NULL DEFAULT '{}',
  items integer NOT NULL DEFAULT 0 CHECK (items >= 0),
  note text,
  error text CHECK (error IS NULL OR length(error) <= 4096),
  created_at timestamptz NOT NULL DEFAULT now(),
  started_at timestamptz,
  finished_at timestamptz,
  CONSTRAINT workflow_node_runs_key UNIQUE (run_id, node_id, iteration),
  FOREIGN KEY (project_id, run_id)
    REFERENCES workflow_runs(project_id, id) ON DELETE CASCADE
);

CREATE INDEX workflow_node_runs_run_idx ON workflow_node_runs(run_id, created_at);

-- Identifiers a publication alert already reported, per workflow.
CREATE TABLE workflow_seen_items (
  workflow_id uuid NOT NULL REFERENCES workflows(id) ON DELETE CASCADE,
  identifier text NOT NULL CHECK (length(identifier) BETWEEN 1 AND 500),
  first_seen_at timestamptz NOT NULL DEFAULT now(),
  PRIMARY KEY (workflow_id, identifier)
);

-- Secret values (tokens, webhook addresses) are kept out of the graph JSON so
-- they never travel back to the browser.
CREATE TABLE workflow_secrets (
  workflow_id uuid NOT NULL REFERENCES workflows(id) ON DELETE CASCADE,
  node_id text NOT NULL,
  key text NOT NULL,
  value text NOT NULL CHECK (length(value) <= 4096),
  updated_at timestamptz NOT NULL DEFAULT now(),
  PRIMARY KEY (workflow_id, node_id, key)
);

-- Files produced by export blocks.
CREATE TABLE workflow_files (
  id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
  project_id uuid NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
  run_id uuid NOT NULL REFERENCES workflow_runs(id) ON DELETE CASCADE,
  node_id text NOT NULL,
  name text NOT NULL CHECK (length(btrim(name)) BETWEEN 1 AND 200),
  content_type text NOT NULL,
  content bytea NOT NULL CHECK (octet_length(content) <= 16777216),
  created_at timestamptz NOT NULL DEFAULT now()
);

CREATE INDEX workflow_files_run_idx ON workflow_files(run_id);
