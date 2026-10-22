-- phase: expand
-- AI usage ledger reservations, durable attribution and hierarchical limits.
-- The old ledger remains the N-1 write surface. Its insert trigger fills the
-- new tenant/period identity and adds its charge to the same monthly counters
-- that N checks before opening a provider call.

ALTER TABLE ai_usage_ledger
  DROP CONSTRAINT IF EXISTS ai_usage_ledger_project_id_fkey;

ALTER TABLE ai_usage_ledger
  ADD COLUMN IF NOT EXISTS workspace_id uuid,
  ADD COLUMN IF NOT EXISTS principal_kind text,
  ADD COLUMN IF NOT EXISTS principal_id text,
  ADD COLUMN IF NOT EXISTS run_id uuid,
  ADD COLUMN IF NOT EXISTS run_budget_id uuid,
  ADD COLUMN IF NOT EXISTS step_id text,
  ADD COLUMN IF NOT EXISTS reasoning_tokens bigint,
  ADD COLUMN IF NOT EXISTS commercial_units bigint,
  ADD COLUMN IF NOT EXISTS price_version text,
  ADD COLUMN IF NOT EXISTS current_price_version text,
  ADD COLUMN IF NOT EXISTS pricing_decision text,
  ADD COLUMN IF NOT EXISTS period_start date,
  ADD COLUMN IF NOT EXISTS reservation_id uuid,
  ADD COLUMN IF NOT EXISTS provider_request_id text,
  ADD COLUMN IF NOT EXISTS provider_response_id text;

ALTER TABLE ai_usage_ledger
  DROP CONSTRAINT IF EXISTS ai_usage_ledger_purpose_check;
ALTER TABLE ai_usage_ledger
  ADD CONSTRAINT ai_usage_ledger_purpose_check
  CHECK (purpose IN ('structured', 'chat', 'assistant')) NOT VALID;
ALTER TABLE ai_usage_ledger
  ALTER COLUMN input_tokens DROP NOT NULL,
  ALTER COLUMN output_tokens DROP NOT NULL;
ALTER TABLE ai_usage_ledger
  ADD CONSTRAINT ai_usage_ledger_period_start_present_check
  CHECK (period_start IS NOT NULL) NOT VALID;

ALTER TABLE ai_usage_ledger
  ADD CONSTRAINT ai_usage_ledger_reasoning_tokens_check
    CHECK (reasoning_tokens >= 0) NOT VALID,
  ADD CONSTRAINT ai_usage_ledger_commercial_units_check
    CHECK (commercial_units IS NULL OR commercial_units >= 0) NOT VALID,
  ADD CONSTRAINT ai_usage_ledger_principal_check
    CHECK ((principal_kind IS NULL) = (principal_id IS NULL)
       AND (principal_kind IS NULL OR principal_kind IN ('user', 'service'))) NOT VALID,
  ADD CONSTRAINT ai_usage_ledger_step_id_check
    CHECK (step_id IS NULL OR length(step_id) BETWEEN 1 AND 200) NOT VALID;

COMMENT ON COLUMN ai_usage_ledger.cost_micros IS
  'Provider cost in micro-dollars. Kept separate from nullable commercial_units.';
COMMENT ON COLUMN ai_usage_ledger.commercial_units IS
  'Provider-neutral commercial meter quantity. NULL until a commercial meter is selected.';
COMMENT ON COLUMN ai_usage_ledger.workspace_id IS
  'Immutable tenant identity retained with the five-year usage fact; not a foreign key.';
COMMENT ON COLUMN ai_usage_ledger.period_start IS
  'UTC month charged by this usage fact. A fact has exactly one period.';

-- Every persisted route must have a hard output cap before metering becomes
-- the only paid execution path. Preserve model choices and set a safe default.
UPDATE ai_model_routes
SET parameters = jsonb_set(parameters, '{max_tokens}', '2048'::jsonb, true)
WHERE jsonb_typeof(parameters) = 'object'
  AND CASE
    WHEN jsonb_typeof(parameters->'max_tokens') = 'number'
      THEN (parameters->>'max_tokens')::numeric <= 0
    ELSE TRUE
  END;

