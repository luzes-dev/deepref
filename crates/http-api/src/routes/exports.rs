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
            Self::PrismaJson => "prisma.json",
            Self::PrismaSvg => "prisma.svg",
            Self::AuditCsv => "audit.csv",
            Self::ProtocolJson => "protocol.json",
        }
    }

    const fn content_type(self) -> &'static str {
        match self {
            Self::ReportsCsv | Self::AuditCsv => "text/csv; charset=utf-8",
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
    screening_status: String,
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
        ("export_kind" = String, Path, description = "One of: reports.csv, reports.json, reports.ris, reports.bib, prisma.json, prisma.svg, audit.csv, protocol.json")
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
        ExportKind::ReportsRis => reports_ris(&state, project_id).await?,
        ExportKind::ReportsBib => reports_bib(&state, project_id).await?,
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
                  r.work_type, r.authors,
                  COALESCE(ss.final_status, 'unscreened') AS screening_status,
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
                screening_status: row.get("screening_status"),
                study_id: row.get("study_id"),
                study_title: row.get("study_title"),
                appraisal_completed: row.get("appraisal_completed"),
            })
        })
        .collect::<Result<Vec<_>, ApiError>>()
}

async fn reports_csv(state: &AppState, project_id: Uuid) -> Result<String, ApiError> {
    let reports = reports(state, project_id).await?;
    let mut csv = String::from(
        "report_id,doi,title,publication_year,journal,container_title,publisher,url,work_type,authors,screening_status,study_id,study_title,appraisal_completed\n",
    );
    for report in reports {
        let values = [
            report.report_id.to_string(),
            report.doi.unwrap_or_default(),
            report.title.unwrap_or_default(),
            report
                .publication_year
                .map_or_else(String::new, |value| value.to_string()),
            report.journal.unwrap_or_default(),
            report.container_title.unwrap_or_default(),
            report.publisher.unwrap_or_default(),
            report.url.unwrap_or_default(),
            report.work_type.unwrap_or_default(),
            serde_json::to_string(&report.authors)
                .map_err(|error| ApiError::Internal(anyhow::anyhow!(error)))?,
            report.screening_status,
            report
                .study_id
                .map_or_else(String::new, |value| value.to_string()),
            report.study_title.unwrap_or_default(),
            report.appraisal_completed.to_string(),
        ];
        csv.push_str(&values.map(|value| csv_field(&value)).join(","));
        csv.push('\n');
    }
    Ok(csv)
}

async fn reports_ris(state: &AppState, project_id: Uuid) -> Result<String, ApiError> {
    Ok(render_reports_ris(&reports(state, project_id).await?))
}

fn render_reports_ris(reports: &[ReportExport]) -> String {
    let mut output = String::new();
    for report in reports {
        output.push_str(&format!(
            "TY  - {}\n",
            ris_type(report.work_type.as_deref())
        ));
        output.push_str(&format!("ID  - {}\n", report.report_id));
        for author in &report.authors {
            if let Some(author) = ris_author(author) {
                output.push_str(&format!("AU  - {author}\n"));
            }
        }
        if let Some(title) = &report.title {
            output.push_str(&format!("TI  - {}\n", ris_value(title)));
        }
        if let Some(doi) = &report.doi {
            output.push_str(&format!("DO  - {}\n", ris_value(doi)));
        }
        if let Some(year) = report.publication_year {
            output.push_str(&format!("PY  - {year}\n"));
        }
        if ris_journal_tag(report.work_type.as_deref())
            && let Some(journal) = &report.journal
        {
            output.push_str(&format!("JO  - {}\n", ris_value(journal)));
        }
        if let Some(container_title) = &report.container_title {
            output.push_str(&format!("T2  - {}\n", ris_value(container_title)));
        }
        if let Some(publisher) = &report.publisher {
            output.push_str(&format!("PB  - {}\n", ris_value(publisher)));
        }
        if let Some(url) = &report.url {
            output.push_str(&format!("UR  - {}\n", ris_value(url)));
        }
        output.push_str("ER  -\n\n");
    }
    output
}

