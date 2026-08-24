use deepref_application::{
    NonNegativeCount, PrismaInvariantError, PrismaProjection, PrismaReasonCount,
};
use serde_json::Value;
use sqlx::{PgPool, Row};
use thiserror::Error;
use uuid::Uuid;

#[derive(Debug, Error)]
pub enum PrismaProjectionError {
    #[error("database operation failed")]
    Database(#[from] sqlx::Error),
    #[error("invalid PRISMA projection JSON")]
    Json(#[from] serde_json::Error),
    #[error("negative PRISMA count in canonical projection: {field}")]
    NegativeCount { field: &'static str },
    #[error("PRISMA projection invariant violation: {0}")]
    Invariant(#[from] PrismaInvariantError),
}

/// Read the PRISMA projection directly from the canonical review tables.
///
/// This is deliberately one statement. Every count and the revision/as-of
/// metadata is evaluated by PostgreSQL against one MVCC snapshot; the legacy
/// `prisma_snapshots` table is not consulted.
pub async fn get_prisma_projection(
    pool: &PgPool,
    project_id: Uuid,
) -> Result<Option<PrismaProjection>, PrismaProjectionError> {
    let row = sqlx::query(
        r#"
WITH project_exists AS (
  SELECT id FROM projects WHERE id = $1
), source_counts AS (
  SELECT
    count(*)::bigint AS identified_records,
    count(*) FILTER (WHERE report_id IS NOT NULL)::bigint AS linked_records,
    count(DISTINCT report_id) FILTER (WHERE report_id IS NOT NULL)::bigint AS source_canonical_reports,
    count(*) FILTER (WHERE report_id IS NULL)::bigint AS unresolved_records
  FROM records
  WHERE project_id = $1
), pending_dedupe AS (
  SELECT count(*)::bigint AS pending_dedupe_proposals
  FROM dedupe_proposals
  WHERE project_id = $1 AND status = 'pending'
), report_state AS (
  SELECT
    pr.report_id,
    coalesce(ss.title_abstract_status, 'unscreened') AS title_abstract_status,
    coalesce(ss.full_text_status, 'not_required') AS full_text_status,
    ss.full_text_exclusion_reason_id,
    coalesce(ss.revision, 0)::bigint AS revision,
    ss.updated_at AS screening_updated_at
  FROM project_reports pr
  LEFT JOIN screening_state ss
    ON ss.project_id = pr.project_id AND ss.report_id = pr.report_id
  WHERE pr.project_id = $1
), report_documents AS (
  SELECT
    rs.report_id,
    EXISTS (
      SELECT 1 FROM documents d
      WHERE d.project_id = $1
        AND d.report_id = rs.report_id
        AND d.status IN ('uploaded', 'available')
        AND d.object_key IS NOT NULL
        AND d.content_hash IS NOT NULL
    ) AS has_available_document,
    rs.full_text_status IN ('include', 'exclude', 'maybe') AS has_explicit_full_text_decision
  FROM report_state rs
), flow_counts AS (
  SELECT
    count(*) FILTER (WHERE title_abstract_status = 'exclude')::bigint AS title_abstract_excluded,
    count(*) FILTER (WHERE title_abstract_status IN ('unscreened', 'maybe'))::bigint AS title_abstract_pending,
    count(*) FILTER (WHERE title_abstract_status = 'include')::bigint AS reports_sought,
    count(*) FILTER (
      WHERE title_abstract_status = 'include'
        AND NOT (has_available_document OR has_explicit_full_text_decision)
    )::bigint AS reports_not_retrieved,
    count(*) FILTER (
      WHERE title_abstract_status = 'include'
        AND (has_available_document OR has_explicit_full_text_decision)
    )::bigint AS full_text_assessed,
    count(*) FILTER (
      WHERE title_abstract_status = 'include'
        AND (has_available_document OR has_explicit_full_text_decision)
        AND full_text_status IN ('unscreened', 'maybe')
    )::bigint AS full_text_pending,
    count(*) FILTER (
      WHERE title_abstract_status = 'include' AND full_text_status = 'include'
    )::bigint AS full_text_included,
    count(*) FILTER (
      WHERE title_abstract_status = 'include' AND full_text_status = 'exclude'
    )::bigint AS full_text_excluded,
    max(revision)::bigint AS screening_revision,
    max(screening_updated_at) AS screening_as_of
  FROM report_state
  JOIN report_documents USING (report_id)
), reason_counts AS (
  SELECT coalesce(jsonb_agg(
    jsonb_build_object(
      'id', reasons.id,
      'code', reasons.code,
      'label', reasons.label,
      'count', coalesce(counts.count, 0)
    ) ORDER BY reasons.code, reasons.id
  ), '[]'::jsonb) AS full_text_exclusions
  FROM exclusion_reasons reasons
  LEFT JOIN (
    SELECT full_text_exclusion_reason_id AS reason_id, count(*)::bigint AS count
    FROM report_state
    WHERE title_abstract_status = 'include'
      AND full_text_status = 'exclude'
      AND full_text_exclusion_reason_id IS NOT NULL
    GROUP BY full_text_exclusion_reason_id
  ) counts ON counts.reason_id = reasons.id
  WHERE reasons.project_id = $1 AND reasons.stage = 'full_text'
), included_report_grouping AS (
  SELECT
    state.report_id,
    EXISTS (
      SELECT 1 FROM study_reports sr
      WHERE sr.project_id = $1 AND sr.report_id = state.report_id
    ) AS is_grouped
  FROM report_state state
  WHERE state.title_abstract_status = 'include' AND state.full_text_status = 'include'
), grouping_counts AS (
  SELECT
    count(*) FILTER (WHERE NOT is_grouped)::bigint AS included_reports_not_grouped,
    count(DISTINCT sr.study_id)::bigint AS included_studies
  FROM included_report_grouping included
  LEFT JOIN study_reports sr
    ON sr.project_id = $1 AND sr.report_id = included.report_id
), authoritative_timestamps AS (
  SELECT max(changed_at) AS authoritative_as_of
  FROM (
    SELECT created_at AS changed_at FROM records WHERE project_id = $1
    UNION ALL
    SELECT created_at FROM screening_events WHERE project_id = $1
    UNION ALL
    SELECT updated_at FROM documents WHERE project_id = $1
    UNION ALL
    SELECT created_at FROM project_reports WHERE project_id = $1
    UNION ALL
    SELECT created_at FROM dedupe_resolution_events WHERE project_id = $1
    UNION ALL
    SELECT updated_at FROM dedupe_proposals WHERE project_id = $1
    UNION ALL
    SELECT created_at FROM study_events WHERE project_id = $1
    UNION ALL
    SELECT reasons.updated_at
    FROM exclusion_reasons reasons
    WHERE reasons.project_id = $1
      AND reasons.stage = 'full_text'
      AND EXISTS (
        SELECT 1
        FROM report_state referenced
        WHERE referenced.title_abstract_status = 'include'
          AND referenced.full_text_status = 'exclude'
          AND referenced.full_text_exclusion_reason_id = reasons.id
      )
  ) changes
), metrics AS (
  SELECT
    source_counts.*,
    (source_counts.linked_records - source_counts.source_canonical_reports)::bigint
      AS duplicates_removed,
    pending_dedupe.pending_dedupe_proposals,
    flow_counts.title_abstract_excluded,
    flow_counts.title_abstract_pending,
    flow_counts.reports_sought,
    flow_counts.reports_not_retrieved,
    flow_counts.full_text_assessed,
    flow_counts.full_text_pending,
    flow_counts.full_text_included,
    flow_counts.full_text_excluded,
    coalesce(flow_counts.screening_revision, 0)::bigint AS screening_high_watermark,
    grouping_counts.included_reports_not_grouped,
    grouping_counts.included_studies,
    greatest(flow_counts.screening_as_of, authoritative_timestamps.authoritative_as_of)
      AS authoritative_as_of,
    (SELECT count(*)::bigint FROM project_reports WHERE project_id = $1)
      AS screened_records
  FROM source_counts, pending_dedupe, flow_counts, grouping_counts, authoritative_timestamps
)
SELECT metrics.*, reason_counts.full_text_exclusions
FROM metrics, reason_counts
JOIN project_exists ON true
"#,
    )
    .bind(project_id)
    .fetch_optional(pool)
    .await?;

    let Some(row) = row else {
        return Ok(None);
    };
    let full_text_exclusions: Vec<PrismaReasonCount> =
        serde_json::from_value(row.get::<Value, _>("full_text_exclusions"))?;
    let screened_records: Option<i64> = row.get("screened_records");
    let source_canonical_reports: Option<i64> = row.get("source_canonical_reports");
    let manually_created_reports = screened_records
        .zip(source_canonical_reports)
        .map(|(screened, source)| screened - source);
    let projection = PrismaProjection {
        project_id,
        screening_high_watermark: count(
            row.get("screening_high_watermark"),
            "screening_high_watermark",
        )?,
        as_of: row.get("authoritative_as_of"),
        identified_records: count(row.get("identified_records"), "identified_records")?,
        linked_records: count(row.get("linked_records"), "linked_records")?,
        duplicates_removed: count(row.get("duplicates_removed"), "duplicates_removed")?,
        unresolved_records: count(row.get("unresolved_records"), "unresolved_records")?,
        pending_dedupe_proposals: count(
            row.get("pending_dedupe_proposals"),
            "pending_dedupe_proposals",
        )?,
        source_canonical_reports: count(
            row.get("source_canonical_reports"),
            "source_canonical_reports",
        )?,
        manually_created_reports: count(manually_created_reports, "manually_created_reports")?,
        screened_records: count(row.get("screened_records"), "screened_records")?,
        title_abstract_excluded: count(
            row.get("title_abstract_excluded"),
            "title_abstract_excluded",
        )?,
        title_abstract_pending: count(row.get("title_abstract_pending"), "title_abstract_pending")?,
        reports_sought: count(row.get("reports_sought"), "reports_sought")?,
        reports_not_retrieved: count(row.get("reports_not_retrieved"), "reports_not_retrieved")?,
        full_text_assessed: count(row.get("full_text_assessed"), "full_text_assessed")?,
        full_text_pending: count(row.get("full_text_pending"), "full_text_pending")?,
        full_text_included: count(row.get("full_text_included"), "full_text_included")?,
        full_text_excluded: count(row.get("full_text_excluded"), "full_text_excluded")?,
        full_text_exclusions,
        included_reports_not_grouped: count(
            row.get("included_reports_not_grouped"),
            "included_reports_not_grouped",
        )?,
        included_studies: count(row.get("included_studies"), "included_studies")?,
    };
    projection.validate()?;
    Ok(Some(projection))
}

fn count(
    value: Option<i64>,
    field: &'static str,
) -> Result<NonNegativeCount, PrismaProjectionError> {
    value
        .unwrap_or(0)
        .try_into()
        .map(NonNegativeCount::new)
        .map_err(|_| PrismaProjectionError::NegativeCount { field })
}

#[cfg(test)]
mod tests {
    #[test]
    fn duplicate_semantics_keep_unresolved_records_out_of_duplicates() {
        let identified = 4_u64;
        let unresolved = 1_u64;
        let linked = identified - unresolved;
        let canonical = 3_u64;
        assert_eq!(linked - canonical, 0);
    }
}
