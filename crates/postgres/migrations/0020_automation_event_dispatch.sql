-- PR14: dispatch the closed automation catalog from authoritative domain events.
-- Each trigger runs in the source write transaction, so a rolled-back domain
-- event cannot leave behind a durable automation run or job.

CREATE OR REPLACE FUNCTION normalize_automation_event_actor(
  source_actor_kind text,
  source_actor_id text
)
RETURNS TABLE (
  normalized_actor_kind text,
  normalized_actor_id text
)
LANGUAGE sql
IMMUTABLE
PARALLEL SAFE
AS $$
  SELECT
    CASE
      WHEN source_actor_kind IN ('user', 'automation', 'system')
        AND source_actor_id IS NOT NULL
        AND length(btrim(source_actor_id)) > 0
        AND octet_length(source_actor_id) <= 200
        THEN source_actor_kind
      ELSE 'system'
    END,
    CASE
      WHEN source_actor_kind IN ('user', 'automation', 'system')
        AND source_actor_id IS NOT NULL
        AND length(btrim(source_actor_id)) > 0
        AND octet_length(source_actor_id) <= 200
        THEN source_actor_id
      ELSE 'automation-event-bridge'
    END
$$;

CREATE OR REPLACE FUNCTION dispatch_automation_domain_event(
  target_project_id uuid,
  target_trigger_kind text,
  target_source_identity text,
  target_actor_kind text,
  target_actor_id text
)
RETURNS void
LANGUAGE plpgsql
AS $$
DECLARE
  definition_row RECORD;
BEGIN
  IF target_project_id IS NULL
      OR target_project_id = '00000000-0000-0000-0000-000000000000'::uuid THEN
    RAISE EXCEPTION 'automation event project id is invalid'
      USING ERRCODE = 'invalid_parameter_value';
  END IF;
  IF target_trigger_kind IS NULL OR target_trigger_kind NOT IN (
    'report_added', 'acquisition_completed', 'full_text_attached',
    'report_included', 'study_created', 'appraisal_completed'
  ) THEN
    RAISE EXCEPTION 'automation event trigger kind is unknown'
      USING ERRCODE = 'invalid_parameter_value';
  END IF;
  IF target_source_identity IS NULL
      OR length(btrim(target_source_identity)) NOT BETWEEN 1 AND 200 THEN
    RAISE EXCEPTION 'automation event source identity is invalid'
      USING ERRCODE = 'invalid_parameter_value';
  END IF;
  SELECT normalized_actor_kind, normalized_actor_id
  INTO target_actor_kind, target_actor_id
  FROM normalize_automation_event_actor(target_actor_kind, target_actor_id);

  -- Lock the active definition rows while dispatching. This prevents a
  -- concurrent pause/delete from turning a successful source write into a
  -- failed transaction between the catalog lookup and the closed dispatcher.
  FOR definition_row IN
    SELECT d.id
    FROM automation_definitions AS d
    WHERE d.project_id = target_project_id
      AND d.trigger_kind = target_trigger_kind
      AND d.status = 'active'
    ORDER BY d.id
    FOR SHARE
  LOOP
    PERFORM dispatch_automation_trigger(
      target_project_id,
      definition_row.id,
      target_trigger_kind,
      target_source_identity,
      target_source_identity,
      target_actor_kind,
      target_actor_id
    );
  END LOOP;
END;
$$;

CREATE OR REPLACE FUNCTION dispatch_automation_project_report_added()
RETURNS trigger
LANGUAGE plpgsql
AS $$
DECLARE
  source_identity text;
BEGIN
  source_identity := format('project_report:%s:%s', NEW.project_id, NEW.report_id);
  PERFORM dispatch_automation_domain_event(
    NEW.project_id, 'report_added', source_identity, 'system', 'automation-event-bridge'
  );
  RETURN NEW;
END;
$$;

DROP TRIGGER IF EXISTS automation_project_report_added ON project_reports;
CREATE TRIGGER automation_project_report_added
AFTER INSERT ON project_reports
FOR EACH ROW EXECUTE FUNCTION dispatch_automation_project_report_added();

CREATE OR REPLACE FUNCTION dispatch_automation_acquisition_completed()
RETURNS trigger
LANGUAGE plpgsql
AS $$
DECLARE
  source_identity text;
