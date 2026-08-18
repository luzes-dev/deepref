CREATE EXTENSION IF NOT EXISTS pg_trgm;

-- DeepRef v2 evidence identity. The legacy DOI-keyed works/citations tables remain
-- available during the compatibility importer phase.
CREATE TABLE IF NOT EXISTS acquisition_runs (
  id uuid PRIMARY KEY,
  project_id uuid NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
  strategy text NOT NULL,
  source text NOT NULL,
  query_metadata jsonb NOT NULL DEFAULT '{}'::jsonb,
  status text NOT NULL DEFAULT 'queued' CHECK (status IN ('queued', 'running', 'completed', 'failed', 'cancelled')),
  started_at timestamptz,
  completed_at timestamptz,
  created_at timestamptz NOT NULL DEFAULT now()
);

CREATE TABLE IF NOT EXISTS reports (
  id uuid PRIMARY KEY,
  title text,
  normalized_title text,
  abstract_text text,
  publication_year integer,
  journal text,
  raw jsonb NOT NULL DEFAULT '{}'::jsonb,
  created_at timestamptz NOT NULL DEFAULT now(),
  updated_at timestamptz NOT NULL DEFAULT now()
);

CREATE TABLE IF NOT EXISTS report_identifiers (
  id uuid PRIMARY KEY,
  report_id uuid NOT NULL REFERENCES reports(id) ON DELETE CASCADE,
  scheme text NOT NULL,
  value text NOT NULL,
  normalized_value text NOT NULL,
  created_at timestamptz NOT NULL DEFAULT now(),
  UNIQUE (scheme, normalized_value),
  UNIQUE (report_id, scheme, normalized_value)
);

CREATE TABLE IF NOT EXISTS records (
  id uuid PRIMARY KEY,
  acquisition_run_id uuid NOT NULL REFERENCES acquisition_runs(id) ON DELETE CASCADE,
  source text NOT NULL,
  source_record_id text,
  raw jsonb NOT NULL,
  resolved_report_id uuid REFERENCES reports(id) ON DELETE SET NULL,
  created_at timestamptz NOT NULL DEFAULT now(),
  UNIQUE (acquisition_run_id, source, source_record_id)
);

CREATE TABLE IF NOT EXISTS project_reports (
  project_id uuid NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
  report_id uuid NOT NULL REFERENCES reports(id) ON DELETE CASCADE,
  first_seen_acquisition_run_id uuid REFERENCES acquisition_runs(id) ON DELETE SET NULL,
  seed boolean NOT NULL DEFAULT false,
  min_depth integer NOT NULL DEFAULT 0,
  internal_citations integer NOT NULL DEFAULT 0,
  outbound_internal_references integer NOT NULL DEFAULT 0,
  rank_score double precision NOT NULL DEFAULT 0,
  metrics_computed_at timestamptz,
  created_at timestamptz NOT NULL DEFAULT now(),
  PRIMARY KEY (project_id, report_id)
);

-- Transitional name while the legacy citations table is still consumed by the v1 API.
CREATE TABLE IF NOT EXISTS citations_v2 (
  project_id uuid NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
  source_report_id uuid NOT NULL REFERENCES reports(id) ON DELETE CASCADE,
  target_report_id uuid NOT NULL REFERENCES reports(id) ON DELETE CASCADE,
  source text NOT NULL DEFAULT 'unknown',
  first_seen_acquisition_run_id uuid REFERENCES acquisition_runs(id) ON DELETE SET NULL,
  created_at timestamptz NOT NULL DEFAULT now(),
  PRIMARY KEY (project_id, source_report_id, target_report_id),
  CHECK (source_report_id <> target_report_id)
);

CREATE TABLE IF NOT EXISTS studies (
  id uuid PRIMARY KEY,
  project_id uuid NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
  name text NOT NULL,
  description text,
  created_at timestamptz NOT NULL DEFAULT now(),
  updated_at timestamptz NOT NULL DEFAULT now()
);

CREATE TABLE IF NOT EXISTS study_reports (
  study_id uuid NOT NULL REFERENCES studies(id) ON DELETE CASCADE,
  report_id uuid NOT NULL REFERENCES reports(id) ON DELETE CASCADE,
  relationship text NOT NULL DEFAULT 'report',
  created_at timestamptz NOT NULL DEFAULT now(),
  PRIMARY KEY (study_id, report_id)
);

CREATE TABLE IF NOT EXISTS protocol_versions (
  id uuid PRIMARY KEY,
  project_id uuid NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
  version integer NOT NULL,
  framework text NOT NULL DEFAULT 'custom',
  question jsonb NOT NULL DEFAULT '{}'::jsonb,
  status text NOT NULL DEFAULT 'draft' CHECK (status IN ('draft', 'published', 'superseded')),
  amendment_reason text,
  published_at timestamptz,
  created_at timestamptz NOT NULL DEFAULT now(),
  UNIQUE (project_id, version)
);

CREATE UNIQUE INDEX IF NOT EXISTS protocol_versions_one_draft_per_project_idx
  ON protocol_versions(project_id) WHERE status = 'draft';

