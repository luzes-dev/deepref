-- phase: expand
-- Durable per-fact target bindings and operator manual reconciliation records for usage exports.
CREATE TABLE billing_usage_fact_targets (
  fact_id uuid PRIMARY KEY REFERENCES outbox_events(event_id),
  workspace_id uuid NOT NULL,
  provider_scope text NOT NULL,
  customer_ref text NOT NULL,
  account_generation bigint NOT NULL,
  bound_at timestamptz NOT NULL DEFAULT now()
);

CREATE TABLE billing_usage_resolutions (
  fact_id uuid PRIMARY KEY REFERENCES outbox_events(event_id),
  workspace_id uuid NOT NULL,
  resolution_id uuid NOT NULL UNIQUE,
  resolved_by uuid NOT NULL REFERENCES users(id),
  action text NOT NULL CHECK (action IN ('manual_reconciliation', 'reopened_retry', 'refused')),
  reason text NOT NULL CHECK (reason IN (
    'external_adjustment_confirmed',
    'provider_duplicate_verified',
    'manual_credit_issued',
    'rejected_unrecoverable'
  )),
  notes text,
  resolved_at timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX billing_usage_resolutions_workspace_idx ON billing_usage_resolutions(workspace_id);
