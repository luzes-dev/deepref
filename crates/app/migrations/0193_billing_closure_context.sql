-- phase: expand
-- Retain the original restricted closure context before0167 detaches the active
-- mapping. Existing detached accounts stay Unknown; no historical backfill can
-- manufacture missing provider references. Existing five-year expiry is intact.
ALTER TABLE billing_accounts
  ADD COLUMN closure_workspace_id uuid,
  ADD COLUMN closure_customer_ref text
    CHECK (length(closure_customer_ref) BETWEEN 1 AND 128),
  ADD COLUMN closure_subscription_ref text
    CHECK (length(closure_subscription_ref) BETWEEN 1 AND 128),
  ADD COLUMN closure_captured_at timestamptz,
  ADD CONSTRAINT billing_closure_context_shape CHECK (
    (closure_captured_at IS NULL AND closure_workspace_id IS NULL
      AND closure_customer_ref IS NULL AND closure_subscription_ref IS NULL)
    OR (closure_captured_at IS NOT NULL AND provisioning='detached')
  );

COMMENT ON COLUMN billing_accounts.closure_workspace_id IS
  'Restricted historical billing tenant, deliberately without a live workspace FK. Not authority to restore a tenant or evidence of provider closure.';
COMMENT ON COLUMN billing_accounts.closure_captured_at IS
  'Actual original-context capture during detachment; NULL on unknown historical tombstones. Does not change purge_after or prove provider closure.';

CREATE FUNCTION billing_preserve_closure_context() RETURNS trigger
LANGUAGE plpgsql SET search_path=pg_catalog,public AS $$
BEGIN
  IF TG_OP='INSERT' THEN
    IF NEW.closure_captured_at IS NOT NULL OR NEW.closure_workspace_id IS NOT NULL
      OR NEW.closure_customer_ref IS NOT NULL OR NEW.closure_subscription_ref IS NOT NULL THEN
      RAISE EXCEPTION 'closure context must come from original active mapping'
        USING ERRCODE='23514';
    END IF;
    RETURN NEW;
  END IF;

  IF OLD.closure_captured_at IS NOT NULL THEN
    IF NEW.closure_captured_at IS DISTINCT FROM OLD.closure_captured_at
      OR NEW.closure_workspace_id IS DISTINCT FROM OLD.closure_workspace_id
      OR NEW.closure_customer_ref IS DISTINCT FROM OLD.closure_customer_ref
      OR NEW.closure_subscription_ref IS DISTINCT FROM OLD.closure_subscription_ref
      OR NEW.provider_scope IS DISTINCT FROM OLD.provider_scope THEN
      RAISE EXCEPTION 'original closure context is immutable'
        USING ERRCODE='23514';
    END IF;
  ELSIF OLD.provisioning<>'detached' AND NEW.provisioning='detached' THEN
    IF NEW.provider_scope IS DISTINCT FROM OLD.provider_scope THEN
      RAISE EXCEPTION 'original closure scope cannot change during detachment'
        USING ERRCODE='23514';
    END IF;
    NEW.closure_workspace_id := OLD.workspace_id;
    NEW.closure_customer_ref := OLD.customer_ref;
    NEW.closure_subscription_ref := OLD.subscription_ref;
    NEW.closure_captured_at := clock_timestamp();
  ELSIF NEW.closure_captured_at IS NOT NULL OR NEW.closure_workspace_id IS NOT NULL
    OR NEW.closure_customer_ref IS NOT NULL OR NEW.closure_subscription_ref IS NOT NULL THEN
    RAISE EXCEPTION 'unknown historical closure context cannot be invented'
      USING ERRCODE='23514';
  END IF;
  RETURN NEW;
END $$;
REVOKE ALL ON FUNCTION billing_preserve_closure_context() FROM PUBLIC;
CREATE TRIGGER billing_preserve_closure_context_trigger
  BEFORE INSERT OR UPDATE ON billing_accounts
  FOR EACH ROW EXECUTE FUNCTION billing_preserve_closure_context();