CREATE TABLE ai_usage_limits (
  scope_kind text NOT NULL CHECK (scope_kind IN ('platform', 'workspace', 'project')),
  scope_id uuid NOT NULL,
  limit_micros bigint NOT NULL CHECK (limit_micros >= 0),
  updated_at timestamptz NOT NULL DEFAULT now(),
  PRIMARY KEY (scope_kind, scope_id),
  CHECK ((scope_kind = 'platform' AND scope_id = '00000000-0000-0000-0000-000000000001'::uuid)
      OR scope_kind <> 'platform')
);

CREATE TABLE ai_usage_period_buckets (
  scope_kind text NOT NULL CHECK (scope_kind IN ('platform', 'workspace', 'project', 'run')),
  scope_id uuid NOT NULL,
  workspace_id uuid,
  project_id uuid,
  run_budget_id uuid,
  period_start date NOT NULL,
  limit_micros bigint NOT NULL CHECK (limit_micros >= 0),
  spent_micros bigint NOT NULL DEFAULT 0 CHECK (spent_micros >= 0),
  reserved_micros bigint NOT NULL DEFAULT 0 CHECK (reserved_micros >= 0),
  updated_at timestamptz NOT NULL DEFAULT now(),
  PRIMARY KEY (scope_kind, scope_id, period_start),
  CHECK (scope_kind <> 'run' OR period_start = DATE '0001-01-01'),
  CHECK (
    (scope_kind = 'platform' AND workspace_id IS NULL AND project_id IS NULL AND run_budget_id IS NULL)
    OR (scope_kind = 'workspace' AND workspace_id = scope_id AND project_id IS NULL AND run_budget_id IS NULL)
    OR (scope_kind = 'project' AND project_id = scope_id AND workspace_id IS NOT NULL AND run_budget_id IS NULL)
    OR (scope_kind = 'run' AND run_budget_id = scope_id AND workspace_id IS NOT NULL)
  )
);

CREATE TABLE ai_usage_reservations (
  id uuid PRIMARY KEY,
  workspace_id uuid NOT NULL,
  project_id uuid,
  principal_kind text,
  principal_id text,
  run_id uuid NOT NULL,
  run_budget_id uuid NOT NULL,
  step_id text,
  job_id uuid,
  attempt_fence bigint,
  profile text NOT NULL,
  provider text NOT NULL,
  model text NOT NULL,
  purpose text NOT NULL CHECK (purpose IN ('structured', 'chat', 'assistant')),
  input_token_bound bigint NOT NULL CHECK (input_token_bound >= 0),
  max_output_tokens bigint NOT NULL CHECK (max_output_tokens >= 0),
  max_reasoning_tokens bigint NOT NULL DEFAULT 0 CHECK (max_reasoning_tokens >= 0),
  input_micros_per_million_tokens bigint NOT NULL CHECK (input_micros_per_million_tokens >= 0),
  output_micros_per_million_tokens bigint NOT NULL CHECK (output_micros_per_million_tokens >= 0),
  reasoning_micros_per_million_tokens bigint CHECK (reasoning_micros_per_million_tokens IS NULL OR reasoning_micros_per_million_tokens >= 0),
  price_version text NOT NULL CHECK (length(price_version) BETWEEN 1 AND 128),
  request_fingerprint text NOT NULL CHECK (length(request_fingerprint) = 64),
  reserved_cost_micros bigint NOT NULL CHECK (reserved_cost_micros >= 0),
  period_start date NOT NULL,
  state text NOT NULL DEFAULT 'open' CHECK (state IN ('open', 'observed', 'unresolved', 'settled', 'voided')),
  observed_input_tokens bigint CHECK (observed_input_tokens IS NULL OR observed_input_tokens >= 0),
  observed_output_tokens bigint CHECK (observed_output_tokens IS NULL OR observed_output_tokens >= 0),
  observed_reasoning_tokens bigint CHECK (observed_reasoning_tokens IS NULL OR observed_reasoning_tokens >= 0),
  observed_commercial_units bigint CHECK (observed_commercial_units IS NULL OR observed_commercial_units >= 0),
  recognized_cost_micros bigint NOT NULL DEFAULT 0 CHECK (recognized_cost_micros >= 0),
  actual_cost_micros bigint CHECK (actual_cost_micros IS NULL OR actual_cost_micros >= 0),
  overage_micros bigint NOT NULL DEFAULT 0 CHECK (overage_micros >= 0),
  current_price_version text,
  pricing_decision text,
  unresolved_reason text,
  void_reason text,
  provider_request_id text,
  provider_response_id text,
  resolved_by_user_id uuid,
  resolution_reason text,
  opened_at timestamptz NOT NULL DEFAULT now(),
  settled_at timestamptz,
  CHECK ((principal_kind IS NULL) = (principal_id IS NULL)
     AND (principal_kind IS NULL OR principal_kind IN ('user', 'service'))),
  CHECK ((job_id IS NULL) = (attempt_fence IS NULL)),
  CHECK (step_id IS NULL OR length(step_id) BETWEEN 1 AND 200),
  CHECK (attempt_fence IS NULL OR attempt_fence > 0),
  CHECK (state <> 'settled' OR (actual_cost_micros IS NOT NULL AND settled_at IS NOT NULL)),
  CHECK (state <> 'voided' OR void_reason IS NOT NULL)
);

