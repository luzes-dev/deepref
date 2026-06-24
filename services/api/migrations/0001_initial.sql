CREATE TABLE IF NOT EXISTS settings (
  id integer PRIMARY KEY DEFAULT 1 CHECK (id = 1),
  crossref_mailto text NOT NULL DEFAULT '',
  default_max_depth integer NOT NULL DEFAULT 2,
  max_concurrency integer NOT NULL DEFAULT 8,
  rate_limit_per_second integer NOT NULL DEFAULT 1,
  retry_attempts integer NOT NULL DEFAULT 5,
  metadata_provider text NOT NULL DEFAULT 'crossref',
  citation_provider text NOT NULL DEFAULT 'crossref',
  created_at timestamptz NOT NULL DEFAULT now(),
  updated_at timestamptz NOT NULL DEFAULT now()
);

CREATE TABLE IF NOT EXISTS projects (
  id uuid PRIMARY KEY,
  name text NOT NULL,
  description text,
  default_max_depth integer NOT NULL DEFAULT 2,
  created_at timestamptz NOT NULL DEFAULT now(),
  updated_at timestamptz NOT NULL DEFAULT now()
);

CREATE TABLE IF NOT EXISTS ingestions (
  id uuid PRIMARY KEY,
  project_id uuid NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
  status text NOT NULL DEFAULT 'queued' CHECK (status IN ('queued', 'running', 'completed', 'failed', 'cancelled')),
  max_depth integer NOT NULL DEFAULT 2,
  seed_count integer NOT NULL DEFAULT 0,
  queued_count integer NOT NULL DEFAULT 0,
  fetched_count integer NOT NULL DEFAULT 0,
  failed_count integer NOT NULL DEFAULT 0,
  metadata_provider text NOT NULL DEFAULT 'crossref',
  citation_provider text NOT NULL DEFAULT 'crossref',
  created_at timestamptz NOT NULL DEFAULT now(),
  started_at timestamptz,
  completed_at timestamptz
);

CREATE TABLE IF NOT EXISTS ingestion_items (
  ingestion_id uuid NOT NULL REFERENCES ingestions(id) ON DELETE CASCADE,
  project_id uuid NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
  canonical_doi text NOT NULL,
  depth integer NOT NULL,
  parent_doi text,
  status text NOT NULL DEFAULT 'queued' CHECK (status IN ('queued', 'fetching', 'fetched', 'skipped', 'not_found', 'failed')),
  attempts integer NOT NULL DEFAULT 0,
  last_error text,
  queued_at timestamptz NOT NULL DEFAULT now(),
  fetched_at timestamptz,
  PRIMARY KEY (ingestion_id, canonical_doi)
);

CREATE TABLE IF NOT EXISTS doi_fetch_state (
  canonical_doi text PRIMARY KEY,
  status text NOT NULL DEFAULT 'queued' CHECK (status IN ('queued', 'fetching', 'fetched', 'not_found', 'failed')),
  locked_at timestamptz,
  fetched_at timestamptz,
  last_error text,
  updated_at timestamptz NOT NULL DEFAULT now()
);

CREATE TABLE IF NOT EXISTS works (
  canonical_doi text PRIMARY KEY,
  title text,
  abstract_text text,
  work_type text,
  publisher text,
  container_title text,
  issued_year integer,
  published_year integer,
  url text,
  total_citations integer NOT NULL DEFAULT 0,
  references_count integer NOT NULL DEFAULT 0,
  metadata_provider text NOT NULL DEFAULT 'crossref',
  citation_provider text NOT NULL DEFAULT 'crossref',
  fetch_status text NOT NULL DEFAULT 'stub' CHECK (fetch_status IN ('stub', 'fetched', 'not_found', 'failed')),
  fetched_at timestamptz,
  raw jsonb NOT NULL DEFAULT '{}'::jsonb
);

CREATE TABLE IF NOT EXISTS project_works (
  project_id uuid NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
  canonical_doi text NOT NULL REFERENCES works(canonical_doi) ON DELETE CASCADE,
  first_seen_ingestion_id uuid REFERENCES ingestions(id) ON DELETE SET NULL,
  seed boolean NOT NULL DEFAULT false,
  min_depth integer NOT NULL DEFAULT 0,
  internal_citations integer NOT NULL DEFAULT 0,
  outbound_internal_references integer NOT NULL DEFAULT 0,
  rank_score double precision NOT NULL DEFAULT 0,
  metrics_computed_at timestamptz,
  PRIMARY KEY (project_id, canonical_doi)
);

CREATE TABLE IF NOT EXISTS citations (
  project_id uuid NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
  source_doi text NOT NULL REFERENCES works(canonical_doi) ON DELETE CASCADE,
  target_doi text NOT NULL REFERENCES works(canonical_doi) ON DELETE CASCADE,
  source text NOT NULL DEFAULT 'crossref-reference',
  first_seen_ingestion_id uuid REFERENCES ingestions(id) ON DELETE SET NULL,
  created_at timestamptz NOT NULL DEFAULT now(),
  PRIMARY KEY (project_id, source_doi, target_doi)
);

CREATE TABLE IF NOT EXISTS unresolved_references (
  id text PRIMARY KEY,
  project_id uuid NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
  source_doi text NOT NULL REFERENCES works(canonical_doi) ON DELETE CASCADE,
  raw_unstructured text,
  article_title text,
  author text,
  year text,
  volume text,
  first_page text,
  created_at timestamptz NOT NULL DEFAULT now()
);

CREATE TABLE IF NOT EXISTS crossref_raw_records (
  canonical_doi text PRIMARY KEY REFERENCES works(canonical_doi) ON DELETE CASCADE,
  raw jsonb NOT NULL,
  fetched_at timestamptz NOT NULL DEFAULT now()
);

CREATE TABLE IF NOT EXISTS event_outbox (
  id uuid PRIMARY KEY,
  subject text NOT NULL,
  payload jsonb NOT NULL,
  published_at timestamptz,
  created_at timestamptz NOT NULL DEFAULT now()
);

CREATE TABLE IF NOT EXISTS processed_events (
  event_id uuid PRIMARY KEY,
  processed_at timestamptz NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS ingestion_items_status_idx ON ingestion_items (ingestion_id, status);
CREATE INDEX IF NOT EXISTS project_works_rank_idx ON project_works (project_id, rank_score DESC);
CREATE INDEX IF NOT EXISTS citations_project_target_idx ON citations (project_id, target_doi);
