-- PR7 title/abstract screening: audit snapshots, deterministic legacy replay,
-- bounded queue indexes, and final event/projection invariants.
ALTER TABLE screening_events
  ADD COLUMN IF NOT EXISTS event_kind text NOT NULL DEFAULT 'decision',
  ADD COLUMN IF NOT EXISTS undoes_event_id uuid REFERENCES screening_events(id),
  ADD COLUMN IF NOT EXISTS previous_title_abstract_status text NOT NULL DEFAULT 'unscreened',
  ADD COLUMN IF NOT EXISTS previous_full_text_status text NOT NULL DEFAULT 'not_required',
  ADD COLUMN IF NOT EXISTS previous_full_text_exclusion_reason_id uuid REFERENCES exclusion_reasons(id),
  ADD COLUMN IF NOT EXISTS previous_final_status text NOT NULL DEFAULT 'unscreened',
  ADD COLUMN IF NOT EXISTS result_title_abstract_status text NOT NULL DEFAULT 'unscreened',
  ADD COLUMN IF NOT EXISTS result_full_text_status text NOT NULL DEFAULT 'not_required',
  ADD COLUMN IF NOT EXISTS result_full_text_exclusion_reason_id uuid REFERENCES exclusion_reasons(id),
  ADD COLUMN IF NOT EXISTS result_final_status text NOT NULL DEFAULT 'unscreened';

ALTER TABLE screening_events
  ALTER COLUMN decision DROP NOT NULL;

ALTER TABLE screening_events
  DROP CONSTRAINT IF EXISTS screening_events_check,
  DROP CONSTRAINT IF EXISTS screening_events_decision_shape_check,
  DROP CONSTRAINT IF EXISTS screening_events_event_kind_check,
  DROP CONSTRAINT IF EXISTS screening_events_event_shape_check,
  DROP CONSTRAINT IF EXISTS screening_events_previous_title_status_check,
  DROP CONSTRAINT IF EXISTS screening_events_previous_full_status_check,
  DROP CONSTRAINT IF EXISTS screening_events_previous_final_status_check,
  DROP CONSTRAINT IF EXISTS screening_events_result_title_status_check,
  DROP CONSTRAINT IF EXISTS screening_events_result_full_status_check,
  DROP CONSTRAINT IF EXISTS screening_events_result_final_status_check,
  DROP CONSTRAINT IF EXISTS screening_events_previous_reason_shape_check,
  DROP CONSTRAINT IF EXISTS screening_events_result_reason_shape_check,
  DROP CONSTRAINT IF EXISTS screening_events_actor_id_check;

-- Replay the original event stream in a stable order. The projection is
-- rebuilt as each event is applied, so pre-PR7 snapshots and revisions are not
-- inferred from migration defaults.
-- The pre-PR7 schema could not encode undo events. A database that already
-- contains an event_kind=undo row is therefore a partial rollout, and this
-- migration aborts instead of treating defaulted snapshots as authoritative.
DO $$
DECLARE
  event_row RECORD;
  current_project uuid;
  current_report uuid;
  title_status text;
  full_text_status text;
  full_text_reason uuid;
  final_status text;
  revision bigint;
  previous_stage_event uuid;
BEGIN
  FOR event_row IN
    SELECT *
    FROM screening_events
    ORDER BY project_id, report_id, created_at, id
  LOOP
	IF event_row.event_kind = 'undo' THEN
	  RAISE EXCEPTION 'cannot replay a pre-existing undo event without authoritative snapshots';
	END IF;

    IF current_project IS DISTINCT FROM event_row.project_id
       OR current_report IS DISTINCT FROM event_row.report_id THEN
      current_project := event_row.project_id;
      current_report := event_row.report_id;
      title_status := 'unscreened';
      full_text_status := 'not_required';
      full_text_reason := NULL;
      final_status := 'unscreened';
      revision := 0;
    END IF;

    UPDATE screening_events
    SET previous_title_abstract_status = title_status,
        previous_full_text_status = full_text_status,
        previous_full_text_exclusion_reason_id = full_text_reason,
        previous_final_status = final_status
    WHERE id = event_row.id;

    SELECT id
    INTO previous_stage_event
    FROM screening_events
    WHERE project_id = event_row.project_id
      AND report_id = event_row.report_id
      AND stage = event_row.stage
      AND (created_at, id) < (event_row.created_at, event_row.id)
    ORDER BY created_at DESC, id DESC
    LIMIT 1;

    IF event_row.stage = 'title_abstract' THEN
      title_status := coalesce(event_row.decision, title_status);
      IF title_status <> 'include' THEN
        full_text_status := 'not_required';
        full_text_reason := NULL;
      END IF;
    ELSIF event_row.stage = 'full_text' AND title_status = 'include' THEN
      full_text_status := coalesce(event_row.decision, full_text_status);
      IF full_text_status = 'exclude' THEN
        full_text_reason := event_row.exclusion_reason_id;
      ELSE
        full_text_reason := NULL;
      END IF;
    END IF;

    final_status := CASE
      WHEN title_status = 'unscreened' THEN 'unscreened'
      WHEN title_status = 'exclude' THEN 'exclude'
      WHEN title_status = 'maybe' THEN 'maybe'
      WHEN full_text_status = 'include' THEN 'include'
      WHEN full_text_status = 'exclude' THEN 'exclude'
      WHEN full_text_status = 'maybe' THEN 'maybe'
      ELSE 'pending_full_text'
    END;
    revision := revision + 1;

    UPDATE screening_events
    SET supersedes_event_id = CASE
          WHEN event_row.event_kind = 'decision' THEN previous_stage_event
          ELSE supersedes_event_id
        END,
        result_title_abstract_status = title_status,
        result_full_text_status = full_text_status,
        result_full_text_exclusion_reason_id = full_text_reason,
        result_final_status = final_status
    WHERE id = event_row.id;

    INSERT INTO screening_state (
      project_id, report_id, title_abstract_status, full_text_status,
      full_text_exclusion_reason_id, final_status, revision, last_event_id
    ) VALUES (
      event_row.project_id, event_row.report_id, title_status, full_text_status,
      full_text_reason, final_status, revision, event_row.id
    )
    ON CONFLICT (project_id, report_id) DO UPDATE SET
      title_abstract_status = EXCLUDED.title_abstract_status,
      full_text_status = EXCLUDED.full_text_status,
      full_text_exclusion_reason_id = EXCLUDED.full_text_exclusion_reason_id,
      final_status = EXCLUDED.final_status,
      revision = EXCLUDED.revision,
      last_event_id = EXCLUDED.last_event_id,
      updated_at = now();
  END LOOP;
