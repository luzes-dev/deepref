BEGIN;

INSERT INTO settings (
  id,
  crossref_mailto,
  default_max_depth,
  max_concurrency,
  rate_limit_per_second,
  retry_attempts,
  metadata_provider,
  citation_provider,
  created_at,
  updated_at
)
VALUES (
  1,
  'local@example.invalid',
  2,
  8,
  1,
  5,
  'crossref',
  'crossref',
  '2026-01-01T00:00:00Z',
  '2026-01-01T00:00:00Z'
)
ON CONFLICT (id) DO NOTHING;

INSERT INTO projects (id, name, description, default_max_depth, created_at, updated_at)
VALUES (
  '00000000-0000-4000-8000-000000000001',
  'Local citation map',
  'Deterministic fixture data loaded by just seed.',
  2,
  '2026-01-01T00:00:00Z',
  '2026-01-01T00:00:00Z'
)
ON CONFLICT (id) DO UPDATE SET
  name = EXCLUDED.name,
  description = EXCLUDED.description,
  default_max_depth = EXCLUDED.default_max_depth,
  updated_at = EXCLUDED.updated_at;

INSERT INTO works (
  canonical_doi,
  title,
  work_type,
  publisher,
  container_title,
  issued_year,
  published_year,
  url,
  total_citations,
  references_count,
  fetch_status,
  fetched_at,
  raw
)
VALUES
  (
    '10.5555/deepref.seed.1',
    'Durable citation graphs',
    'journal-article',
    'DeepRef Fixtures',
    'Journal of Local Development',
    2024,
    2024,
    'https://example.invalid/works/1',
    42,
    2,
    'fetched',
    '2026-01-01T00:00:00Z',
    '{"fixture":true,"ordinal":1}'::jsonb
  ),
  (
    '10.5555/deepref.seed.2',
    'Idempotent event processing',
    'journal-article',
    'DeepRef Fixtures',
    'Journal of Local Development',
    2023,
    2023,
    'https://example.invalid/works/2',
    17,
    1,
    'fetched',
    '2026-01-01T00:00:00Z',
    '{"fixture":true,"ordinal":2}'::jsonb
  ),
  (
    '10.5555/deepref.seed.3',
    'Rebuildable graph projections',
    'proceedings-article',
    'DeepRef Fixtures',
    'Proceedings of Deterministic Systems',
    2025,
    2025,
    'https://example.invalid/works/3',
    8,
    0,
    'fetched',
    '2026-01-01T00:00:00Z',
    '{"fixture":true,"ordinal":3}'::jsonb
  )
ON CONFLICT (canonical_doi) DO UPDATE SET
  title = EXCLUDED.title,
  work_type = EXCLUDED.work_type,
  publisher = EXCLUDED.publisher,
  container_title = EXCLUDED.container_title,
  issued_year = EXCLUDED.issued_year,
  published_year = EXCLUDED.published_year,
  url = EXCLUDED.url,
  total_citations = EXCLUDED.total_citations,
  references_count = EXCLUDED.references_count,
  fetch_status = EXCLUDED.fetch_status,
  fetched_at = EXCLUDED.fetched_at,
  raw = EXCLUDED.raw;

INSERT INTO project_works (
  project_id,
  canonical_doi,
  seed,
  min_depth,
  metrics_computed_at
)
VALUES
  ('00000000-0000-4000-8000-000000000001', '10.5555/deepref.seed.1', true, 0, '2026-01-01T00:00:00Z'),
  ('00000000-0000-4000-8000-000000000001', '10.5555/deepref.seed.2', false, 1, '2026-01-01T00:00:00Z'),
  ('00000000-0000-4000-8000-000000000001', '10.5555/deepref.seed.3', false, 1, '2026-01-01T00:00:00Z')
ON CONFLICT (project_id, canonical_doi) DO UPDATE SET
  seed = EXCLUDED.seed,
  min_depth = EXCLUDED.min_depth;

INSERT INTO legacy_citations (
  project_id,
  source_doi,
  target_doi,
  source,
  created_at
)
VALUES
  (
    '00000000-0000-4000-8000-000000000001',
    '10.5555/deepref.seed.1',
    '10.5555/deepref.seed.2',
    'local-fixture',
    '2026-01-01T00:00:00Z'
  ),
  (
    '00000000-0000-4000-8000-000000000001',
    '10.5555/deepref.seed.1',
    '10.5555/deepref.seed.3',
    'local-fixture',
    '2026-01-01T00:00:00Z'
  ),
  (
    '00000000-0000-4000-8000-000000000001',
    '10.5555/deepref.seed.2',
    '10.5555/deepref.seed.3',
    'local-fixture',
    '2026-01-01T00:00:00Z'
  )
