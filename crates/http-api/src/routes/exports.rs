use axum::{
    body::Body,
    extract::{Path, State},
    http::{HeaderValue, StatusCode, header},
    response::Response,
};
use chrono::{DateTime, Utc};
use deepref_application::render_prisma_svg;
use deepref_domain::{EligibilityCriterion, ProtocolFramework, ProtocolStatus};
use serde::Serialize;
use sqlx::Row;
use utoipa::openapi::{
    RefOr, Schema,
    schema::{KnownFormat, ObjectBuilder, SchemaFormat, Type},
};
use utoipa::{PartialSchema, ToSchema};
use uuid::Uuid;

use crate::{
    error::{ApiError, ErrorResponse},
    state::AppState,
};

mod bibliography;
mod csv_output;
mod review_tables;

const MAX_EXPORT_ROWS: usize = 100_000;

struct BinaryAttachment;

impl PartialSchema for BinaryAttachment {
    fn schema() -> RefOr<Schema> {
        ObjectBuilder::new()
            .schema_type(Type::String)
            .format(Some(SchemaFormat::KnownFormat(KnownFormat::Binary)))
            .build()
            .into()
    }
}

impl ToSchema for BinaryAttachment {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ExportKind {
    ReportsCsv,
    ReportsJson,
    ReportsRis,
    ReportsBib,
    ExtractionCsv,
    AppraisalCsv,
    IncludedStudiesCsv,
    PrismaJson,
    PrismaSvg,
    AuditCsv,
    ProtocolJson,
}

impl ExportKind {
    fn parse(value: &str) -> Option<Self> {
        Some(match value {
            "reports.csv" => Self::ReportsCsv,
            "reports.json" => Self::ReportsJson,
            "reports.ris" => Self::ReportsRis,
            "reports.bib" => Self::ReportsBib,
            "extraction.csv" => Self::ExtractionCsv,
            "appraisal.csv" => Self::AppraisalCsv,
            "included_studies.csv" => Self::IncludedStudiesCsv,
            "prisma.json" => Self::PrismaJson,
            "prisma.svg" => Self::PrismaSvg,
            "audit.csv" => Self::AuditCsv,
            "protocol.json" => Self::ProtocolJson,
            _ => return None,
        })
    }

    const fn filename(self) -> &'static str {
        match self {
            Self::ReportsCsv => "reports.csv",
            Self::ReportsJson => "reports.json",
            Self::ReportsRis => "reports.ris",
            Self::ReportsBib => "reports.bib",
            Self::ExtractionCsv => "extraction.csv",
            Self::AppraisalCsv => "appraisal.csv",
            Self::IncludedStudiesCsv => "included_studies.csv",
            Self::PrismaJson => "prisma.json",
            Self::PrismaSvg => "prisma.svg",
            Self::AuditCsv => "audit.csv",
            Self::ProtocolJson => "protocol.json",
        }
    }

    const fn content_type(self) -> &'static str {
        match self {
            Self::ReportsCsv
            | Self::ExtractionCsv
            | Self::AppraisalCsv
            | Self::IncludedStudiesCsv
            | Self::AuditCsv => "text/csv; charset=utf-8",
            Self::ReportsJson | Self::PrismaJson | Self::ProtocolJson => {
                "application/json; charset=utf-8"
            }
            Self::ReportsRis => "application/x-research-info-systems; charset=utf-8",
            Self::ReportsBib => "application/x-bibtex; charset=utf-8",
            Self::PrismaSvg => "image/svg+xml; charset=utf-8",
        }
    }
}

#[derive(Debug, Clone, Serialize)]
struct ReportExport {
    report_id: Uuid,
    doi: Option<String>,
    title: Option<String>,
    publication_year: Option<i32>,
    journal: Option<String>,
    container_title: Option<String>,
    publisher: Option<String>,
    url: Option<String>,
    work_type: Option<String>,
    authors: Vec<ExportAuthor>,
    abstract_text: Option<String>,
    volume: Option<String>,
    issue: Option<String>,
    pages: Option<String>,
    screening_status: String,
    title_abstract_decision: String,
    full_text_decision: String,
    exclusion_reason_code: Option<String>,
    exclusion_reason_label: Option<String>,
    study_id: Option<Uuid>,
    study_title: Option<String>,
    appraisal_completed: bool,
}