CREATE TABLE ai_usage_workspace_blocks (
  workspace_id uuid PRIMARY KEY,
  overage_micros bigint NOT NULL CHECK (overage_micros > 0),
  reservation_id uuid NOT NULL,
  blocked_at timestamptz NOT NULL DEFAULT now(),
  cleared_at timestamptz,
  cleared_by_user_id uuid,
  clear_reason text,
  CHECK ((cleared_at IS NULL) = (cleared_by_user_id IS NULL)),
  CHECK ((cleared_at IS NULL) = (clear_reason IS NULL))
);

-- This row is the admission mutex for a workspace. It is always locked after
-- the workspace period bucket, including when a settlement is for a prior
-- month, so a block cannot race a new-month reservation.
CREATE TABLE ai_usage_workspace_admission_state (
  workspace_id uuid PRIMARY KEY,
  blocked boolean NOT NULL DEFAULT false,
  updated_at timestamptz NOT NULL DEFAULT now()
);
INSERT INTO ai_usage_workspace_admission_state (workspace_id)
SELECT id FROM workspaces ON CONFLICT DO NOTHING;

CREATE TABLE ai_usage_rollout_state (
  singleton boolean PRIMARY KEY DEFAULT true CHECK (singleton),
  ai_hard_caps_enforced boolean NOT NULL DEFAULT false,
  last_legacy_ledger_insert_at timestamptz,
  hard_caps_enabled_at timestamptz,
  hard_caps_disabled_at timestamptz,
  updated_at timestamptz NOT NULL DEFAULT now()
);
INSERT INTO ai_usage_rollout_state (singleton) VALUES (true);

CREATE TABLE ai_usage_backfill_state (
  singleton boolean PRIMARY KEY DEFAULT true CHECK (singleton),
  completed_at timestamptz,
  processed_rows bigint NOT NULL DEFAULT 0 CHECK (processed_rows >= 0),
  updated_at timestamptz NOT NULL DEFAULT now()
);
INSERT INTO ai_usage_backfill_state (singleton) VALUES (true);

-- Each legacy ledger row is marked in the same transaction as its bucket
-- increment, so the bounded backfill can resume without double counting.
CREATE TABLE ai_usage_legacy_backfill (
  ledger_id uuid PRIMARY KEY,
  processed_at timestamptz NOT NULL DEFAULT now()
);

-- Detached projects keep their caps and buckets while reservations remain
-- open, observed or unresolved. IDs are historical identities, not FKs.
CREATE TABLE ai_usage_detached_projects (
  project_id uuid PRIMARY KEY,
  workspace_id uuid NOT NULL,
  detached_at timestamptz NOT NULL DEFAULT now()
);

CREATE TABLE ai_usage_corrections (
  id uuid PRIMARY KEY,
  ledger_id uuid NOT NULL,
  original_cost_micros bigint NOT NULL CHECK (original_cost_micros >= 0),
  corrected_cost_micros bigint NOT NULL CHECK (corrected_cost_micros >= 0),
  original_price_version text NOT NULL,
  corrected_price_version text NOT NULL,
  reason text NOT NULL CHECK (length(reason) BETWEEN 1 AND 500),
  created_at timestamptz NOT NULL DEFAULT now()
);