ON CONFLICT (project_id, source_doi, target_doi) DO UPDATE SET
  source = EXCLUDED.source,
  created_at = EXCLUDED.created_at;

UPDATE project_works
SET
  internal_citations = CASE canonical_doi
    WHEN '10.5555/deepref.seed.1' THEN 0
    WHEN '10.5555/deepref.seed.2' THEN 1
    WHEN '10.5555/deepref.seed.3' THEN 2
  END,
  outbound_internal_references = CASE canonical_doi
    WHEN '10.5555/deepref.seed.1' THEN 2
    WHEN '10.5555/deepref.seed.2' THEN 1
    WHEN '10.5555/deepref.seed.3' THEN 0
  END,
  rank_score = CASE canonical_doi
    WHEN '10.5555/deepref.seed.1' THEN 0.70
    WHEN '10.5555/deepref.seed.2' THEN 0.50
    WHEN '10.5555/deepref.seed.3' THEN 0.40
  END,
  metrics_computed_at = '2026-01-01T00:00:00Z'
WHERE project_id = '00000000-0000-4000-8000-000000000001'
  AND canonical_doi IN (
    '10.5555/deepref.seed.1',
    '10.5555/deepref.seed.2',
    '10.5555/deepref.seed.3'
  );

-- Seed the v2 evidence workspace after the legacy fixture rows exist. The production migration
-- performs the same deterministic compatibility import for data that already exists.
INSERT INTO reports (
  id, title, abstract_text, publication_year, journal, container_title, url,
  work_type, publisher, total_citations, references_count, raw
)
SELECT
  format('%s-%s-%s-%s-%s', substr(md5('deepref:report:' || w.canonical_doi), 1, 8),
    substr(md5('deepref:report:' || w.canonical_doi), 9, 4), substr(md5('deepref:report:' || w.canonical_doi), 13, 4),
    substr(md5('deepref:report:' || w.canonical_doi), 17, 4), substr(md5('deepref:report:' || w.canonical_doi), 21, 12))::uuid,
  w.title, w.abstract_text, COALESCE(w.published_year, w.issued_year), w.container_title,
  w.container_title, w.url, w.work_type, w.publisher, w.total_citations, w.references_count, w.raw
FROM works w
ON CONFLICT (id) DO UPDATE SET
  title = EXCLUDED.title,
  abstract_text = EXCLUDED.abstract_text,
  publication_year = EXCLUDED.publication_year,
  journal = EXCLUDED.journal,
  container_title = EXCLUDED.container_title,
  url = EXCLUDED.url,
  work_type = EXCLUDED.work_type,
  publisher = EXCLUDED.publisher,
  total_citations = EXCLUDED.total_citations,
  references_count = EXCLUDED.references_count,
  raw = EXCLUDED.raw,
  updated_at = now();

INSERT INTO report_identifiers (id, report_id, scheme, value, normalized_value)
SELECT gen_random_uuid(), r.id, 'doi', w.canonical_doi, lower(w.canonical_doi)
FROM works w
JOIN reports r ON r.id = format('%s-%s-%s-%s-%s', substr(md5('deepref:report:' || w.canonical_doi), 1, 8),
  substr(md5('deepref:report:' || w.canonical_doi), 9, 4), substr(md5('deepref:report:' || w.canonical_doi), 13, 4),
  substr(md5('deepref:report:' || w.canonical_doi), 17, 4), substr(md5('deepref:report:' || w.canonical_doi), 21, 12))::uuid
ON CONFLICT (scheme, normalized_value) DO NOTHING;

INSERT INTO records (id, project_id, report_id, source, source_key, title, publication_year, raw)
SELECT
  format('%s-%s-%s-%s-%s', substr(md5('deepref:record:' || pw.project_id::text || ':' || pw.canonical_doi), 1, 8),
    substr(md5('deepref:record:' || pw.project_id::text || ':' || pw.canonical_doi), 9, 4), substr(md5('deepref:record:' || pw.project_id::text || ':' || pw.canonical_doi), 13, 4),
    substr(md5('deepref:record:' || pw.project_id::text || ':' || pw.canonical_doi), 17, 4), substr(md5('deepref:record:' || pw.project_id::text || ':' || pw.canonical_doi), 21, 12))::uuid,
  pw.project_id, r.id, 'legacy_project_works', pw.canonical_doi, w.title,
  COALESCE(w.published_year, w.issued_year), jsonb_build_object('legacy_doi', pw.canonical_doi, 'seed', pw.seed)