#[derive(Debug, Clone, Serialize, serde::Deserialize)]
struct ExportAuthor {
    given: Option<String>,
    family: Option<String>,
    literal: Option<String>,
}

impl ExportAuthor {
    /// `Family, Given`, or the literal name when the source only has that.
    fn display_name(&self) -> Option<String> {
        if let Some(literal) = present(self.literal.as_deref()) {
            return Some(collapse_whitespace(literal));
        }
        match (
            present(self.family.as_deref()),
            present(self.given.as_deref()),
        ) {
            (Some(family), Some(given)) => Some(collapse_whitespace(&format!("{family}, {given}"))),
            (Some(family), None) => Some(collapse_whitespace(family)),
            (None, Some(given)) => Some(collapse_whitespace(given)),
            (None, None) => None,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
struct ProtocolExport {
    project_id: Uuid,
    id: Uuid,
    version: i32,
    name: String,
    status: ProtocolStatus,
    revision: i64,
    published_at: Option<DateTime<Utc>>,
    amendment_of: Option<Uuid>,
    framework: ProtocolFramework,
    objective: String,
    question: String,
    criteria: Vec<EligibilityCriterion>,
}

#[utoipa::path(
    get,
    path = "/projects/{project_id}/exports/{export_kind}",
    operation_id = "exportProjectArtifact",
    tag = "exports",
    params(
        ("project_id" = Uuid, Path, description = "Project identifier"),
        ("export_kind" = String, Path, description = "One of: reports.csv, reports.json, reports.ris, reports.bib, extraction.csv, appraisal.csv, included_studies.csv, prisma.json, prisma.svg, audit.csv, protocol.json")
    ),
    responses(
        (status = 200, description = "Deterministic project-scoped binary attachment", content(
            (BinaryAttachment = "text/csv"),
            (BinaryAttachment = "application/json"),
            (BinaryAttachment = "application/x-research-info-systems"),
            (BinaryAttachment = "application/x-bibtex"),
            (BinaryAttachment = "image/svg+xml")
        ), headers(
            ("Content-Disposition" = String, description = "Deterministic attachment filename"),
            ("Content-Type" = String, description = "Artifact media type")
        )),
        (status = 400, description = "Unknown export kind", body = ErrorResponse),
        (status = 404, description = "Project or published protocol not found", body = ErrorResponse),
        (status = 413, description = "Export exceeds the deterministic row limit", body = ErrorResponse),
        (status = 500, description = "Internal server error", body = ErrorResponse)
    )
)]
pub(crate) async fn export_project_artifact(
    State(state): State<AppState>,
    Path((project_id, export_kind)): Path<(Uuid, String)>,
) -> Result<Response, ApiError> {
    let kind = ExportKind::parse(&export_kind)
        .ok_or_else(|| ApiError::BadRequest("unknown export kind".to_owned()))?;
    ensure_project(&state, project_id).await?;
    let body = match kind {
        ExportKind::ReportsCsv => reports_csv(&state, project_id).await?,
        ExportKind::ReportsJson => serialize_export(&reports(&state, project_id).await?)?,
        ExportKind::ReportsRis => {
            bibliography::render_reports_ris(&reports(&state, project_id).await?)
        }
        ExportKind::ReportsBib => {
            bibliography::render_reports_bib(&reports(&state, project_id).await?)
        }
        ExportKind::ExtractionCsv => review_tables::extraction_csv(&state, project_id).await?,
        ExportKind::AppraisalCsv => review_tables::appraisal_csv(&state, project_id).await?,
        ExportKind::IncludedStudiesCsv => {
            review_tables::included_studies_csv(&state, project_id).await?
        }
        ExportKind::PrismaJson => serialize_export(&prisma(&state, project_id).await?)?,
        ExportKind::PrismaSvg => render_prisma_svg(&prisma(&state, project_id).await?),
        ExportKind::AuditCsv => audit_csv(&state, project_id).await?,
        ExportKind::ProtocolJson => serialize_export(&protocol(&state, project_id).await?)?,
    };
    let mut response = Response::new(Body::from(body));
    *response.status_mut() = StatusCode::OK;
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static(kind.content_type()),
    );
    let disposition = format!(
        "attachment; filename=\"deepref-{project_id}-{}\"",
        kind.filename()
    );
    response.headers_mut().insert(
        header::CONTENT_DISPOSITION,
        HeaderValue::from_str(&disposition)
            .map_err(|_| ApiError::Internal(anyhow::anyhow!("invalid export filename")))?,
    );
    Ok(response)
}

