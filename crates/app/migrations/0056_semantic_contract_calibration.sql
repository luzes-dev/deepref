-- Structured semantic contracts for calibration evidence (identity scheme 3).
--
-- Scheme 2 stored a decomposed component map: definition, prompt, schema,
-- policy, parser, protocol, models, provider endpoint, golden render/parse
-- fixtures, an implementation source hash and a dependency closure hash.
-- Scheme 3 stores a ReviewSemanticContract with only consequential behavior:
-- definition, semantic_version, stage, prompt, schema, policy, workflow,
-- parser, models and protocol.
--
-- Source-tree hashes, dependency closures, provider endpoints and golden
-- fixtures moved out of calibration identity: build provenance, request keys
-- and CI regression snapshots own them now, so they no longer gate
-- calibration compatibility.
--
-- Existing scheme 1 and scheme 2 rows are untouched. They remain stored for
-- audit, and admission refuses them as an incompatible identity scheme, so
-- they can never admit automation under the new recipe. The BEFORE INSERT
-- trigger still refuses scheme 1; scheme 2 rows stay writable through raw SQL
-- for the same audit reason, but no application code writes them anymore.

ALTER TABLE review_calibration_bundles
  DROP CONSTRAINT review_calibration_bundles_identity_shape_check,
  ADD CONSTRAINT review_calibration_bundles_identity_shape_check CHECK ((
    (identity_scheme = 1 AND identity_snapshot IS NULL AND stage IS NULL)
    OR (
      identity_scheme = 2
      AND identity_snapshot IS NOT NULL
      AND jsonb_typeof(identity_snapshot) = 'object'
      AND identity_snapshot->'scheme' = to_jsonb(identity_scheme)
      AND identity_snapshot->>'definition' = definition_key
      AND (identity_snapshot->>'stage') IS NOT DISTINCT FROM stage
      AND jsonb_typeof(identity_snapshot->'components') = 'object'
    )
    OR (
      identity_scheme = 3
      AND identity_snapshot IS NOT NULL
      AND jsonb_typeof(identity_snapshot) = 'object'
      AND identity_snapshot->'scheme' = to_jsonb(identity_scheme)
      AND identity_snapshot->>'definition' = definition_key
      AND (identity_snapshot->>'stage') IS NOT DISTINCT FROM stage
      AND jsonb_typeof(identity_snapshot->'semantic_version') = 'number'
      AND jsonb_typeof(identity_snapshot->'models') = 'array'
    )
  ) IS TRUE);
