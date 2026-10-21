-- phase: expand
-- One shared identity namespace for usage facts and correction records
-- (handoff 09 slice B). A fact id is also its export event id, so a correction
-- record reusing a usage fact id — or an unrelated outbox event squatting one
-- — would make the replay treat the other record as already emitted and lose
-- it. The registry primary key is the namespace; triggers on both fact tables
-- route every writer through it, including N-1 ledger inserts that know
-- nothing about the export.

-- Audit existing overlaps first: this migration refuses to proceed on a
-- database whose namespace is already ambiguous.
DO $$
DECLARE
  overlap_count bigint;
BEGIN
  SELECT count(*) INTO overlap_count
  FROM ai_usage_corrections c JOIN ai_usage_ledger l ON l.id = c.id;
  IF overlap_count > 0 THEN
    RAISE EXCEPTION
      'ai_usage fact id namespace overlap: % correction ids equal usage fact ids', overlap_count;
  END IF;
  SELECT count(*) INTO overlap_count FROM (
    SELECT l.id FROM ai_usage_ledger l
    JOIN outbox_events e ON e.event_id = l.id
    WHERE NOT (e.message_type = 'ai_usage.fact' AND e.payload->>'fact_id' = l.id::text)
    UNION ALL
    SELECT c.id FROM ai_usage_corrections c
    JOIN outbox_events e ON e.event_id = c.id
    WHERE NOT (e.message_type = 'ai_usage.fact' AND e.payload->>'fact_id' = c.id::text)
  ) collisions;
  IF overlap_count > 0 THEN
    RAISE EXCEPTION
      'ai_usage fact ids collide with % unrelated outbox events', overlap_count;
  END IF;
END $$;

CREATE TABLE ai_usage_fact_ids (
  fact_id uuid PRIMARY KEY,
  fact_kind text NOT NULL CHECK (fact_kind IN ('usage', 'correction')),
  recorded_at timestamptz NOT NULL DEFAULT now()
);

COMMENT ON TABLE ai_usage_fact_ids IS
  'The shared identity namespace of usage facts and correction records. A fact id is also its export event id and is never reissued or rewritten.';
COMMENT ON COLUMN ai_usage_fact_ids.fact_id IS
  'Immutable fact identity; also the export event identity of this fact.';

-- Historical rows join the namespace (the audit above proves this cannot
-- conflict). One pass over retained facts; nothing else scans these tables.
INSERT INTO ai_usage_fact_ids (fact_id, fact_kind)
SELECT id, 'usage' FROM ai_usage_ledger;
INSERT INTO ai_usage_fact_ids (fact_id, fact_kind)
SELECT id, 'correction' FROM ai_usage_corrections;

CREATE OR REPLACE FUNCTION ai_usage_register_fact_id()
RETURNS trigger LANGUAGE plpgsql AS $$
DECLARE
  v_kind text := TG_ARGV[0];
BEGIN
  IF TG_OP = 'UPDATE' THEN
    RAISE EXCEPTION 'ai_usage fact ids are immutable (%, id %)', TG_TABLE_NAME, NEW.id;
  END IF;
  INSERT INTO ai_usage_fact_ids (fact_id, fact_kind) VALUES (NEW.id, v_kind);
  RETURN NEW;
EXCEPTION WHEN unique_violation THEN
  -- Re-registering the same fact row is harmless: that row's own primary key
  -- decides the insert. A different fact kind is a namespace collision.
  IF EXISTS (
    SELECT 1 FROM ai_usage_fact_ids WHERE fact_id = NEW.id AND fact_kind = v_kind
  ) THEN
    RETURN NEW;
  END IF;
  RAISE EXCEPTION 'ai_usage fact id % is already used by another fact record', NEW.id;
END;
$$;

CREATE TRIGGER ai_usage_register_usage_fact_id_trigger
BEFORE INSERT OR UPDATE OF id ON ai_usage_ledger
FOR EACH ROW EXECUTE FUNCTION ai_usage_register_fact_id('usage');

CREATE TRIGGER ai_usage_register_correction_fact_id_trigger
BEFORE INSERT OR UPDATE OF id ON ai_usage_corrections
FOR EACH ROW EXECUTE FUNCTION ai_usage_register_fact_id('correction');
