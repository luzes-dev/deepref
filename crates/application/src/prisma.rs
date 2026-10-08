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

    /// Every equation the flow must satisfy, and the ones that do not hold. An empty
    /// list means the counts reconcile. Real data can break an equation (for example a
    /// reason whose stage changed after the decision), so readers show these as warnings
    /// instead of refusing to show the projection.
    pub fn reconciliation_failures(&self) -> Vec<PrismaInvariantError> {
        let mut failures = Vec::new();
        let identified = self.identified_records.get();
        let linked = self.linked_records.get();
        let unresolved = self.unresolved_records.get();
        let duplicates = self.duplicates_removed.get();
        let source_canonical = self.source_canonical_reports.get();
        let manually_created = self.manually_created_reports.get();
        let screened = self.screened_records.get();

        if linked.checked_add(unresolved) != Some(identified) {
            failures.push(PrismaInvariantError::SourceRecordsDoNotReconcile);
        }
        if duplicates.checked_add(source_canonical) != Some(linked) {
            failures.push(PrismaInvariantError::DuplicateArithmeticDoesNotReconcile);
        }
        if source_canonical.checked_add(manually_created) != Some(screened) {
            failures.push(PrismaInvariantError::ScreeningReportsDoNotReconcile);
        }
        let title_abstract_total = self
            .title_abstract_excluded
            .get()
            .checked_add(self.title_abstract_pending.get())
            .and_then(|value| value.checked_add(self.reports_sought.get()));
        if title_abstract_total != Some(screened) {
            failures.push(PrismaInvariantError::TitleAbstractFlowDoesNotReconcile);
        }
        if self
            .reports_not_retrieved
            .get()
            .checked_add(self.full_text_assessed.get())
            != Some(self.reports_sought.get())
        {
            failures.push(PrismaInvariantError::RetrievalFlowDoesNotReconcile);
        }
        let full_text_total = self
            .full_text_included
            .get()
            .checked_add(self.full_text_excluded.get())
            .and_then(|value| value.checked_add(self.full_text_pending.get()));
        if full_text_total != Some(self.full_text_assessed.get()) {
            failures.push(PrismaInvariantError::FullTextFlowDoesNotReconcile);
        }
        match self
            .full_text_exclusions
            .iter()
            .try_fold(0_u64, |total, reason| total.checked_add(reason.count.get()))
        {
            None => failures.push(PrismaInvariantError::ReasonCountsOverflow),
            Some(total) if total != self.full_text_excluded.get() => {
                failures.push(PrismaInvariantError::ReasonCountsDoNotReconcile);
            }
            Some(_) => {}
        }
        if self.included_reports_not_grouped.get() > self.full_text_included.get()
            || self.included_studies.get() > self.full_text_included.get()
        {
            failures.push(PrismaInvariantError::GroupingCountsExceedIncludedReports);
        }
        failures
    }

    /// The first failed equation, if any. Use [`Self::reconciliation_failures`] to list them all.
    pub fn validate(&self) -> Result<(), PrismaInvariantError> {
        match self.reconciliation_failures().first() {
            Some(error) => Err(*error),
            None => Ok(()),
        }
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

const LABEL_X: u32 = 20;
const MAIN_X: u32 = 170;
const MAIN_W: u32 = 270;
const SIDE_X: u32 = 500;
const SIDE_W: u32 = 300;
const VIEW_WIDTH: u32 = 820;
const BOX_H: u32 = 76;
const ROW_GAP: u32 = 52;
const STACK_GAP: u32 = 24;
const REASON_LINE: u32 = 18;
const MAX_REASON_CHARS: usize = 40;

/// Colours are CSS custom properties with light fallbacks. The exported SVG is
/// therefore a light, print-ready publication figure, while the in-app diagram
/// (which inlines this markup) takes its colours from the active theme.
const PRISMA_STYLE: &str = "\
#prisma-flow .canvas{fill:var(--prisma-canvas,#ffffff)}\
#prisma-flow .box{fill:var(--prisma-box,#f8fafc);stroke:var(--prisma-edge,#64748b);stroke-width:1.5}\
#prisma-flow .box.awaiting{fill:var(--prisma-awaiting,#fffbeb);stroke:var(--prisma-awaiting-edge,#b45309);stroke-dasharray:6 4}\
#prisma-flow .label,#prisma-flow .count,#prisma-flow .note,#prisma-flow .reason,#prisma-flow .phase{font-family:var(--prisma-font,system-ui,sans-serif)}\
#prisma-flow .label,#prisma-flow .count,#prisma-flow .note{text-anchor:middle}\
#prisma-flow .label{font-size:14px;fill:var(--prisma-ink,#0f172a)}\
#prisma-flow .count{font-size:18px;font-weight:700;fill:var(--prisma-ink,#0f172a)}\
#prisma-flow .note{font-size:12px;fill:var(--prisma-muted,#475569)}\
#prisma-flow .reason{font-size:12px;fill:var(--prisma-ink,#0f172a)}\
#prisma-flow .phase{font-size:12px;font-weight:700;letter-spacing:.12em;fill:var(--prisma-muted,#475569)}\
#prisma-flow .warning{font-size:12px;font-weight:700;font-family:var(--prisma-font,system-ui,sans-serif);fill:var(--prisma-warning,#b91c1c)}\
#prisma-flow .connector{fill:none;stroke:var(--prisma-arrow,#94a3b8);stroke-width:2;marker-end:url(#prisma-arrow)}\
#prisma-flow marker path{fill:var(--prisma-arrow,#94a3b8)}";

fn truncate_label(value: &str, max_chars: usize) -> String {
    if value.chars().count() <= max_chars {
        return value.to_owned();
    }
    let mut out: String = value.chars().take(max_chars.saturating_sub(1)).collect();
    out.push('…');
    out
}

/// One positioned box of the flow diagram.
struct FlowBox<'a> {
    id: &'a str,
    label: &'a str,
    count: NonNegativeCount,
    note: Option<String>,
    x: u32,
    y: u32,
    width: u32,
    height: u32,
    /// A box that still waits for a decision is dashed: it is not yet in the flow.
    awaiting: bool,
    reasons: &'a [PrismaReasonCount],
}

