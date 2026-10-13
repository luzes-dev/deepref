-- PR3: PostgreSQL owns the graph read model and durable worker queue.
-- Migrations 0001-0007 remain immutable; legacy DOI tables are retained only
-- for import/ingestion compatibility and are not used by public graph reads.

ALTER TABLE reports
  ADD COLUMN IF NOT EXISTS work_type text,
  ADD COLUMN IF NOT EXISTS publisher text,
  ADD COLUMN IF NOT EXISTS container_title text,
  ADD COLUMN IF NOT EXISTS total_citations bigint NOT NULL DEFAULT 0,
  ADD COLUMN IF NOT EXISTS references_count bigint NOT NULL DEFAULT 0;

ALTER TABLE project_reports
  ADD COLUMN IF NOT EXISTS total_citations bigint NOT NULL DEFAULT 0,
  ADD COLUMN IF NOT EXISTS references_count bigint NOT NULL DEFAULT 0,
  ADD COLUMN IF NOT EXISTS internal_citations bigint NOT NULL DEFAULT 0,
  ADD COLUMN IF NOT EXISTS outbound_internal_references bigint NOT NULL DEFAULT 0,
  ADD COLUMN IF NOT EXISTS rank_score double precision NOT NULL DEFAULT 0,
  ADD COLUMN IF NOT EXISTS metrics_computed_at timestamptz;

-- Fill v2 metadata for reports created by the immutable compatibility migration.
UPDATE reports r
SET work_type = w.work_type,
    publisher = w.publisher,
    container_title = w.container_title,
    total_citations = w.total_citations,
    references_count = w.references_count
FROM report_identifiers ri
JOIN works w ON w.canonical_doi = ri.normalized_value
WHERE ri.report_id = r.id AND ri.scheme = 'doi';

CREATE INDEX IF NOT EXISTS project_reports_rank_idx
  ON project_reports (project_id, rank_score DESC, internal_citations DESC, report_id);
CREATE INDEX IF NOT EXISTS citations_project_source_idx
  ON citations (project_id, source_report_id, target_report_id);

-- Durable jobs are claimed by the worker with row locks, not by an external
-- broker. The obsolete compatibility outbox is no longer part of the runtime.
DROP TABLE IF EXISTS event_outbox;

ALTER TABLE jobs
  ADD COLUMN IF NOT EXISTS lease_renewed_at timestamptz;

DO $$
BEGIN
  IF EXISTS (
    SELECT 1
    FROM information_schema.columns
    WHERE table_schema = current_schema()
      AND table_name = 'dead_letter_records'
      AND column_name = 'outbox_event_id'
  ) THEN
    ALTER TABLE dead_letter_records RENAME COLUMN outbox_event_id TO job_event_id;
  END IF;
END;
$$;

CREATE INDEX IF NOT EXISTS jobs_expired_running_idx
  ON jobs (leased_until)
  WHERE state = 'running';
CREATE INDEX IF NOT EXISTS jobs_dedupe_kind_idx ON jobs (kind, dedupe_key);

-- The table was introduced before the infrastructure collapse. Keep the
-- durable status endpoint, but make its meaning explicit: it describes the
-- PostgreSQL graph metrics projection.
UPDATE projection_state
SET projection_name = 'postgres_graph'
WHERE projection_name = 'graph';

CREATE OR REPLACE FUNCTION recompute_project_report_metrics(target_project_id uuid)
RETURNS void
LANGUAGE plpgsql
AS $$
DECLARE
  max_total double precision;
  max_internal double precision;
  max_outbound double precision;
  current_revision bigint;
  computed_at timestamptz := now();
  report_count bigint;
  edge_count bigint;