async fn ensure_project(state: &AppState, project_id: Uuid) -> Result<(), ApiError> {
    let exists: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM projects WHERE id=$1)")
        .bind(project_id)
        .fetch_one(&state.pool)
        .await?;
    if exists {
        Ok(())
    } else {
        Err(ApiError::NotFound("project not found".to_owned()))
    }
}

async fn prisma(
    state: &AppState,
    project_id: Uuid,
) -> Result<deepref_application::PrismaProjection, ApiError> {
    deepref_postgres::get_prisma_projection(&state.pool, project_id)
        .await
        .map_err(|error| ApiError::DataIntegrity(error.to_string()))?
        .ok_or_else(|| ApiError::NotFound("project not found".to_owned()))
}

async fn reports(state: &AppState, project_id: Uuid) -> Result<Vec<ReportExport>, ApiError> {
    let rows = sqlx::query(
        r#"SELECT r.id AS report_id, doi.value AS doi, r.title, r.publication_year,
                  r.journal, r.container_title, r.publisher, r.url,
                  r.work_type, r.authors, r.abstract_text,
                  COALESCE(NULLIF(r.raw->>'volume', ''), NULLIF(r.raw->'fields'->'VL'->>0, '')) AS volume,
                  COALESCE(NULLIF(r.raw->>'issue', ''), NULLIF(r.raw->'fields'->'IS'->>0, '')) AS issue,
                  COALESCE(NULLIF(r.raw->>'page', ''),
                           NULLIF(concat_ws('-', r.raw->'fields'->'SP'->>0, r.raw->'fields'->'EP'->>0), '')) AS pages,
                  COALESCE(ss.final_status, 'unscreened') AS screening_status,
                  COALESCE(ss.title_abstract_status, 'unscreened') AS title_abstract_decision,
                  COALESCE(ss.full_text_status, 'not_required') AS full_text_decision,
                  er.code AS exclusion_reason_code, er.label AS exclusion_reason_label,
                  study.id AS study_id, study.title AS study_title,
                  EXISTS (SELECT 1 FROM appraisal_assessments aa
                          WHERE aa.project_id = pr.project_id AND aa.report_id = pr.report_id) AS appraisal_completed
           FROM project_reports pr
           JOIN reports r ON r.id = pr.report_id
           LEFT JOIN LATERAL (
             SELECT value FROM report_identifiers
             WHERE report_id = r.id AND scheme = 'doi'
             ORDER BY created_at, id LIMIT 1
           ) doi ON true
           LEFT JOIN screening_state ss
             ON ss.project_id = pr.project_id AND ss.report_id = pr.report_id
           LEFT JOIN exclusion_reasons er
             ON er.project_id = pr.project_id AND er.id = ss.full_text_exclusion_reason_id
           LEFT JOIN LATERAL (
             SELECT s.id, s.title
             FROM study_reports sr JOIN studies s ON s.project_id = sr.project_id AND s.id = sr.study_id
             WHERE sr.project_id = pr.project_id AND sr.report_id = pr.report_id
             ORDER BY s.id LIMIT 1
           ) study ON true
           WHERE pr.project_id = $1
           ORDER BY r.id
           LIMIT $2"#,
    )
    .bind(project_id)
    .bind((MAX_EXPORT_ROWS + 1) as i64)
    .fetch_all(&state.pool)
    .await?;
    enforce_export_cap("reports", rows.len())?;
    rows.into_iter()
        .map(|row| {
            let authors = serde_json::from_value(row.get("authors"))
                .map_err(|error| ApiError::Internal(anyhow::anyhow!(error)))?;
            Ok(ReportExport {
                report_id: row.get("report_id"),
                doi: row.get("doi"),
                title: row.get("title"),
                publication_year: row.get("publication_year"),
                journal: row.get("journal"),
                container_title: row.get("container_title"),
                publisher: row.get("publisher"),
                url: row.get("url"),
                work_type: row.get("work_type"),
                authors,
                abstract_text: row.get("abstract_text"),
                volume: row.get("volume"),
                issue: row.get("issue"),
                pages: row.get("pages"),
                screening_status: row.get("screening_status"),
                title_abstract_decision: row.get("title_abstract_decision"),
                full_text_decision: row.get("full_text_decision"),
                exclusion_reason_code: row.get("exclusion_reason_code"),
                exclusion_reason_label: row.get("exclusion_reason_label"),
                study_id: row.get("study_id"),
                study_title: row.get("study_title"),
                appraisal_completed: row.get("appraisal_completed"),
            })
        })
        .collect::<Result<Vec<_>, ApiError>>()
}

