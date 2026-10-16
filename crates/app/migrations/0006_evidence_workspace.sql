-- DeepRef v2 evidence workspace primitives.
-- The legacy DOI tables remain readable during migration; v2 owns new review state.
CREATE EXTENSION IF NOT EXISTS pgcrypto;
CREATE EXTENSION IF NOT EXISTS pg_trgm;

CREATE TABLE IF NOT EXISTS reports (
  id uuid PRIMARY KEY,
  title text,
  abstract_text text,
  publication_year integer,
  journal text,
  url text,
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
  UNIQUE (scheme, normalized_value)
);

CREATE TABLE IF NOT EXISTS records (
  id uuid PRIMARY KEY,
  project_id uuid NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
  report_id uuid REFERENCES reports(id) ON DELETE SET NULL,
  source text NOT NULL,
  source_key text,
  title text,
  abstract_text text,
  publication_year integer,
  raw jsonb NOT NULL DEFAULT '{}'::jsonb,
  created_at timestamptz NOT NULL DEFAULT now(),
  UNIQUE (project_id, source, source_key)
);

CREATE TABLE IF NOT EXISTS project_reports (
  project_id uuid NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
  report_id uuid NOT NULL REFERENCES reports(id) ON DELETE CASCADE,
  first_seen_record_id uuid REFERENCES records(id) ON DELETE SET NULL,
  lifecycle_status text NOT NULL DEFAULT 'discovered'
    CHECK (lifecycle_status IN ('discovered', 'screening', 'included', 'excluded', 'maybe')),
  created_at timestamptz NOT NULL DEFAULT now(),
  PRIMARY KEY (project_id, report_id)
);

CREATE INDEX IF NOT EXISTS reports_title_trgm_idx
  ON reports USING gin (lower(coalesce(title, '')) gin_trgm_ops);
CREATE INDEX IF NOT EXISTS records_project_created_idx
  ON records (project_id, created_at DESC);

CREATE TABLE IF NOT EXISTS studies (
  id uuid PRIMARY KEY,
  project_id uuid NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
  title text,
  design text,
  raw jsonb NOT NULL DEFAULT '{}'::jsonb,
  created_at timestamptz NOT NULL DEFAULT now(),
  updated_at timestamptz NOT NULL DEFAULT now()
);

CREATE TABLE IF NOT EXISTS study_reports (
  study_id uuid NOT NULL REFERENCES studies(id) ON DELETE CASCADE,
  report_id uuid NOT NULL REFERENCES reports(id) ON DELETE CASCADE,
  relationship text NOT NULL DEFAULT 'report_of_study',
  created_at timestamptz NOT NULL DEFAULT now(),
  PRIMARY KEY (study_id, report_id)
);

CREATE TABLE IF NOT EXISTS protocol_versions (
  id uuid PRIMARY KEY,
  project_id uuid NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
  version integer NOT NULL,
  name text NOT NULL,
  status text NOT NULL CHECK (status IN ('draft', 'published', 'superseded')),
  criteria jsonb NOT NULL DEFAULT '[]'::jsonb,
  published_at timestamptz,
  created_at timestamptz NOT NULL DEFAULT now(),
  UNIQUE (project_id, version)
);

CREATE TABLE IF NOT EXISTS eligibility_criteria (
  id uuid PRIMARY KEY,
  protocol_version_id uuid NOT NULL REFERENCES protocol_versions(id) ON DELETE CASCADE,
  criterion_type text NOT NULL CHECK (criterion_type IN ('include', 'exclude')),
  label text NOT NULL,
  description text NOT NULL,
  ordinal integer NOT NULL DEFAULT 0,
  UNIQUE (protocol_version_id, ordinal)
);

CREATE TABLE IF NOT EXISTS exclusion_reasons (
  id uuid PRIMARY KEY,
  project_id uuid NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
  code text NOT NULL,
  label text NOT NULL,
  stage text NOT NULL DEFAULT 'full_text' CHECK (stage IN ('title_abstract', 'full_text')),
  UNIQUE (project_id, code)
);

