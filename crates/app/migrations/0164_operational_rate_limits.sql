-- phase: expand
-- One counter per verified key or workflow hook kind; no per-input/window rows.
-- PostgreSQL is the ceiling for both Valkey-assisted and fallback populations.
CREATE TABLE operational_rate_limits (
  subject_kind text NOT NULL CHECK (subject_kind IN ('api_key', 'workflow_webhook', 'workflow_email')),
  subject_id uuid NOT NULL,
  api_key_id uuid REFERENCES api_keys(id) ON DELETE CASCADE,
  workflow_id uuid REFERENCES workflows(id) ON DELETE CASCADE,
  window_started_at timestamptz NOT NULL,
  last_allowed boolean NOT NULL,
  used integer NOT NULL CHECK (used BETWEEN 1 AND 10000),
  ceiling integer NOT NULL CHECK (ceiling BETWEEN 1 AND 10000),
  PRIMARY KEY (subject_kind, subject_id),
  CHECK (
    (subject_kind = 'api_key' AND api_key_id IS NOT NULL AND api_key_id = subject_id AND workflow_id IS NULL)
    OR (subject_kind IN ('workflow_webhook', 'workflow_email') AND workflow_id IS NOT NULL AND workflow_id = subject_id AND api_key_id IS NULL)
  )
);
CREATE INDEX operational_rate_limits_api_key_idx ON operational_rate_limits(api_key_id) WHERE api_key_id IS NOT NULL;
CREATE INDEX operational_rate_limits_workflow_idx ON operational_rate_limits(workflow_id) WHERE workflow_id IS NOT NULL;