async fn reports_bib(state: &AppState, project_id: Uuid) -> Result<String, ApiError> {
    Ok(render_reports_bib(&reports(state, project_id).await?))
}

fn render_reports_bib(reports: &[ReportExport]) -> String {
    let mut output = String::new();
    for report in reports {
        output.push_str(&format!(
            "@{}{{report-{},\n",
            bib_type(report.work_type.as_deref()),
            report.report_id
        ));
        if let Some(title) = &report.title {
            output.push_str(&format!("  title = {{{}}},\n", bib_value(title)));
        }
        if let Some(doi) = &report.doi {
            output.push_str(&format!("  doi = {{{}}},\n", bib_value(doi)));
        }
        if let Some(year) = report.publication_year {
            output.push_str(&format!("  year = {{{year}}},\n"));
        }
        if !report.authors.is_empty() {
            let authors = report
                .authors
                .iter()
                .filter_map(bib_author)
                .collect::<Vec<_>>()
                .join(" and ");
            if !authors.is_empty() {
                output.push_str(&format!("  author = {{{authors}}},\n"));
            }
        }
        match bib_type(report.work_type.as_deref()) {
            "article" => {
                if let Some(journal) = &report.journal {
                    output.push_str(&format!("  journal = {{{}}},\n", bib_value(journal)));
                }
            }
            "incollection" | "inproceedings" => {
                if let Some(container_title) = &report.container_title {
                    output.push_str(&format!(
                        "  booktitle = {{{}}},\n",
                        bib_value(container_title)
                    ));
                }
            }
            "misc" => {
                if let Some(container_title) = &report.container_title {
                    output.push_str(&format!(
                        "  container = {{{}}},\n",
                        bib_value(container_title)
                    ));
                }
            }
            _ => {}
        }
        if let Some(publisher) = &report.publisher {
            output.push_str(&format!("  publisher = {{{}}},\n", bib_value(publisher)));
        }
        if let Some(url) = &report.url {
            output.push_str(&format!("  url = {{{}}},\n", bib_value(url)));
        }
        output.push_str("}\n\n");
    }
    output
}

