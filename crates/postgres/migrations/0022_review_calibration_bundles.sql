-- Expert-adjudicated admission records for automation-triggered review runs.
-- The semantic bundle hash binds definitions, workflow, prompts, schemas,
-- policy, parser, protocol, model routes, and runtime build.

CREATE TABLE review_calibration_bundles (
  id uuid PRIMARY KEY,
  project_id uuid NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
  definition_key text NOT NULL CHECK (definition_key IN (
    'screening', 'duplicate_detection', 'study_classification',
    'study_grouping', 'appraisal_prefill', 'data_extraction'
  )),
  semantic_bundle_hash text NOT NULL CHECK (semantic_bundle_hash ~ '^[0-9a-f]{64}$'),
  evaluation_set_id text NOT NULL CHECK (length(btrim(evaluation_set_id)) BETWEEN 1 AND 500),
  thresholds jsonb NOT NULL CHECK (jsonb_typeof(thresholds) = 'object'),
  metrics jsonb NOT NULL CHECK (jsonb_typeof(metrics) = 'object'),
  reviewer_metadata jsonb NOT NULL CHECK (jsonb_typeof(reviewer_metadata) = 'object'),
  status text NOT NULL CHECK (status IN ('passing', 'failed')),
  evaluated_at timestamptz NOT NULL,
  created_at timestamptz NOT NULL DEFAULT now(),
  UNIQUE (project_id, id)
);

CREATE INDEX review_calibration_bundles_exact_idx
  ON review_calibration_bundles(project_id, definition_key, semantic_bundle_hash, status);

CREATE OR REPLACE FUNCTION reject_review_calibration_bundle_changes()
RETURNS trigger
LANGUAGE plpgsql
AS $$
BEGIN
  RAISE EXCEPTION 'review calibration bundles are immutable'
    USING ERRCODE = 'integrity_constraint_violation';
END;
$$;

CREATE TRIGGER review_calibration_bundle_immutable_trigger
BEFORE UPDATE ON review_calibration_bundles
FOR EACH ROW EXECUTE FUNCTION reject_review_calibration_bundle_changes();