impl FlowBox<'_> {
    fn render(&self) -> String {
        let cx = self.x + self.width / 2;
        let class = if self.awaiting { "box awaiting" } else { "box" };
        let mut out = format!(
            r#"<g id="{}"><rect class="{class}" x="{}" y="{}" width="{}" height="{}" rx="8"/><text class="label" x="{cx}" y="{}">{}</text><text class="count" x="{cx}" y="{}">n = {}</text>"#,
            self.id,
            self.x,
            self.y,
            self.width,
            self.height,
            self.y + 26,
            xml_escape(self.label),
            self.y + 52,
            self.count.get(),
        );
        if let Some(note) = &self.note {
            out.push_str(&format!(
                r#"<text class="note" x="{cx}" y="{}">{}</text>"#,
                self.y + 68,
                xml_escape(note),
            ));
        }
        for (index, reason) in self.reasons.iter().enumerate() {
            out.push_str(&format!(
                r#"<text class="reason" x="{}" y="{}">{} (n = {})</text>"#,
                self.x + 16,
                self.y + BOX_H + 18 + REASON_LINE * index as u32,
                xml_escape(&truncate_label(&reason.label, MAX_REASON_CHARS)),
                reason.count.get(),
            ));
        }
        out.push_str("</g>");
        out
    }
}

fn svg_connector(from: &str, to: &str, d: &str, kind: &str) -> String {
    format!(
        r#"<path id="connector-{from}-{to}" class="connector {kind}" data-from="{from}" data-to="{to}" d="{d}"/>"#
    )
}

