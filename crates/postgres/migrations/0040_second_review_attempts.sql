-- Automatic AI second review: one attempt per record, stage and protocol version.
-- The primary key makes the worker sweep idempotent, and a record whose attempt
-- failed for a model reason is not retried on every tick (no flood of runs).
CREATE TABLE IF NOT EXISTS second_review_attempts (
  project_id uuid NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
  report_id uuid NOT NULL REFERENCES reports(id) ON DELETE CASCADE,
  stage text NOT NULL CHECK (stage IN ('title_abstract', 'full_text')),
  protocol_version_id uuid NOT NULL,
  created_at timestamptz NOT NULL DEFAULT now(),
  PRIMARY KEY (project_id, report_id, stage, protocol_version_id)
);

-- The calibration refusal the sweep met last, per project and stage. A calibration
-- made for an earlier compiled review is stale. A refused admission writes no run,
-- so nothing else records that, and the status would otherwise still read "automatic".
CREATE TABLE IF NOT EXISTS second_review_gate (
  project_id uuid NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
  stage text NOT NULL CHECK (stage IN ('title_abstract', 'full_text')),
  refusal text NOT NULL CHECK (refusal IN ('calibration_stale')),
  observed_at timestamptz NOT NULL DEFAULT now(),
  PRIMARY KEY (project_id, stage)
);