async fn audit_csv(state: &AppState, project_id: Uuid) -> Result<String, ApiError> {
    let rows = sqlx::query(
        r#"SELECT id, created_at, event_type, aggregate_type, aggregate_id,
                  actor_kind, actor_id, protocol_version_id, stage, decision,
                  reason_id, event_kind, supersedes_event_id, undoes_event_id,
                  previous_snapshot, result_snapshot, notes, payload, provenance
           FROM (
             SELECT id, created_at, 'screening' AS event_type, 'screening' AS aggregate_type,
                    report_id AS aggregate_id, actor_kind, actor_id, protocol_version_id,
                    stage, decision, exclusion_reason_id AS reason_id, event_kind,
                    supersedes_event_id, undoes_event_id,
                    jsonb_build_object(
                      'title_abstract_status', previous_title_abstract_status,
                      'full_text_status', previous_full_text_status,
                      'full_text_exclusion_reason_id', previous_full_text_exclusion_reason_id,
                      'final_status', previous_final_status
                    ) AS previous_snapshot,
                    jsonb_build_object(
                      'title_abstract_status', result_title_abstract_status,
                      'full_text_status', result_full_text_status,
                      'full_text_exclusion_reason_id', result_full_text_exclusion_reason_id,
                      'final_status', result_final_status
                    ) AS result_snapshot,
                    notes,
                    jsonb_build_object(
                      'stage', stage, 'decision', decision,
                      'exclusion_reason_id', exclusion_reason_id,
                      'event_kind', event_kind, 'notes', notes
                    ) AS payload,
                    jsonb_build_object('actor_kind', actor_kind, 'actor_id', actor_id,
                                       'protocol_version_id', protocol_version_id) AS provenance
             FROM screening_events WHERE project_id = $1
             UNION ALL
             SELECT id, created_at, event_type, 'study' AS aggregate_type,
                    study_id AS aggregate_id, actor_kind, actor_id, NULL::uuid,
                    NULL::text, NULL::text, NULL::uuid, event_type,
                    NULL::uuid, NULL::uuid,
                    jsonb_build_object('study_id', before_study_id,
                                       'revision', before_revision,
                                       'snapshot', before_snapshot) AS previous_snapshot,
                    jsonb_build_object('study_id', result_study_id,
                                       'revision', result_revision,
                                       'snapshot', result_snapshot) AS result_snapshot,
                    NULL::text, payload,
                    jsonb_build_object('actor_kind', actor_kind, 'actor_id', actor_id,
                                       'report_id', report_id, 'study_id', study_id) AS provenance
             FROM study_events WHERE project_id = $1
             UNION ALL
             SELECT id, created_at, event_type, 'appraisal' AS aggregate_type,
                    assessment_id AS aggregate_id, actor_kind, actor_id, NULL::uuid,
                    NULL::text, NULL::text, NULL::uuid, event_type,
                    NULL::uuid, NULL::uuid, '{}'::jsonb, '{}'::jsonb,
                    NULL::text, payload,
                    jsonb_build_object('actor_kind', actor_kind, 'actor_id', actor_id,
                                       'report_id', report_id, 'assessment_id', assessment_id) AS provenance
             FROM appraisal_events WHERE project_id = $1
             UNION ALL
             SELECT id, created_at, 'dedupe_resolution' AS event_type, 'dedupe_record' AS aggregate_type,
                    record_id AS aggregate_id, actor_kind, actor_id, NULL::uuid,
                    'dedupe'::text AS stage, action AS decision, NULL::uuid AS reason_id,
                    action AS event_kind, NULL::uuid AS supersedes_event_id,
                    reverted_event_id AS undoes_event_id,
                    jsonb_build_object(
                      'prior_report_id', prior_report_id,
                      'proposal_id', proposal_id
                    ) AS previous_snapshot,
                    jsonb_build_object(
                      'resolved_report_id', resolved_report_id,
                      'action', action
                    ) AS result_snapshot,
                    reason AS notes,
                    jsonb_build_object(
                      'action', action, 'reason', reason,
                      'prior_report_id', prior_report_id,
                      'resolved_report_id', resolved_report_id,
                      'proposal_id', proposal_id,
                      'reverted_event_id', reverted_event_id
                    ) AS payload,
                    jsonb_build_object(
                      'actor_kind', actor_kind, 'actor_id', actor_id,
                      'record_id', record_id, 'proposal_id', proposal_id,
                      'prior_report_id', prior_report_id,
                      'resolved_report_id', resolved_report_id
                    ) AS provenance
             FROM dedupe_resolution_events WHERE project_id = $1
           ) events
           ORDER BY created_at, id
           LIMIT $2"#,
    )
    .bind(project_id)
    .bind((MAX_EXPORT_ROWS + 1) as i64)
    .fetch_all(&state.pool)
    .await?;
    enforce_export_cap("audit", rows.len())?;
    let mut csv = String::from(
        "id,created_at,event_type,aggregate_type,aggregate_id,actor_kind,actor_id,protocol_version_id,stage,decision,reason_id,event_kind,supersedes_event_id,undoes_event_id,previous_snapshot,result_snapshot,notes,payload,provenance\n",
    );
    for row in rows {
        let values = [
            row.get::<Uuid, _>("id").to_string(),
            row.get::<DateTime<Utc>, _>("created_at").to_rfc3339(),
            row.get("event_type"),
            row.get("aggregate_type"),
            row.get::<Uuid, _>("aggregate_id").to_string(),
            row.get("actor_kind"),
            row.get("actor_id"),
            row.get::<Option<Uuid>, _>("protocol_version_id")
                .map_or_else(String::new, |value| value.to_string()),
            row.get::<Option<String>, _>("stage").unwrap_or_default(),
            row.get::<Option<String>, _>("decision").unwrap_or_default(),
            row.get::<Option<Uuid>, _>("reason_id")
                .map_or_else(String::new, |value| value.to_string()),
            row.get("event_kind"),
            row.get::<Option<Uuid>, _>("supersedes_event_id")
                .map_or_else(String::new, |value| value.to_string()),
            row.get::<Option<Uuid>, _>("undoes_event_id")
                .map_or_else(String::new, |value| value.to_string()),
            row.get::<serde_json::Value, _>("previous_snapshot")
                .to_string(),
            row.get::<serde_json::Value, _>("result_snapshot")
                .to_string(),
            row.get::<Option<String>, _>("notes").unwrap_or_default(),
            row.get::<serde_json::Value, _>("payload").to_string(),
            row.get::<serde_json::Value, _>("provenance").to_string(),
        ];
        csv.push_str(&values.map(|value| csv_field(&value)).join(","));
        csv.push('\n');
    }
    Ok(csv)
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

fn csv_field(value: &str) -> String {
    format!("\"{}\"", value.replace('"', "\"\""))
}

fn ris_value(value: &str) -> String {
    value.replace(['\r', '\n'], " ")
}

fn ris_type(work_type: Option<&str>) -> &'static str {
    match work_type.map(str::to_ascii_lowercase).as_deref() {
        Some("article") | Some("journal-article") | Some("journal_article") => "JOUR",
        Some("book") => "BOOK",
        Some("book-chapter") | Some("book_chapter") => "CHAP",
        Some("conference-paper") | Some("conference_paper") | Some("proceedings-article") => "CONF",
        Some("dataset") => "DATA",
        Some("report") => "RPRT",
        Some("dissertation") | Some("thesis") => "THES",
        _ => "GEN",
    }
}