CREATE TABLE IF NOT EXISTS screening_events (
  id uuid PRIMARY KEY,
  project_id uuid NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
  report_id uuid NOT NULL REFERENCES reports(id) ON DELETE CASCADE,
  stage text NOT NULL CHECK (stage IN ('title_abstract', 'full_text')),
  decision text NOT NULL CHECK (decision IN ('include', 'exclude', 'maybe')),
  exclusion_reason_id uuid REFERENCES exclusion_reasons(id),
  notes text,
  protocol_version_id uuid NOT NULL REFERENCES protocol_versions(id),
  actor_kind text NOT NULL CHECK (actor_kind IN ('user', 'automation', 'system')),
  actor_id text NOT NULL,
  supersedes_event_id uuid REFERENCES screening_events(id),
  created_at timestamptz NOT NULL DEFAULT now(),
  CHECK (stage <> 'full_text' OR decision <> 'exclude' OR exclusion_reason_id IS NOT NULL)
);

CREATE INDEX IF NOT EXISTS screening_events_report_created_idx
  ON screening_events (project_id, report_id, created_at DESC);

CREATE TABLE IF NOT EXISTS screening_state (
  project_id uuid NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
  report_id uuid NOT NULL REFERENCES reports(id) ON DELETE CASCADE,
  title_abstract_status text NOT NULL DEFAULT 'unscreened'
    CHECK (title_abstract_status IN ('unscreened', 'include', 'exclude', 'maybe')),
  full_text_status text NOT NULL DEFAULT 'not_required'
    CHECK (full_text_status IN ('not_required', 'unscreened', 'include', 'exclude', 'maybe')),
  full_text_exclusion_reason_id uuid REFERENCES exclusion_reasons(id),
  final_status text NOT NULL DEFAULT 'unscreened'
    CHECK (final_status IN ('unscreened', 'pending_full_text', 'include', 'exclude', 'maybe')),
  revision bigint NOT NULL DEFAULT 0 CHECK (revision >= 0),
  last_event_id uuid REFERENCES screening_events(id),
  updated_at timestamptz NOT NULL DEFAULT now(),
  PRIMARY KEY (project_id, report_id),
  CHECK (full_text_status <> 'exclude' OR full_text_exclusion_reason_id IS NOT NULL)
);

CREATE TABLE IF NOT EXISTS review_events (
  id uuid PRIMARY KEY,
  project_id uuid NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
  event_type text NOT NULL,
  aggregate_type text NOT NULL,
  aggregate_id uuid NOT NULL,
  payload jsonb NOT NULL,
  actor_kind text NOT NULL,
  actor_id text NOT NULL,
  created_at timestamptz NOT NULL DEFAULT now()
);

CREATE TABLE IF NOT EXISTS jobs (
  id uuid PRIMARY KEY,
  kind text NOT NULL,
  payload jsonb NOT NULL DEFAULT '{}'::jsonb,
  state text NOT NULL DEFAULT 'queued' CHECK (state IN ('queued', 'running', 'completed', 'failed', 'dead')),
  priority integer NOT NULL DEFAULT 0,
  attempts integer NOT NULL DEFAULT 0,
  max_attempts integer NOT NULL DEFAULT 5,
  available_at timestamptz NOT NULL DEFAULT now(),
  leased_until timestamptz,
  lease_owner text,
  dedupe_key text UNIQUE,
  last_error text,
  created_at timestamptz NOT NULL DEFAULT now(),
  completed_at timestamptz
);

CREATE INDEX IF NOT EXISTS jobs_claim_idx
  ON jobs (state, priority DESC, available_at ASC)
  WHERE state = 'queued';

CREATE TABLE IF NOT EXISTS prisma_snapshots (
  project_id uuid PRIMARY KEY REFERENCES projects(id) ON DELETE CASCADE,
  records_identified bigint NOT NULL DEFAULT 0,
  records_deduplicated bigint NOT NULL DEFAULT 0,
  title_abstract_pending bigint NOT NULL DEFAULT 0,
  title_abstract_included bigint NOT NULL DEFAULT 0,
  title_abstract_excluded bigint NOT NULL DEFAULT 0,
  full_text_pending bigint NOT NULL DEFAULT 0,
  full_text_included bigint NOT NULL DEFAULT 0,
  full_text_excluded bigint NOT NULL DEFAULT 0,
  revision bigint NOT NULL DEFAULT 0,
  updated_at timestamptz NOT NULL DEFAULT now()
);