CREATE TABLE ai_usage_limit_changes (
  id uuid PRIMARY KEY,
  scope_kind text NOT NULL CHECK (scope_kind IN ('platform', 'workspace', 'project')),
  scope_id uuid NOT NULL,
  workspace_id uuid,
  project_id uuid,
  old_limit_micros bigint NOT NULL CHECK (old_limit_micros >= 0),
  new_limit_micros bigint NOT NULL CHECK (new_limit_micros >= 0),
  changed_by_user_id uuid NOT NULL,
  changed_at timestamptz NOT NULL DEFAULT now(),
  CHECK (
    (scope_kind = 'platform' AND workspace_id IS NULL AND project_id IS NULL)
    OR (scope_kind = 'workspace' AND workspace_id = scope_id AND project_id IS NULL)
    OR (scope_kind = 'project' AND workspace_id IS NOT NULL AND project_id = scope_id)
  )
);

-- Migration rule: every project's existing monthly budget remains unchanged;
-- each workspace receives the sum of its projects, and platform receives the
-- sum of workspace caps. This is the baseline N enforces after cutover.
INSERT INTO ai_usage_limits (scope_kind, scope_id, limit_micros)
SELECT 'project', p.id, p.ai_monthly_budget_micros FROM projects p
ON CONFLICT (scope_kind, scope_id) DO NOTHING;

INSERT INTO ai_usage_limits (scope_kind, scope_id, limit_micros)
SELECT 'workspace', p.workspace_id, SUM(p.ai_monthly_budget_micros)::bigint
FROM projects p WHERE p.workspace_id IS NOT NULL
GROUP BY p.workspace_id
ON CONFLICT (scope_kind, scope_id) DO NOTHING;

INSERT INTO ai_usage_limits (scope_kind, scope_id, limit_micros)
VALUES ('platform', '00000000-0000-0000-0000-000000000001',
  COALESCE((SELECT SUM(limit_micros)::bigint FROM ai_usage_limits WHERE scope_kind = 'workspace'), 0))
ON CONFLICT (scope_kind, scope_id) DO NOTHING;

INSERT INTO ai_usage_period_buckets (scope_kind, scope_id, workspace_id, project_id, period_start, limit_micros)
SELECT 'project', p.id, p.workspace_id, p.id, date_trunc('month', now() AT TIME ZONE 'UTC')::date, lim.limit_micros
FROM projects p JOIN ai_usage_limits lim ON lim.scope_kind = 'project' AND lim.scope_id = p.id
WHERE p.workspace_id IS NOT NULL
ON CONFLICT DO NOTHING;
INSERT INTO ai_usage_period_buckets (scope_kind, scope_id, workspace_id, period_start, limit_micros)
SELECT 'workspace', l.scope_id, l.scope_id, date_trunc('month', now() AT TIME ZONE 'UTC')::date, l.limit_micros
FROM ai_usage_limits l WHERE l.scope_kind = 'workspace'
ON CONFLICT DO NOTHING;
INSERT INTO ai_usage_period_buckets (scope_kind, scope_id, period_start, limit_micros)
SELECT 'platform', scope_id, date_trunc('month', now() AT TIME ZONE 'UTC')::date, limit_micros
FROM ai_usage_limits WHERE scope_kind = 'platform'
ON CONFLICT DO NOTHING;

CREATE OR REPLACE FUNCTION ai_usage_ledger_fill_legacy_identity()
RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
  IF NEW.period_start IS NULL THEN
    NEW.period_start := date_trunc('month', NEW.created_at AT TIME ZONE 'UTC')::date;
  END IF;
  IF NEW.workspace_id IS NULL AND NEW.project_id IS NOT NULL THEN
    SELECT p.workspace_id INTO NEW.workspace_id FROM projects p WHERE p.id = NEW.project_id;
  END IF;
  IF NEW.run_budget_id IS NULL THEN
    NEW.run_budget_id := NEW.run_id;
  END IF;
  RETURN NEW;
END;
$$;

CREATE TRIGGER ai_usage_ledger_fill_legacy_identity_trigger
BEFORE INSERT ON ai_usage_ledger
FOR EACH ROW EXECUTE FUNCTION ai_usage_ledger_fill_legacy_identity();

CREATE OR REPLACE FUNCTION ai_usage_ledger_count_legacy_spend()
RETURNS trigger LANGUAGE plpgsql AS $$
DECLARE
  v_workspace_id uuid;
  v_project_workspace_id uuid;
  v_period date;
  v_platform_limit bigint;
  v_workspace_limit bigint;
  v_project_limit bigint;
  v_prior_overage bigint;
  v_overage bigint;
