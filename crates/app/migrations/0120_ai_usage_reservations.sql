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
  ADD COLUMN IF NOT EXISTS step_id text,
  ADD COLUMN IF NOT EXISTS reasoning_tokens bigint NOT NULL DEFAULT 0,
  ADD COLUMN IF NOT EXISTS commercial_units bigint,
  ADD COLUMN IF NOT EXISTS price_version text,
  ADD COLUMN IF NOT EXISTS current_price_version text,
  ADD COLUMN IF NOT EXISTS pricing_decision text,
  ADD COLUMN IF NOT EXISTS period_start date,
  ADD COLUMN IF NOT EXISTS reservation_id uuid;

ALTER TABLE ai_usage_ledger
  DROP CONSTRAINT IF EXISTS ai_usage_ledger_purpose_check;
ALTER TABLE ai_usage_ledger
  ADD CONSTRAINT ai_usage_ledger_purpose_check
  CHECK (purpose IN ('structured', 'chat', 'assistant')) NOT VALID;

UPDATE ai_usage_ledger l
SET workspace_id = p.workspace_id,
    period_start = date_trunc('month', l.created_at AT TIME ZONE 'UTC')::date
FROM projects p
WHERE l.project_id = p.id
  AND (l.workspace_id IS NULL OR l.period_start IS NULL);
UPDATE ai_usage_ledger l
SET period_start = date_trunc('month', l.created_at AT TIME ZONE 'UTC')::date
WHERE l.period_start IS NULL;

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
  run_id uuid,
  period_start date NOT NULL,
  limit_micros bigint NOT NULL CHECK (limit_micros >= 0),
  spent_micros bigint NOT NULL DEFAULT 0 CHECK (spent_micros >= 0),
  reserved_micros bigint NOT NULL DEFAULT 0 CHECK (reserved_micros >= 0),
  updated_at timestamptz NOT NULL DEFAULT now(),
  PRIMARY KEY (scope_kind, scope_id, period_start),
  CHECK (
    (scope_kind = 'platform' AND workspace_id IS NULL AND project_id IS NULL AND run_id IS NULL)
    OR (scope_kind = 'workspace' AND workspace_id = scope_id AND project_id IS NULL AND run_id IS NULL)
    OR (scope_kind = 'project' AND project_id = scope_id AND workspace_id IS NOT NULL AND run_id IS NULL)
    OR (scope_kind = 'run' AND run_id = scope_id AND workspace_id IS NOT NULL)
  )
);