END $$;

ALTER TABLE screening_events
  ADD CONSTRAINT screening_events_event_kind_check
    CHECK (event_kind IN ('decision', 'undo')),
  ADD CONSTRAINT screening_events_event_shape_check CHECK (
    (event_kind = 'decision'
      AND decision IS NOT NULL
      AND undoes_event_id IS NULL
      AND (
        (stage = 'title_abstract' AND exclusion_reason_id IS NULL)
        OR (stage = 'full_text'
          AND ((decision = 'exclude' AND exclusion_reason_id IS NOT NULL)
            OR (decision IN ('include', 'maybe') AND exclusion_reason_id IS NULL)))
      ))
    OR (event_kind = 'undo'
      AND decision IS NULL
      AND exclusion_reason_id IS NULL
      AND undoes_event_id IS NOT NULL)
  ),
  ADD CONSTRAINT screening_events_previous_title_status_check CHECK (
    previous_title_abstract_status IN ('unscreened', 'include', 'exclude', 'maybe')
  ),
  ADD CONSTRAINT screening_events_previous_full_status_check CHECK (
    previous_full_text_status IN ('not_required', 'unscreened', 'include', 'exclude', 'maybe')
  ),
  ADD CONSTRAINT screening_events_previous_final_status_check CHECK (
    previous_final_status IN ('unscreened', 'pending_full_text', 'include', 'exclude', 'maybe')
  ),
  ADD CONSTRAINT screening_events_result_title_status_check CHECK (
    result_title_abstract_status IN ('unscreened', 'include', 'exclude', 'maybe')
  ),
  ADD CONSTRAINT screening_events_result_full_status_check CHECK (
    result_full_text_status IN ('not_required', 'unscreened', 'include', 'exclude', 'maybe')
  ),
  ADD CONSTRAINT screening_events_result_final_status_check CHECK (
    result_final_status IN ('unscreened', 'pending_full_text', 'include', 'exclude', 'maybe')
  ),
  ADD CONSTRAINT screening_events_previous_reason_shape_check CHECK (
    (previous_full_text_status = 'exclude' AND previous_full_text_exclusion_reason_id IS NOT NULL)
    OR (previous_full_text_status <> 'exclude' AND previous_full_text_exclusion_reason_id IS NULL)
  ),
  ADD CONSTRAINT screening_events_result_reason_shape_check CHECK (
    (result_full_text_status = 'exclude' AND result_full_text_exclusion_reason_id IS NOT NULL)
    OR (result_full_text_status <> 'exclude' AND result_full_text_exclusion_reason_id IS NULL)
  ),
  ADD CONSTRAINT screening_events_actor_id_check CHECK (length(btrim(actor_id)) > 0);

ALTER TABLE screening_state
  DROP CONSTRAINT IF EXISTS screening_state_check,
  DROP CONSTRAINT IF EXISTS screening_state_full_text_exclusion_reason_check,
  DROP CONSTRAINT IF EXISTS screening_state_reason_shape_check,
  DROP CONSTRAINT IF EXISTS screening_state_stage_shape_check;

ALTER TABLE screening_state
  ADD CONSTRAINT screening_state_reason_shape_check CHECK (
    (full_text_status = 'exclude' AND full_text_exclusion_reason_id IS NOT NULL)
    OR (full_text_status <> 'exclude' AND full_text_exclusion_reason_id IS NULL)
  ),
  ADD CONSTRAINT screening_state_stage_shape_check CHECK (
    (title_abstract_status = 'include')
    OR (full_text_status = 'not_required' AND full_text_exclusion_reason_id IS NULL)
  );

CREATE INDEX IF NOT EXISTS project_reports_screening_queue_idx
  ON project_reports (project_id, created_at, report_id);

CREATE INDEX IF NOT EXISTS screening_state_title_status_idx
  ON screening_state (project_id, title_abstract_status, report_id);

CREATE INDEX IF NOT EXISTS reports_title_abstract_trgm_idx
  ON reports USING gin (
    lower(coalesce(title, '') || ' ' || coalesce(abstract_text, '')) gin_trgm_ops
  );

CREATE INDEX IF NOT EXISTS screening_events_history_idx
  ON screening_events (project_id, report_id, created_at DESC, id DESC);