const REPORTS_CSV_COLUMNS: &[&str] = &[
    "report_id",
    "doi",
    "title",
    "publication_year",
    "journal",
    "container_title",
    "publisher",
    "url",
    "work_type",
    "authors",
    "screening_status",
    "study_id",
    "study_title",
    "appraisal_completed",
    "title_abstract_decision",
    "full_text_decision",
    "exclusion_reason_code",
    "exclusion_reason_label",
    "abstract",
    "volume",
    "issue",
    "pages",
];

async fn reports_csv(state: &AppState, project_id: Uuid) -> Result<String, ApiError> {
    let reports = reports(state, project_id).await?;
    let mut csv = csv_output::header(REPORTS_CSV_COLUMNS);
    for report in &reports {
        csv.push_str(&csv_output::record(&report_csv_fields(report)));
    }
    Ok(csv)
}

fn report_csv_fields(report: &ReportExport) -> Vec<String> {
    vec![
        report.report_id.to_string(),
        optional(report.doi.clone()),
        optional(report.title.clone()),
        optional(report.publication_year.map(|year| year.to_string())),
        optional(report.journal.clone()),
        optional(report.container_title.clone()),
        optional(report.publisher.clone()),
        optional(report.url.clone()),
        optional(report.work_type.clone()),
        authors_display(&report.authors),
        report.screening_status.clone(),
        optional(report.study_id.map(|id| id.to_string())),
        optional(report.study_title.clone()),
        report.appraisal_completed.to_string(),
        report.title_abstract_decision.clone(),
        report.full_text_decision.clone(),
        optional(report.exclusion_reason_code.clone()),
        optional(report.exclusion_reason_label.clone()),
        optional(report.abstract_text.clone()),
        optional(report.volume.clone()),
        optional(report.issue.clone()),
        optional(report.pages.clone()),
    ]
}

/// Authors as `Family, Given; Family, Given`.
fn authors_display(authors: &[ExportAuthor]) -> String {
    authors
        .iter()
        .filter_map(ExportAuthor::display_name)
        .collect::<Vec<_>>()
        .join("; ")
}