fn ris_journal_tag(work_type: Option<&str>) -> bool {
    !matches!(
        ris_type(work_type),
        "BOOK" | "CHAP" | "CONF" | "DATA" | "RPRT" | "THES"
    )
}

fn bib_type(work_type: Option<&str>) -> &'static str {
    match work_type.map(str::to_ascii_lowercase).as_deref() {
        Some("article") | Some("journal-article") | Some("journal_article") => "article",
        Some("book") => "book",
        Some("book-chapter") | Some("book_chapter") => "incollection",
        Some("conference-paper") | Some("conference_paper") | Some("proceedings-article") => {
            "inproceedings"
        }
        Some("dataset") => "dataset",
        Some("report") => "techreport",
        Some("dissertation") | Some("thesis") => "thesis",
        _ => "misc",
    }
}

fn ris_author(author: &ExportAuthor) -> Option<String> {
    author
        .literal
        .as_deref()
        .or(author.family.as_deref())
        .map(|family| {
            author.literal.as_deref().map_or_else(
                || {
                    author
                        .given
                        .as_deref()
                        .map_or_else(|| family.to_owned(), |given| format!("{family}, {given}"))
                },
                str::to_owned,
            )
        })
        .map(|value| ris_value(&value))
}

fn bib_author(author: &ExportAuthor) -> Option<String> {
    ris_author(author).map(|value| bib_value(&value))
}

