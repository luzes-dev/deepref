-- PR4: generic acquisition provenance and source-record metadata.

ALTER TABLE acquisition_runs
  ALTER COLUMN legacy_ingestion_id DROP NOT NULL,
  ALTER COLUMN source SET DEFAULT 'acquisition',
  ALTER COLUMN status SET DEFAULT 'queued',
  ALTER COLUMN max_depth SET DEFAULT 0,
  ALTER COLUMN seed_count SET DEFAULT 0,
  ALTER COLUMN queued_count SET DEFAULT 0,
  ALTER COLUMN fetched_count SET DEFAULT 0,
  ALTER COLUMN failed_count SET DEFAULT 0,
  ALTER COLUMN metadata_provider SET DEFAULT '',
  ALTER COLUMN citation_provider SET DEFAULT '';

ALTER TABLE acquisition_runs
  ADD COLUMN IF NOT EXISTS strategy text NOT NULL DEFAULT 'legacy_ingestion',
  ADD COLUMN IF NOT EXISTS format text,
  ADD COLUMN IF NOT EXISTS idempotency_key text,
  ADD COLUMN IF NOT EXISTS config jsonb NOT NULL DEFAULT '{}'::jsonb,
  ADD COLUMN IF NOT EXISTS metadata jsonb NOT NULL DEFAULT '{}'::jsonb,
  ADD COLUMN IF NOT EXISTS error text;

CREATE UNIQUE INDEX IF NOT EXISTS acquisition_runs_project_idempotency_idx
  ON acquisition_runs (project_id, idempotency_key)
  WHERE idempotency_key IS NOT NULL;
CREATE INDEX IF NOT EXISTS acquisition_runs_project_created_idx
  ON acquisition_runs (project_id, created_at DESC, id DESC);

ALTER TABLE records
  ADD COLUMN IF NOT EXISTS journal text,
  ADD COLUMN IF NOT EXISTS authors jsonb NOT NULL DEFAULT '[]'::jsonb,
  ADD COLUMN IF NOT EXISTS source_identifiers jsonb NOT NULL DEFAULT '[]'::jsonb;

CREATE TABLE IF NOT EXISTS record_identifiers (
  id uuid PRIMARY KEY,
  record_id uuid NOT NULL REFERENCES records(id) ON DELETE CASCADE,
  scheme text NOT NULL,
  value text NOT NULL,
  normalized_value text NOT NULL,
  created_at timestamptz NOT NULL DEFAULT now(),
  UNIQUE (record_id, scheme, normalized_value)
);

CREATE INDEX IF NOT EXISTS record_identifiers_lookup_idx
  ON record_identifiers (scheme, normalized_value);