const AUDIT_CSV_COLUMNS: &[&str] = &[
    "id",
    "created_at",
    "event_type",
    "aggregate_type",
    "aggregate_id",
    "actor_kind",
    "actor_id",
    "actor_label",
    "protocol_version_id",
    "stage",
    "decision",
    "reason_id",
    "event_kind",
    "supersedes_event_id",
    "undoes_event_id",
    "notes",
    "previous_snapshot",
    "result_snapshot",
    "payload",
    "provenance",
];

async fn audit_csv(state: &AppState, project_id: Uuid) -> Result<String, ApiError> {
    let rows = deepref_postgres::load_audit_export_rows(
        &state.pool,
        project_id,
        (MAX_EXPORT_ROWS + 1) as i64,
    )
    .await?;
    enforce_export_cap("audit", rows.len())?;
    let mut csv = csv_output::header(AUDIT_CSV_COLUMNS);
    for row in rows {
        let label = actor_label(row.actor_kind.as_deref(), row.actor_id.as_deref());
        // The JSON snapshots stay unchanged, in the last four columns.
        let values = [
            row.id.to_string(),
            row.created_at.to_rfc3339(),
            row.event_type,
            row.aggregate_type,
            row.aggregate_id.to_string(),
            optional(row.actor_kind),
            optional(row.actor_id),
            label,
            optional(row.protocol_version_id.map(|value| value.to_string())),
            optional(row.stage),
            optional(row.decision),
            optional(row.reason_id.map(|value| value.to_string())),
            row.event_kind,
            optional(row.supersedes_event_id.map(|value| value.to_string())),
            optional(row.undoes_event_id.map(|value| value.to_string())),
            optional(row.notes),
            row.previous_snapshot.to_string(),
            row.result_snapshot.to_string(),
            row.payload.to_string(),
            row.provenance.to_string(),
        ];
        csv.push_str(&csv_output::record(&values));
    }
    Ok(csv)
}

/// A readable name for an actor, such as `Local user`, `AI model glm-5.3-flash` or
/// `Worker <id>`. Some audit rows carry an id without a kind, and some carry only a kind; both
/// still get a label. Shapes that are not known are shown as they are stored.
fn actor_label(kind: Option<&str>, id: Option<&str>) -> String {
    let Some(id) = id.map(str::trim).filter(|id| !id.is_empty()) else {
        return match kind {
            Some("user") => "User".to_owned(),
            Some("automation") => "Automation".to_owned(),
            Some("system") => "System".to_owned(),
            _ => String::new(),
        };
    };
    if let Some(worker) = id.strip_prefix("deepref-worker-") {
        return format!("Worker {worker}");
    }
    match kind {
        Some("user") if id == "local-user" => "Local user".to_owned(),
        Some("user") => format!("User {id}"),
        Some("automation") => match (id.strip_prefix("ai:"), id.strip_prefix("workflow:")) {
            (Some(model), _) => format!("AI model {model}"),
            (None, Some(workflow)) => format!("Workflow {workflow}"),
            (None, None) => format!("Automation {id}"),
        },
        Some("system") => format!("System {id}"),
        Some(other) => format!("{other} {id}"),
        None => id.to_owned(),
    }
}

async fn protocol(state: &AppState, project_id: Uuid) -> Result<ProtocolExport, ApiError> {
    let document = deepref_postgres::get_published_protocol(&state.pool, project_id)
        .await
        .map_err(|error| match error {
            deepref_postgres::ProtocolError::ProjectNotFound
            | deepref_postgres::ProtocolError::NotFound => {
                ApiError::NotFound("published protocol not found".to_owned())
            }
            deepref_postgres::ProtocolError::Database(error) => ApiError::Database(error),
            deepref_postgres::ProtocolError::DataIntegrity(message) => {
                ApiError::DataIntegrity(message)
            }
            deepref_postgres::ProtocolError::Serialization(error) => {
                ApiError::Internal(error.into())
            }
            other => ApiError::Internal(anyhow::anyhow!(other)),
        })?;
    Ok(ProtocolExport {
        project_id,
        id: document.id,
        version: document.version,
        name: document.name,
        status: document.status,
        revision: document.revision,
        published_at: document.published_at,
        amendment_of: document.amendment_of,
        framework: document.framework,
        objective: document.objective,
        question: document.question,
        criteria: document.criteria,
    })
}