CREATE TABLE IF NOT EXISTS documents (
  id uuid PRIMARY KEY,
  report_id uuid NOT NULL REFERENCES reports(id) ON DELETE CASCADE,
  object_key text NOT NULL,
  content_hash text NOT NULL,
  mime_type text NOT NULL,
  byte_size bigint NOT NULL CHECK (byte_size >= 0),
  parser_version text,
  parse_status text NOT NULL DEFAULT 'pending'
    CHECK (parse_status IN ('pending', 'parsed', 'failed', 'ocr_required')),
  created_at timestamptz NOT NULL DEFAULT now(),
  UNIQUE (content_hash)
);

CREATE TABLE IF NOT EXISTS document_blocks (
  id uuid PRIMARY KEY,
  document_id uuid NOT NULL REFERENCES documents(id) ON DELETE CASCADE,
  parser_version text NOT NULL,
  page_number integer NOT NULL CHECK (page_number > 0),
  kind text NOT NULL,
  section_path text[] NOT NULL DEFAULT '{}',
  ordinal integer NOT NULL,
  text text NOT NULL,
  bbox jsonb,
  search_vector tsvector GENERATED ALWAYS AS (to_tsvector('simple', text)) STORED,
  content_hash text NOT NULL,
  UNIQUE (document_id, parser_version, ordinal)
);
CREATE INDEX IF NOT EXISTS document_blocks_fts_idx ON document_blocks USING gin (search_vector);

CREATE TABLE IF NOT EXISTS ai_runs (
  id uuid PRIMARY KEY,
  project_id uuid REFERENCES projects(id) ON DELETE CASCADE,
  task_kind text NOT NULL,
  provider text NOT NULL,
  model text NOT NULL,
  prompt_version text NOT NULL,
  input_hash text NOT NULL,
  output jsonb,
  status text NOT NULL CHECK (status IN ('running', 'completed', 'failed', 'abstained')),
  created_at timestamptz NOT NULL DEFAULT now(),
  completed_at timestamptz,
  UNIQUE (task_kind, provider, model, prompt_version, input_hash)
);

CREATE TABLE IF NOT EXISTS ai_proposals (
  id uuid PRIMARY KEY,
  project_id uuid NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
  ai_run_id uuid NOT NULL REFERENCES ai_runs(id),
  proposal_type text NOT NULL,
  payload jsonb NOT NULL,
  status text NOT NULL DEFAULT 'pending' CHECK (status IN ('pending', 'accepted', 'rejected', 'expired')),
  decided_by text,
  decided_at timestamptz,
  created_at timestamptz NOT NULL DEFAULT now()
);

-- One-time compatibility import. UUIDs are deterministic so rerunning the migration/import is safe.
INSERT INTO reports (id, title, abstract_text, publication_year, journal, url, raw)
SELECT
  format('%s-%s-%s-%s-%s', substr(md5('deepref:report:' || w.canonical_doi), 1, 8),
    substr(md5('deepref:report:' || w.canonical_doi), 9, 4),
    substr(md5('deepref:report:' || w.canonical_doi), 13, 4),
    substr(md5('deepref:report:' || w.canonical_doi), 17, 4),
    substr(md5('deepref:report:' || w.canonical_doi), 21, 12))::uuid,
  w.title, w.abstract_text, COALESCE(w.published_year, w.issued_year), w.container_title, w.url, w.raw
FROM works w
ON CONFLICT (id) DO UPDATE SET
  title = EXCLUDED.title,
  abstract_text = EXCLUDED.abstract_text,
  publication_year = EXCLUDED.publication_year,
  journal = EXCLUDED.journal,
  url = EXCLUDED.url,
  raw = EXCLUDED.raw,
  updated_at = now();

