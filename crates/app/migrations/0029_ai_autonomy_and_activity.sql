-- AI autonomy per project and task, the activity feed that records what the
-- AI / automations / assistant did (with enough state to undo it), the AI
-- "second reviewer" decisions used for screening conflicts, and the
-- "to verify" flag on AI-entered extraction values.

-- Which tasks may be configured. Protocol publishing and final exclusion of a
-- study are deliberately NOT in this list: they are never automatic, so they
-- cannot even be stored. The second CHECK limits which levels make sense for
-- each task (the AI never finalizes a screening decision alone).
CREATE TABLE project_ai_autonomy (
  project_id uuid NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
  task text NOT NULL CHECK (task IN (
    'exact_duplicates', 'fuzzy_duplicates', 'title_abstract_screening',
    'full_text_screening', 'extraction', 'appraisal', 'study_grouping'
  )),
  level text NOT NULL CHECK (level IN ('off', 'suggest', 'second_reviewer', 'act')),
  updated_by_kind text NOT NULL,
  updated_by_id text NOT NULL,
  updated_at timestamptz NOT NULL DEFAULT now(),
  PRIMARY KEY (project_id, task),
  CONSTRAINT project_ai_autonomy_level_allowed CHECK (
    (task IN ('title_abstract_screening', 'full_text_screening') AND level <> 'act')
    OR (task IN ('appraisal', 'study_grouping') AND level IN ('off', 'suggest'))
    OR (task = 'exact_duplicates' AND level IN ('suggest', 'act'))
    OR (task IN ('fuzzy_duplicates', 'extraction') AND level <> 'second_reviewer')
  )
);

-- One row per thing the AI, an automation or the assistant did. `before_state`
-- and `after_state` hold what is needed to undo it; `undo_kind` names how.
CREATE TABLE ai_activity (
  id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
  project_id uuid NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
  actor_type text NOT NULL CHECK (actor_type IN ('ai', 'automation', 'assistant')),
  actor_label text NOT NULL CHECK (length(btrim(actor_label)) > 0),
  actor_kind text NOT NULL,
  actor_id text NOT NULL,
  task text NOT NULL CHECK (length(btrim(task)) > 0),
  action text NOT NULL CHECK (length(btrim(action)) > 0),
  summary text NOT NULL CHECK (length(btrim(summary)) > 0),
  affected jsonb NOT NULL DEFAULT '[]'::jsonb CHECK (jsonb_typeof(affected) = 'array'),
  before_state jsonb NOT NULL DEFAULT '{}'::jsonb,
  after_state jsonb NOT NULL DEFAULT '{}'::jsonb,
  undo_kind text CHECK (undo_kind IS NULL OR undo_kind IN (
    'extraction_values', 'duplicate_link', 'screening_event', 'reviewer_decision'
  )),
  batch_id uuid,
  ai_run_id uuid,
  proposal_id uuid,
  model text,
  prompt_version text,
  evidence jsonb NOT NULL DEFAULT '[]'::jsonb CHECK (jsonb_typeof(evidence) = 'array'),
  created_at timestamptz NOT NULL DEFAULT now(),
  undone_at timestamptz,
  undone_by_kind text,
  undone_by_id text,
  CONSTRAINT ai_activity_undone_shape CHECK (
    (undone_at IS NULL AND undone_by_kind IS NULL AND undone_by_id IS NULL)
    OR (undone_at IS NOT NULL AND undone_by_kind IS NOT NULL AND undone_by_id IS NOT NULL)
  )
);
CREATE INDEX ai_activity_project_idx ON ai_activity (project_id, created_at DESC, id DESC);
CREATE INDEX ai_activity_batch_idx ON ai_activity (project_id, batch_id) WHERE batch_id IS NOT NULL;
CREATE INDEX ai_activity_task_idx ON ai_activity (project_id, task, created_at DESC);

-- The AI's independent screening opinion (second reviewer). It never changes
-- screening_state. A conflict exists when the human decision for the same
-- stage differs from it.
CREATE TABLE ai_reviewer_decisions (
  id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
  project_id uuid NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
  report_id uuid NOT NULL REFERENCES reports(id) ON DELETE CASCADE,
  stage text NOT NULL CHECK (stage IN ('title_abstract', 'full_text')),
  decision text NOT NULL CHECK (decision IN ('include', 'exclude', 'maybe')),
  rationale text NOT NULL CHECK (length(btrim(rationale)) > 0),
  evidence jsonb NOT NULL DEFAULT '[]'::jsonb CHECK (jsonb_typeof(evidence) = 'array'),
  source text NOT NULL DEFAULT 'ai' CHECK (source IN ('ai', 'workflow')),
  proposal_id uuid,
  ai_run_id uuid,
  model text,
  prompt_version text,
  activity_id uuid REFERENCES ai_activity(id) ON DELETE SET NULL,
  created_at timestamptz NOT NULL DEFAULT now(),
  voided_at timestamptz,
  human_decision_before text,
  resolved_at timestamptz,
  resolved_by_kind text,
  resolved_by_id text,
  resolution text CHECK (resolution IS NULL OR resolution IN ('kept_human', 'adopted_ai', 'other')),
  resolution_note text,
  FOREIGN KEY (project_id, report_id) REFERENCES project_reports(project_id, report_id) ON DELETE CASCADE
);
CREATE UNIQUE INDEX ai_reviewer_decisions_current_uq
  ON ai_reviewer_decisions (project_id, report_id, stage) WHERE voided_at IS NULL;
CREATE INDEX ai_reviewer_decisions_project_idx
  ON ai_reviewer_decisions (project_id, stage, created_at DESC);

-- AI-entered extraction values are flagged until a person confirms them.
ALTER TABLE extraction_values
  ADD COLUMN needs_verification boolean NOT NULL DEFAULT false,
  ADD COLUMN verified_at timestamptz,
  ADD COLUMN verified_by_actor_kind text,
  ADD COLUMN verified_by_actor_id text,
  ADD COLUMN activity_id uuid;
CREATE INDEX extraction_values_to_verify_idx
  ON extraction_values (project_id, study_id) WHERE needs_verification AND superseded_at IS NULL;

-- Reprocessing records who asked for it without overwriting the creator.
ALTER TABLE documents
  ADD COLUMN reparsed_by_kind text,
  ADD COLUMN reparsed_by_id text,
  ADD COLUMN reparsed_at timestamptz;
