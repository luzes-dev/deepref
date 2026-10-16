-- PostgreSQL CHECK accepts NULL; missing JSON keys must fail rather than leave
-- the snapshot-shape expression unknown. Preserve the explicit legacy exception.
ALTER TABLE review_calibration_bundles
  DROP CONSTRAINT review_calibration_bundles_identity_shape_check,
  ADD CONSTRAINT review_calibration_bundles_identity_shape_check CHECK ((
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
  ) IS TRUE);
