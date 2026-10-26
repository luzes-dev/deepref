-- phase: expand
-- Durable canonical wire freeze and dispatch intent claim for usage export facts.
ALTER TABLE billing_usage_fact_targets
  ADD COLUMN account_id uuid REFERENCES billing_accounts(id),
  ADD COLUMN mode text,
  ADD COLUMN business_id text,
  ADD COLUMN event_name text,
  ADD COLUMN original_timestamp timestamptz,
  ADD COLUMN metadata jsonb,
  ADD COLUMN wire_digest text,
  ADD COLUMN dispatch_claimed_at timestamptz,
  ADD COLUMN dispatch_state text NOT NULL DEFAULT 'bound'
    CHECK (dispatch_state IN ('bound', 'dispatched', 'quarantined', 'duplicate', 'delivered'));

CREATE INDEX IF NOT EXISTS billing_usage_fact_targets_workspace_scope_idx
  ON billing_usage_fact_targets(workspace_id, provider_scope);
