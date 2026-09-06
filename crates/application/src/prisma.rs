use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use uuid::Uuid;

/// A non-negative count returned by the canonical PRISMA projection.
///
/// Counts are constructed at the database seam after checking that PostgreSQL
/// did not return a negative value. Keeping the wrapper here prevents a
/// negative scientific count from being represented inside the application.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct NonNegativeCount(u64);

impl NonNegativeCount {
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    pub const fn get(self) -> u64 {
        self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PrismaReasonCount {
    pub id: Uuid,
    pub code: String,
    pub label: String,
    pub count: NonNegativeCount,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PrismaProjection {
    pub project_id: Uuid,
    /// A diagnostic maximum of per-report screening revisions. It is not a
    /// project-wide concurrency token or a scientific snapshot revision.
    pub screening_high_watermark: NonNegativeCount,
    pub as_of: Option<DateTime<Utc>>,
    pub identified_records: NonNegativeCount,
    pub linked_records: NonNegativeCount,
    pub duplicates_removed: NonNegativeCount,
    pub unresolved_records: NonNegativeCount,
    pub pending_dedupe_proposals: NonNegativeCount,
    pub source_canonical_reports: NonNegativeCount,
    pub manually_created_reports: NonNegativeCount,
    pub screened_records: NonNegativeCount,
    pub title_abstract_excluded: NonNegativeCount,
    pub title_abstract_pending: NonNegativeCount,
    pub reports_sought: NonNegativeCount,
    pub reports_not_retrieved: NonNegativeCount,
    pub full_text_assessed: NonNegativeCount,
    pub full_text_pending: NonNegativeCount,
    pub full_text_included: NonNegativeCount,
    pub full_text_excluded: NonNegativeCount,
    pub full_text_exclusions: Vec<PrismaReasonCount>,
    pub included_reports_not_grouped: NonNegativeCount,
    pub included_studies: NonNegativeCount,
}

impl PrismaProjection {
    pub fn grouped_reports(&self) -> Result<NonNegativeCount, PrismaInvariantError> {
        self.full_text_included
            .get()
            .checked_sub(self.included_reports_not_grouped.get())
            .map(NonNegativeCount::new)
            .ok_or(PrismaInvariantError::GroupedReportsUnderflow)
    }

    pub fn validate(&self) -> Result<(), PrismaInvariantError> {
        let identified = self.identified_records.get();
        let linked = self.linked_records.get();
        let unresolved = self.unresolved_records.get();
        let duplicates = self.duplicates_removed.get();
        let source_canonical = self.source_canonical_reports.get();
        let manually_created = self.manually_created_reports.get();
        let screened = self.screened_records.get();

        if linked.checked_add(unresolved) != Some(identified) {
            return Err(PrismaInvariantError::SourceRecordsDoNotReconcile);
        }
        if duplicates.checked_add(source_canonical) != Some(linked) {
            return Err(PrismaInvariantError::DuplicateArithmeticDoesNotReconcile);
        }
        if source_canonical.checked_add(manually_created) != Some(screened) {
            return Err(PrismaInvariantError::ScreeningReportsDoNotReconcile);
        }
        if self
            .title_abstract_excluded
            .get()
            .checked_add(self.title_abstract_pending.get())
            .and_then(|value| value.checked_add(self.reports_sought.get()))
            != Some(screened)
        {
            return Err(PrismaInvariantError::TitleAbstractFlowDoesNotReconcile);
        }
        if self
            .reports_not_retrieved
            .get()
            .checked_add(self.full_text_assessed.get())
            != Some(self.reports_sought.get())
        {
            return Err(PrismaInvariantError::RetrievalFlowDoesNotReconcile);
        }
        if self
            .full_text_included
            .get()
            .checked_add(self.full_text_excluded.get())
            .and_then(|value| value.checked_add(self.full_text_pending.get()))
            != Some(self.full_text_assessed.get())
        {
            return Err(PrismaInvariantError::FullTextFlowDoesNotReconcile);
        }
        let reason_total = self
            .full_text_exclusions
            .iter()
            .try_fold(0_u64, |total, reason| total.checked_add(reason.count.get()))
            .ok_or(PrismaInvariantError::ReasonCountsOverflow)?;
        if reason_total != self.full_text_excluded.get() {
            return Err(PrismaInvariantError::ReasonCountsDoNotReconcile);
        }
        if self.included_reports_not_grouped.get() > self.full_text_included.get()
            || self.included_studies.get() > self.full_text_included.get()
        {
            return Err(PrismaInvariantError::GroupingCountsExceedIncludedReports);
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum PrismaInvariantError {
    #[error("identified source records do not equal linked plus unresolved records")]
    SourceRecordsDoNotReconcile,
    #[error("linked source records do not equal duplicates removed plus source canonical reports")]
    DuplicateArithmeticDoesNotReconcile,
    #[error("screening reports do not equal source canonical plus manually-created reports")]
    ScreeningReportsDoNotReconcile,
    #[error("title/abstract flow does not reconcile")]
    TitleAbstractFlowDoesNotReconcile,
    #[error("retrieval flow does not reconcile")]
    RetrievalFlowDoesNotReconcile,
    #[error("full-text flow does not reconcile")]
    FullTextFlowDoesNotReconcile,
    #[error("full-text reason counts overflow")]
    ReasonCountsOverflow,
    #[error("full-text reason counts do not reconcile")]
    ReasonCountsDoNotReconcile,
    #[error("grouping counts exceed included reports")]
    GroupingCountsExceedIncludedReports,
    #[error("grouped reports would be negative")]
    GroupedReportsUnderflow,
}

fn xml_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

fn svg_box(id: &str, label: &str, count: NonNegativeCount, x: u32, y: u32) -> String {
    format!(
        r#"<g id="{id}"><rect x="{x}" y="{y}" width="230" height="64" rx="8"/><text x="{}" y="{}" class="label">{}</text><text x="{}" y="{}" class="count">{}</text></g>"#,
        x + 115,
        y + 25,
        xml_escape(label),
        x + 115,
        y + 49,
        count.get(),
    )
}

fn svg_connector(from: &str, to: &str, d: &str, kind: &str) -> String {
    format!(
        r#"<path id="connector-{from}-{to}" class="connector {kind}" data-from="{from}" data-to="{to}" d="{d}"/>"#
    )
}

/// Render the canonical projection as a deterministic, accessible SVG.
/// Ordering and labels are intentionally stable so exports can be diffed and
/// cached without a layout engine.
pub fn render_prisma_svg(projection: &PrismaProjection) -> Result<String, PrismaInvariantError> {
    let grouped_reports = projection.grouped_reports()?;
    let boxes = [
        (
            "identified-records",
            "Records identified",
            projection.identified_records,
            550,
            24,
        ),
        (
            "unresolved-records",
            "Unresolved source records",
            projection.unresolved_records,
            40,
            24,
        ),
        (
            "linked-records",
            "Linked records",
            projection.linked_records,
            550,
            120,
        ),
        (
            "duplicates-removed",
            "Duplicates removed",
            projection.duplicates_removed,
            40,
            120,
        ),
        (
            "pending-dedupe-proposals",
            "Dedupe proposals pending",
            projection.pending_dedupe_proposals,
            1040,
            120,
        ),
        (
            "source-canonical-reports",
            "Source canonical reports",
            projection.source_canonical_reports,
            550,
            216,
        ),
        (
            "manually-created-reports",
            "Manually created reports",
            projection.manually_created_reports,
            1040,
            216,
        ),
        (
            "screened-records",
            "Records screened",
            projection.screened_records,
            550,
            312,
        ),
        (
            "title-abstract-excluded",
            "Title/abstract excluded",
            projection.title_abstract_excluded,
            40,
            312,
        ),
        (
            "title-abstract-pending",
            "Title/abstract pending",
            projection.title_abstract_pending,
            1040,
            312,
        ),
        (
            "reports-sought",
            "Reports sought",
            projection.reports_sought,
            550,
            420,
        ),
        (
            "reports-not-retrieved",
            "Reports not retrieved",
            projection.reports_not_retrieved,
            40,
            420,
        ),
        (
            "full-text-assessed",
            "Full texts assessed",
            projection.full_text_assessed,
            550,
            528,
        ),
        (
            "full-text-pending",
            "Full-text pending",
            projection.full_text_pending,
            40,
            528,
        ),
        (
            "full-text-excluded",
            "Full-text excluded",
            projection.full_text_excluded,
            1040,
            528,
        ),
        (
            "included-reports",
            "Reports included",
            projection.full_text_included,
            550,
            636,
        ),
        (
            "included-reports-not-grouped",
            "Included reports not grouped",
            projection.included_reports_not_grouped,
            40,
            636,
        ),
        (
            "grouped-reports",
            "Grouped reports",
            grouped_reports,
            550,
            744,
        ),
        (
            "included-studies",
            "Included studies (distinct)",
            projection.included_studies,
            1040,
            744,
        ),
    ];
    let mut svg = String::from(
        r##"<svg xmlns="http://www.w3.org/2000/svg" role="img" aria-labelledby="prisma-title prisma-description" viewBox="0 0 1320 840"><title id="prisma-title">PRISMA flow diagram</title><desc id="prisma-description">Deterministic counts derived from persisted records, screening state, retrieval state, and study grouping. Main equations are vertical; unresolved, duplicate, pending, exclusion, and grouping reconciliation branches attach to their parent counts.</desc><style>rect{fill:#f8fafc;stroke:#64748b;stroke-width:1.5}.label,.count{text-anchor:middle;font-family:system-ui,sans-serif}.label{font-size:14px;fill:#0f172a}.count{font-size:18px;font-weight:700;fill:#0f172a}.connector{fill:none;stroke:#94a3b8;stroke-width:2;marker-end:url(#arrow)}.connector.grouping{stroke-dasharray:6 4}</style><defs><marker id="arrow" markerWidth="8" markerHeight="8" refX="7" refY="4" orient="auto"><path d="M0,0 L8,4 L0,8 z" fill="#94a3b8"/></marker></defs>"##,
    );
    for (id, label, count, x, y) in boxes {
        svg.push_str(&svg_box(id, label, count, x, y));
    }
    let connectors = [
        (
            "identified-records",
            "unresolved-records",
            "M550 56H260",
            "branch",
        ),
        (
            "identified-records",
            "linked-records",
            "M665 88V120",
            "main",
        ),
        (
            "linked-records",
            "duplicates-removed",
            "M550 152H260",
            "branch",
        ),
        (
            "linked-records",
            "pending-dedupe-proposals",
            "M780 152H1040",
            "branch",
        ),
        (
            "linked-records",
            "source-canonical-reports",
            "M665 184V216",
            "main",
        ),
        (
            "source-canonical-reports",
            "screened-records",
            "M665 280V312",
            "main",
        ),
        (
            "manually-created-reports",
            "screened-records",
            "M1040 248H900V344H780",
            "branch",
        ),
        (
            "screened-records",
            "title-abstract-excluded",
            "M550 344H260",
            "branch",
        ),
        (
            "screened-records",
            "title-abstract-pending",
            "M780 344H1040",
            "branch",
        ),
        ("screened-records", "reports-sought", "M665 376V420", "main"),
        (
            "reports-sought",
            "reports-not-retrieved",
            "M550 452H260",
            "branch",
        ),
        (
            "reports-sought",
            "full-text-assessed",
            "M665 484V528",
            "main",
        ),
        (
            "full-text-assessed",
            "full-text-pending",
            "M550 560H260",
            "branch",
        ),
        (
            "full-text-assessed",
            "full-text-excluded",
            "M780 560H1040",
            "branch",
        ),
        (
            "full-text-assessed",
            "included-reports",
            "M665 592V636",
            "main",
        ),
        (
            "included-reports",
            "included-reports-not-grouped",
            "M550 668H260",
            "branch",
        ),
        (
            "included-reports",
            "grouped-reports",
            "M665 700V744",
            "main",
        ),
        (
            "grouped-reports",
            "included-studies",
            "M780 776H1040",
            "grouping",
        ),
    ];
    for (from, to, path, kind) in connectors {
        svg.push_str(&svg_connector(from, to, path, kind));
    }
    svg.push_str("</svg>");
    Ok(svg)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn projection() -> PrismaProjection {
        PrismaProjection {
            project_id: Uuid::nil(),
            screening_high_watermark: NonNegativeCount::new(3),
            as_of: None,
            identified_records: NonNegativeCount::new(9),
            linked_records: NonNegativeCount::new(8),
            duplicates_removed: NonNegativeCount::new(2),
            unresolved_records: NonNegativeCount::new(1),
            pending_dedupe_proposals: NonNegativeCount::new(1),
            source_canonical_reports: NonNegativeCount::new(6),
            manually_created_reports: NonNegativeCount::new(2),
            screened_records: NonNegativeCount::new(8),
            title_abstract_excluded: NonNegativeCount::new(2),
            title_abstract_pending: NonNegativeCount::new(1),
            reports_sought: NonNegativeCount::new(5),
            reports_not_retrieved: NonNegativeCount::new(1),
            full_text_assessed: NonNegativeCount::new(4),
            full_text_pending: NonNegativeCount::new(0),
            full_text_included: NonNegativeCount::new(3),
            full_text_excluded: NonNegativeCount::new(1),
            full_text_exclusions: Vec::new(),
            included_reports_not_grouped: NonNegativeCount::new(1),
            included_studies: NonNegativeCount::new(2),
        }
    }

    #[test]
    fn grouped_reports_are_derived_without_underflow() {
        assert_eq!(projection().grouped_reports().unwrap().get(), 2);
    }

    #[test]
    fn projection_invariants_require_every_equation() {
        let mut value = projection();
        value.full_text_exclusions.push(PrismaReasonCount {
            id: Uuid::nil(),
            code: "wrong_design".to_owned(),
            label: "Wrong design".to_owned(),
            count: NonNegativeCount::new(1),
        });
        assert!(value.validate().is_ok());
        value.linked_records = NonNegativeCount::new(7);
        assert!(value.validate().is_err());
        value.linked_records = NonNegativeCount::new(8);
        value.full_text_exclusions[0].count = NonNegativeCount::new(0);
        assert!(value.validate().is_err());
        value.full_text_exclusions[0].count = NonNegativeCount::new(1);
        value.included_reports_not_grouped = NonNegativeCount::new(4);
        assert_eq!(
            value.grouped_reports(),
            Err(PrismaInvariantError::GroupedReportsUnderflow)
        );
    }

    #[test]
    fn svg_order_and_accessibility_are_stable() {
        let svg = render_prisma_svg(&projection()).expect("valid SVG");
        assert!(svg.starts_with("<svg xmlns="));
        assert!(svg.contains("role=\"img\""));
        assert!(svg.find("identified-records") < svg.find("included-studies"));
        for (from, to) in [
            ("identified-records", "linked-records"),
            ("identified-records", "unresolved-records"),
            ("linked-records", "duplicates-removed"),
            ("reports-sought", "reports-not-retrieved"),
            ("reports-sought", "full-text-assessed"),
            ("full-text-assessed", "full-text-pending"),
            ("full-text-assessed", "full-text-excluded"),
            ("full-text-assessed", "included-reports"),
            ("included-reports", "included-reports-not-grouped"),
        ] {
            assert!(
                svg.contains(&format!("id=\"connector-{from}-{to}\"")),
                "missing connector {from} -> {to}"
            );
        }
        assert!(!svg.contains("connector-full-text-assessed-reports-not-retrieved"));
        assert_eq!(svg.matches("class=\"connector ").count(), 18);
    }
}