fn serialize_export<T: Serialize>(value: &T) -> Result<String, ApiError> {
    serde_json::to_string(value).map_err(|error| ApiError::Internal(error.into()))
}

fn enforce_export_cap(kind: &str, row_count: usize) -> Result<(), ApiError> {
    if row_count > MAX_EXPORT_ROWS {
        return Err(ApiError::PayloadTooLarge(format!(
            "{kind} export exceeds the maximum of {MAX_EXPORT_ROWS} rows"
        )));
    }
    Ok(())
}

/// Trimmed text, or `None` when the value is absent or blank.
fn present(value: Option<&str>) -> Option<&str> {
    value.map(str::trim).filter(|value| !value.is_empty())
}

fn collapse_whitespace(value: &str) -> String {
    value.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn optional(value: Option<String>) -> String {
    value.unwrap_or_default()
}

/// A report with every field the exports read, including characters that need escaping.
#[cfg(test)]
fn sample_report(work_type: Option<&str>) -> ReportExport {
    ReportExport {
        report_id: Uuid::from_u128(0x0123_4567_89ab_cdef_0123_4567_89ab_cdef),
        doi: Some("10.1000/deepref.export-7".to_owned()),
        title: Some("A {multiline}\ntitle & more: Müller's café".to_owned()),
        publication_year: Some(2026),
        journal: Some("Journal & Name".to_owned()),
        container_title: Some("Proceedings {Container}".to_owned()),
        publisher: Some("Publisher & Sons".to_owned()),
        url: Some("https://example.test/a?x=1&y=2".to_owned()),
        work_type: work_type.map(str::to_owned),
        authors: vec![
            ExportAuthor {
                given: Some("Ana\nMaria".to_owned()),
                family: Some("O'Neil & Co.".to_owned()),
                literal: None,
            },
            ExportAuthor {
                given: Some("J".to_owned()),
                family: Some("Smith".to_owned()),
                literal: None,
            },
        ],
        abstract_text: Some(
            "Background {x} & y.\nResults: 50% of 2213 participants; _p_ = .01 #1 ~ $5 ^2 \\ end."
                .to_owned(),
        ),
        volume: Some("49".to_owned()),
        issue: Some("3".to_owned()),
        pages: Some("414-418".to_owned()),
        screening_status: "exclude".to_owned(),
        title_abstract_decision: "include".to_owned(),
        full_text_decision: "exclude".to_owned(),
        exclusion_reason_code: Some("wrong-population".to_owned()),
        exclusion_reason_label: Some("Wrong population, adults only".to_owned()),
        study_id: None,
        study_title: None,
        appraisal_completed: false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn export_row_cap_rejects_only_the_maximum_plus_one_boundary() {
        assert!(enforce_export_cap("reports", MAX_EXPORT_ROWS).is_ok());
        let error = enforce_export_cap("audit", MAX_EXPORT_ROWS + 1)
            .expect_err("the sentinel row must make an export fail");
        assert!(matches!(error, ApiError::PayloadTooLarge(message) if message.contains("100000")));
    }

    #[test]
    fn export_kinds_map_names_to_their_files_and_media_types() {
        for (name, content_type) in [
            ("reports.csv", "text/csv; charset=utf-8"),
            ("extraction.csv", "text/csv; charset=utf-8"),
            ("appraisal.csv", "text/csv; charset=utf-8"),
            ("included_studies.csv", "text/csv; charset=utf-8"),
            ("audit.csv", "text/csv; charset=utf-8"),
            (
                "reports.ris",
                "application/x-research-info-systems; charset=utf-8",
            ),
            ("reports.bib", "application/x-bibtex; charset=utf-8"),
        ] {
            let kind = ExportKind::parse(name);
            assert_eq!(kind.map(ExportKind::filename), Some(name), "{name}");
            assert_eq!(
                kind.map(ExportKind::content_type),
                Some(content_type),
                "{name}"
            );
        }
        assert_eq!(ExportKind::parse("extraction.json"), None);
    }

    #[test]
    fn report_csv_header_matches_every_row_and_flattens_authors() {
        let report = sample_report(Some("article"));
        let fields = report_csv_fields(&report);
        assert_eq!(fields.len(), REPORTS_CSV_COLUMNS.len());
        let header = csv_output::header(REPORTS_CSV_COLUMNS);
        assert!(header.starts_with("report_id,doi,title,publication_year,journal,"));
        assert!(header.ends_with("abstract,volume,issue,pages\r\n"));
        let column = |name: &str| {
            REPORTS_CSV_COLUMNS
                .iter()
                .position(|candidate| *candidate == name)
                .and_then(|index| fields.get(index))
                .cloned()
                .unwrap_or_default()
        };
        assert_eq!(column("authors"), "O'Neil & Co., Ana Maria; Smith, J");
        assert_eq!(column("title_abstract_decision"), "include");
        assert_eq!(column("full_text_decision"), "exclude");
        assert_eq!(column("exclusion_reason_code"), "wrong-population");
        assert_eq!(
            column("exclusion_reason_label"),
            "Wrong population, adults only"
        );
        assert_eq!(
            column("abstract"),
            "Background {x} & y.\nResults: 50% of 2213 participants; _p_ = .01 #1 ~ $5 ^2 \\ end."
        );
        assert_eq!(column("volume"), "49");
        assert_eq!(column("issue"), "3");
        assert_eq!(column("pages"), "414-418");
    }

    #[test]
    fn author_names_fall_back_to_literal_and_partial_names() {
        let literal = ExportAuthor {
            given: None,
            family: None,
            literal: Some("  World   Health Organization  ".to_owned()),
        };
        assert_eq!(
            literal.display_name().as_deref(),
            Some("World Health Organization")
        );
        let family_only = ExportAuthor {
            given: None,
            family: Some("Ng".to_owned()),
            literal: None,
        };
        assert_eq!(family_only.display_name().as_deref(), Some("Ng"));
        let nameless = ExportAuthor {
            given: Some(" ".to_owned()),
            family: None,
            literal: None,
        };
        assert_eq!(nameless.display_name(), None);
    }

    #[test]
    fn audit_labels_readable_actors_and_keeps_snapshots_last() {
        assert_eq!(actor_label(Some("user"), Some("local-user")), "Local user");
        assert_eq!(
            actor_label(Some("user"), Some("reviewer-1")),
            "User reviewer-1"
        );
        assert_eq!(
            actor_label(Some("automation"), Some("ai:glm-5.3-flash")),
            "AI model glm-5.3-flash"
        );
        assert_eq!(
            actor_label(Some("automation"), Some("workflow:ae77")),
            "Workflow ae77"
        );
        assert_eq!(
            actor_label(Some("system"), Some("dedupe-test")),
            "System dedupe-test"
        );
        assert_eq!(actor_label(None, None), "");
        assert_eq!(
            actor_label(None, Some("deepref-worker-abc")),
            "Worker abc",
            "worker rows carry an id without a kind"
        );
        assert_eq!(actor_label(Some("user"), None), "User");
        assert_eq!(actor_label(Some("system"), Some("  ")), "System");
        assert_eq!(actor_label(None, Some("raw-id")), "raw-id");
        let tail = &AUDIT_CSV_COLUMNS[AUDIT_CSV_COLUMNS.len() - 4..];
        assert_eq!(
            tail,
            [
                "previous_snapshot",
                "result_snapshot",
                "payload",
                "provenance"
            ]
        );
        assert_eq!(&AUDIT_CSV_COLUMNS[..3], ["id", "created_at", "event_type"]);
        assert_eq!(AUDIT_CSV_COLUMNS[7], "actor_label");
    }
}
