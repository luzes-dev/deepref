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
    FROM citations
    WHERE project_id = target_project_id
    GROUP BY target_doi
  ) counts
  WHERE pw.project_id = target_project_id AND pw.canonical_doi = counts.canonical_doi;

  UPDATE project_works
  SET internal_citations = 0
  WHERE project_id = target_project_id
    AND canonical_doi NOT IN (
      SELECT target_doi FROM citations WHERE project_id = target_project_id
    );

  UPDATE project_works pw
  SET outbound_internal_references = counts.outbound_count
  FROM (
    SELECT source_doi AS canonical_doi, count(DISTINCT target_doi)::int AS outbound_count
    FROM citations
    WHERE project_id = target_project_id
    GROUP BY source_doi
  ) counts
  WHERE pw.project_id = target_project_id AND pw.canonical_doi = counts.canonical_doi;

  UPDATE project_works
  SET outbound_internal_references = 0
  WHERE project_id = target_project_id
    AND canonical_doi NOT IN (
      SELECT source_doi FROM citations WHERE project_id = target_project_id
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