FROM project_works pw
JOIN works w ON w.canonical_doi = pw.canonical_doi
JOIN report_identifiers ri ON ri.scheme = 'doi' AND ri.normalized_value = lower(pw.canonical_doi)
JOIN reports r ON r.id = ri.report_id
ON CONFLICT (id) DO NOTHING;

INSERT INTO project_reports (project_id, report_id, first_seen_record_id)
SELECT project_id, report_id, id FROM records WHERE project_id = '00000000-0000-4000-8000-000000000001' AND report_id IS NOT NULL
ON CONFLICT (project_id, report_id) DO UPDATE SET first_seen_record_id = COALESCE(project_reports.first_seen_record_id, EXCLUDED.first_seen_record_id);

INSERT INTO citations (
  project_id, source_report_id, target_report_id, source,
  legacy_source_doi, legacy_target_doi, created_at
)
SELECT
  '00000000-0000-4000-8000-000000000001', source_report.report_id, target_report.report_id,
  'local-fixture', edge.source_doi, edge.target_doi, '2026-01-01T00:00:00Z'
FROM (
  VALUES
    ('10.5555/deepref.seed.1', '10.5555/deepref.seed.2'),
    ('10.5555/deepref.seed.1', '10.5555/deepref.seed.3'),
    ('10.5555/deepref.seed.2', '10.5555/deepref.seed.3')
) AS edge(source_doi, target_doi)
JOIN report_identifiers source_report_identifier
  ON source_report_identifier.scheme = 'doi'
 AND source_report_identifier.normalized_value = edge.source_doi
JOIN report_identifiers target_report_identifier
  ON target_report_identifier.scheme = 'doi'
 AND target_report_identifier.normalized_value = edge.target_doi
JOIN reports source_report ON source_report.id = source_report_identifier.report_id
JOIN reports target_report ON target_report.id = target_report_identifier.report_id
ON CONFLICT (project_id, source_report_id, target_report_id) DO UPDATE SET
  source = EXCLUDED.source,
  legacy_source_doi = EXCLUDED.legacy_source_doi,
  legacy_target_doi = EXCLUDED.legacy_target_doi,
  created_at = EXCLUDED.created_at;

SELECT recompute_project_report_metrics('00000000-0000-4000-8000-000000000001');

INSERT INTO protocol_versions (id, project_id, version, name, status, criteria, published_at)
VALUES (
  '00000000-0000-4000-8000-000000000101',
  '00000000-0000-4000-8000-000000000001',
  1,
  'Default evidence screening protocol',
  'published',
  '[{"id":"population","label":"Population","description":"Matches the review population."},{"id":"intervention","label":"Intervention or exposure","description":"Matches the intervention or exposure of interest."},{"id":"outcome","label":"Outcome","description":"Reports a relevant outcome."}]'::jsonb,
  '2026-01-01T00:00:00Z'
)
ON CONFLICT (id) DO NOTHING;

INSERT INTO exclusion_reasons (id, project_id, code, label, stage)
VALUES
  ('00000000-0000-4000-8000-000000000111', '00000000-0000-4000-8000-000000000001', 'wrong_population', 'Wrong population', 'full_text'),
  ('00000000-0000-4000-8000-000000000112', '00000000-0000-4000-8000-000000000001', 'wrong_intervention', 'Wrong intervention or exposure', 'full_text'),
  ('00000000-0000-4000-8000-000000000113', '00000000-0000-4000-8000-000000000001', 'wrong_outcome', 'Wrong outcome', 'full_text'),
  ('00000000-0000-4000-8000-000000000114', '00000000-0000-4000-8000-000000000001', 'wrong_design', 'Wrong study design', 'full_text'),
  ('00000000-0000-4000-8000-000000000115', '00000000-0000-4000-8000-000000000001', 'no_usable_full_text', 'No usable full text', 'full_text')
ON CONFLICT (id) DO NOTHING;

COMMIT;