CREATE TABLE IF NOT EXISTS eligibility_criteria (
  id uuid PRIMARY KEY,
  protocol_version_id uuid NOT NULL REFERENCES protocol_versions(id) ON DELETE CASCADE,
  position integer NOT NULL,
  kind text NOT NULL CHECK (kind IN ('inclusion', 'exclusion')),
  stage text NOT NULL CHECK (stage IN ('title_abstract', 'full_text', 'both')),
  dimension text NOT NULL,
  description text NOT NULL,
  created_at timestamptz NOT NULL DEFAULT now(),
  UNIQUE (protocol_version_id, position)
);

CREATE TABLE IF NOT EXISTS exclusion_reasons (
  id uuid PRIMARY KEY,
  project_id uuid NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
  code text NOT NULL,
  label text NOT NULL,
  position integer NOT NULL DEFAULT 0,
  active boolean NOT NULL DEFAULT true,
  created_at timestamptz NOT NULL DEFAULT now(),
  UNIQUE (project_id, code)
);

CREATE TABLE IF NOT EXISTS screening_events (
  id uuid PRIMARY KEY,
  project_id uuid NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
  report_id uuid NOT NULL REFERENCES reports(id) ON DELETE CASCADE,
  stage text NOT NULL CHECK (stage IN ('title_abstract', 'full_text')),
  decision text NOT NULL CHECK (decision IN ('include', 'exclude', 'maybe')),
  exclusion_reason_id uuid REFERENCES exclusion_reasons(id) ON DELETE RESTRICT,
  notes text,
  protocol_version_id uuid NOT NULL REFERENCES protocol_versions(id) ON DELETE RESTRICT,
  actor_kind text NOT NULL CHECK (actor_kind IN ('user', 'automation', 'system')),
  actor_id text,
  supersedes_event_id uuid REFERENCES screening_events(id) ON DELETE RESTRICT,
  created_at timestamptz NOT NULL DEFAULT now(),
  CHECK (decision = 'exclude' OR exclusion_reason_id IS NULL),
  CHECK (stage <> 'full_text' OR decision <> 'exclude' OR exclusion_reason_id IS NOT NULL)
);

CREATE TABLE IF NOT EXISTS screening_state (
  project_id uuid NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
  report_id uuid NOT NULL REFERENCES reports(id) ON DELETE CASCADE,
  title_abstract_status text NOT NULL DEFAULT 'unscreened',
  full_text_status text NOT NULL DEFAULT 'unscreened',
  final_status text NOT NULL DEFAULT 'unscreened',
  revision bigint NOT NULL DEFAULT 0 CHECK (revision >= 0),
  last_event_id uuid REFERENCES screening_events(id) ON DELETE RESTRICT,
  updated_at timestamptz NOT NULL DEFAULT now(),
  PRIMARY KEY (project_id, report_id)
);

CREATE TABLE IF NOT EXISTS study_classifications (
  id uuid PRIMARY KEY,
  study_id uuid NOT NULL REFERENCES studies(id) ON DELETE CASCADE,
  design text NOT NULL,
  source text NOT NULL DEFAULT 'human',
  created_at timestamptz NOT NULL DEFAULT now()
);

CREATE TABLE IF NOT EXISTS appraisal_assessments (
  id uuid PRIMARY KEY,
  study_id uuid NOT NULL REFERENCES studies(id) ON DELETE CASCADE,
  report_id uuid REFERENCES reports(id) ON DELETE SET NULL,
  definition_id text NOT NULL,
  definition_version text NOT NULL,
  responses jsonb NOT NULL DEFAULT '{}'::jsonb,
  overall_judgment text,
  completed_at timestamptz,
  created_at timestamptz NOT NULL DEFAULT now(),
  updated_at timestamptz NOT NULL DEFAULT now()
);

CREATE TABLE IF NOT EXISTS review_events (
  id uuid PRIMARY KEY,
  project_id uuid NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
  event_type text NOT NULL,
  entity_type text NOT NULL,
  entity_id uuid,
  payload jsonb NOT NULL,
  created_at timestamptz NOT NULL DEFAULT now()
);

CREATE TABLE IF NOT EXISTS jobs (
  id uuid PRIMARY KEY,
  kind text NOT NULL,
  payload jsonb NOT NULL,
  state text NOT NULL DEFAULT 'queued' CHECK (state IN ('queued', 'running', 'completed', 'dead')),
  priority integer NOT NULL DEFAULT 0,
  attempts integer NOT NULL DEFAULT 0 CHECK (attempts >= 0),
  max_attempts integer NOT NULL DEFAULT 5 CHECK (max_attempts > 0),
  available_at timestamptz NOT NULL DEFAULT now(),
  leased_until timestamptz,
  lease_owner text,
  dedupe_key text UNIQUE,
  last_error text,
  created_at timestamptz NOT NULL DEFAULT now(),
  completed_at timestamptz
);

