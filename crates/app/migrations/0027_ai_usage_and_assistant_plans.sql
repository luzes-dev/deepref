-- AI usage ledger, per-project monthly budget, and confirm-before-execute
-- assistant plans.

-- Monthly AI budget per project in micro-dollars (1e-6 USD). Default US$5.00.
ALTER TABLE projects
  ADD COLUMN IF NOT EXISTS ai_monthly_budget_micros bigint NOT NULL DEFAULT 5000000
    CHECK (ai_monthly_budget_micros >= 0);

-- One row per successful model call. project_id is NULL for workspace-level
-- calls; those are recorded but not budget-limited.
CREATE TABLE IF NOT EXISTS ai_usage_ledger (
  id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
  project_id uuid REFERENCES projects(id) ON DELETE CASCADE,
  profile text NOT NULL,
  provider text NOT NULL,
  model text NOT NULL,
  purpose text NOT NULL CHECK (purpose IN ('structured', 'chat')),
  input_tokens bigint NOT NULL CHECK (input_tokens >= 0),
  output_tokens bigint NOT NULL CHECK (output_tokens >= 0),
  cost_micros bigint NOT NULL CHECK (cost_micros >= 0),
  created_at timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX IF NOT EXISTS ai_usage_ledger_project_month_idx
  ON ai_usage_ledger (project_id, created_at DESC);

-- A plan is what the assistant proposes to change. Nothing in `actions` runs
-- until a user confirms the plan.
CREATE TABLE IF NOT EXISTS assistant_plans (
  id uuid PRIMARY KEY,
  project_id uuid NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
  conversation_id uuid NOT NULL REFERENCES assistant_conversations(id) ON DELETE CASCADE,
  status text NOT NULL DEFAULT 'pending'
    CHECK (status IN ('pending', 'confirmed', 'rejected', 'executed', 'failed')),
  summary text NOT NULL,
  actions jsonb NOT NULL CHECK (jsonb_typeof(actions) = 'array'),
  results jsonb CHECK (results IS NULL OR jsonb_typeof(results) = 'array'),
  error text,
  created_by_kind text NOT NULL,
  created_by_id text NOT NULL,
  model text NOT NULL,
  prompt_version text NOT NULL,
  evidence jsonb NOT NULL DEFAULT '[]'::jsonb CHECK (jsonb_typeof(evidence) = 'array'),
  created_at timestamptz NOT NULL DEFAULT now(),
  resolved_by_kind text,
  resolved_by_id text,
  resolved_at timestamptz
);
CREATE INDEX IF NOT EXISTS assistant_plans_conversation_idx
  ON assistant_plans (conversation_id, created_at DESC);
CREATE INDEX IF NOT EXISTS assistant_plans_project_status_idx
  ON assistant_plans (project_id, status, created_at DESC);
