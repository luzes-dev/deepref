-- phase: expand
-- Billing 08c: provider-side closure tracking and the five-year financial
-- retention gate (BUSINESS_DECISIONS "Billing and AI usage records").
--
-- Provider closure is a local record of what DeepRef asked the provider to do
-- and what evidence it holds. It is never a claim that the provider deleted
-- anything. The ranked states only move forward:
--   cancel_pending  (0) a captured subscription still has to be cancelled
--   cancel_failed   (1) the provider refused cancellation; operator step needed
--   customer_manual (2) no subscription remains; the provider customer record
--                       has no closure API, so its erasure is a manual request
--   closed          (3) an operator recorded evidence of provider-side closure
ALTER TABLE public.billing_accounts
  ADD COLUMN provider_closure text,
  ADD COLUMN provider_closure_at timestamp with time zone,
  ADD COLUMN provider_closure_evidence uuid;

ALTER TABLE public.billing_accounts
  ADD CONSTRAINT billing_accounts_provider_closure_shape CHECK (
    (provider_closure IS NULL AND provider_closure_at IS NULL AND provider_closure_evidence IS NULL)
    OR (provider_closure IN ('cancel_pending','cancel_failed','customer_manual')
        AND provider_closure_at IS NOT NULL AND provider_closure_evidence IS NULL
        AND closure_captured_at IS NOT NULL)
    OR (provider_closure = 'closed' AND provider_closure_at IS NOT NULL
        AND provider_closure_evidence IS NOT NULL AND closure_captured_at IS NOT NULL)
  ) NOT VALID;

COMMENT ON COLUMN public.billing_accounts.provider_closure IS
  'Provider-side closure progress for a detached account. Local record only; never proof of provider deletion without operator evidence.';

CREATE FUNCTION public.billing_track_provider_closure() RETURNS trigger
    LANGUAGE plpgsql
    SET search_path TO 'pg_catalog', 'public'
    AS $$
DECLARE
  old_rank integer;
  new_rank integer;
BEGIN
  -- Original-context capture (billing_preserve_closure_context runs first,
  -- trigger names sort before this one) starts closure tracking.
  IF OLD.closure_captured_at IS NULL AND NEW.closure_captured_at IS NOT NULL THEN
    NEW.provider_closure := CASE WHEN NEW.closure_subscription_ref IS NOT NULL
      THEN 'cancel_pending' ELSE 'customer_manual' END;
    NEW.provider_closure_at := clock_timestamp();
    NEW.provider_closure_evidence := NULL;
    -- Due for the reconciler's closure pass immediately.
    NEW.reconcile_after := now();
    RETURN NEW;
  END IF;
  IF NEW.provider_closure IS NOT DISTINCT FROM OLD.provider_closure THEN
    IF NEW.provider_closure_evidence IS DISTINCT FROM OLD.provider_closure_evidence THEN
      RAISE EXCEPTION 'provider closure evidence is immutable' USING ERRCODE='23514';
    END IF;
    RETURN NEW;
  END IF;
  old_rank := CASE OLD.provider_closure WHEN 'cancel_pending' THEN 0 WHEN 'cancel_failed' THEN 1
    WHEN 'customer_manual' THEN 2 WHEN 'closed' THEN 3 END;
  new_rank := CASE NEW.provider_closure WHEN 'cancel_pending' THEN 0 WHEN 'cancel_failed' THEN 1
    WHEN 'customer_manual' THEN 2 WHEN 'closed' THEN 3 END;
  IF OLD.provider_closure IS NULL OR NEW.provider_closure IS NULL OR new_rank <= old_rank THEN
    RAISE EXCEPTION 'provider closure only moves forward' USING ERRCODE='23514';
  END IF;
  RETURN NEW;
END $$;

CREATE TRIGGER billing_track_provider_closure_trigger
  BEFORE UPDATE ON public.billing_accounts
  FOR EACH ROW EXECUTE FUNCTION public.billing_track_provider_closure();

