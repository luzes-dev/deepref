-- phase: expand
-- Compatibility is enforced at the DB write boundary: N-1 writers enqueue,
-- retry, recover and finish with the same atomic wakeup/receipt obligations.
ALTER TABLE jobs
  ADD COLUMN wakeup_generation bigint NOT NULL DEFAULT 1,
  ADD COLUMN wakeup_emitted_generation bigint NOT NULL DEFAULT 0;

CREATE FUNCTION job_wakeup_event_id(job_id uuid, generation bigint) RETURNS uuid
LANGUAGE sql IMMUTABLE STRICT AS $$
  SELECT md5('deepref.work:' || job_id::text || ':' || generation::text)::uuid
$$;

CREATE FUNCTION jobs_prepare_wakeup() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
  IF TG_OP = 'UPDATE' THEN
    IF NEW.state IN ('queued', 'capability_wait') AND
       (OLD.state NOT IN ('queued', 'capability_wait') OR NEW.available_at IS DISTINCT FROM OLD.available_at) THEN
      NEW.wakeup_generation := OLD.wakeup_generation + 1;
    END IF;
  END IF;
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
    NEW.wakeup_emitted_generation := NEW.wakeup_generation;
  END IF;
  RETURN NEW;
END $$;
CREATE TRIGGER jobs_z_prepare_wakeup BEFORE INSERT OR UPDATE OF state, available_at, wakeup_emitted_generation
  ON jobs FOR EACH ROW EXECUTE FUNCTION jobs_prepare_wakeup();

-- A wakeup's effect is durable lifecycle settlement. The receipt commits in
-- the very same transaction, including retries and crashed-lease recovery.
CREATE FUNCTION jobs_settle_wakeup() RETURNS trigger LANGUAGE plpgsql AS $$
DECLARE event uuid;
BEGIN
  IF (OLD.state = 'running' AND NEW.state <> 'running')
     OR NEW.wakeup_generation <> OLD.wakeup_generation
     OR NEW.state IN ('completed', 'failed', 'dead') THEN
    event := job_wakeup_event_id(OLD.id, OLD.wakeup_generation);
    INSERT INTO outbox_receipts (event_id, consumer, state, delivered_at)
      SELECT event, 'durable-work', 'delivered', now()
      WHERE EXISTS (SELECT 1 FROM outbox_events WHERE event_id = event)
      ON CONFLICT (event_id, consumer) DO UPDATE SET state='delivered', delivered_at=now(), last_error=NULL;
    UPDATE outbox_events SET delivery_state='delivered', delivered_at=now(),
      lease_owner=NULL, lease_expires_at=NULL
      WHERE event_id=event;
  END IF;
  RETURN NEW;
END $$;
CREATE TRIGGER jobs_settle_wakeup AFTER UPDATE OF state, available_at ON jobs
  FOR EACH ROW EXECUTE FUNCTION jobs_settle_wakeup();
-- Existing queued rows are deliberately picked up by the bounded due publisher;
-- this migration does not rewrite historical/terminal jobs.