BEGIN
  IF NEW.reservation_id IS NOT NULL THEN
    RETURN NULL;
  END IF;
  UPDATE ai_usage_rollout_state
  SET last_legacy_ledger_insert_at = now(), updated_at = now()
  WHERE singleton;
  -- A legacy budget write can already own this row. Lock it before any
  -- hierarchy bucket so it follows project → platform → workspace → project.
  IF NEW.project_id IS NOT NULL THEN
    SELECT p.workspace_id INTO v_project_workspace_id FROM projects p
    WHERE p.id = NEW.project_id FOR NO KEY UPDATE;
  END IF;
  -- N-1 writes only legacy ledger columns. Prefer the locked current tenant;
  -- the fallback covers ledger rows whose project has already been removed.
  v_workspace_id := COALESCE(v_project_workspace_id, NEW.workspace_id);
  v_period := COALESCE(NEW.period_start, date_trunc('month', NEW.created_at AT TIME ZONE 'UTC')::date);
  INSERT INTO ai_usage_period_buckets (scope_kind, scope_id, period_start, limit_micros)
  SELECT 'platform', scope_id, v_period, limit_micros FROM ai_usage_limits
  WHERE scope_kind = 'platform' AND scope_id = '00000000-0000-0000-0000-000000000001'
  ON CONFLICT DO NOTHING;
  SELECT limit_micros INTO v_platform_limit FROM ai_usage_period_buckets
    WHERE scope_kind = 'platform' AND scope_id = '00000000-0000-0000-0000-000000000001'
      AND period_start = v_period FOR UPDATE;
  IF v_platform_limit IS NULL THEN
    RAISE EXCEPTION 'AI platform usage bucket is missing for legacy spend';
  END IF;
  IF v_workspace_id IS NOT NULL THEN
    INSERT INTO ai_usage_period_buckets (scope_kind, scope_id, workspace_id, period_start, limit_micros)
    SELECT 'workspace', scope_id, scope_id, v_period, limit_micros FROM ai_usage_limits
    WHERE scope_kind = 'workspace' AND scope_id = v_workspace_id
    ON CONFLICT DO NOTHING;
    SELECT limit_micros INTO v_workspace_limit FROM ai_usage_period_buckets
      WHERE scope_kind = 'workspace' AND scope_id = v_workspace_id AND period_start = v_period FOR UPDATE;
    IF v_workspace_limit IS NULL THEN
      RAISE EXCEPTION 'AI workspace usage bucket is missing for legacy spend';
    END IF;
    INSERT INTO ai_usage_workspace_admission_state (workspace_id) VALUES (v_workspace_id)
      ON CONFLICT DO NOTHING;
    PERFORM 1 FROM ai_usage_workspace_admission_state
      WHERE workspace_id = v_workspace_id FOR UPDATE;
  END IF;
  IF v_workspace_id IS NOT NULL AND NEW.project_id IS NOT NULL THEN
    INSERT INTO ai_usage_period_buckets
      (scope_kind, scope_id, workspace_id, project_id, period_start, limit_micros)
    SELECT 'project', scope_id, v_workspace_id, scope_id, v_period, limit_micros FROM ai_usage_limits
    WHERE scope_kind = 'project' AND scope_id = NEW.project_id
    ON CONFLICT DO NOTHING;
    SELECT limit_micros INTO v_project_limit FROM ai_usage_period_buckets
      WHERE scope_kind = 'project' AND scope_id = NEW.project_id AND period_start = v_period FOR UPDATE;
    IF v_project_limit IS NULL THEN
      RAISE EXCEPTION 'AI project usage bucket is missing for legacy spend';
    END IF;
  END IF;
  SELECT COALESCE(MAX(GREATEST(0, b.spent_micros + b.reserved_micros - b.limit_micros)), 0)
  INTO v_prior_overage
  FROM ai_usage_period_buckets b
  WHERE b.period_start = v_period
    AND ((b.scope_kind = 'platform' AND b.scope_id = '00000000-0000-0000-0000-000000000001'::uuid)
      OR (v_workspace_id IS NOT NULL AND b.scope_kind = 'workspace' AND b.scope_id = v_workspace_id)
      OR (v_workspace_id IS NOT NULL AND NEW.project_id IS NOT NULL
          AND b.scope_kind = 'project' AND b.scope_id = NEW.project_id));
  UPDATE ai_usage_period_buckets SET spent_micros = spent_micros + NEW.cost_micros, updated_at = now()
    WHERE scope_kind = 'platform' AND scope_id = '00000000-0000-0000-0000-000000000001'
      AND period_start = v_period;
  IF v_workspace_id IS NOT NULL THEN
    UPDATE ai_usage_period_buckets SET spent_micros = spent_micros + NEW.cost_micros, updated_at = now()
      WHERE scope_kind = 'workspace' AND scope_id = v_workspace_id AND period_start = v_period;
  END IF;
  IF v_workspace_id IS NOT NULL AND NEW.project_id IS NOT NULL THEN
    UPDATE ai_usage_period_buckets SET spent_micros = spent_micros + NEW.cost_micros, updated_at = now()
      WHERE scope_kind = 'project' AND scope_id = NEW.project_id AND period_start = v_period;
  END IF;
  INSERT INTO ai_usage_legacy_backfill (ledger_id) VALUES (NEW.id) ON CONFLICT DO NOTHING;
  SELECT COALESCE(MAX(GREATEST(0, b.spent_micros + b.reserved_micros - b.limit_micros)), 0)
  INTO v_overage
  FROM ai_usage_period_buckets b
  WHERE b.period_start = v_period
    AND ((b.scope_kind = 'platform' AND b.scope_id = '00000000-0000-0000-0000-000000000001'::uuid)
      OR (v_workspace_id IS NOT NULL AND b.scope_kind = 'workspace' AND b.scope_id = v_workspace_id)
      OR (v_workspace_id IS NOT NULL AND NEW.project_id IS NOT NULL
          AND b.scope_kind = 'project' AND b.scope_id = NEW.project_id));
  v_overage := GREATEST(0, v_overage - v_prior_overage);
  IF v_overage > 0 AND v_workspace_id IS NOT NULL THEN
    UPDATE ai_usage_workspace_admission_state SET blocked = true, updated_at = now()
      WHERE workspace_id = v_workspace_id;
    INSERT INTO ai_usage_workspace_blocks (workspace_id, overage_micros, reservation_id)
    VALUES (v_workspace_id, v_overage, COALESCE(NEW.reservation_id, NEW.id))
    ON CONFLICT (workspace_id) DO UPDATE
      SET overage_micros = ai_usage_workspace_blocks.overage_micros + EXCLUDED.overage_micros,
          reservation_id = EXCLUDED.reservation_id,
          blocked_at = LEAST(ai_usage_workspace_blocks.blocked_at, now()),
          cleared_at = NULL, cleared_by_user_id = NULL, clear_reason = NULL;
  END IF;
  RETURN NULL;
