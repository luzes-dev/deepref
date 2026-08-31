-- Durable execution state for checked-in compiled review definitions.
-- Review workflows remain private Rust assets. The automation tables schedule
-- one closed recipe; node attempts and artifacts record the internal lineage.

ALTER TABLE automation_definitions
  DROP CONSTRAINT IF EXISTS automation_definitions_recipe_id_check,
  ADD CONSTRAINT automation_definitions_recipe_id_check CHECK (recipe_id IN (
    'project_maintenance',
    'review_screening',
    'review_duplicate_detection',
    'review_study_classification',
    'review_study_grouping',
    'review_appraisal_prefill',
    'review_data_extraction'
  ));

ALTER TABLE automation_runs
  DROP CONSTRAINT IF EXISTS automation_runs_recipe_id_check,
  ADD CONSTRAINT automation_runs_recipe_id_check CHECK (recipe_id IN (
    'project_maintenance',
    'review_screening',
    'review_duplicate_detection',
    'review_study_classification',
    'review_study_grouping',
    'review_appraisal_prefill',
    'review_data_extraction'
  ));

CREATE OR REPLACE FUNCTION automation_builtin_recipe_steps(
  target_recipe_id text,
  target_recipe_version integer
)
RETURNS TABLE (
  ordinal integer,
  step_key text,
  step_kind text
)
LANGUAGE sql
IMMUTABLE
PARALLEL SAFE
AS $$
  SELECT recipe.ordinal, recipe.step_key, recipe.step_kind
  FROM (VALUES
    ('project_maintenance', 1, 0, 'recompute_project_metrics', 'deterministic_action'),
    ('review_screening', 1, 0, 'execute_compiled_review', 'ai_task'),
    ('review_duplicate_detection', 1, 0, 'execute_compiled_review', 'ai_task'),
    ('review_study_classification', 1, 0, 'execute_compiled_review', 'ai_task'),
    ('review_study_grouping', 1, 0, 'execute_compiled_review', 'ai_task'),
    ('review_appraisal_prefill', 1, 0, 'execute_compiled_review', 'ai_task'),
    ('review_data_extraction', 1, 0, 'execute_compiled_review', 'ai_task')
  ) AS recipe(recipe_id, recipe_version, ordinal, step_key, step_kind)
  WHERE recipe.recipe_id = target_recipe_id
    AND recipe.recipe_version = target_recipe_version
$$;

CREATE TABLE review_run_manifests (
  project_id uuid NOT NULL,
  automation_run_id uuid NOT NULL,
  definition_key text NOT NULL CHECK (definition_key IN (
    'screening', 'duplicate_detection', 'study_classification',
    'study_grouping', 'appraisal_prefill', 'data_extraction'
  )),
  definition_id text NOT NULL CHECK (length(btrim(definition_id)) BETWEEN 1 AND 200),
  definition_version integer NOT NULL CHECK (definition_version > 0),
  manifest_hash text NOT NULL CHECK (manifest_hash ~ '^[0-9a-f]{64}$'),
  semantic_bundle_hash text NOT NULL CHECK (semantic_bundle_hash ~ '^[0-9a-f]{64}$'),
  manifest jsonb NOT NULL CHECK (jsonb_typeof(manifest) = 'object'),
  subject jsonb NOT NULL CHECK (jsonb_typeof(subject) = 'object'),
  origin jsonb NOT NULL CHECK (jsonb_typeof(origin) = 'object'),
  prepared_task jsonb NOT NULL CHECK (jsonb_typeof(prepared_task) = 'object'),
  state text NOT NULL DEFAULT 'queued'
    CHECK (state IN ('queued', 'running', 'blocked', 'failed', 'completed')),
  state_code text,
  state_message text,
  candidate_hash text CHECK (candidate_hash IS NULL OR candidate_hash ~ '^[0-9a-f]{64}$'),
  proposal_id uuid,
  created_at timestamptz NOT NULL DEFAULT now(),
  started_at timestamptz,
  finished_at timestamptz,
  PRIMARY KEY (project_id, automation_run_id),
  UNIQUE (automation_run_id),
  FOREIGN KEY (project_id, automation_run_id)
    REFERENCES automation_runs(project_id, id) ON DELETE CASCADE,
  FOREIGN KEY (project_id, proposal_id)
    REFERENCES ai_proposals(project_id, id)
    DEFERRABLE INITIALLY DEFERRED,
  CHECK (state_code IS NULL OR length(btrim(state_code)) BETWEEN 1 AND 100),
  CHECK (state_message IS NULL OR length(btrim(state_message)) BETWEEN 1 AND 4096),
  CHECK (
    (state = 'queued' AND started_at IS NULL AND finished_at IS NULL
      AND state_code IS NULL AND state_message IS NULL AND proposal_id IS NULL)
    OR (state = 'running' AND started_at IS NOT NULL AND finished_at IS NULL
      AND state_code IS NULL AND state_message IS NULL AND proposal_id IS NULL)
    OR (state = 'blocked' AND started_at IS NOT NULL AND finished_at IS NOT NULL
      AND state_code IS NOT NULL AND state_message IS NOT NULL AND proposal_id IS NULL)
    OR (state = 'failed' AND started_at IS NOT NULL AND finished_at IS NOT NULL
      AND state_code IS NOT NULL AND state_message IS NOT NULL AND proposal_id IS NULL)
    OR (state = 'completed' AND started_at IS NOT NULL AND finished_at IS NOT NULL
      AND state_code IS NULL AND state_message IS NULL
      AND candidate_hash IS NOT NULL AND proposal_id IS NOT NULL)
  )
);

