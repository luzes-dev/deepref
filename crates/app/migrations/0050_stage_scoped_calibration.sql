-- Stage-scoped calibration evidence, component identity snapshots and typed
-- admission refusals (identity scheme 2).
--
-- Legacy screening bundles stay scheme 1 with a NULL stage. A bundle written
-- before this migration stores one aggregate semantic hash and nothing that
-- proves which screening stage it was made for, so this migration does not
-- assign one: that would fabricate provenance. Legacy rows keep
-- identity_scheme = 1, stage = NULL and identity_snapshot = NULL. They remain
-- stored for audit, but no stage-scoped lookup selects them and admission
-- refuses them as an incompatible identity scheme, so they can never admit
-- automation.
--
-- This migration performs no UPDATE on review_calibration_bundles, and the
-- immutability trigger is untouched. The only column with a default is the
-- scheme, and its constant default lets PostgreSQL 11 and later add it without
-- rewriting rows or firing row triggers. The default is dropped straight after,
-- so a new bundle must state its scheme explicitly.
--
-- A new bundle must be scheme 2 or later. Screening bundles need a stage, other
-- definitions must not have one, and the component snapshot must agree with the
-- definition, stage and scheme columns. The BEFORE INSERT trigger refuses scheme
-- 1, so the legacy shape cannot be created again.

ALTER TABLE review_calibration_bundles
  ADD COLUMN stage text NULL CHECK (stage IN ('title_abstract', 'full_text')),
  ADD COLUMN identity_scheme integer NOT NULL DEFAULT 1,
  ADD COLUMN identity_snapshot jsonb NULL;

ALTER TABLE review_calibration_bundles
  ALTER COLUMN identity_scheme DROP DEFAULT;

-- A CHECK passes when its expression is NULL, so the shape check states
-- identity_snapshot IS NOT NULL explicitly. Without it a scheme 2 row with no
-- snapshot would evaluate to NULL and be accepted.
ALTER TABLE review_calibration_bundles
  ADD CONSTRAINT review_calibration_bundles_stage_scope_check CHECK (
    (definition_key = 'screening' AND (stage IS NOT NULL OR identity_scheme = 1))
    OR (definition_key <> 'screening' AND stage IS NULL)
  ),
  ADD CONSTRAINT review_calibration_bundles_identity_shape_check CHECK (
    (identity_scheme = 1 AND identity_snapshot IS NULL AND stage IS NULL)
    OR (
      identity_scheme >= 2
      AND identity_snapshot IS NOT NULL
      AND jsonb_typeof(identity_snapshot) = 'object'
      AND identity_snapshot->'scheme' = to_jsonb(identity_scheme)
      AND identity_snapshot->>'definition' = definition_key
      AND (identity_snapshot->>'stage') IS NOT DISTINCT FROM stage
      AND jsonb_typeof(identity_snapshot->'components') = 'object'
    )
  );

CREATE OR REPLACE FUNCTION reject_legacy_calibration_identity()
RETURNS trigger
LANGUAGE plpgsql
AS $$
BEGIN
  IF NEW.identity_scheme < 2 THEN
    RAISE EXCEPTION 'legacy calibration identity scheme 1 is read-only; new bundles need a component identity snapshot'
      USING ERRCODE = 'integrity_constraint_violation';
  END IF;
  RETURN NEW;
END;
$$;

CREATE TRIGGER review_calibration_bundle_scheme_insert_trigger
BEFORE INSERT ON review_calibration_bundles
FOR EACH ROW EXECUTE FUNCTION reject_legacy_calibration_identity();

-- Admission and the second-review sweep both ask for the newest passing bundle
-- of one definition and stage.
CREATE INDEX review_calibration_bundles_stage_admission_idx
  ON review_calibration_bundles(project_id, definition_key, stage, status, evaluated_at DESC);

-- A refusal now names its kind. Stage mismatch and incompatible identity are
-- recorded beside stale calibration, and `reasons` lists the changed components
-- of a stale refusal, so the gate can say what changed.
ALTER TABLE second_review_gate
  DROP CONSTRAINT second_review_gate_refusal_check,
  ADD CONSTRAINT second_review_gate_refusal_check CHECK (refusal IN (
    'calibration_stale', 'calibration_stage_mismatch', 'calibration_incompatible'
  )),
  ADD COLUMN reasons text[] NOT NULL DEFAULT '{}';
