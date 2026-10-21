-- phase: expand
-- Projects may remain workspace-less while the tenancy expand phase is live.
-- They keep their project limit but receive no usage bucket until a workspace
-- is assigned; the workspace update then creates the complete hierarchy.

CREATE OR REPLACE FUNCTION ai_usage_sync_new_project_limit()
RETURNS trigger LANGUAGE plpgsql AS $$
DECLARE
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
  -- Project budget or workspace assignment updates use the same bucket order
  -- as admissions. A child update never changes parent allowance.
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
  SELECT spent_micros, reserved_micros INTO v_spent, v_reserved
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

DROP TRIGGER ai_usage_sync_new_project_limit_trigger ON projects;
CREATE TRIGGER ai_usage_sync_new_project_limit_trigger
AFTER INSERT OR UPDATE OF ai_monthly_budget_micros, workspace_id ON projects
FOR EACH ROW EXECUTE FUNCTION ai_usage_sync_new_project_limit();