CREATE INDEX review_run_manifests_project_created_idx
  ON review_run_manifests(project_id, created_at DESC, automation_run_id DESC);
CREATE INDEX review_run_manifests_semantic_bundle_idx
  ON review_run_manifests(project_id, semantic_bundle_hash, definition_key);

CREATE TABLE review_artifacts (
  id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
  project_id uuid NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
  content_hash text NOT NULL CHECK (content_hash ~ '^[0-9a-f]{64}$'),
  media_type text NOT NULL CHECK (length(btrim(media_type)) BETWEEN 1 AND 200),
  payload jsonb NOT NULL,
  created_at timestamptz NOT NULL DEFAULT now(),
  UNIQUE (project_id, id),
  UNIQUE (project_id, content_hash)
);

CREATE TABLE review_step_attempts (
  id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
  project_id uuid NOT NULL,
  automation_run_id uuid NOT NULL,
  node_id text NOT NULL CHECK (length(btrim(node_id)) BETWEEN 1 AND 200),
  node_version integer NOT NULL CHECK (node_version > 0),
  attempt_number integer NOT NULL CHECK (attempt_number > 0),
  input_fingerprint text NOT NULL CHECK (input_fingerprint ~ '^[0-9a-f]{64}$'),
  status text NOT NULL CHECK (status IN ('running', 'completed', 'failed')),
  worker_id text NOT NULL CHECK (length(btrim(worker_id)) BETWEEN 1 AND 200),
  artifact_id uuid,
  model_run_id uuid,
  error_code text,
  error_message text,
  started_at timestamptz NOT NULL DEFAULT now(),
  finished_at timestamptz,
  accepted_at timestamptz,
  UNIQUE (project_id, automation_run_id, node_id, attempt_number),
  UNIQUE (id),
  FOREIGN KEY (project_id, automation_run_id)
    REFERENCES review_run_manifests(project_id, automation_run_id) ON DELETE CASCADE,
  FOREIGN KEY (project_id, artifact_id)
    REFERENCES review_artifacts(project_id, id)
    DEFERRABLE INITIALLY DEFERRED,
  FOREIGN KEY (model_run_id, project_id)
    REFERENCES ai_runs(id, project_id)
    DEFERRABLE INITIALLY DEFERRED,
  CHECK (error_code IS NULL OR length(btrim(error_code)) BETWEEN 1 AND 100),
  CHECK (error_message IS NULL OR length(btrim(error_message)) BETWEEN 1 AND 4096),
  CHECK (
    (status = 'running' AND finished_at IS NULL AND accepted_at IS NULL
      AND artifact_id IS NULL AND error_code IS NULL AND error_message IS NULL)
    OR (status = 'completed' AND finished_at IS NOT NULL AND artifact_id IS NOT NULL
      AND error_code IS NULL AND error_message IS NULL)
    OR (status = 'failed' AND finished_at IS NOT NULL AND accepted_at IS NULL
      AND artifact_id IS NULL AND error_code IS NOT NULL AND error_message IS NOT NULL)
  )
);

CREATE UNIQUE INDEX review_step_attempts_accepted_fingerprint_uq
  ON review_step_attempts(project_id, node_id, input_fingerprint)
  WHERE accepted_at IS NOT NULL;
CREATE INDEX review_step_attempts_run_node_idx
  ON review_step_attempts(project_id, automation_run_id, node_id, attempt_number DESC);

CREATE TABLE review_artifact_lineage (
  project_id uuid NOT NULL,
  artifact_id uuid NOT NULL,
  predecessor_artifact_id uuid NOT NULL,
  PRIMARY KEY (project_id, artifact_id, predecessor_artifact_id),
  FOREIGN KEY (project_id, artifact_id)
    REFERENCES review_artifacts(project_id, id) ON DELETE CASCADE,
  FOREIGN KEY (project_id, predecessor_artifact_id)
    REFERENCES review_artifacts(project_id, id)
    DEFERRABLE INITIALLY DEFERRED,
  CHECK (artifact_id <> predecessor_artifact_id)
);