fn bib_value(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());
    for character in value.replace(['\r', '\n'], " ").chars() {
        match character {
            '\\' => escaped.push_str("\\textbackslash{}"),
            '{' => escaped.push_str("\\{"),
            '}' => escaped.push_str("\\}"),
            '&' => escaped.push_str("\\&"),
            '%' => escaped.push_str("\\%"),
            '#' => escaped.push_str("\\#"),
            '_' => escaped.push_str("\\_"),
            '$' => escaped.push_str("\\$"),
            '^' => escaped.push_str("\\textasciicircum{}"),
            '~' => escaped.push_str("\\textasciitilde{}"),
            character => escaped.push(character),
        }
    }
    escaped
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(work_type: Option<&str>) -> ReportExport {
        ReportExport {
            report_id: Uuid::new_v4(),
            doi: Some("10.5555/a&b".to_owned()),
            title: Some("A {multiline}\ntitle & more".to_owned()),
            publication_year: Some(2026),
            journal: Some("Journal & Name".to_owned()),
            container_title: Some("Proceedings {Container}".to_owned()),
            publisher: Some("Publisher & Sons".to_owned()),
            url: Some("https://example.test/a?x=1&y=2".to_owned()),
            work_type: work_type.map(str::to_owned),
            authors: vec![ExportAuthor {
                given: Some("Ana\nMaria".to_owned()),
                family: Some("O'Neil & Co.".to_owned()),
                literal: None,
            }],
            screening_status: "include".to_owned(),
            study_id: None,
            study_title: None,
            appraisal_completed: false,
        }
    }

    #[test]
    fn bibliography_maps_article_book_chapter_conference_and_unknown_types() {
        assert_eq!(bib_type(Some("article")), "article");
        assert_eq!(bib_type(Some("book")), "book");
        assert_eq!(bib_type(Some("book-chapter")), "incollection");
        assert_eq!(bib_type(Some("conference-paper")), "inproceedings");
        assert_eq!(bib_type(Some("something-new")), "misc");

        let article = render_reports_bib(&[fixture(Some("article"))]);
        assert!(article.starts_with("@article{"));
        assert!(article.contains("journal = {Journal \\& Name}"));
        assert!(!article.contains("booktitle ="));

        let conference = render_reports_bib(&[fixture(Some("conference-paper"))]);
        assert!(conference.starts_with("@inproceedings{"));
        assert!(conference.contains("booktitle = {Proceedings \\{Container\\}}"));
        assert!(!conference.contains("journal ="));

        let unknown = render_reports_bib(&[fixture(Some("something-new"))]);
        assert!(unknown.starts_with("@misc{"));
        assert!(unknown.contains("container = {Proceedings \\{Container\\}}"));
        assert!(unknown.contains("publisher = {Publisher \\& Sons}"));
        assert!(unknown.contains("title = {A \\{multiline\\} title \\& more}"));
        assert!(unknown.contains("author = {O'Neil \\& Co., Ana Maria}"));
    }

    #[test]
    fn ris_preserves_container_and_publisher_without_fabricating_journal_tags() {
        let article = render_reports_ris(&[fixture(Some("article"))]);
        assert!(article.contains("TY  - JOUR"));
        assert!(article.contains("JO  - Journal & Name"));
        assert!(article.contains("T2  - Proceedings {Container}"));
        assert!(article.contains("PB  - Publisher & Sons"));
        assert!(article.contains("TI  - A {multiline} title & more"));
        assert!(article.contains("AU  - O'Neil & Co., Ana Maria"));

        let book = render_reports_ris(&[fixture(Some("book"))]);
        assert!(book.contains("TY  - BOOK"));
        assert!(!book.contains("JO  -"));
        assert!(book.contains("T2  - Proceedings {Container}"));

        let unknown = render_reports_ris(&[fixture(None)]);
        assert!(unknown.contains("TY  - GEN"));
        assert!(unknown.contains("PB  - Publisher & Sons"));
    }

    #[test]
    fn export_row_cap_rejects_only_the_maximum_plus_one_boundary() {
        assert!(enforce_export_cap("reports", MAX_EXPORT_ROWS).is_ok());
        let error = enforce_export_cap("audit", MAX_EXPORT_ROWS + 1)
            .expect_err("the sentinel row must make an export fail");
        assert!(matches!(error, ApiError::PayloadTooLarge(message) if message.contains("100000")));
    }
}
