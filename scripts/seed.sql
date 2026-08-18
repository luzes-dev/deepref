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

INSERT INTO citations (
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

COMMIT;
