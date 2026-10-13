-- PR14 typed automation persistence core.
--
-- This migration deliberately stores recipe steps relationally.  The public
-- application API selects a built-in recipe; callers cannot submit an
-- arbitrary JSON workflow or step list.

CREATE EXTENSION IF NOT EXISTS pgcrypto;

-- Existing jobs predate explicit queue ownership. Backfill every durable job
-- shape that can still be active, then require all newly queued/running jobs
-- to carry a project. The NOT VALID constraint deliberately tolerates
-- terminal historical rows whose payload cannot establish ownership.
ALTER TABLE jobs
  ADD COLUMN IF NOT EXISTS project_id uuid REFERENCES projects(id) ON DELETE CASCADE;

UPDATE jobs
SET project_id = CASE kind
  WHEN 'work_fetch_requested' THEN NULLIF(payload #>> '{payload,project_id}', '')::uuid
  WHEN 'recompute_metrics' THEN NULLIF(payload #>> '{payload,project_id}', '')::uuid
  ELSE project_id
END
WHERE project_id IS NULL
  AND kind IN ('work_fetch_requested', 'recompute_metrics');

UPDATE jobs AS job
SET project_id = document.project_id
FROM documents AS document
WHERE job.project_id IS NULL
  AND job.kind IN ('parse_document', 'retrieve_document')
  AND document.id = NULLIF(job.payload ->> 'document_id', '')::uuid;

ALTER TABLE jobs
  DROP CONSTRAINT IF EXISTS jobs_active_project_id_check;
ALTER TABLE jobs
  ADD CONSTRAINT jobs_active_project_id_check
  CHECK (project_id IS NOT NULL OR state IN ('completed', 'failed', 'dead'))
  NOT VALID;

ALTER TABLE jobs
  DROP CONSTRAINT IF EXISTS jobs_project_id_id_key;
ALTER TABLE jobs
  ADD CONSTRAINT jobs_project_id_id_key UNIQUE (project_id, id);

CREATE OR REPLACE FUNCTION automation_uuid_from_text(prefix text, identity text)
RETURNS uuid
LANGUAGE sql
IMMUTABLE
AS $$
  SELECT format(
    '%s-%s-%s-%s-%s',
    substr(md5(prefix || ':' || identity), 1, 8),
    substr(md5(prefix || ':' || identity), 9, 4),
    substr(md5(prefix || ':' || identity), 13, 4),
    substr(md5(prefix || ':' || identity), 17, 4),
    substr(md5(prefix || ':' || identity), 21, 12)
  )::uuid
$$;

CREATE TABLE automation_definitions (
  id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
  project_id uuid NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
  name text NOT NULL
    CHECK (length(btrim(name)) BETWEEN 1 AND 200),
  trigger_kind text NOT NULL CHECK (trigger_kind IN (
    'report_added', 'acquisition_completed', 'full_text_attached',
    'report_included', 'study_created', 'appraisal_completed', 'manual'
  )),
  recipe_id text NOT NULL CHECK (recipe_id = 'project_maintenance'),
  recipe_version integer NOT NULL CHECK (recipe_version = 1),
  status text NOT NULL CHECK (status IN ('active', 'paused')),
  actor_kind text NOT NULL CHECK (actor_kind IN ('user', 'automation', 'system')),
  actor_id text NOT NULL CHECK (length(btrim(actor_id)) BETWEEN 1 AND 200),
  created_at timestamptz NOT NULL DEFAULT now(),
  updated_at timestamptz NOT NULL DEFAULT now(),
  CONSTRAINT automation_definitions_project_id_key UNIQUE (project_id, id),
  CONSTRAINT automation_definitions_project_name_key UNIQUE (project_id, name)
);

CREATE INDEX automation_definitions_project_trigger_idx
  ON automation_definitions(project_id, trigger_kind, status, name);

CREATE TABLE automation_definition_steps (
  project_id uuid NOT NULL,
  definition_id uuid NOT NULL,
  ordinal integer NOT NULL CHECK (ordinal BETWEEN 0 AND 99),
  step_key text NOT NULL CHECK (length(btrim(step_key)) BETWEEN 1 AND 200),
  step_kind text NOT NULL CHECK (step_kind IN (
    'deterministic_action', 'ai_task', 'agent', 'notification',
    'domain_command', 'domain_proposal'
  )),
  PRIMARY KEY (project_id, definition_id, ordinal),
  UNIQUE (project_id, definition_id, step_key),
  FOREIGN KEY (project_id, definition_id)
    REFERENCES automation_definitions(project_id, id) ON DELETE CASCADE
);

CREATE TABLE automation_runs (
  id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
  project_id uuid NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
  definition_id uuid NOT NULL,
  job_id uuid NOT NULL,
  recipe_id text NOT NULL CHECK (recipe_id = 'project_maintenance'),
  recipe_version integer NOT NULL CHECK (recipe_version = 1),
  trigger_kind text NOT NULL CHECK (trigger_kind IN (
    'report_added', 'acquisition_completed', 'full_text_attached',
    'report_included', 'study_created', 'appraisal_completed', 'manual'
  )),
  trigger_reference text
    CHECK (trigger_reference IS NULL OR length(btrim(trigger_reference)) BETWEEN 1 AND 500),
  idempotency_key text NOT NULL
    CHECK (length(btrim(idempotency_key)) BETWEEN 1 AND 200),
  actor_kind text NOT NULL CHECK (actor_kind IN ('user', 'automation', 'system')),
  actor_id text NOT NULL CHECK (length(btrim(actor_id)) BETWEEN 1 AND 200),
  status text NOT NULL DEFAULT 'queued'
    CHECK (status IN ('queued', 'running', 'completed', 'failed')),
  created_at timestamptz NOT NULL DEFAULT now(),
  started_at timestamptz,
  finished_at timestamptz,
  error text,
  CONSTRAINT automation_runs_project_id_key UNIQUE (project_id, id),
  CONSTRAINT automation_runs_idempotency_key
    UNIQUE (project_id, definition_id, trigger_kind, idempotency_key),
  FOREIGN KEY (project_id, definition_id)
    REFERENCES automation_definitions(project_id, id) ON DELETE RESTRICT,
  FOREIGN KEY (project_id, job_id)
    REFERENCES jobs(project_id, id) ON DELETE RESTRICT,
  CHECK (error IS NULL OR length(btrim(error)) BETWEEN 1 AND 4096),
  CHECK (
    (status = 'queued' AND started_at IS NULL AND finished_at IS NULL AND error IS NULL)
    OR (status = 'running' AND started_at IS NOT NULL AND finished_at IS NULL AND error IS NULL)
    OR (status = 'completed' AND started_at IS NOT NULL AND finished_at IS NOT NULL AND error IS NULL)
    OR (status = 'failed' AND started_at IS NOT NULL AND finished_at IS NOT NULL AND error IS NOT NULL)
  )
);

CREATE INDEX automation_runs_project_created_idx
  ON automation_runs(project_id, created_at DESC, id DESC);
CREATE INDEX automation_runs_job_idx
  ON automation_runs(project_id, job_id);

CREATE TABLE automation_step_runs (
  id uuid NOT NULL DEFAULT gen_random_uuid(),
  project_id uuid NOT NULL,
  automation_run_id uuid NOT NULL,
  ordinal integer NOT NULL CHECK (ordinal BETWEEN 0 AND 99),
  step_key text NOT NULL CHECK (length(btrim(step_key)) BETWEEN 1 AND 200),
  step_kind text NOT NULL CHECK (step_kind IN (
    'deterministic_action', 'ai_task', 'agent', 'notification',
    'domain_command', 'domain_proposal'
  )),
  status text NOT NULL DEFAULT 'pending'
    CHECK (status IN ('pending', 'running', 'completed', 'failed')),
  attempts integer NOT NULL DEFAULT 0 CHECK (attempts >= 0),
  claimed_by text CHECK (claimed_by IS NULL OR length(btrim(claimed_by)) BETWEEN 1 AND 200),
  claimed_job_attempt integer,
  started_at timestamptz,
  finished_at timestamptz,
  error text,
  output jsonb,
  PRIMARY KEY (project_id, automation_run_id, ordinal),
  UNIQUE (project_id, automation_run_id, step_key),
  UNIQUE (id),
  FOREIGN KEY (project_id, automation_run_id)
    REFERENCES automation_runs(project_id, id) ON DELETE CASCADE,
  CHECK (claimed_job_attempt IS NULL OR claimed_job_attempt > 0),
  CHECK (error IS NULL OR length(btrim(error)) BETWEEN 1 AND 4096),
  CHECK (output IS NULL OR jsonb_typeof(output) = 'object'),
  CHECK (
    (status = 'pending' AND started_at IS NULL AND finished_at IS NULL
      AND claimed_by IS NULL AND claimed_job_attempt IS NULL AND error IS NULL)
    OR (status = 'running' AND attempts > 0 AND started_at IS NOT NULL
      AND finished_at IS NULL AND claimed_by IS NOT NULL
      AND claimed_job_attempt IS NOT NULL AND error IS NULL)
    OR (status = 'completed' AND attempts > 0 AND started_at IS NOT NULL
      AND finished_at IS NOT NULL AND claimed_by IS NULL
      AND claimed_job_attempt IS NULL AND error IS NULL)
    OR (status = 'failed' AND attempts > 0 AND started_at IS NOT NULL
      AND finished_at IS NOT NULL AND claimed_by IS NULL
      AND claimed_job_attempt IS NULL AND error IS NOT NULL)
  )
);

CREATE INDEX automation_step_runs_next_idx
  ON automation_step_runs(project_id, automation_run_id, status, ordinal);

ALTER TABLE ai_runs
  ALTER COLUMN project_id SET NOT NULL;
ALTER TABLE ai_runs
  DROP CONSTRAINT IF EXISTS ai_runs_parent_automation_project_fk,
  DROP CONSTRAINT IF EXISTS ai_runs_parent_automation_run_fk;
ALTER TABLE ai_runs
  ADD CONSTRAINT ai_runs_parent_automation_run_fk
  FOREIGN KEY (parent_automation_run_id)
  REFERENCES automation_runs(id) ON DELETE SET NULL;

CREATE OR REPLACE FUNCTION enforce_ai_run_parent_automation_project()
RETURNS trigger
LANGUAGE plpgsql
AS $$
BEGIN
  IF NEW.parent_automation_run_id IS NOT NULL
      AND NOT EXISTS (
        SELECT 1
        FROM automation_runs AS r
        WHERE r.id = NEW.parent_automation_run_id
          AND r.project_id = NEW.project_id
      ) THEN
    RAISE EXCEPTION 'AI run parent automation run belongs to another project'
      USING ERRCODE = 'foreign_key_violation';
  END IF;
  RETURN NEW;
END;
$$;

DROP TRIGGER IF EXISTS ai_run_parent_automation_project_trigger ON ai_runs;
CREATE TRIGGER ai_run_parent_automation_project_trigger
BEFORE INSERT OR UPDATE OF project_id, parent_automation_run_id ON ai_runs
FOR EACH ROW EXECUTE FUNCTION enforce_ai_run_parent_automation_project();

CREATE INDEX ai_runs_parent_automation_idx
  ON ai_runs(parent_automation_run_id, project_id);

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
  SELECT
    0,
    'recompute_project_metrics'::text,
    'deterministic_action'::text
  WHERE target_recipe_id = 'project_maintenance'
    AND target_recipe_version = 1
$$;

CREATE OR REPLACE FUNCTION configure_automation_definition(
  target_project_id uuid,
  target_name text,
  target_trigger_kind text,
  target_recipe_id text,
  target_recipe_version integer,
  target_status text,
  target_actor_kind text,
  target_actor_id text
)
RETURNS TABLE (
  id uuid,
  project_id uuid,
  name text,
  trigger_kind text,
  recipe_id text,
  recipe_version integer,
  status text,
  actor_kind text,
  actor_id text,
  created_at timestamptz,
  updated_at timestamptz
)
LANGUAGE plpgsql
AS $$
DECLARE
  definition_row_id uuid;
BEGIN
  IF NOT EXISTS (SELECT 1 FROM projects WHERE projects.id = target_project_id) THEN
    RAISE EXCEPTION 'automation project does not exist'
      USING ERRCODE = 'foreign_key_violation';
  END IF;
  IF target_name IS NULL OR length(btrim(target_name)) NOT BETWEEN 1 AND 200 THEN
    RAISE EXCEPTION 'automation name is invalid' USING ERRCODE = 'invalid_parameter_value';
  END IF;
  IF target_trigger_kind IS NULL OR target_trigger_kind NOT IN (
    'report_added', 'acquisition_completed', 'full_text_attached',
    'report_included', 'study_created', 'appraisal_completed', 'manual'
  ) THEN
    RAISE EXCEPTION 'automation trigger kind is unknown' USING ERRCODE = 'invalid_parameter_value';
  END IF;
  IF target_status IS NULL OR target_status NOT IN ('active', 'paused') THEN
    RAISE EXCEPTION 'automation definition status is unknown' USING ERRCODE = 'invalid_parameter_value';
  END IF;
  IF target_actor_kind IS NULL OR target_actor_kind NOT IN ('user', 'automation', 'system')
      OR target_actor_id IS NULL OR length(btrim(target_actor_id)) NOT BETWEEN 1 AND 200 THEN
    RAISE EXCEPTION 'automation actor is invalid' USING ERRCODE = 'invalid_parameter_value';
  END IF;
  IF NOT EXISTS (
    SELECT 1
    FROM automation_builtin_recipe_steps(target_recipe_id, target_recipe_version)
  ) THEN
    RAISE EXCEPTION 'automation recipe is not a supported built-in recipe'
      USING ERRCODE = 'invalid_parameter_value';
  END IF;

  INSERT INTO automation_definitions
    (project_id, name, trigger_kind, recipe_id, recipe_version, status,
     actor_kind, actor_id)
  VALUES
    (target_project_id, target_name, target_trigger_kind, target_recipe_id,
     target_recipe_version, target_status, target_actor_kind, target_actor_id)
  ON CONFLICT ON CONSTRAINT automation_definitions_project_name_key DO UPDATE SET
    trigger_kind = EXCLUDED.trigger_kind,
    recipe_id = EXCLUDED.recipe_id,
    recipe_version = EXCLUDED.recipe_version,
    status = EXCLUDED.status,
    actor_kind = EXCLUDED.actor_kind,
    actor_id = EXCLUDED.actor_id,
    updated_at = now()
  RETURNING automation_definitions.id INTO definition_row_id;

  DELETE FROM automation_definition_steps
  WHERE automation_definition_steps.project_id = target_project_id
    AND automation_definition_steps.definition_id = definition_row_id;

  INSERT INTO automation_definition_steps
    (project_id, definition_id, ordinal, step_key, step_kind)
  SELECT target_project_id, definition_row_id, steps.ordinal, steps.step_key,
         steps.step_kind
  FROM automation_builtin_recipe_steps(target_recipe_id, target_recipe_version) AS steps;

  RETURN QUERY
  SELECT d.id, d.project_id, d.name, d.trigger_kind, d.recipe_id, d.recipe_version,
         d.status, d.actor_kind, d.actor_id, d.created_at, d.updated_at
  FROM automation_definitions AS d
  WHERE d.project_id = target_project_id AND d.id = definition_row_id;
END;
$$;

CREATE OR REPLACE FUNCTION list_automation_definitions(target_project_id uuid)
RETURNS TABLE (
  id uuid,
  project_id uuid,
  name text,
  trigger_kind text,
  recipe_id text,
  recipe_version integer,
  status text,
  actor_kind text,
  actor_id text,
  created_at timestamptz,
  updated_at timestamptz,
  steps jsonb
)
LANGUAGE sql
STABLE
AS $$
  SELECT d.id, d.project_id, d.name, d.trigger_kind, d.recipe_id, d.recipe_version,
         d.status, d.actor_kind, d.actor_id, d.created_at, d.updated_at,
         COALESCE(
           jsonb_agg(
             jsonb_build_object(
               'ordinal', s.ordinal,
               'key', s.step_key,
               'kind', s.step_kind
             ) ORDER BY s.ordinal
           ) FILTER (WHERE s.ordinal IS NOT NULL),
           '[]'::jsonb
         ) AS steps
  FROM automation_definitions AS d
  LEFT JOIN automation_definition_steps AS s
    ON s.project_id = d.project_id AND s.definition_id = d.id
  WHERE d.project_id = target_project_id
  GROUP BY d.id
  ORDER BY d.name, d.id
$$;

CREATE OR REPLACE FUNCTION dispatch_automation_trigger(
  target_project_id uuid,
  target_definition_id uuid,
  target_trigger_kind text,
  target_trigger_reference text,
  target_idempotency_key text,
  target_actor_kind text,
  target_actor_id text
)
RETURNS TABLE (
  run_id uuid,
  job_id uuid,
  created boolean
)
LANGUAGE plpgsql
AS $$
DECLARE
  existing_run_id uuid;
  existing_job_id uuid;
  definition_row automation_definitions%ROWTYPE;
  identity text;
  stable_run_id uuid;
  stable_job_id uuid;
  persisted_job_project_id uuid;
  persisted_job_kind text;
  step_count bigint;
BEGIN
  IF target_project_id IS NULL OR target_project_id = '00000000-0000-0000-0000-000000000000'::uuid THEN
    RAISE EXCEPTION 'automation project id is invalid' USING ERRCODE = 'invalid_parameter_value';
  END IF;
  IF target_definition_id IS NULL OR target_definition_id = '00000000-0000-0000-0000-000000000000'::uuid THEN
    RAISE EXCEPTION 'automation definition id is invalid' USING ERRCODE = 'invalid_parameter_value';
  END IF;
  IF target_trigger_kind IS NULL OR target_trigger_kind NOT IN (
    'report_added', 'acquisition_completed', 'full_text_attached',
    'report_included', 'study_created', 'appraisal_completed', 'manual'
  ) THEN
    RAISE EXCEPTION 'automation trigger kind is unknown' USING ERRCODE = 'invalid_parameter_value';
  END IF;
  IF target_trigger_reference IS NOT NULL
      AND length(btrim(target_trigger_reference)) NOT BETWEEN 1 AND 500 THEN
    RAISE EXCEPTION 'automation trigger reference is invalid'
      USING ERRCODE = 'invalid_parameter_value';
  END IF;
  IF target_idempotency_key IS NULL
      OR length(btrim(target_idempotency_key)) NOT BETWEEN 1 AND 200 THEN
    RAISE EXCEPTION 'automation idempotency key is invalid'
      USING ERRCODE = 'invalid_parameter_value';
  END IF;
  IF target_actor_kind IS NULL OR target_actor_kind NOT IN ('user', 'automation', 'system')
      OR target_actor_id IS NULL OR length(btrim(target_actor_id)) NOT BETWEEN 1 AND 200 THEN
    RAISE EXCEPTION 'automation actor is invalid' USING ERRCODE = 'invalid_parameter_value';
  END IF;

  -- Look up the idempotency key before checking whether the definition is
  -- still active: a retry of an already-created run must return its durable
  -- identity even if an administrator paused the definition meanwhile.
  SELECT r.id, r.job_id
  INTO existing_run_id, existing_job_id
  FROM automation_runs AS r
  WHERE r.project_id = target_project_id
    AND r.definition_id = target_definition_id
    AND r.trigger_kind = target_trigger_kind
    AND r.idempotency_key = target_idempotency_key
  FOR UPDATE;
  IF existing_run_id IS NOT NULL THEN
    RETURN QUERY SELECT existing_run_id, existing_job_id, false;
    RETURN;
  END IF;

  SELECT d.*
  INTO definition_row
  FROM automation_definitions AS d
  WHERE d.project_id = target_project_id
    AND d.id = target_definition_id
    AND d.trigger_kind = target_trigger_kind
  FOR SHARE;
  IF NOT FOUND THEN
    RAISE EXCEPTION 'automation definition does not exist for this project and trigger'
      USING ERRCODE = 'no_data_found';
  END IF;
  IF definition_row.status <> 'active' THEN
    RAISE EXCEPTION 'automation definition is paused'
      USING ERRCODE = 'object_not_in_prerequisite_state';
  END IF;

  identity := target_project_id::text || ':' || target_definition_id::text || ':'
    || target_trigger_kind || ':' || target_idempotency_key;
  stable_run_id := automation_uuid_from_text('automation-run', identity);
  stable_job_id := automation_uuid_from_text('automation-job', identity);

  INSERT INTO jobs
    (id, project_id, kind, payload, state, priority, max_attempts, dedupe_key)
  VALUES
    (stable_job_id, target_project_id, 'automation_run',
     jsonb_build_object('automation_run_id', stable_run_id),
     'queued', 0, 5, 'automation:' || identity)
  ON CONFLICT (dedupe_key) DO UPDATE SET id = jobs.id
  RETURNING jobs.id, jobs.project_id, jobs.kind
  INTO stable_job_id, persisted_job_project_id, persisted_job_kind;

  IF persisted_job_project_id IS DISTINCT FROM target_project_id
      OR persisted_job_kind <> 'automation_run' THEN
    RAISE EXCEPTION 'automation job dedupe key belongs to another job'
      USING ERRCODE = 'unique_violation';
  END IF;

  INSERT INTO automation_runs
    (id, project_id, definition_id, job_id, recipe_id, recipe_version,
     trigger_kind, trigger_reference, idempotency_key, actor_kind, actor_id)
  VALUES
    (stable_run_id, target_project_id, target_definition_id, stable_job_id,
     definition_row.recipe_id, definition_row.recipe_version, target_trigger_kind,
     target_trigger_reference, target_idempotency_key, target_actor_kind, target_actor_id)
  ON CONFLICT ON CONSTRAINT automation_runs_idempotency_key DO NOTHING
  RETURNING automation_runs.id INTO existing_run_id;

  IF existing_run_id IS NULL THEN
    SELECT r.id, r.job_id
    INTO existing_run_id, existing_job_id
    FROM automation_runs AS r
    WHERE r.project_id = target_project_id
      AND r.definition_id = target_definition_id
      AND r.trigger_kind = target_trigger_kind
      AND r.idempotency_key = target_idempotency_key
    FOR UPDATE;
    RETURN QUERY SELECT existing_run_id, existing_job_id, false;
    RETURN;
  END IF;

  INSERT INTO automation_step_runs
    (id, project_id, automation_run_id, ordinal, step_key, step_kind)
  SELECT
    automation_uuid_from_text('automation-step', identity || ':' || steps.ordinal::text),
    target_project_id, existing_run_id, steps.ordinal, steps.step_key,
    steps.step_kind
  FROM automation_definition_steps AS steps
  WHERE steps.project_id = target_project_id
    AND steps.definition_id = target_definition_id
  ORDER BY steps.ordinal;
  GET DIAGNOSTICS step_count = ROW_COUNT;
  IF step_count = 0 THEN
    RAISE EXCEPTION 'automation definition has no persisted steps'
      USING ERRCODE = 'object_not_in_prerequisite_state';
  END IF;

  RETURN QUERY SELECT existing_run_id, stable_job_id, true;
END;
$$;

CREATE OR REPLACE FUNCTION start_automation_manually(
  target_project_id uuid,
  target_definition_id uuid,
  target_idempotency_key text,
  target_actor_kind text,
  target_actor_id text
)
RETURNS TABLE (
  run_id uuid,
  job_id uuid,
  created boolean
)
LANGUAGE sql
AS $$
  SELECT *
  FROM dispatch_automation_trigger(
    target_project_id,
    target_definition_id,
    'manual',
    NULL,
    target_idempotency_key,
    target_actor_kind,
    target_actor_id
  )
$$;

CREATE OR REPLACE VIEW automation_run_details AS
SELECT
  r.id AS run_id,
  r.project_id,
  r.definition_id,
  d.name AS definition_name,
  r.job_id,
  r.recipe_id,
  r.recipe_version,
  r.trigger_kind,
  r.trigger_reference,
  r.idempotency_key,
  r.actor_kind,
  r.actor_id,
  r.status,
  r.created_at,
  r.started_at,
  r.finished_at,
  r.error AS run_error,
  j.state AS job_state,
  j.attempts AS job_attempts,
  j.max_attempts AS job_max_attempts,
  j.available_at AS job_available_at,
  j.leased_until AS job_leased_until,
  j.last_error AS job_last_error,
  COALESCE(step_data.steps, '[]'::jsonb) AS steps,
  COALESCE(ai_usage.input_tokens, 0)::bigint AS input_tokens,
  COALESCE(ai_usage.output_tokens, 0)::bigint AS output_tokens,
  COALESCE(ai_usage.cost_micros, 0)::bigint AS cost_micros
FROM automation_runs AS r
JOIN automation_definitions AS d
  ON d.project_id = r.project_id AND d.id = r.definition_id
JOIN jobs AS j
  ON j.project_id = r.project_id AND j.id = r.job_id
LEFT JOIN LATERAL (
  SELECT jsonb_agg(
    jsonb_build_object(
      'id', s.id,
      'ordinal', s.ordinal,
      'key', s.step_key,
      'kind', s.step_kind,
      'status', s.status,
      'attempts', s.attempts,
      'claimed_by', s.claimed_by,
      'started_at', s.started_at,
      'finished_at', s.finished_at,
      'error', s.error
    ) ORDER BY s.ordinal
  ) AS steps
  FROM automation_step_runs AS s
  WHERE s.project_id = r.project_id AND s.automation_run_id = r.id
) AS step_data ON true
LEFT JOIN LATERAL (
  SELECT
    sum(a.input_tokens) AS input_tokens,
    sum(a.output_tokens) AS output_tokens,
    sum(a.cost_micros) AS cost_micros
  FROM ai_runs AS a
  WHERE a.project_id = r.project_id
    AND a.parent_automation_run_id = r.id
) AS ai_usage ON true;

CREATE OR REPLACE FUNCTION list_automation_runs(
  target_project_id uuid,
  requested_limit integer
)
RETURNS SETOF automation_run_details
LANGUAGE plpgsql
STABLE
AS $$
BEGIN
  IF requested_limit NOT BETWEEN 1 AND 100 THEN
    RAISE EXCEPTION 'automation run list limit is invalid'
      USING ERRCODE = 'invalid_parameter_value';
  END IF;
  RETURN QUERY
  SELECT *
  FROM automation_run_details
  WHERE automation_run_details.project_id = target_project_id
  ORDER BY created_at DESC, run_id DESC
  LIMIT requested_limit;
END;
$$;

CREATE OR REPLACE FUNCTION get_automation_run(
  target_project_id uuid,
  target_run_id uuid
)
RETURNS SETOF automation_run_details
LANGUAGE sql
STABLE
AS $$
  SELECT *
  FROM automation_run_details
  WHERE automation_run_details.project_id = target_project_id
    AND automation_run_details.run_id = target_run_id
$$;

CREATE OR REPLACE FUNCTION enforce_automation_run_transition()
RETURNS trigger
LANGUAGE plpgsql
AS $$
BEGIN
  IF OLD.status = NEW.status THEN
    RETURN NEW;
  END IF;
  IF NOT (
    (OLD.status = 'queued' AND NEW.status = 'running')
    OR (OLD.status = 'running' AND NEW.status IN ('completed', 'failed'))
    OR (OLD.status = 'failed' AND NEW.status = 'queued')
  ) THEN
    RAISE EXCEPTION 'invalid automation run transition from % to %', OLD.status, NEW.status
      USING ERRCODE = 'invalid_parameter_value';
  END IF;
  RETURN NEW;
END;
$$;

DROP TRIGGER IF EXISTS automation_run_transition_trigger ON automation_runs;
CREATE TRIGGER automation_run_transition_trigger
BEFORE UPDATE OF status ON automation_runs
FOR EACH ROW EXECUTE FUNCTION enforce_automation_run_transition();

CREATE OR REPLACE FUNCTION enforce_automation_step_transition()
RETURNS trigger
LANGUAGE plpgsql
AS $$
BEGIN
  IF OLD.status = NEW.status THEN
    RETURN NEW;
  END IF;
  IF NOT (
    (OLD.status = 'pending' AND NEW.status = 'running')
    OR (OLD.status = 'running' AND NEW.status IN ('completed', 'failed'))
    OR (OLD.status = 'failed' AND NEW.status = 'running')
  ) THEN
    RAISE EXCEPTION 'invalid automation step transition from % to %', OLD.status, NEW.status
      USING ERRCODE = 'invalid_parameter_value';
  END IF;
  RETURN NEW;
END;
$$;

DROP TRIGGER IF EXISTS automation_step_transition_trigger ON automation_step_runs;
CREATE TRIGGER automation_step_transition_trigger
BEFORE UPDATE OF status ON automation_step_runs
FOR EACH ROW EXECUTE FUNCTION enforce_automation_step_transition();

CREATE OR REPLACE FUNCTION enforce_automation_run_snapshot()
RETURNS trigger
LANGUAGE plpgsql
AS $$
BEGIN
  IF OLD.project_id IS DISTINCT FROM NEW.project_id
      OR OLD.definition_id IS DISTINCT FROM NEW.definition_id
      OR OLD.job_id IS DISTINCT FROM NEW.job_id
      OR OLD.recipe_id IS DISTINCT FROM NEW.recipe_id
      OR OLD.recipe_version IS DISTINCT FROM NEW.recipe_version
      OR OLD.trigger_kind IS DISTINCT FROM NEW.trigger_kind
      OR OLD.trigger_reference IS DISTINCT FROM NEW.trigger_reference
      OR OLD.idempotency_key IS DISTINCT FROM NEW.idempotency_key
      OR OLD.actor_kind IS DISTINCT FROM NEW.actor_kind
      OR OLD.actor_id IS DISTINCT FROM NEW.actor_id THEN
    RAISE EXCEPTION 'automation run identity and recipe snapshot are immutable'
      USING ERRCODE = 'invalid_parameter_value';
  END IF;
  RETURN NEW;
END;
$$;

DROP TRIGGER IF EXISTS automation_run_snapshot_trigger ON automation_runs;
CREATE TRIGGER automation_run_snapshot_trigger
BEFORE UPDATE ON automation_runs
FOR EACH ROW EXECUTE FUNCTION enforce_automation_run_snapshot();

CREATE OR REPLACE FUNCTION enforce_automation_step_snapshot()
RETURNS trigger
LANGUAGE plpgsql
AS $$
BEGIN
  IF OLD.project_id IS DISTINCT FROM NEW.project_id
      OR OLD.automation_run_id IS DISTINCT FROM NEW.automation_run_id
      OR OLD.ordinal IS DISTINCT FROM NEW.ordinal
      OR OLD.step_key IS DISTINCT FROM NEW.step_key
      OR OLD.step_kind IS DISTINCT FROM NEW.step_kind THEN
    RAISE EXCEPTION 'automation step recipe snapshot is immutable'
      USING ERRCODE = 'invalid_parameter_value';
  END IF;
  RETURN NEW;
END;
$$;

DROP TRIGGER IF EXISTS automation_step_snapshot_trigger ON automation_step_runs;
CREATE TRIGGER automation_step_snapshot_trigger
BEFORE UPDATE ON automation_step_runs
FOR EACH ROW EXECUTE FUNCTION enforce_automation_step_snapshot();