ALTER TABLE automation_step_runs
  ADD COLUMN accepted_attempt_id uuid REFERENCES review_step_attempts(id)
    DEFERRABLE INITIALLY DEFERRED,
  ADD COLUMN input_fingerprint text
    CHECK (input_fingerprint IS NULL OR input_fingerprint ~ '^[0-9a-f]{64}$');

CREATE TABLE review_proposal_finalizations (
  project_id uuid NOT NULL,
  automation_run_id uuid NOT NULL,
  candidate_hash text NOT NULL CHECK (candidate_hash ~ '^[0-9a-f]{64}$'),
  proposal_id uuid NOT NULL,
  model_run_id uuid NOT NULL,
  created_at timestamptz NOT NULL DEFAULT now(),
  PRIMARY KEY (project_id, automation_run_id, candidate_hash),
  UNIQUE (proposal_id),
  FOREIGN KEY (project_id, automation_run_id)
    REFERENCES review_run_manifests(project_id, automation_run_id) ON DELETE CASCADE,
  FOREIGN KEY (project_id, proposal_id)
    REFERENCES ai_proposals(project_id, id)
    DEFERRABLE INITIALLY DEFERRED,
  FOREIGN KEY (model_run_id, project_id)
    REFERENCES ai_runs(id, project_id)
    DEFERRABLE INITIALLY DEFERRED
);

CREATE OR REPLACE FUNCTION reject_immutable_review_manifest_changes()
RETURNS trigger
LANGUAGE plpgsql
AS $$
BEGIN
  IF NEW.project_id IS DISTINCT FROM OLD.project_id
      OR NEW.automation_run_id IS DISTINCT FROM OLD.automation_run_id
      OR NEW.definition_key IS DISTINCT FROM OLD.definition_key
      OR NEW.definition_id IS DISTINCT FROM OLD.definition_id
      OR NEW.definition_version IS DISTINCT FROM OLD.definition_version
      OR NEW.manifest_hash IS DISTINCT FROM OLD.manifest_hash
      OR NEW.semantic_bundle_hash IS DISTINCT FROM OLD.semantic_bundle_hash
      OR NEW.manifest IS DISTINCT FROM OLD.manifest
      OR NEW.subject IS DISTINCT FROM OLD.subject
      OR NEW.origin IS DISTINCT FROM OLD.origin
      OR NEW.prepared_task IS DISTINCT FROM OLD.prepared_task
      OR NEW.created_at IS DISTINCT FROM OLD.created_at THEN
    RAISE EXCEPTION 'review run manifest identity is immutable'
      USING ERRCODE = 'integrity_constraint_violation';
  END IF;
  RETURN NEW;
END;
$$;

CREATE TRIGGER review_run_manifest_immutable_trigger
BEFORE UPDATE ON review_run_manifests
FOR EACH ROW EXECUTE FUNCTION reject_immutable_review_manifest_changes();

CREATE OR REPLACE FUNCTION reject_terminal_review_attempt_changes()
RETURNS trigger
LANGUAGE plpgsql
AS $$
BEGIN
  IF OLD.status IN ('completed', 'failed') THEN
    RAISE EXCEPTION 'terminal review step attempts are immutable'
      USING ERRCODE = 'integrity_constraint_violation';
  END IF;
  IF NEW.project_id IS DISTINCT FROM OLD.project_id
      OR NEW.automation_run_id IS DISTINCT FROM OLD.automation_run_id
      OR NEW.node_id IS DISTINCT FROM OLD.node_id
      OR NEW.node_version IS DISTINCT FROM OLD.node_version
      OR NEW.attempt_number IS DISTINCT FROM OLD.attempt_number
      OR NEW.input_fingerprint IS DISTINCT FROM OLD.input_fingerprint
      OR NEW.worker_id IS DISTINCT FROM OLD.worker_id
      OR NEW.started_at IS DISTINCT FROM OLD.started_at THEN
    RAISE EXCEPTION 'review step attempt identity is immutable'
      USING ERRCODE = 'integrity_constraint_violation';
  END IF;
  RETURN NEW;
END;
$$;

CREATE TRIGGER review_step_attempt_terminal_immutable_trigger
BEFORE UPDATE ON review_step_attempts
FOR EACH ROW EXECUTE FUNCTION reject_terminal_review_attempt_changes();

CREATE OR REPLACE FUNCTION reject_review_artifact_changes()
RETURNS trigger
LANGUAGE plpgsql
AS $$
BEGIN
  RAISE EXCEPTION 'review artifacts are immutable'
    USING ERRCODE = 'integrity_constraint_violation';
END;
$$;

CREATE TRIGGER review_artifact_immutable_trigger
BEFORE UPDATE ON review_artifacts
FOR EACH ROW EXECUTE FUNCTION reject_review_artifact_changes();
