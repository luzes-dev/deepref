-- phase: expand
-- Opt-in local projection. No backfill infers free from missing provider data.
CREATE TABLE billing_accounts (
  id uuid PRIMARY KEY,
  workspace_id uuid REFERENCES workspaces(id) ON DELETE SET NULL,
  provider_scope text NOT NULL CHECK (length(provider_scope) BETWEEN 1 AND 256),
  operation_id uuid NOT NULL UNIQUE,
  customer_ref text CHECK (length(customer_ref) BETWEEN 1 AND 128),
  provisioning text NOT NULL CHECK (provisioning IN ('ambiguous','mapped','detached')),
  subscription_ref text CHECK (length(subscription_ref) BETWEEN 1 AND 128),
  product_ref text CHECK (length(product_ref) BETWEEN 1 AND 128),
  CHECK ((subscription_ref IS NULL) = (product_ref IS NULL)),
  generation bigint NOT NULL DEFAULT 0 CHECK (generation >= 0),
  projection jsonb NOT NULL DEFAULT '{"kind":"unknown"}',
  review_required boolean NOT NULL DEFAULT false,
  review_id uuid,
  resolved_review_id uuid,
  reconcile_after timestamptz NOT NULL DEFAULT now(),
  reconciled_at timestamptz,
  created_at timestamptz NOT NULL DEFAULT now(),
  detached_at timestamptz,
  purge_after timestamptz,
  UNIQUE(workspace_id, provider_scope),
  UNIQUE(provider_scope, customer_ref),
  CHECK ((provisioning = 'mapped') = (customer_ref IS NOT NULL)),
  CHECK ((provisioning = 'detached') = (detached_at IS NOT NULL))
);
CREATE INDEX billing_accounts_due ON billing_accounts(reconcile_after, id) WHERE provisioning <> 'detached';
-- A free enrollment is an explicit product-policy decision. A provisioning row
-- and a free enrollment cannot be concurrently created through application locks.
CREATE TABLE billing_free_enrollments (
  workspace_id uuid PRIMARY KEY REFERENCES workspaces(id) ON DELETE CASCADE,
  operation_id uuid NOT NULL UNIQUE,
  entitlements jsonb NOT NULL,
  enrolled_at timestamptz NOT NULL DEFAULT now()
);

-- Fence an in-flight provider response before the tenant can disappear. Keep
-- only a detached operation tombstone for restricted financial retention.
CREATE FUNCTION billing_detach_workspace() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
  UPDATE billing_accounts SET workspace_id=NULL, customer_ref=NULL, subscription_ref=NULL, product_ref=NULL,
    generation=generation+1, provisioning='detached', detached_at=now(),
    purge_after=now()+interval '5 years', projection='{"kind":"unknown"}',
    review_required=true
    WHERE workspace_id=OLD.id;
  RETURN OLD;
END $$;
CREATE TRIGGER billing_workspace_detach BEFORE DELETE ON workspaces
  FOR EACH ROW EXECUTE FUNCTION billing_detach_workspace();

-- One invalidation effect per existing inbox identity. No payload copy.
CREATE TABLE billing_inbox_invalidations (
  inbox_id bigint PRIMARY KEY REFERENCES integration_inbox_events(id),
  account_id uuid NOT NULL REFERENCES billing_accounts(id) ON DELETE CASCADE
);