CREATE TABLE ai_usage_reservations (
  id uuid PRIMARY KEY,
  workspace_id uuid NOT NULL,
  project_id uuid,
  principal_kind text,
  principal_id text,
  run_id uuid NOT NULL,
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

-- A provider-neutral outbox record is committed with the usage fact. Export
-- credentials, provider events and delivery are owned by the later billing
-- provider slice.
CREATE TABLE ai_usage_outbox (
  id uuid PRIMARY KEY,
  reservation_id uuid NOT NULL UNIQUE,
  event_type text NOT NULL CHECK (event_type = 'ai.usage.settled'),
  payload jsonb NOT NULL CHECK (jsonb_typeof(payload) = 'object'),
  created_at timestamptz NOT NULL DEFAULT now(),
  delivered_at timestamptz
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

-- Backfill the legacy spend into the same monthly buckets. N-1 writes remain
-- visible through the insert trigger below; N writes carry reservation_id and
-- update the counters in the settlement transaction instead.
INSERT INTO ai_usage_period_buckets
  (scope_kind, scope_id, workspace_id, project_id, period_start, limit_micros, spent_micros)
SELECT 'project', l.project_id, p.workspace_id, l.project_id,
       COALESCE(l.period_start, date_trunc('month', l.created_at AT TIME ZONE 'UTC')::date),
       lim.limit_micros, SUM(l.cost_micros)::bigint
FROM ai_usage_ledger l
JOIN projects p ON p.id = l.project_id
JOIN ai_usage_limits lim ON lim.scope_kind = 'project' AND lim.scope_id = l.project_id
GROUP BY l.project_id, p.workspace_id, COALESCE(l.period_start, date_trunc('month', l.created_at AT TIME ZONE 'UTC')::date), lim.limit_micros
ON CONFLICT (scope_kind, scope_id, period_start) DO UPDATE
SET spent_micros = GREATEST(ai_usage_period_buckets.spent_micros, EXCLUDED.spent_micros);

INSERT INTO ai_usage_period_buckets
  (scope_kind, scope_id, workspace_id, period_start, limit_micros, spent_micros)
SELECT 'workspace', p.workspace_id, p.workspace_id,
       COALESCE(l.period_start, date_trunc('month', l.created_at AT TIME ZONE 'UTC')::date),
       lim.limit_micros, SUM(l.cost_micros)::bigint
FROM ai_usage_ledger l
JOIN projects p ON p.id = l.project_id
JOIN ai_usage_limits lim ON lim.scope_kind = 'workspace' AND lim.scope_id = p.workspace_id
GROUP BY p.workspace_id, COALESCE(l.period_start, date_trunc('month', l.created_at AT TIME ZONE 'UTC')::date), lim.limit_micros
ON CONFLICT (scope_kind, scope_id, period_start) DO UPDATE
SET spent_micros = GREATEST(ai_usage_period_buckets.spent_micros, EXCLUDED.spent_micros);

INSERT INTO ai_usage_period_buckets
  (scope_kind, scope_id, period_start, limit_micros, spent_micros)
SELECT 'platform', '00000000-0000-0000-0000-000000000001'::uuid,
       COALESCE(l.period_start, date_trunc('month', l.created_at AT TIME ZONE 'UTC')::date),
       lim.limit_micros, SUM(l.cost_micros)::bigint
FROM ai_usage_ledger l
JOIN ai_usage_limits lim ON lim.scope_kind = 'platform'
GROUP BY COALESCE(l.period_start, date_trunc('month', l.created_at AT TIME ZONE 'UTC')::date), lim.limit_micros
ON CONFLICT (scope_kind, scope_id, period_start) DO UPDATE
SET spent_micros = GREATEST(ai_usage_period_buckets.spent_micros, EXCLUDED.spent_micros);

INSERT INTO ai_usage_period_buckets (scope_kind, scope_id, workspace_id, project_id, period_start, limit_micros)
SELECT 'project', p.id, p.workspace_id, p.id, date_trunc('month', now() AT TIME ZONE 'UTC')::date, lim.limit_micros
FROM projects p JOIN ai_usage_limits lim ON lim.scope_kind = 'project' AND lim.scope_id = p.id
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
  v_period date;
  v_project_limit bigint;
  v_workspace_limit bigint;
  v_platform_limit bigint;
  v_overage bigint;
BEGIN
  IF NEW.reservation_id IS NOT NULL THEN
    RETURN NULL;
  END IF;
  v_workspace_id := NEW.workspace_id;
  v_period := COALESCE(NEW.period_start, date_trunc('month', NEW.created_at AT TIME ZONE 'UTC')::date);
  IF v_workspace_id IS NULL OR NEW.project_id IS NULL THEN
    RETURN NULL;
  END IF;
  SELECT limit_micros INTO v_project_limit FROM ai_usage_limits WHERE scope_kind = 'project' AND scope_id = NEW.project_id;
  SELECT limit_micros INTO v_workspace_limit FROM ai_usage_limits WHERE scope_kind = 'workspace' AND scope_id = v_workspace_id;
  SELECT limit_micros INTO v_platform_limit FROM ai_usage_limits WHERE scope_kind = 'platform' AND scope_id = '00000000-0000-0000-0000-000000000001';
  IF v_project_limit IS NULL OR v_workspace_limit IS NULL OR v_platform_limit IS NULL THEN
    RAISE EXCEPTION 'AI usage limits are missing for legacy spend';
  END IF;
  -- Keep the legacy trigger's row-lock order identical to N reservations.
  INSERT INTO ai_usage_period_buckets (scope_kind, scope_id, period_start, limit_micros, spent_micros)
  VALUES ('platform', '00000000-0000-0000-0000-000000000001', v_period, v_platform_limit, NEW.cost_micros)
  ON CONFLICT (scope_kind, scope_id, period_start) DO UPDATE
  SET spent_micros = ai_usage_period_buckets.spent_micros + EXCLUDED.spent_micros, updated_at = now();
  INSERT INTO ai_usage_period_buckets (scope_kind, scope_id, workspace_id, period_start, limit_micros, spent_micros)
  VALUES ('workspace', v_workspace_id, v_workspace_id, v_period, v_workspace_limit, NEW.cost_micros)
  ON CONFLICT (scope_kind, scope_id, period_start) DO UPDATE
  SET spent_micros = ai_usage_period_buckets.spent_micros + EXCLUDED.spent_micros, updated_at = now();
  INSERT INTO ai_usage_period_buckets (scope_kind, scope_id, workspace_id, project_id, period_start, limit_micros, spent_micros)
  VALUES ('project', NEW.project_id, v_workspace_id, NEW.project_id, v_period, v_project_limit, NEW.cost_micros)
  ON CONFLICT (scope_kind, scope_id, period_start) DO UPDATE
  SET spent_micros = ai_usage_period_buckets.spent_micros + EXCLUDED.spent_micros, updated_at = now();
  SELECT COALESCE(MAX(GREATEST(0, b.spent_micros + b.reserved_micros - b.limit_micros)), 0)
  INTO v_overage
  FROM ai_usage_period_buckets b
    WHERE b.period_start = v_period AND b.scope_kind IN ('project', 'workspace', 'platform')
      AND b.scope_id IN (NEW.project_id, v_workspace_id, '00000000-0000-0000-0000-000000000001'::uuid)
  ;
  IF v_overage > 0 THEN
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
  v_workspace_limit bigint;
BEGIN
  INSERT INTO ai_usage_limits (scope_kind, scope_id, limit_micros)
  VALUES ('project', NEW.id, NEW.ai_monthly_budget_micros)
  ON CONFLICT (scope_kind, scope_id) DO UPDATE
    SET limit_micros = EXCLUDED.limit_micros, updated_at = now();
  UPDATE ai_usage_period_buckets
  SET limit_micros = NEW.ai_monthly_budget_micros, updated_at = now()
  WHERE scope_kind = 'project' AND scope_id = NEW.id
    AND period_start = date_trunc('month', now() AT TIME ZONE 'UTC')::date;
  IF TG_OP = 'INSERT' AND NEW.workspace_id IS NOT NULL THEN
    INSERT INTO ai_usage_limits (scope_kind, scope_id, limit_micros)
    VALUES ('workspace', NEW.workspace_id, NEW.ai_monthly_budget_micros)
    ON CONFLICT (scope_kind, scope_id) DO UPDATE
      SET limit_micros = ai_usage_limits.limit_micros + EXCLUDED.limit_micros, updated_at = now();
    UPDATE ai_usage_limits SET limit_micros = limit_micros + NEW.ai_monthly_budget_micros, updated_at = now()
    WHERE scope_kind = 'platform' AND scope_id = '00000000-0000-0000-0000-000000000001';
  END IF;
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