END;
$$;

CREATE TRIGGER ai_usage_ledger_count_legacy_spend_trigger
AFTER INSERT ON ai_usage_ledger
FOR EACH ROW EXECUTE FUNCTION ai_usage_ledger_count_legacy_spend();

CREATE OR REPLACE FUNCTION ai_usage_sync_new_project_limit()
RETURNS trigger LANGUAGE plpgsql AS $$
DECLARE
  v_project_limit bigint;
  v_period date := date_trunc('month', now() AT TIME ZONE 'UTC')::date;
  v_spent bigint;
  v_reserved bigint;
BEGIN
  IF TG_OP = 'INSERT' THEN
    INSERT INTO ai_usage_limits (scope_kind, scope_id, limit_micros)
    VALUES ('project', NEW.id, NEW.ai_monthly_budget_micros) ON CONFLICT DO NOTHING;
    IF NEW.workspace_id IS NULL THEN
      RETURN NEW;
    END IF;
    INSERT INTO ai_usage_limits (scope_kind, scope_id, limit_micros)
    VALUES ('workspace', NEW.workspace_id, 0) ON CONFLICT DO NOTHING;
    INSERT INTO ai_usage_period_buckets
      (scope_kind, scope_id, workspace_id, period_start, limit_micros)
    SELECT 'workspace', NEW.workspace_id, NEW.workspace_id, v_period, limit_micros
    FROM ai_usage_limits WHERE scope_kind = 'workspace' AND scope_id = NEW.workspace_id
    ON CONFLICT DO NOTHING;
    INSERT INTO ai_usage_period_buckets
      (scope_kind, scope_id, workspace_id, project_id, period_start, limit_micros)
    VALUES ('project', NEW.id, NEW.workspace_id, NEW.id, v_period, NEW.ai_monthly_budget_micros)
    ON CONFLICT DO NOTHING;
    RETURN NEW;
  END IF;

  IF NEW.workspace_id IS NULL THEN
    RETURN NEW;
  END IF;
  -- UPDATE already holds the project row lock before this AFTER trigger
  -- reaches the hierarchy buckets, including a NULL → assigned transition.
  -- Project budget writes use the same bucket order as admissions. A child
  -- update never changes workspace or platform allowance.
  INSERT INTO ai_usage_limits (scope_kind, scope_id, limit_micros)
  VALUES ('workspace', NEW.workspace_id, 0) ON CONFLICT DO NOTHING;
  INSERT INTO ai_usage_period_buckets (scope_kind, scope_id, period_start, limit_micros)
  SELECT 'platform', scope_id, v_period, limit_micros FROM ai_usage_limits
  WHERE scope_kind = 'platform' AND scope_id = '00000000-0000-0000-0000-000000000001'
  ON CONFLICT DO NOTHING;
  INSERT INTO ai_usage_period_buckets (scope_kind, scope_id, workspace_id, period_start, limit_micros)
  SELECT 'workspace', scope_id, scope_id, v_period, limit_micros FROM ai_usage_limits
  WHERE scope_kind = 'workspace' AND scope_id = NEW.workspace_id
  ON CONFLICT DO NOTHING;
  INSERT INTO ai_usage_period_buckets
    (scope_kind, scope_id, workspace_id, project_id, period_start, limit_micros)
  SELECT 'project', scope_id, NEW.workspace_id, scope_id, v_period, limit_micros
  FROM ai_usage_limits WHERE scope_kind = 'project' AND scope_id = NEW.id
  ON CONFLICT DO NOTHING;
  PERFORM 1 FROM ai_usage_period_buckets WHERE scope_kind = 'platform'
    AND scope_id = '00000000-0000-0000-0000-000000000001' AND period_start = v_period FOR UPDATE;
  PERFORM 1 FROM ai_usage_period_buckets WHERE scope_kind = 'workspace'
    AND scope_id = NEW.workspace_id AND period_start = v_period FOR UPDATE;
  PERFORM 1 FROM ai_usage_period_buckets WHERE scope_kind = 'project'
    AND scope_id = NEW.id AND period_start = v_period FOR UPDATE;
  SELECT spent_micros, reserved_micros, limit_micros INTO v_spent, v_reserved, v_project_limit
    FROM ai_usage_period_buckets WHERE scope_kind = 'project'
      AND scope_id = NEW.id AND period_start = v_period;
  IF v_spent + v_reserved > NEW.ai_monthly_budget_micros THEN
    RAISE EXCEPTION 'AI project cap is below its current spend and reservations';
  END IF;
  UPDATE ai_usage_limits SET limit_micros = NEW.ai_monthly_budget_micros, updated_at = now()
    WHERE scope_kind = 'project' AND scope_id = NEW.id;
  UPDATE ai_usage_period_buckets SET limit_micros = NEW.ai_monthly_budget_micros, updated_at = now()
    WHERE scope_kind = 'project' AND scope_id = NEW.id AND period_start = v_period;
  RETURN NEW;
END;
$$;

CREATE TRIGGER ai_usage_sync_new_project_limit_trigger
AFTER INSERT OR UPDATE OF ai_monthly_budget_micros ON projects
FOR EACH ROW EXECUTE FUNCTION ai_usage_sync_new_project_limit();

-- Existing legacy project-budget writes update the new project child limit.
-- Parent caps intentionally remain separate: a child setting never creates
-- allowance beyond workspace or platform policy.
UPDATE ai_usage_limits l
SET limit_micros = p.ai_monthly_budget_micros, updated_at = now()
FROM projects p
WHERE l.scope_kind = 'project' AND l.scope_id = p.id
  AND l.limit_micros IS DISTINCT FROM p.ai_monthly_budget_micros;