fn svg_phase(label: &str, y: u32) -> String {
    format!(r#"<text x="{LABEL_X}" y="{y}" class="phase">{label}</text>"#)
}

/// Render the projection as a deterministic, accessible PRISMA 2020 flow
/// diagram. The arithmetic reconciles on the page:
///
/// identified (including records added manually) = duplicates removed + awaiting
/// duplicate check + screened.
///
/// Internal reconciliation counts (linked records, dedupe proposals, ...) stay
/// in the JSON projection and the "Counts & sources" view. Ordering and labels
/// are intentionally stable so exports can be diffed and cached without a
/// layout engine.
pub fn render_prisma_svg(projection: &PrismaProjection) -> String {
    let mid_x = MAIN_X + MAIN_W / 2;
    let main_right = MAIN_X + MAIN_W;
    let elbow_x = (main_right + SIDE_X) / 2;
    // List only reasons that excluded something; zero rows are noise in the diagram.
    let reasons: Vec<PrismaReasonCount> = projection
        .full_text_exclusions
        .iter()
        .filter(|reason| reason.count.get() > 0)
        .cloned()
        .collect();
    let reasons = &reasons;
    let excluded_h = if reasons.is_empty() {
        BOX_H
    } else {
        BOX_H + 12 + REASON_LINE * reasons.len() as u32 + 8
    };

    let show_awaiting = projection.unresolved_records.get() > 0;
    let manually_added = projection.manually_created_reports.get();
    let identified_total =
        NonNegativeCount::new(projection.identified_records.get() + manually_added);
    let identified_note =
        (manually_added > 0).then(|| format!("incl. {manually_added} added manually"));
    let awaiting_note = (projection.pending_dedupe_proposals.get() > 0).then(|| {
        let pending = projection.pending_dedupe_proposals.get();
        let noun = if pending == 1 { "match" } else { "matches" };
        format!("{pending} possible {noun}")
    });
    let screened_note = pending_note(
        projection.title_abstract_pending.get(),
        "awaiting screening",
    );
    let assessed_note = pending_note(projection.full_text_pending.get(), "awaiting decision");
    let reports_note = pending_note(
        projection.included_reports_not_grouped.get(),
        "not yet grouped into studies",
    );

    let identification_h = if show_awaiting {
        BOX_H + STACK_GAP + BOX_H
    } else {
        BOX_H
    };
    let y_identified = 40;
    let y_awaiting = y_identified + BOX_H + STACK_GAP;
    let y_screened = y_identified + identification_h + ROW_GAP;
    let y_sought = y_screened + BOX_H + ROW_GAP;
    let y_assessed = y_sought + BOX_H + ROW_GAP;
    let y_studies = y_assessed + BOX_H.max(excluded_h) + ROW_GAP;
    let y_reports = y_studies + BOX_H + ROW_GAP;
    let height = y_reports + BOX_H + 24;

    let mut svg = format!(
        r##"<svg id="prisma-flow" xmlns="http://www.w3.org/2000/svg" role="img" aria-labelledby="prisma-title prisma-description" viewBox="0 0 {VIEW_WIDTH} {height}"><title id="prisma-title">PRISMA 2020 flow diagram</title><desc id="prisma-description">Records identified, duplicate records removed, records awaiting duplicate check, records screened and excluded, reports sought and not retrieved, reports assessed for eligibility and excluded with reasons, and studies included. Counts are derived from persisted records, screening state, retrieval state, and study grouping.</desc><style>{PRISMA_STYLE}</style><defs><marker id="prisma-arrow" markerWidth="8" markerHeight="8" refX="7" refY="4" orient="auto"><path d="M0,0 L8,4 L0,8 z"/></marker></defs><rect class="canvas" width="{VIEW_WIDTH}" height="{height}"/>"##
    );
    svg.push_str(&svg_phase("IDENTIFICATION", y_identified + BOX_H / 2 + 4));
    svg.push_str(&svg_phase("SCREENING", y_screened + BOX_H / 2 + 4));
    svg.push_str(&svg_phase("ELIGIBILITY", y_assessed + BOX_H / 2 + 4));
    svg.push_str(&svg_phase("INCLUDED", y_studies + BOX_H / 2 + 4));

    let none: &[PrismaReasonCount] = &[];
    let mut boxes = vec![
        FlowBox {
            id: "identified-records",
            label: "Records identified",
            count: identified_total,
            note: identified_note,
            x: MAIN_X,
            y: y_identified,
            width: MAIN_W,
            height: BOX_H,
            awaiting: false,
            reasons: none,
        },
        FlowBox {
            id: "duplicates-removed",
            label: "Duplicate records removed",
            count: projection.duplicates_removed,
            note: None,
            x: SIDE_X,
            y: y_identified,
            width: SIDE_W,
            height: BOX_H,
            awaiting: false,
            reasons: none,
        },
        FlowBox {
            id: "screened-records",
            label: "Records screened",
            count: projection.screened_records,
            note: screened_note,
            x: MAIN_X,
            y: y_screened,
            width: MAIN_W,
            height: BOX_H,
            awaiting: false,
            reasons: none,
        },
        FlowBox {
            id: "title-abstract-excluded",
            label: "Records excluded",
            count: projection.title_abstract_excluded,
            note: None,
            x: SIDE_X,
            y: y_screened,
            width: SIDE_W,
            height: BOX_H,
            awaiting: false,
            reasons: none,
        },
        FlowBox {
            id: "reports-sought",
            label: "Reports sought for retrieval",
            count: projection.reports_sought,
            note: None,
            x: MAIN_X,
            y: y_sought,
            width: MAIN_W,
            height: BOX_H,
            awaiting: false,
            reasons: none,
        },
        FlowBox {
            id: "reports-not-retrieved",
            label: "Reports not retrieved",
            count: projection.reports_not_retrieved,
            note: None,
            x: SIDE_X,
            y: y_sought,
            width: SIDE_W,
            height: BOX_H,
            awaiting: false,
            reasons: none,
        },
        FlowBox {
            id: "full-text-assessed",
            label: "Reports assessed for eligibility",
            count: projection.full_text_assessed,
            note: assessed_note,
            x: MAIN_X,
            y: y_assessed,
            width: MAIN_W,
            height: BOX_H,
            awaiting: false,
            reasons: none,
        },
        FlowBox {
            id: "full-text-excluded",
            label: "Reports excluded",
            count: projection.full_text_excluded,
            note: None,
            x: SIDE_X,
            y: y_assessed,
            width: SIDE_W,
            height: excluded_h,
            awaiting: false,
            reasons,
        },
        FlowBox {
            id: "included-studies",
            label: "Studies included",
            count: projection.included_studies,
            note: None,
            x: MAIN_X,
            y: y_studies,
            width: MAIN_W,
            height: BOX_H,
            awaiting: false,
            reasons: none,
        },
        FlowBox {
            id: "included-reports",
            label: "Reports of included studies",
            count: projection.full_text_included,
            note: reports_note,
            x: MAIN_X,
            y: y_reports,
            width: MAIN_W,
            height: BOX_H,
            awaiting: false,
            reasons: none,
        },
    ];
    if show_awaiting {
        boxes.push(FlowBox {
            id: "awaiting-duplicate-check",
            label: "Awaiting duplicate check",
            count: projection.unresolved_records,
            note: awaiting_note,
            x: SIDE_X,
            y: y_awaiting,
            width: SIDE_W,
            height: BOX_H,
            awaiting: true,
            reasons: none,
        });
    }
    for item in &boxes {
        svg.push_str(&item.render());
    }

    let side = |y: u32| format!("M{main_right} {}H{SIDE_X}", y + BOX_H / 2);
    let down = |from_y: u32, to_y: u32| format!("M{mid_x} {}V{to_y}", from_y + BOX_H);
    let mut connectors = vec![
        (
            "identified-records",
            "duplicates-removed",
            side(y_identified),
            "branch",
        ),
        (
            "identified-records",
            "screened-records",
            down(y_identified, y_screened),
            "main",
        ),
        (
            "screened-records",
            "title-abstract-excluded",
            side(y_screened),
            "branch",
        ),
        (
            "screened-records",
            "reports-sought",
            down(y_screened, y_sought),
            "main",
        ),
        (
            "reports-sought",
            "reports-not-retrieved",
            side(y_sought),
            "branch",
        ),
        (
            "reports-sought",
            "full-text-assessed",
            down(y_sought, y_assessed),
            "main",
        ),
        (
            "full-text-assessed",
            "full-text-excluded",
            side(y_assessed),
            "branch",
        ),
        (
            "full-text-assessed",
            "included-studies",
            format!("M{mid_x} {}V{y_studies}", y_assessed + BOX_H),
            "main",
        ),
        (
            "included-studies",
            "included-reports",
            down(y_studies, y_reports),
            "main",
        ),
    ];
    if show_awaiting {
        // Elbow from the identified box into the awaiting box, so the branch
        // does not run through the duplicates box.
        connectors.push((
            "identified-records",
            "awaiting-duplicate-check",
            format!(
                "M{main_right} {}H{elbow_x}V{}H{SIDE_X}",
                y_identified + BOX_H / 2,
                y_awaiting + BOX_H / 2
            ),
            "branch",
        ));
    }
    for (from, to, path, kind) in connectors {
        svg.push_str(&svg_connector(from, to, &path, kind));
    }
    if !projection.reconciliation_failures().is_empty() {
        // The figure is still drawn from the counts it has, but it must not pass as reconciled.
        svg.push_str(&format!(
            r#"<text class="warning" x="{LABEL_X}" y="{}">Counts do not reconcile: check the PRISMA counts before publishing.</text>"#,
            height - 8,
        ));
    }
    svg.push_str("</svg>");
    svg
}

fn pending_note(count: u64, text: &str) -> Option<String> {
    (count > 0).then(|| format!("{count} {text}"))
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

    fn with_wrong_design(mut value: PrismaProjection) -> PrismaProjection {
        value.full_text_exclusions.push(PrismaReasonCount {
            id: Uuid::nil(),
            code: "wrong_design".to_owned(),
            label: "Wrong design".to_owned(),
            count: NonNegativeCount::new(1),
        });
        value
    }

    /// Reads the `n = X` count of one diagram box from the rendered markup.
    fn box_count(svg: &str, id: &str) -> Option<u64> {
        let group = svg
            .split(&format!("<g id=\"{id}\">"))
            .nth(1)?
            .split("</g>")
            .next()?;
        let count = group.split("class=\"count\"").nth(1)?;
        count
            .split("n = ")
            .nth(1)?
            .chars()
            .take_while(char::is_ascii_digit)
            .collect::<String>()
            .parse()
            .ok()
    }

    #[test]
    fn broken_equations_are_reported_and_still_render_a_warning() {
        let mut value = with_wrong_design(projection());
        value.linked_records = NonNegativeCount::new(7);
        let failures = value.reconciliation_failures();
        assert!(failures.contains(&PrismaInvariantError::SourceRecordsDoNotReconcile));
        assert!(value.validate().is_err());
        let svg = render_prisma_svg(&value);
        assert!(svg.contains("Counts do not reconcile"));
        assert!(svg.contains("id=\"identified-records\""));
        let clean = render_prisma_svg(&with_wrong_design(projection()));
        assert!(!clean.contains("Counts do not reconcile"));
    }

    #[test]
    fn grouped_reports_are_derived_without_underflow() {
        assert_eq!(projection().grouped_reports().unwrap().get(), 2);
    }

    #[test]
    fn projection_invariants_require_every_equation() {
        let mut value = with_wrong_design(projection());
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
    fn svg_follows_prisma_2020_and_hides_internal_reconciliation() {
        let svg = render_prisma_svg(&with_wrong_design(projection()));
        assert!(svg.starts_with("<svg id=\"prisma-flow\" xmlns="));
        assert!(svg.contains("role=\"img\""));
        assert!(svg.find("identified-records") < svg.find("included-reports"));
        for (from, to) in [
            ("identified-records", "duplicates-removed"),
            ("identified-records", "screened-records"),
            ("identified-records", "awaiting-duplicate-check"),
            ("screened-records", "title-abstract-excluded"),
            ("screened-records", "reports-sought"),
            ("reports-sought", "reports-not-retrieved"),
            ("reports-sought", "full-text-assessed"),
            ("full-text-assessed", "full-text-excluded"),
            ("full-text-assessed", "included-studies"),
            ("included-studies", "included-reports"),
        ] {
            assert!(
                svg.contains(&format!("id=\"connector-{from}-{to}\"")),
                "missing connector {from} -> {to}"
            );
        }
        assert!(svg.contains("Wrong design (n = 1)"));
        assert!(svg.contains("Duplicate records removed"));
        for internal in [
            "Linked records",
            "Unresolved source records",
            "Source canonical reports",
            "Manually created reports",
            "Dedupe proposals pending",
            "Included reports not grouped",
        ] {
            assert!(!svg.contains(internal), "internal box leaked: {internal}");
        }
        assert_eq!(svg.matches("class=\"connector ").count(), 10);
    }

    #[test]
    fn diagram_arithmetic_reconciles_identified_removed_awaiting_and_screened() {
        let value = projection();
        let svg = render_prisma_svg(&value);
        let identified = box_count(&svg, "identified-records").expect("identified box");
        let removed = box_count(&svg, "duplicates-removed").expect("removed box");
        let awaiting = box_count(&svg, "awaiting-duplicate-check").expect("awaiting box");
        let screened = box_count(&svg, "screened-records").expect("screened box");
        // Manually added reports enter the identified total, so the flow balances.
        assert_eq!(identified, 11);
        assert_eq!(identified, removed + awaiting + screened);
        assert_eq!(awaiting, value.unresolved_records.get());
        assert!(svg.contains("incl. 2 added manually"));
        assert!(svg.contains("1 possible match"));
    }

    #[test]
    fn awaiting_box_is_hidden_when_no_records_wait_for_duplicate_check() {
        let mut value = with_wrong_design(projection());
        value.linked_records = NonNegativeCount::new(9);
        value.unresolved_records = NonNegativeCount::new(0);
        value.duplicates_removed = NonNegativeCount::new(3);
        value.pending_dedupe_proposals = NonNegativeCount::new(0);
        assert!(value.validate().is_ok());
        let svg = render_prisma_svg(&value);
        assert!(!svg.contains("awaiting-duplicate-check"));
        assert_eq!(svg.matches("class=\"connector ").count(), 9);
        assert_eq!(box_count(&svg, "identified-records"), Some(11));
    }

    #[test]
    fn awaiting_note_pluralises_possible_matches() {
        let mut value = projection();
        value.pending_dedupe_proposals = NonNegativeCount::new(2);
        let svg = render_prisma_svg(&value);
        assert!(svg.contains("2 possible matches"));
    }

    #[test]
    fn svg_colours_come_from_theme_variables_with_light_fallbacks() {
        let svg = render_prisma_svg(&projection());
        assert!(svg.contains("fill:var(--prisma-box,#f8fafc)"));
        assert!(svg.contains("fill:var(--prisma-canvas,#ffffff)"));
        assert!(svg.contains("fill:var(--prisma-ink,#0f172a)"));
        // Selectors are scoped to the diagram so the inlined markup cannot restyle the app.
        assert!(!svg.contains("<style>rect{"));
    }
}