-- A workspace's billing and usage records are past retention only when the
-- workspace no longer exists, every billing account that names it (live or as
-- the restricted closure tenant) has passed its five-year purge_after, and the
-- record itself is older than five years.
CREATE FUNCTION public.billing_retention_expired(p_workspace uuid, p_recorded_at timestamp with time zone)
    RETURNS boolean
    LANGUAGE sql STABLE
    SET search_path TO 'pg_catalog', 'public'
    AS $$
  SELECT p_recorded_at < now() - interval '5 years'
    AND (p_workspace IS NULL OR (
      NOT EXISTS (SELECT 1 FROM public.workspaces w WHERE w.id = p_workspace)
      AND NOT EXISTS (
        SELECT 1 FROM public.billing_accounts b
        WHERE (b.workspace_id = p_workspace OR b.closure_workspace_id = p_workspace)
          AND (b.purge_after IS NULL OR b.purge_after >= now()))))
$$;

-- Financial records are deleted only by the bounded retention purge, which sets
-- the transaction-local gate, and only once retention has expired. Everything
-- else, including ordinary runtime code, is refused at the database boundary.
CREATE FUNCTION public.billing_guard_financial_delete() RETURNS trigger
    LANGUAGE plpgsql
    SET search_path TO 'pg_catalog', 'public'
    AS $$
DECLARE
  eligible boolean;
BEGIN
  IF current_setting('deepref.billing_purge', true) IS DISTINCT FROM 'on' THEN
    RAISE EXCEPTION '% rows are retained financial records', TG_TABLE_NAME USING ERRCODE='23514';
  END IF;
  -- Separate branches: each statement only names fields of its own table.
  IF TG_TABLE_NAME = 'billing_accounts' THEN
    eligible := OLD.provisioning = 'detached' AND OLD.purge_after < now();
  ELSIF TG_TABLE_NAME = 'billing_usage_fact_targets' THEN
    eligible := public.billing_retention_expired(OLD.workspace_id, OLD.bound_at);
  ELSIF TG_TABLE_NAME = 'billing_usage_resolutions' THEN
    eligible := public.billing_retention_expired(OLD.workspace_id, OLD.resolved_at);
  ELSIF TG_TABLE_NAME = 'ai_usage_ledger' THEN
    eligible := public.billing_retention_expired(OLD.workspace_id, OLD.created_at);
  ELSIF TG_TABLE_NAME = 'ai_usage_reservations' THEN
    eligible := OLD.state IN ('settled','voided')
      AND public.billing_retention_expired(OLD.workspace_id, coalesce(OLD.settled_at, OLD.opened_at));
  ELSIF TG_TABLE_NAME = 'ai_usage_corrections' THEN
    eligible := OLD.created_at < now() - interval '5 years'
      AND NOT EXISTS (SELECT 1 FROM public.ai_usage_ledger l WHERE l.id = OLD.ledger_id);
  ELSE
    eligible := false;
  END IF;
  IF NOT coalesce(eligible, false) THEN
    RAISE EXCEPTION '% row is still inside its five-year retention', TG_TABLE_NAME USING ERRCODE='23514';
  END IF;
  RETURN OLD;
END $$;

CREATE TRIGGER billing_guard_financial_delete_trigger BEFORE DELETE ON public.billing_accounts
  FOR EACH ROW EXECUTE FUNCTION public.billing_guard_financial_delete();
CREATE TRIGGER billing_guard_financial_delete_trigger BEFORE DELETE ON public.billing_usage_fact_targets
  FOR EACH ROW EXECUTE FUNCTION public.billing_guard_financial_delete();
CREATE TRIGGER billing_guard_financial_delete_trigger BEFORE DELETE ON public.billing_usage_resolutions
  FOR EACH ROW EXECUTE FUNCTION public.billing_guard_financial_delete();
CREATE TRIGGER billing_guard_financial_delete_trigger BEFORE DELETE ON public.ai_usage_ledger
  FOR EACH ROW EXECUTE FUNCTION public.billing_guard_financial_delete();
CREATE TRIGGER billing_guard_financial_delete_trigger BEFORE DELETE ON public.ai_usage_reservations
  FOR EACH ROW EXECUTE FUNCTION public.billing_guard_financial_delete();
CREATE TRIGGER billing_guard_financial_delete_trigger BEFORE DELETE ON public.ai_usage_corrections
  FOR EACH ROW EXECUTE FUNCTION public.billing_guard_financial_delete();