CREATE TABLE IF NOT EXISTS documents (
  id uuid PRIMARY KEY,
  report_id uuid NOT NULL REFERENCES reports(id) ON DELETE CASCADE,
  state text NOT NULL DEFAULT 'uploaded' CHECK (state IN ('missing', 'external', 'uploaded', 'retrieving', 'available', 'failed')),
  original_name text,
  object_key text,
  external_url text,
  mime_type text,
  size_bytes bigint CHECK (size_bytes IS NULL OR size_bytes >= 0),
  sha256 text,
  parser_version text,
  created_at timestamptz NOT NULL DEFAULT now(),
  updated_at timestamptz NOT NULL DEFAULT now(),
  UNIQUE (report_id, sha256)
);

CREATE TABLE IF NOT EXISTS document_blocks (
  id uuid PRIMARY KEY,
  document_id uuid NOT NULL REFERENCES documents(id) ON DELETE CASCADE,
  parser_version text NOT NULL,
  page_number integer NOT NULL CHECK (page_number > 0),
  kind text NOT NULL,
  section_path text[] NOT NULL DEFAULT '{}',
  ordinal integer NOT NULL CHECK (ordinal >= 0),
  text text NOT NULL,
  bbox jsonb,
  search_vector tsvector GENERATED ALWAYS AS (to_tsvector('simple', coalesce(text, ''))) STORED,
  content_hash text NOT NULL,
  created_at timestamptz NOT NULL DEFAULT now(),
  UNIQUE (document_id, parser_version, ordinal)
);

CREATE TABLE IF NOT EXISTS automation_definitions (
  id uuid PRIMARY KEY,
  project_id uuid NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
  name text NOT NULL,
  trigger_kind text NOT NULL,
  definition jsonb NOT NULL,
  enabled boolean NOT NULL DEFAULT true,
  created_at timestamptz NOT NULL DEFAULT now(),
  updated_at timestamptz NOT NULL DEFAULT now()
);

CREATE TABLE IF NOT EXISTS automation_runs (
  id uuid PRIMARY KEY,
  automation_definition_id uuid REFERENCES automation_definitions(id) ON DELETE SET NULL,
  project_id uuid NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
  trigger_event_id uuid REFERENCES review_events(id) ON DELETE SET NULL,
  status text NOT NULL DEFAULT 'queued' CHECK (status IN ('queued', 'running', 'completed', 'failed', 'cancelled')),
  started_at timestamptz,
  completed_at timestamptz,
  created_at timestamptz NOT NULL DEFAULT now()
);

CREATE TABLE IF NOT EXISTS ai_runs (
  id uuid PRIMARY KEY,
  project_id uuid NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
  automation_run_id uuid REFERENCES automation_runs(id) ON DELETE SET NULL,
  task_kind text NOT NULL,
  provider text NOT NULL,
  model text NOT NULL,
  prompt_version text NOT NULL,
  schema_version text NOT NULL,
  input_hash text NOT NULL,
  evidence_refs jsonb NOT NULL DEFAULT '[]'::jsonb,
  output jsonb,
  usage jsonb NOT NULL DEFAULT '{}'::jsonb,
  status text NOT NULL CHECK (status IN ('running', 'completed', 'failed')),
  created_at timestamptz NOT NULL DEFAULT now(),
  completed_at timestamptz
);

CREATE INDEX IF NOT EXISTS ai_runs_reuse_idx
  ON ai_runs(project_id, task_kind, input_hash, prompt_version, model)
  WHERE status = 'completed';

CREATE TABLE IF NOT EXISTS ai_proposals (
  id uuid PRIMARY KEY,
  project_id uuid NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
  entity_type text NOT NULL,
  entity_id uuid,
  operation text NOT NULL,
  schema_version text NOT NULL,
  payload jsonb NOT NULL,
  status text NOT NULL DEFAULT 'pending' CHECK (status IN ('pending', 'accepted', 'rejected', 'expired')),
  model_run_id uuid NOT NULL REFERENCES ai_runs(id) ON DELETE RESTRICT,
  resolved_by text,
  resolved_at timestamptz,
  created_at timestamptz NOT NULL DEFAULT now(),
  CHECK ((status = 'pending' AND resolved_at IS NULL) OR status <> 'pending')
);

CREATE INDEX IF NOT EXISTS reports_title_trgm_idx
  ON reports USING gin (normalized_title gin_trgm_ops);
CREATE INDEX IF NOT EXISTS records_resolved_report_idx ON records(resolved_report_id);
CREATE INDEX IF NOT EXISTS project_reports_rank_idx_v2 ON project_reports(project_id, rank_score DESC);
CREATE INDEX IF NOT EXISTS citations_v2_target_idx ON citations_v2(project_id, target_report_id);
CREATE INDEX IF NOT EXISTS screening_events_report_idx ON screening_events(project_id, report_id, created_at DESC);
CREATE INDEX IF NOT EXISTS screening_state_title_abstract_idx ON screening_state(project_id, title_abstract_status, updated_at);
CREATE INDEX IF NOT EXISTS screening_state_full_text_idx ON screening_state(project_id, full_text_status, updated_at);
CREATE INDEX IF NOT EXISTS jobs_available_idx ON jobs(state, priority DESC, available_at) WHERE state = 'queued';
CREATE INDEX IF NOT EXISTS document_blocks_fts_idx ON document_blocks USING gin(search_vector);
