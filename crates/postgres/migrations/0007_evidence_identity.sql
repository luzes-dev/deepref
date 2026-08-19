-- DeepRef v2 evidence identity compatibility boundary.
-- The old DOI-keyed table remains available to the legacy worker/projector until PR 3.
ALTER TABLE citations RENAME TO legacy_citations;
ALTER INDEX IF EXISTS citations_project_target_idx RENAME TO legacy_citations_project_target_idx;

CREATE TABLE citations (
  project_id uuid NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
  source_report_id uuid NOT NULL REFERENCES reports(id) ON DELETE CASCADE,
  target_report_id uuid NOT NULL REFERENCES reports(id) ON DELETE CASCADE,
  source text NOT NULL DEFAULT 'legacy-citation',
  first_seen_ingestion_id uuid REFERENCES ingestions(id) ON DELETE SET NULL,
  legacy_source_doi text NOT NULL,
  legacy_target_doi text NOT NULL,
  created_at timestamptz NOT NULL DEFAULT now(),
  PRIMARY KEY (project_id, source_report_id, target_report_id)
);

CREATE INDEX citations_project_target_idx ON citations (project_id, target_report_id);

-- An acquisition run is the v2 provenance handle for one legacy ingestion.
-- The importer uses the legacy ingestion UUID as the stable v2 UUID.
CREATE TABLE acquisition_runs (
  id uuid PRIMARY KEY,
  project_id uuid NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
  legacy_ingestion_id uuid NOT NULL UNIQUE REFERENCES ingestions(id) ON DELETE CASCADE,
  source text NOT NULL DEFAULT 'legacy-ingestion',
  status text NOT NULL,
  max_depth integer NOT NULL,
  seed_count integer NOT NULL,
  queued_count integer NOT NULL,
  fetched_count integer NOT NULL,
  failed_count integer NOT NULL,
  metadata_provider text NOT NULL,
  citation_provider text NOT NULL,
  created_at timestamptz NOT NULL,
  started_at timestamptz,
  completed_at timestamptz
);

ALTER TABLE records
  ADD COLUMN acquisition_run_id uuid REFERENCES acquisition_runs(id) ON DELETE SET NULL;

-- Keep every old ingestion item, including repeated observations of the same report.
CREATE TABLE record_provenance (
  record_id uuid NOT NULL REFERENCES records(id) ON DELETE CASCADE,
  acquisition_run_id uuid NOT NULL REFERENCES acquisition_runs(id) ON DELETE CASCADE,
  canonical_doi text NOT NULL,
  depth integer NOT NULL,
  parent_doi text,
  status text NOT NULL,
  attempts integer NOT NULL,
  queued_at timestamptz NOT NULL,
  fetched_at timestamptz,
  last_error text,
  work_event_id uuid,
  PRIMARY KEY (acquisition_run_id, canonical_doi)
);

-- Recreate the legacy metric function against the compatibility table. The function
-- intentionally continues to calculate DOI-era project metrics until PR 3.
CREATE OR REPLACE FUNCTION recompute_project_metrics(target_project_id uuid)
RETURNS void
LANGUAGE plpgsql
AS $$
DECLARE
  max_total double precision;
  max_internal double precision;
  max_outbound double precision;
BEGIN
  UPDATE project_works pw
  SET internal_citations = counts.internal_count
  FROM (
    SELECT target_doi AS canonical_doi, count(DISTINCT source_doi)::int AS internal_count
    FROM legacy_citations
    WHERE project_id = target_project_id
    GROUP BY target_doi
  ) counts
  WHERE pw.project_id = target_project_id AND pw.canonical_doi = counts.canonical_doi;

  UPDATE project_works
  SET internal_citations = 0
  WHERE project_id = target_project_id
    AND canonical_doi NOT IN (
      SELECT target_doi FROM legacy_citations WHERE project_id = target_project_id
    );

  UPDATE project_works pw
  SET outbound_internal_references = counts.outbound_count
  FROM (
    SELECT source_doi AS canonical_doi, count(DISTINCT target_doi)::int AS outbound_count
    FROM legacy_citations
    WHERE project_id = target_project_id
    GROUP BY source_doi
  ) counts
  WHERE pw.project_id = target_project_id AND pw.canonical_doi = counts.canonical_doi;

  UPDATE project_works
  SET outbound_internal_references = 0
  WHERE project_id = target_project_id
    AND canonical_doi NOT IN (
      SELECT source_doi FROM legacy_citations WHERE project_id = target_project_id
    );

  SELECT
    GREATEST(MAX(LOG(GREATEST(w.total_citations, 0) + 1)), 1),
    GREATEST(MAX(pw.internal_citations), 1),
    GREATEST(MAX(pw.outbound_internal_references), 1)
  INTO max_total, max_internal, max_outbound
  FROM project_works pw
  JOIN works w ON w.canonical_doi = pw.canonical_doi
  WHERE pw.project_id = target_project_id;

  UPDATE project_works pw
  SET
    rank_score =
      0.45 * (LOG(GREATEST(w.total_citations, 0) + 1) / max_total) +
      0.40 * (pw.internal_citations::double precision / max_internal) +
      0.10 * (pw.outbound_internal_references::double precision / max_outbound) +
      0.05 * (
        CASE
          WHEN w.issued_year IS NULL THEN 0
          ELSE 1.0 / (1.0 + (GREATEST(EXTRACT(YEAR FROM now())::int - w.issued_year, 0)::double precision / 10.0))
        END
      ),
    metrics_computed_at = now()
  FROM works w
  WHERE pw.project_id = target_project_id AND w.canonical_doi = pw.canonical_doi;
END;
$$;