BEGIN
  IF TG_OP = 'INSERT' THEN
    IF NEW.status <> 'completed' THEN
      RETURN NEW;
    END IF;
  ELSIF OLD.status = 'completed' OR NEW.status <> 'completed' THEN
    RETURN NEW;
  END IF;

  source_identity := format('acquisition_run:%s', NEW.id);
  PERFORM dispatch_automation_domain_event(
    NEW.project_id, 'acquisition_completed', source_identity,
    'system', 'automation-event-bridge'
  );
  RETURN NEW;
END;
$$;

DROP TRIGGER IF EXISTS automation_acquisition_completed ON acquisition_runs;
CREATE TRIGGER automation_acquisition_completed
AFTER INSERT OR UPDATE OF status ON acquisition_runs
FOR EACH ROW EXECUTE FUNCTION dispatch_automation_acquisition_completed();

CREATE OR REPLACE FUNCTION dispatch_automation_full_text_attached()
RETURNS trigger
LANGUAGE plpgsql
AS $$
DECLARE
  became_attached boolean := false;
  source_identity text;
BEGIN
  IF TG_OP = 'INSERT' THEN
    became_attached := NEW.status IN ('uploaded', 'available');
  ELSE
    became_attached := OLD.status NOT IN ('uploaded', 'available')
      AND NEW.status IN ('uploaded', 'available');
  END IF;

  IF NOT became_attached THEN
    RETURN NEW;
  END IF;

  source_identity := format('document:%s', NEW.id);
  PERFORM dispatch_automation_domain_event(
    NEW.project_id, 'full_text_attached', source_identity,
    NEW.actor_kind, NEW.actor_id
  );
  RETURN NEW;
END;
$$;

DROP TRIGGER IF EXISTS automation_full_text_attached ON documents;
CREATE TRIGGER automation_full_text_attached
AFTER INSERT OR UPDATE OF status ON documents
FOR EACH ROW EXECUTE FUNCTION dispatch_automation_full_text_attached();

CREATE OR REPLACE FUNCTION dispatch_automation_report_included()
RETURNS trigger
LANGUAGE plpgsql
AS $$
DECLARE
  source_identity text;
BEGIN
  IF NEW.event_kind <> 'decision'
      OR NEW.previous_final_status IS NOT DISTINCT FROM 'include'
      OR NEW.result_final_status <> 'include' THEN
    RETURN NEW;
  END IF;

  source_identity := format('screening_event:%s', NEW.id);
  PERFORM dispatch_automation_domain_event(
    NEW.project_id, 'report_included', source_identity,
    NEW.actor_kind, NEW.actor_id
  );
  RETURN NEW;
END;
$$;

DROP TRIGGER IF EXISTS automation_report_included ON screening_events;
CREATE TRIGGER automation_report_included
AFTER INSERT ON screening_events
FOR EACH ROW EXECUTE FUNCTION dispatch_automation_report_included();

CREATE OR REPLACE FUNCTION dispatch_automation_study_created()
RETURNS trigger
LANGUAGE plpgsql
AS $$
DECLARE
  source_identity text;
BEGIN
  IF NEW.event_type <> 'study_created' THEN
    RETURN NEW;
  END IF;

  source_identity := format('study_event:%s', NEW.id);
  PERFORM dispatch_automation_domain_event(
    NEW.project_id, 'study_created', source_identity,
    NEW.actor_kind, NEW.actor_id
  );
  RETURN NEW;
END;
$$;

DROP TRIGGER IF EXISTS automation_study_created ON study_events;
CREATE TRIGGER automation_study_created
AFTER INSERT ON study_events
FOR EACH ROW EXECUTE FUNCTION dispatch_automation_study_created();

CREATE OR REPLACE FUNCTION dispatch_automation_appraisal_completed()
RETURNS trigger
LANGUAGE plpgsql
AS $$
DECLARE
  source_identity text;
BEGIN
  IF NEW.event_type <> 'appraisal_completed' THEN
    RETURN NEW;
  END IF;

  source_identity := format('appraisal_event:%s', NEW.id);
  PERFORM dispatch_automation_domain_event(
    NEW.project_id, 'appraisal_completed', source_identity,
    NEW.actor_kind, NEW.actor_id
  );
  RETURN NEW;
END;
$$;

DROP TRIGGER IF EXISTS automation_appraisal_completed ON appraisal_events;
CREATE TRIGGER automation_appraisal_completed
AFTER INSERT ON appraisal_events
FOR EACH ROW EXECUTE FUNCTION dispatch_automation_appraisal_completed();