BEGIN
  UPDATE project_reports pr
  SET total_citations = COALESCE(r.total_citations, 0),
      references_count = COALESCE(r.references_count, 0),
      internal_citations = COALESCE(inbound.internal_count, 0),
      outbound_internal_references = COALESCE(outbound.outbound_count, 0),
      metrics_computed_at = computed_at
  FROM reports r
  LEFT JOIN (
    SELECT c.target_report_id AS report_id,
           count(DISTINCT c.source_report_id)::bigint AS internal_count
    FROM citations c
    JOIN project_reports source_membership
      ON source_membership.project_id = c.project_id
     AND source_membership.report_id = c.source_report_id
    WHERE c.project_id = target_project_id
    GROUP BY c.target_report_id
  ) inbound ON inbound.report_id = r.id
  LEFT JOIN (
    SELECT c.source_report_id AS report_id,
           count(DISTINCT c.target_report_id)::bigint AS outbound_count
    FROM citations c
    JOIN project_reports target_membership
      ON target_membership.project_id = c.project_id
     AND target_membership.report_id = c.target_report_id
    WHERE c.project_id = target_project_id
    GROUP BY c.source_report_id
  ) outbound ON outbound.report_id = r.id
  WHERE pr.project_id = target_project_id AND pr.report_id = r.id;

  SELECT
    GREATEST(COALESCE(MAX(LOG(GREATEST(pr.total_citations, 0) + 1)), 1), 1),
    GREATEST(COALESCE(MAX(pr.internal_citations), 0), 1),
    GREATEST(COALESCE(MAX(pr.outbound_internal_references), 0), 1)
  INTO max_total, max_internal, max_outbound
  FROM project_reports pr
  WHERE pr.project_id = target_project_id;

  UPDATE project_reports pr
  SET rank_score =
        0.45 * (LOG(GREATEST(pr.total_citations, 0) + 1) / max_total) +
        0.40 * (pr.internal_citations::double precision / max_internal) +
        0.10 * (pr.outbound_internal_references::double precision / max_outbound) +
        0.05 * CASE
          WHEN r.publication_year IS NULL THEN 0
          ELSE 1.0 / (1.0 + (GREATEST(EXTRACT(YEAR FROM CURRENT_DATE)::int - r.publication_year, 0)::double precision / 10.0))
        END,
      metrics_computed_at = computed_at
  FROM reports r
  WHERE pr.project_id = target_project_id AND r.id = pr.report_id;

  SELECT COALESCE(MAX(revision), 0)
  INTO current_revision
  FROM domain_events
  WHERE entity_type = 'metric' AND entity_key = target_project_id::text;

  SELECT count(*)::bigint
  INTO report_count
  FROM project_reports
  WHERE project_id = target_project_id;

  SELECT count(*)::bigint
  INTO edge_count
  FROM citations c
  JOIN project_reports source_membership
    ON source_membership.project_id = c.project_id
   AND source_membership.report_id = c.source_report_id
  JOIN project_reports target_membership
    ON target_membership.project_id = c.project_id
   AND target_membership.report_id = c.target_report_id
  WHERE c.project_id = target_project_id;

  INSERT INTO metric_snapshots
    (project_id, revision, metrics_as_of, work_count, edge_count, payload)
  VALUES
    (target_project_id, current_revision, computed_at, report_count, edge_count,
     jsonb_build_object('work_count', report_count, 'edge_count', edge_count))
  ON CONFLICT (project_id, revision) DO UPDATE SET
    metrics_as_of = EXCLUDED.metrics_as_of,
    work_count = EXCLUDED.work_count,
    edge_count = EXCLUDED.edge_count,
    payload = EXCLUDED.payload;

  UPDATE projection_state
  SET state = 'ready',
      revision = current_revision,
      watermark = current_revision,
      lag = 0,
      last_success_at = computed_at,
      last_error = NULL,
      rebuild_state = NULL,
      updated_at = computed_at
  WHERE projection_name = 'postgres_graph'
    AND project_id IS NOT DISTINCT FROM target_project_id;

  IF NOT FOUND THEN
    INSERT INTO projection_state
      (projection_name, project_id, state, revision, watermark, lag, last_success_at, updated_at)
    VALUES
      ('postgres_graph', target_project_id, 'ready', current_revision, current_revision, 0, computed_at, computed_at);
  END IF;
END;
$$;

-- Populate current metrics and freshness for databases upgraded in place.
SELECT recompute_project_report_metrics(id) FROM projects;
