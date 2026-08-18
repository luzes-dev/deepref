ALTER TABLE processed_events
  ADD COLUMN IF NOT EXISTS owner_token uuid,
  ADD COLUMN IF NOT EXISTS lease_expires_at timestamptz,
  ADD COLUMN IF NOT EXISTS attempts integer NOT NULL DEFAULT 0,
  ADD COLUMN IF NOT EXISTS completed_at timestamptz,
  ADD COLUMN IF NOT EXISTS last_error text;

UPDATE processed_events SET completed_at = processed_at WHERE completed_at IS NULL;
ALTER TABLE processed_events ALTER COLUMN processed_at DROP NOT NULL;

ALTER TABLE doi_fetch_state
  ADD COLUMN IF NOT EXISTS owner_token uuid,
  ADD COLUMN IF NOT EXISTS lease_expires_at timestamptz,
  ADD COLUMN IF NOT EXISTS heartbeat_at timestamptz,
  ADD COLUMN IF NOT EXISTS attempts integer NOT NULL DEFAULT 0;

ALTER TABLE ingestion_items
  ADD COLUMN IF NOT EXISTS work_event_id uuid;

ALTER TABLE event_outbox
  ADD COLUMN IF NOT EXISTS next_attempt_at timestamptz NOT NULL DEFAULT now(),
  ADD COLUMN IF NOT EXISTS max_attempts integer NOT NULL DEFAULT 12,
  ADD COLUMN IF NOT EXISTS exhausted_at timestamptz;

CREATE TABLE IF NOT EXISTS fetched_citation_facts (
  source_doi text NOT NULL REFERENCES works(canonical_doi) ON DELETE CASCADE,
  target_doi text NOT NULL REFERENCES works(canonical_doi) ON DELETE CASCADE,
  source text NOT NULL DEFAULT 'crossref-reference',
  created_at timestamptz NOT NULL DEFAULT now(),
  PRIMARY KEY (source_doi, target_doi)
);

CREATE TABLE IF NOT EXISTS fetched_unresolved_reference_facts (
  id text PRIMARY KEY,
  source_doi text NOT NULL REFERENCES works(canonical_doi) ON DELETE CASCADE,
  raw_unstructured text,
  article_title text,
  author text,
  year text,
  volume text,
  first_page text,
  created_at timestamptz NOT NULL DEFAULT now()
);

ALTER TABLE unresolved_references DROP CONSTRAINT IF EXISTS unresolved_references_pkey;
ALTER TABLE unresolved_references
  ADD CONSTRAINT unresolved_references_pkey PRIMARY KEY (project_id, id);

CREATE TABLE IF NOT EXISTS dead_letter_records (
  identity text PRIMARY KEY,
  source_subject text NOT NULL,
  source_event_id uuid,
  delivery_count bigint NOT NULL,
  reason_code text NOT NULL,
  payload_sha256 text NOT NULL,
  payload jsonb NOT NULL DEFAULT '{}'::jsonb,
  outbox_event_id uuid NOT NULL,
  first_seen_at timestamptz NOT NULL DEFAULT now(),
  last_seen_at timestamptz NOT NULL DEFAULT now()
);

CREATE TABLE IF NOT EXISTS provider_rate_state (
  provider text PRIMARY KEY,
  next_permit_at timestamptz NOT NULL DEFAULT now(),
  updated_at timestamptz NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS processed_events_expired_idx ON processed_events (lease_expires_at)
  WHERE completed_at IS NULL;
CREATE INDEX IF NOT EXISTS doi_fetch_state_stale_idx ON doi_fetch_state (lease_expires_at)
  WHERE status = 'fetching';
CREATE INDEX IF NOT EXISTS ingestion_items_nonterminal_idx ON ingestion_items (ingestion_id, queued_at)
  WHERE status IN ('queued', 'fetching');
CREATE INDEX IF NOT EXISTS event_outbox_retry_idx ON event_outbox (next_attempt_at, created_at)
  WHERE published_at IS NULL AND exhausted_at IS NULL;
CREATE INDEX IF NOT EXISTS event_outbox_exhausted_idx ON event_outbox (exhausted_at)
  WHERE exhausted_at IS NOT NULL;
