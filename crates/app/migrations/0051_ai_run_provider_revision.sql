-- The provider's own report of what served each AI run's last call: the response `model` and
-- `system_fingerprint`. Audit data only. The report arrives after the call, so it is never part of
-- the semantic identity, and a provider may expose only a mutable alias, so it records what the
-- provider said rather than a pinned revision. Runs written before this migration have no report.
ALTER TABLE ai_runs
  ADD COLUMN IF NOT EXISTS provider_served_model text,
  ADD COLUMN IF NOT EXISTS provider_system_fingerprint text;

ALTER TABLE ai_runs
  ADD CONSTRAINT ai_runs_provider_served_model_check
    CHECK (provider_served_model IS NULL OR char_length(provider_served_model) BETWEEN 1 AND 128),
  ADD CONSTRAINT ai_runs_provider_system_fingerprint_check
    CHECK (provider_system_fingerprint IS NULL OR char_length(provider_system_fingerprint) BETWEEN 1 AND 128);
