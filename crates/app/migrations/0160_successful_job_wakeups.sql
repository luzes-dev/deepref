-- phase: expand
-- BEFORE INSERT also runs for rows discarded by ON CONFLICT. Only calculate
-- row-local generation there; publish exclusively after a successful write.
CREATE OR REPLACE FUNCTION jobs_prepare_wakeup() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
  IF TG_OP = 'UPDATE' THEN
    IF NEW.state IN ('queued', 'capability_wait') AND
       (OLD.state NOT IN ('queued', 'capability_wait') OR NEW.available_at IS DISTINCT FROM OLD.available_at) THEN
      NEW.wakeup_generation := OLD.wakeup_generation + 1;
    END IF;
  END IF;
  RETURN NEW;
END $$;

CREATE FUNCTION jobs_publish_wakeup() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
  IF NEW.state IN ('queued', 'capability_wait') AND NEW.available_at <= now()
     AND NEW.attempts < NEW.max_attempts
     AND NEW.wakeup_emitted_generation < NEW.wakeup_generation THEN
    INSERT INTO outbox_events (event_id, message_type, message_version, payload, expected_consumers)
    VALUES (job_wakeup_event_id(NEW.id, NEW.wakeup_generation), 'deepref.work', 1,
      jsonb_build_object('job_id', NEW.id, 'generation', NEW.wakeup_generation,
        'workload_class', coalesce(NEW.workload_class, jobs_workload_class_for_kind(NEW.kind))),
      ARRAY['durable-work']) ON CONFLICT (event_id) DO UPDATE
      SET published_at=NULL, publication_epoch=outbox_events.publication_epoch+1,
          publication_next_at=now(), publication_owner=NULL, publication_until=NULL
      WHERE outbox_events.delivery_state='pending';
    -- The nested update cannot publish again: its NEW marker equals generation.
    UPDATE jobs SET wakeup_emitted_generation=NEW.wakeup_generation
      WHERE id=NEW.id AND wakeup_generation=NEW.wakeup_generation
        AND wakeup_emitted_generation<NEW.wakeup_generation;
  END IF;
  RETURN NEW;
END $$;
CREATE TRIGGER jobs_z_publish_wakeup AFTER INSERT OR UPDATE OF state, available_at, wakeup_emitted_generation
  ON jobs FOR EACH ROW EXECUTE FUNCTION jobs_publish_wakeup();