INSERT INTO report_identifiers (id, report_id, scheme, value, normalized_value)
SELECT
  gen_random_uuid(),
  r.id,
  'doi',
  w.canonical_doi,
  lower(w.canonical_doi)
FROM works w
JOIN reports r ON r.id = format('%s-%s-%s-%s-%s', substr(md5('deepref:report:' || w.canonical_doi), 1, 8),
  substr(md5('deepref:report:' || w.canonical_doi), 9, 4), substr(md5('deepref:report:' || w.canonical_doi), 13, 4),
  substr(md5('deepref:report:' || w.canonical_doi), 17, 4), substr(md5('deepref:report:' || w.canonical_doi), 21, 12))::uuid
ON CONFLICT (scheme, normalized_value) DO UPDATE SET value = EXCLUDED.value;

INSERT INTO records (id, project_id, report_id, source, source_key, title, abstract_text, publication_year, raw)
SELECT
  format('%s-%s-%s-%s-%s', substr(md5('deepref:record:' || pw.project_id::text || ':' || pw.canonical_doi), 1, 8),
    substr(md5('deepref:record:' || pw.project_id::text || ':' || pw.canonical_doi), 9, 4),
    substr(md5('deepref:record:' || pw.project_id::text || ':' || pw.canonical_doi), 13, 4),
    substr(md5('deepref:record:' || pw.project_id::text || ':' || pw.canonical_doi), 17, 4),
    substr(md5('deepref:record:' || pw.project_id::text || ':' || pw.canonical_doi), 21, 12))::uuid,
  pw.project_id,
  r.id,
  'legacy_project_works',
  pw.canonical_doi,
  w.title,
  w.abstract_text,
  COALESCE(w.published_year, w.issued_year),
  jsonb_build_object('legacy_doi', pw.canonical_doi, 'seed', pw.seed, 'min_depth', pw.min_depth)
FROM project_works pw
JOIN works w ON w.canonical_doi = pw.canonical_doi
JOIN report_identifiers ri ON ri.scheme = 'doi' AND ri.normalized_value = lower(pw.canonical_doi)
JOIN reports r ON r.id = ri.report_id
ON CONFLICT (id) DO NOTHING;

INSERT INTO project_reports (project_id, report_id, first_seen_record_id)
SELECT rec.project_id, rec.report_id, rec.id
FROM records rec
WHERE rec.report_id IS NOT NULL
ON CONFLICT (project_id, report_id) DO UPDATE SET first_seen_record_id = COALESCE(project_reports.first_seen_record_id, EXCLUDED.first_seen_record_id);

INSERT INTO protocol_versions (id, project_id, version, name, status, criteria, published_at)
SELECT
  format('%s-%s-%s-%s-%s', substr(md5('deepref:protocol:' || p.id::text), 1, 8),
    substr(md5('deepref:protocol:' || p.id::text), 9, 4), substr(md5('deepref:protocol:' || p.id::text), 13, 4),
    substr(md5('deepref:protocol:' || p.id::text), 17, 4), substr(md5('deepref:protocol:' || p.id::text), 21, 12))::uuid,
  p.id, 1, 'Default evidence screening protocol', 'published',
  '[{"id":"population","label":"Population","description":"Matches the review population."},{"id":"intervention","label":"Intervention or exposure","description":"Matches the intervention or exposure of interest."},{"id":"outcome","label":"Outcome","description":"Reports a relevant outcome."}]'::jsonb,
  now()
FROM projects p
ON CONFLICT (id) DO NOTHING;

INSERT INTO exclusion_reasons (id, project_id, code, label, stage)
SELECT
  gen_random_uuid(), p.id, reasons.code, reasons.label, 'full_text'
FROM projects p
CROSS JOIN (VALUES
  ('wrong_population', 'Wrong population'),
  ('wrong_intervention', 'Wrong intervention or exposure'),
  ('wrong_outcome', 'Wrong outcome'),
  ('wrong_design', 'Wrong study design'),
  ('no_usable_full_text', 'No usable full text')
) AS reasons(code, label)
ON CONFLICT (project_id, code) DO NOTHING;
