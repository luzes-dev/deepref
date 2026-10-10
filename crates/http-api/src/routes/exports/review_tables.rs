//! Review tables that need more than the report list: extracted values, appraisal judgments and
//! the included studies. Each file has one row per decision or value so that a reviewer can
//! filter, count and cite it.

use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use deepref_application::{
    AnswerSchema, AppraisalDefinition, all_appraisal_definitions, appraisal::AnswerOption,
};
use serde_json::Value;
use sqlx::Row;
use uuid::Uuid;

use super::{MAX_EXPORT_ROWS, actor_label, csv_output, enforce_export_cap, optional};
use crate::{error::ApiError, state::AppState};

/// Source quotes are cut to this many characters so that one long block cannot dominate a row.
const QUOTE_CHARS: usize = 500;

const EXTRACTION_COLUMNS: &[&str] = &[
    "study_id",
    "study_title",
    "report_id",
    "field_key",
    "field_label",
    "field_type",
    "value",
    "status",
    "ai_proposed",
    "entered_by",
    "confirmed_by",
    "confirmed_at",
    "source_page",
    "source_quote",
    "rationale",
    "value_id",
];

const APPRAISAL_COLUMNS: &[&str] = &[
    "report_id",
    "report_title",
    "screening_status",
    "assessment_id",
    "assessed_at",
    "assessor",
    "tool_id",
    "tool_name",
    "tool_version",
    "item_type",
    "domain_id",
    "domain_label",
    "item_id",
    "item_label",
    "answer",
    "answer_label",
    "evidence_pages",
    "evidence_quotes",
];

const INCLUDED_COLUMNS: &[&str] = &[
    "study_id",
    "study_title",
    "study_design",
    "relationship",
    "report_id",
    "report_title",
    "doi",
    "publication_year",
    "title_abstract_decision",
    "full_text_decision",
    "screening_status",
];

/// One study and one extraction field. The value columns are empty when the study has no
/// current value for the field.
struct ExtractionRow {
    study_id: Uuid,
    study_title: String,
    field_key: String,
    field_label: String,
    field_type: String,
    value_id: Option<Uuid>,
    report_id: Option<Uuid>,
    value: Option<String>,
    needs_verification: bool,
    approved_by_kind: Option<String>,
    approved_by_id: Option<String>,
    verified_at: Option<DateTime<Utc>>,
    verified_by_kind: Option<String>,
    verified_by_id: Option<String>,
    source_page: Option<i32>,
    source_quote: Option<String>,
    rationale: Option<String>,
}

/// The latest completed appraisal of one report.
struct AssessmentRow {
    report_id: Uuid,
    report_title: String,
    screening_status: String,
    assessment_id: Uuid,
    definition_id: String,
    definition_version: i32,
    responses: Value,
    judgments: Value,
    completed_at: DateTime<Utc>,
    actor_kind: String,
    actor_id: String,
}

/// One piece of evidence cited for an appraisal question.
struct EvidenceRow {
    assessment_id: Uuid,
    question_id: String,
    page: Option<i32>,
    quote: Option<String>,
}

struct IncludedRow {
    study_id: Option<Uuid>,
    study_title: Option<String>,
    study_design: Option<String>,
    relationship: Option<String>,
    report_id: Uuid,
    report_title: Option<String>,
    doi: Option<String>,
    publication_year: Option<i32>,
    title_abstract_decision: String,
    full_text_decision: String,
    screening_status: String,
}

/// The extraction table: one row per study and field, with the current value when there is one.
const EXTRACTION_SQL: &str = r#"
WITH fields AS (
  SELECT DISTINCT ON (field_key)
         id, field_key, label, value_type,
         min(created_at) OVER (PARTITION BY field_key) AS first_created_at
  FROM extraction_field_definitions
  WHERE project_id = $1
  ORDER BY field_key, version DESC, id
)
SELECT s.id AS study_id, s.title AS study_title,
       f.field_key, f.label AS field_label, f.value_type AS field_type,
       cur.id AS value_id, cur.report_id,
       COALESCE(cur.text_value, cur.number_value::text, cur.boolean_value::text,
                to_char(cur.date_value, 'YYYY-MM-DD')) AS value,
       COALESCE(cur.needs_verification, false) AS needs_verification,
       cur.approved_by_actor_kind, cur.approved_by_actor_id,
       cur.verified_at, cur.verified_by_actor_kind, cur.verified_by_actor_id,
       cur.source_page, block.text AS source_quote, cur.rationale
FROM studies s
CROSS JOIN fields f
LEFT JOIN LATERAL (
  SELECT v.*
  FROM extraction_values v
  JOIN extraction_field_definitions d ON d.id = v.field_definition_id
  WHERE v.project_id = s.project_id
    AND v.study_id = s.id
    AND d.field_key = f.field_key
    AND v.superseded_at IS NULL
  ORDER BY v.approved_at DESC, v.id DESC
  LIMIT 1
) cur ON true
LEFT JOIN document_blocks block ON block.id = cur.source_block_id
WHERE s.project_id = $1
ORDER BY s.title, s.id, f.first_created_at, f.field_key
LIMIT $2
"#;

/// The latest completed appraisal per report, with the report title and screening decision.
/// An appraisal only exists downstream of an inclusion decision, so a report hidden by an
/// active blind audit is withheld together with its assessment rather than merely masked.
const LATEST_ASSESSMENTS_SQL: &str = r#"
WITH latest AS (
  SELECT DISTINCT ON (a.report_id)
         a.id, a.report_id, a.definition_id, a.definition_version, a.responses, a.judgments,
         a.actor_kind, a.actor_id, a.completed_at
  FROM appraisal_assessments a
  WHERE a.project_id = $1 AND NOT ai_first_audit_masked(a.project_id, a.report_id)
  ORDER BY a.report_id, a.completed_at DESC, a.created_at DESC, a.id DESC
)
SELECT latest.report_id, COALESCE(r.title, '') AS report_title,
       CASE WHEN ai_first_audit_masked($1, latest.report_id)
            THEN 'unscreened' ELSE COALESCE(ss.final_status, 'unscreened') END AS screening_status,
       latest.id AS assessment_id, latest.definition_id, latest.definition_version,
       latest.responses, latest.judgments, latest.completed_at,
       latest.actor_kind, latest.actor_id
FROM latest
JOIN reports r ON r.id = latest.report_id
LEFT JOIN screening_state ss ON ss.project_id = $1 AND ss.report_id = latest.report_id
ORDER BY r.title, latest.report_id
LIMIT $2
"#;

const EVIDENCE_SQL: &str = r#"
SELECT e.assessment_id, e.question_id, b.page_number, b.text AS quote
FROM appraisal_assessment_evidence e
LEFT JOIN document_blocks b ON b.id = e.block_id
WHERE e.project_id = $1
ORDER BY e.assessment_id, e.question_id, b.page_number NULLS LAST, b.ordinal NULLS LAST, e.id
"#;

/// Included studies: every linked report of an included study, then the included reports that
/// are not yet grouped into a study. Included means title/abstract and full text both include,
/// which is the same rule the PRISMA counts use. Every column is derived from a screening
/// decision, so a report hidden by an active blind audit is withheld from the whole table.
const INCLUDED_SQL: &str = r#"
WITH included AS (
  SELECT pr.report_id
  FROM project_reports pr
  JOIN screening_state ss ON ss.project_id = pr.project_id AND ss.report_id = pr.report_id
  WHERE pr.project_id = $1
    AND ss.title_abstract_status = 'include'
    AND ss.full_text_status = 'include'
    AND NOT ai_first_audit_masked(pr.project_id, pr.report_id)
), dois AS (
  SELECT DISTINCT ON (report_id) report_id, value
  FROM report_identifiers
  WHERE scheme = 'doi'
  ORDER BY report_id, created_at, id
), grouped_studies AS (
  SELECT DISTINCT sr.study_id
  FROM study_reports sr
  JOIN included i ON i.report_id = sr.report_id
  WHERE sr.project_id = $1
)
SELECT * FROM (
  SELECT TRUE AS grouped,
         s.id AS study_id, s.title AS study_title, s.design AS study_design,
         sr.relationship, r.id AS report_id, r.title AS report_title, dois.value AS doi,
         r.publication_year,
         COALESCE(ss.title_abstract_status, 'unscreened') AS title_abstract_decision,
         COALESCE(ss.full_text_status, 'not_required') AS full_text_decision,
         COALESCE(ss.final_status, 'unscreened') AS screening_status
  FROM grouped_studies g
  JOIN studies s ON s.project_id = $1 AND s.id = g.study_id
  JOIN study_reports sr ON sr.project_id = $1 AND sr.study_id = s.id
  JOIN reports r ON r.id = sr.report_id
  LEFT JOIN dois ON dois.report_id = r.id
  LEFT JOIN screening_state ss ON ss.project_id = $1 AND ss.report_id = r.id
  WHERE NOT ai_first_audit_masked($1, r.id)
  UNION ALL
  SELECT FALSE AS grouped,
         NULL::uuid, NULL::text, NULL::text, NULL::text,
         r.id, r.title, dois.value, r.publication_year,
         ss.title_abstract_status, ss.full_text_status, ss.final_status
  FROM included i
  JOIN reports r ON r.id = i.report_id
  JOIN screening_state ss ON ss.project_id = $1 AND ss.report_id = i.report_id
  LEFT JOIN dois ON dois.report_id = r.id
  WHERE NOT EXISTS (
    SELECT 1 FROM study_reports sr WHERE sr.project_id = $1 AND sr.report_id = i.report_id
  )
) rows
ORDER BY grouped DESC, study_title, study_id, relationship, report_title, report_id
LIMIT $2
"#;

pub(super) async fn extraction_csv(state: &AppState, project_id: Uuid) -> Result<String, ApiError> {
    let rows = sqlx::query(EXTRACTION_SQL)
        .bind(project_id)
        .bind((MAX_EXPORT_ROWS + 1) as i64)
        .fetch_all(&state.pool)
        .await?;
    let rows: Vec<ExtractionRow> = rows
        .into_iter()
        .map(|row| ExtractionRow {
            study_id: row.get("study_id"),
            study_title: row.get("study_title"),
            field_key: row.get("field_key"),
            field_label: row.get("field_label"),
            field_type: row.get("field_type"),
            value_id: row.get("value_id"),
            report_id: row.get("report_id"),
            value: row.get("value"),
            needs_verification: row.get("needs_verification"),
            approved_by_kind: row.get("approved_by_actor_kind"),
            approved_by_id: row.get("approved_by_actor_id"),
            verified_at: row.get("verified_at"),
            verified_by_kind: row.get("verified_by_actor_kind"),
            verified_by_id: row.get("verified_by_actor_id"),
            source_page: row.get("source_page"),
            source_quote: row.get("source_quote"),
            rationale: row.get("rationale"),
        })
        .collect();
    enforce_export_cap("extraction", rows.len())?;
    Ok(render_extraction_csv(&rows))
}

pub(super) async fn appraisal_csv(state: &AppState, project_id: Uuid) -> Result<String, ApiError> {
    let rows = sqlx::query(LATEST_ASSESSMENTS_SQL)
        .bind(project_id)
        .bind((MAX_EXPORT_ROWS + 1) as i64)
        .fetch_all(&state.pool)
        .await?;
    let assessments: Vec<AssessmentRow> = rows
        .into_iter()
        .map(|row| AssessmentRow {
            report_id: row.get("report_id"),
            report_title: row.get("report_title"),
            screening_status: row.get("screening_status"),
            assessment_id: row.get("assessment_id"),
            definition_id: row.get("definition_id"),
            definition_version: row.get("definition_version"),
            responses: row.get("responses"),
            judgments: row.get("judgments"),
            completed_at: row.get("completed_at"),
            actor_kind: row.get("actor_kind"),
            actor_id: row.get("actor_id"),
        })
        .collect();
    enforce_export_cap("appraisal", assessments.len())?;
    let evidence = sqlx::query(EVIDENCE_SQL)
        .bind(project_id)
        .fetch_all(&state.pool)
        .await?
        .into_iter()
        .map(|row| EvidenceRow {
            assessment_id: row.get("assessment_id"),
            question_id: row.get("question_id"),
            page: row.get("page_number"),
            quote: row.get("quote"),
        })
        .collect::<Vec<_>>();
    Ok(render_appraisal_csv(&assessments, &evidence))
}

pub(super) async fn included_studies_csv(
    state: &AppState,
    project_id: Uuid,
) -> Result<String, ApiError> {
    let rows = sqlx::query(INCLUDED_SQL)
        .bind(project_id)
        .bind((MAX_EXPORT_ROWS + 1) as i64)
        .fetch_all(&state.pool)
        .await?;
    let rows: Vec<IncludedRow> = rows
        .into_iter()
        .map(|row| IncludedRow {
            study_id: row.get("study_id"),
            study_title: row.get("study_title"),
            study_design: row.get("study_design"),
            relationship: row.get("relationship"),
            report_id: row.get("report_id"),
            report_title: row.get("report_title"),
            doi: row.get("doi"),
            publication_year: row.get("publication_year"),
            title_abstract_decision: row.get("title_abstract_decision"),
            full_text_decision: row.get("full_text_decision"),
            screening_status: row.get("screening_status"),
        })
        .collect();
    enforce_export_cap("included studies", rows.len())?;
    Ok(render_included_csv(&rows))
}

fn render_extraction_csv(rows: &[ExtractionRow]) -> String {
    let mut csv = csv_output::header(EXTRACTION_COLUMNS);
    for row in rows {
        csv.push_str(&csv_output::record(&extraction_fields(row)));
    }
    csv
}

fn extraction_fields(row: &ExtractionRow) -> Vec<String> {
    let status = extraction_status(row);
    let has_value = row.value_id.is_some();
    let ai_proposed = row.approved_by_kind.as_deref() == Some("automation");
    let confirmed = status == "confirmed";
    vec![
        row.study_id.to_string(),
        row.study_title.clone(),
        optional(row.report_id.map(|id| id.to_string())),
        row.field_key.clone(),
        row.field_label.clone(),
        row.field_type.clone(),
        optional(row.value.clone()),
        status.to_owned(),
        if has_value {
            ai_proposed.to_string()
        } else {
            String::new()
        },
        if has_value {
            actor_label(
                row.approved_by_kind.as_deref(),
                row.approved_by_id.as_deref(),
            )
        } else {
            String::new()
        },
        if confirmed {
            actor_label(
                row.verified_by_kind.as_deref(),
                row.verified_by_id.as_deref(),
            )
        } else {
            String::new()
        },
        if confirmed {
            optional(row.verified_at.map(|at| at.to_rfc3339()))
        } else {
            String::new()
        },
        optional(row.source_page.map(|page| page.to_string())),
        optional(
            row.source_quote
                .as_deref()
                .map(|quote| truncate_chars(quote, QUOTE_CHARS)),
        ),
        optional(row.rationale.clone()),
        optional(row.value_id.map(|id| id.to_string())),
    ]
}

/// `not_extracted` has no value. `to_verify` is an AI value waiting for a person. `confirmed`
/// is an AI value a person has confirmed. `entered` is a value a person entered directly.
fn extraction_status(row: &ExtractionRow) -> &'static str {
    if row.value_id.is_none() {
        "not_extracted"
    } else if row.needs_verification {
        "to_verify"
    } else if row.verified_at.is_some() {
        "confirmed"
    } else {
        "entered"
    }
}

fn render_appraisal_csv(assessments: &[AssessmentRow], evidence: &[EvidenceRow]) -> String {
    let definitions = all_appraisal_definitions().unwrap_or_default();
    let mut evidence_by_question: BTreeMap<(Uuid, &str), Vec<&EvidenceRow>> = BTreeMap::new();
    for item in evidence {
        evidence_by_question
            .entry((item.assessment_id, item.question_id.as_str()))
            .or_default()
            .push(item);
    }
    let mut csv = csv_output::header(APPRAISAL_COLUMNS);
    for assessment in assessments {
        let definition = definitions.iter().find(|definition| {
            definition.id.as_str() == assessment.definition_id
                && i32::try_from(definition.version.get()).ok()
                    == Some(assessment.definition_version)
        });
        let tool = ToolIdentity::of(assessment, definition);
        for item in appraisal_items(assessment, definition, &evidence_by_question) {
            let mut fields = vec![
                assessment.report_id.to_string(),
                assessment.report_title.clone(),
                assessment.screening_status.clone(),
                assessment.assessment_id.to_string(),
                assessment.completed_at.to_rfc3339(),
                actor_label(
                    Some(assessment.actor_kind.as_str()),
                    Some(assessment.actor_id.as_str()),
                ),
                tool.id.clone(),
                tool.name.clone(),
                tool.version.clone(),
            ];
            fields.extend(item);
            csv.push_str(&csv_output::record(&fields));
        }
    }
    csv
}

/// Which tool produced an assessment. An assessment made with a definition that is no longer
/// shipped keeps its stored identifier as the tool name.
struct ToolIdentity {
    id: String,
    name: String,
    version: String,
}

impl ToolIdentity {
    fn of(assessment: &AssessmentRow, definition: Option<&AppraisalDefinition>) -> Self {
        match definition {
            Some(definition) => Self {
                id: definition.id.as_str().to_owned(),
                name: definition.name.clone(),
                version: definition.version.get().to_string(),
            },
            None => Self {
                id: assessment.definition_id.clone(),
                name: assessment.definition_id.clone(),
                version: assessment.definition_version.to_string(),
            },
        }
    }
}

/// The per-item columns of `appraisal.csv`, after the report and tool columns: every question
/// answered, then each domain judgment, then the overall judgment.
fn appraisal_items(
    assessment: &AssessmentRow,
    definition: Option<&AppraisalDefinition>,
    evidence_by_question: &BTreeMap<(Uuid, &str), Vec<&EvidenceRow>>,
) -> Vec<Vec<String>> {
    let mut items = Vec::new();
    let responses = object_entries(&assessment.responses);
    let mut answered = Vec::new();
    if let Some(definition) = definition {
        for domain in &definition.domains {
            for question in &domain.questions {
                let Some(answer) = responses.get(&question.id) else {
                    continue;
                };
                answered.push(question.id.clone());
                let evidence = evidence_by_question
                    .get(&(assessment.assessment_id, question.id.as_str()))
                    .map(Vec::as_slice)
                    .unwrap_or_default();
                let (pages, quotes) = evidence_columns(evidence);
                items.push(item_fields(
                    "question",
                    (&domain.id, &domain.label),
                    (&question.id, &question.label),
                    (
                        &answer_text(answer),
                        &answer_label(Some(&question.answer_schema), answer),
                    ),
                    (&pages, &quotes),
                ));
            }
        }
    }
    for (question_id, answer) in &responses {
        if answered.contains(question_id) {
            continue;
        }
        let evidence = evidence_by_question
            .get(&(assessment.assessment_id, question_id.as_str()))
            .map(Vec::as_slice)
            .unwrap_or_default();
        let (pages, quotes) = evidence_columns(evidence);
        items.push(item_fields(
            "question",
            ("", ""),
            (question_id, question_id),
            (&answer_text(answer), &answer_label(None, answer)),
            (&pages, &quotes),
        ));
    }

    let judgments = &assessment.judgments;
    let domain_judgments = judgments
        .get("domains")
        .map(object_entries)
        .unwrap_or_default();
    let mut judged = Vec::new();
    if let Some(definition) = definition {
        for domain in &definition.domains {
            let Some(judgment) = domain_judgments.get(&domain.id) else {
                continue;
            };
            judged.push(domain.id.clone());
            items.push(item_fields(
                "domain_judgment",
                (&domain.id, &domain.label),
                (&domain.id, &format!("Judgment: {}", domain.label)),
                (
                    &answer_text(judgment),
                    &option_label(&domain.judgment.options, judgment),
                ),
                ("", ""),
            ));
        }
    }
    for (domain_id, judgment) in &domain_judgments {
        if judged.contains(domain_id) {
            continue;
        }
        items.push(item_fields(
            "domain_judgment",
            (domain_id, domain_id),
            (domain_id, &format!("Judgment: {domain_id}")),
            (&answer_text(judgment), ""),
            ("", ""),
        ));
    }
    if let Some(overall) = judgments.get("overall") {
        let label = definition
            .map(|definition| option_label(&definition.overall_judgment.options, overall))
            .unwrap_or_default();
        items.push(item_fields(
            "overall_judgment",
            ("", ""),
            ("overall", "Overall judgment"),
            (&answer_text(overall), &label),
            ("", ""),
        ));
    }
    items
}

/// One appraisal item as the columns after the tool columns. Each pair is (identifier, label),
/// (answer, answer label) or (pages, quotes).
fn item_fields(
    item_type: &str,
    domain: (&str, &str),
    item: (&str, &str),
    answer: (&str, &str),
    evidence: (&str, &str),
) -> Vec<String> {
    vec![
        item_type.to_owned(),
        domain.0.to_owned(),
        domain.1.to_owned(),
        item.0.to_owned(),
        item.1.to_owned(),
        answer.0.to_owned(),
        answer.1.to_owned(),
        evidence.0.to_owned(),
        evidence.1.to_owned(),
    ]
}

/// Pages are listed as `2; 5`. Quotes carry their page, as `p. 2: text | p. 5: text`.
fn evidence_columns(evidence: &[&EvidenceRow]) -> (String, String) {
    let pages = evidence
        .iter()
        .filter_map(|item| item.page)
        .map(|page| page.to_string())
        .collect::<Vec<_>>()
        .join("; ");
    let quotes = evidence
        .iter()
        .filter_map(|item| {
            item.quote.as_deref().map(|quote| {
                let page = item
                    .page
                    .map_or_else(String::new, |page| format!("p. {page}: "));
                format!("{page}{}", truncate_chars(quote, QUOTE_CHARS))
            })
        })
        .collect::<Vec<_>>()
        .join(" | ");
    (pages, quotes)
}

fn render_included_csv(rows: &[IncludedRow]) -> String {
    let mut csv = csv_output::header(INCLUDED_COLUMNS);
    for row in rows {
        csv.push_str(&csv_output::record(&[
            optional(row.study_id.map(|id| id.to_string())),
            optional(row.study_title.clone()),
            optional(row.study_design.clone()),
            optional(row.relationship.clone()),
            row.report_id.to_string(),
            optional(row.report_title.clone()),
            optional(row.doi.clone()),
            optional(row.publication_year.map(|year| year.to_string())),
            row.title_abstract_decision.clone(),
            row.full_text_decision.clone(),
            row.screening_status.clone(),
        ]));
    }
    csv
}

/// The entries of a JSON object, or nothing when the value is not an object.
fn object_entries(value: &Value) -> BTreeMap<String, Value> {
    value
        .as_object()
        .map(|object| {
            object
                .iter()
                .map(|(key, value)| (key.clone(), value.clone()))
                .collect()
        })
        .unwrap_or_default()
}

fn answer_text(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        Value::Bool(flag) => flag.to_string(),
        Value::Number(number) => number.to_string(),
        Value::Null => String::new(),
        other => other.to_string(),
    }
}

fn answer_label(schema: Option<&AnswerSchema>, answer: &Value) -> String {
    match schema {
        Some(AnswerSchema::Enum { options }) => option_label(options, answer),
        Some(AnswerSchema::Boolean) => match answer {
            Value::Bool(true) => "Yes".to_owned(),
            Value::Bool(false) => "No".to_owned(),
            _ => String::new(),
        },
        Some(AnswerSchema::Scale { labels, .. }) => labels
            .get(&answer_text(answer))
            .cloned()
            .unwrap_or_default(),
        Some(AnswerSchema::Text { .. }) | None => String::new(),
    }
}

fn option_label(options: &[AnswerOption], value: &Value) -> String {
    let value = answer_text(value);
    options
        .iter()
        .find(|option| option.value == value)
        .map(|option| option.label.clone())
        .unwrap_or_default()
}

/// Trimmed text cut to `limit` characters (not bytes), with an ellipsis when it was cut.
fn truncate_chars(text: &str, limit: usize) -> String {
    let text = text.trim();
    if text.chars().count() <= limit {
        return text.to_owned();
    }
    let mut truncated: String = text.chars().take(limit).collect();
    truncated.push('…');
    truncated
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone;
    use serde_json::json;

    use super::*;

    fn csv_rows(text: &str) -> Vec<Vec<String>> {
        ::csv::ReaderBuilder::new()
            .has_headers(false)
            .from_reader(text.as_bytes())
            .records()
            .map(|row| row.map(|row| row.iter().map(str::to_owned).collect::<Vec<String>>()))
            .collect::<Result<Vec<_>, _>>()
            .unwrap_or_default()
    }

    /// The value of a named column in a parsed row, or an empty string.
    fn cell(row: &[String], columns: &[&str], name: &str) -> String {
        columns
            .iter()
            .position(|column| *column == name)
            .and_then(|index| row.get(index))
            .cloned()
            .unwrap_or_default()
    }

    fn at(hour: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 10, 8, hour, 0, 0)
            .single()
            .unwrap_or_else(Utc::now)
    }

    fn study_id() -> Uuid {
        Uuid::from_u128(0x1111_1111_1111_1111_1111_1111_1111_1111)
    }

    fn extraction_row() -> ExtractionRow {
        ExtractionRow {
            study_id: study_id(),
            study_title: "Cadmus-Bertram 2015, \"women\"".to_owned(),
            field_key: "sample_size".to_owned(),
            field_label: "Sample size (n)".to_owned(),
            field_type: "number".to_owned(),
            value_id: Some(Uuid::from_u128(0x2)),
            report_id: Some(Uuid::from_u128(0x3)),
            value: Some("51".to_owned()),
            needs_verification: false,
            approved_by_kind: Some("user".to_owned()),
            approved_by_id: Some("local-user".to_owned()),
            verified_at: None,
            verified_by_kind: None,
            verified_by_id: None,
            source_page: Some(3),
            source_quote: Some(
                "Fifty-one women\nwere randomised, a multi-line quote é.".to_owned(),
            ),
            rationale: Some("Stated in the methods, \"Participants\".".to_owned()),
        }
    }

    #[test]
    fn extraction_header_lists_every_column_in_order() {
        assert_eq!(
            render_extraction_csv(&[]),
            csv_output::header(EXTRACTION_COLUMNS)
        );
        assert_eq!(
            EXTRACTION_COLUMNS.first().copied(),
            Some("study_id"),
            "study identity leads the row"
        );
    }

    #[test]
    fn extraction_row_carries_value_status_actors_source_and_escaping() {
        let rows = csv_rows(&render_extraction_csv(&[extraction_row()]));
        assert_eq!(rows.len(), 2);
        let row = &rows[1];
        assert_eq!(
            cell(row, EXTRACTION_COLUMNS, "study_title"),
            "Cadmus-Bertram 2015, \"women\""
        );
        assert_eq!(cell(row, EXTRACTION_COLUMNS, "field_key"), "sample_size");
        assert_eq!(
            cell(row, EXTRACTION_COLUMNS, "field_label"),
            "Sample size (n)"
        );
        assert_eq!(cell(row, EXTRACTION_COLUMNS, "field_type"), "number");
        assert_eq!(cell(row, EXTRACTION_COLUMNS, "value"), "51");
        assert_eq!(cell(row, EXTRACTION_COLUMNS, "status"), "entered");
        assert_eq!(cell(row, EXTRACTION_COLUMNS, "ai_proposed"), "false");
        assert_eq!(cell(row, EXTRACTION_COLUMNS, "entered_by"), "Local user");
        assert_eq!(cell(row, EXTRACTION_COLUMNS, "confirmed_by"), "");
        assert_eq!(cell(row, EXTRACTION_COLUMNS, "source_page"), "3");
        assert_eq!(
            cell(row, EXTRACTION_COLUMNS, "source_quote"),
            "Fifty-one women\nwere randomised, a multi-line quote é."
        );
        assert_eq!(
            cell(row, EXTRACTION_COLUMNS, "rationale"),
            "Stated in the methods, \"Participants\"."
        );
        assert_eq!(
            cell(row, EXTRACTION_COLUMNS, "value_id"),
            Uuid::from_u128(0x2).to_string()
        );
    }

    #[test]
    fn extraction_status_covers_ai_to_verify_confirmed_entered_and_missing() {
        let mut ai = extraction_row();
        ai.approved_by_kind = Some("automation".to_owned());
        ai.approved_by_id = Some("ai:glm-5.3-flash".to_owned());
        ai.needs_verification = true;
        assert_eq!(extraction_status(&ai), "to_verify");
        let fields = extraction_fields(&ai);
        assert_eq!(fields[8], "true", "ai_proposed");
        assert_eq!(fields[9], "AI model glm-5.3-flash", "entered_by");
        assert_eq!(fields[10], "", "nobody has confirmed it yet");

        ai.needs_verification = false;
        ai.verified_at = Some(at(9));
        ai.verified_by_kind = Some("user".to_owned());
        ai.verified_by_id = Some("reviewer-1".to_owned());
        assert_eq!(extraction_status(&ai), "confirmed");
        let fields = extraction_fields(&ai);
        assert_eq!(fields[10], "User reviewer-1", "confirmed_by");
        assert_eq!(fields[11], "2026-10-08T09:00:00+00:00", "confirmed_at");

        let mut missing = extraction_row();
        missing.value_id = None;
        missing.value = None;
        assert_eq!(extraction_status(&missing), "not_extracted");
        let fields = extraction_fields(&missing);
        assert_eq!(fields[6], "", "no value");
        assert_eq!(fields[8], "", "no ai_proposed flag without a value");
        assert_eq!(fields[15], "", "no value id");
    }

    #[test]
    fn source_quotes_are_truncated_by_characters_not_bytes() {
        let long = "é".repeat(QUOTE_CHARS + 10);
        let cut = truncate_chars(&long, QUOTE_CHARS);
        assert_eq!(cut.chars().count(), QUOTE_CHARS + 1);
        assert!(cut.ends_with('…'));
        assert_eq!(truncate_chars("  short  ", QUOTE_CHARS), "short");
    }

    fn assessment(definition_id: &str, version: i32) -> AssessmentRow {
        AssessmentRow {
            report_id: Uuid::from_u128(0x10),
            report_title: "Gait study: \"pilot\", 2015".to_owned(),
            screening_status: "include".to_owned(),
            assessment_id: Uuid::from_u128(0x20),
            definition_id: definition_id.to_owned(),
            definition_version: version,
            responses: json!({
                "allocation_description": "yes",
                "outcome_measure_prespecified": true
            }),
            judgments: json!({
                "domains": {"allocation": "low_concern", "outcome_reporting": "some_concern"},
                "overall": "some_concern"
            }),
            completed_at: at(10),
            actor_kind: "user".to_owned(),
            actor_id: "local-user".to_owned(),
        }
    }

    fn evidence_for(assessment_id: Uuid) -> Vec<EvidenceRow> {
        vec![EvidenceRow {
            assessment_id,
            question_id: "allocation_description".to_owned(),
            page: Some(4),
            quote: Some("Participants were randomised by a computer, 1:1.".to_owned()),
        }]
    }

    #[test]
    #[allow(clippy::panic_in_result_fn)]
    fn appraisal_header_matches_columns_and_known_tool_is_named() -> anyhow::Result<()> {
        let rows = assessment("deepref-rct-generic", 1);
        let evidence = evidence_for(rows.assessment_id);
        let csv = render_appraisal_csv(std::slice::from_ref(&rows), &evidence);
        let parsed = csv_rows(&csv);
        assert_eq!(parsed[0], APPRAISAL_COLUMNS);
        let column = |name: &str| {
            APPRAISAL_COLUMNS
                .iter()
                .position(|column| *column == name)
                .unwrap_or(usize::MAX)
        };
        let question = parsed
            .iter()
            .find(|row| {
                row.get(column("item_id")).map(String::as_str) == Some("allocation_description")
            })
            .ok_or_else(|| anyhow::anyhow!("question row is missing"))?;
        assert_eq!(
            question[column("report_title")],
            "Gait study: \"pilot\", 2015"
        );
        assert_eq!(question[column("assessor")], "Local user");
        assert_eq!(question[column("tool_id")], "deepref-rct-generic");
        assert_eq!(
            question[column("tool_name")],
            "DeepRef generic intervention appraisal"
        );
        assert_eq!(question[column("tool_version")], "1");
        assert_eq!(question[column("item_type")], "question");
        assert_eq!(question[column("domain_id")], "allocation");
        assert_eq!(question[column("answer")], "yes");
        assert_eq!(question[column("answer_label")], "Yes");
        assert_eq!(question[column("evidence_pages")], "4");
        assert_eq!(
            question[column("evidence_quotes")],
            "p. 4: Participants were randomised by a computer, 1:1."
        );
        let boolean = parsed
            .iter()
            .find(|row| {
                row.get(column("item_id")).map(String::as_str)
                    == Some("outcome_measure_prespecified")
            })
            .ok_or_else(|| anyhow::anyhow!("boolean question row is missing"))?;
        assert_eq!(boolean[column("answer")], "true");
        assert_eq!(boolean[column("answer_label")], "Yes");
        let domain = parsed
            .iter()
            .find(|row| {
                row.get(column("item_type")).map(String::as_str) == Some("domain_judgment")
                    && row.get(column("item_id")).map(String::as_str) == Some("outcome_reporting")
            })
            .ok_or_else(|| anyhow::anyhow!("domain judgment row is missing"))?;
        assert_eq!(domain[column("answer")], "some_concern");
        assert_eq!(domain[column("answer_label")], "Some concern");
        assert_eq!(domain[column("domain_label")], "Outcome reporting");
        let overall = parsed
            .iter()
            .find(|row| {
                row.get(column("item_type")).map(String::as_str) == Some("overall_judgment")
            })
            .ok_or_else(|| anyhow::anyhow!("overall judgment row is missing"))?;
        assert_eq!(overall[column("item_label")], "Overall judgment");
        assert_eq!(overall[column("answer_label")], "Some concern");
        assert_eq!(
            parsed.len(),
            1 + 2 + 2 + 1,
            "two questions, two domains and overall"
        );
        Ok(())
    }

    #[test]
    #[allow(clippy::panic_in_result_fn)]
    fn appraisal_with_retired_definition_keeps_stored_identifiers() -> anyhow::Result<()> {
        let rows = assessment("export-fixture", 1);
        let csv = render_appraisal_csv(std::slice::from_ref(&rows), &[]);
        let parsed = csv_rows(&csv);
        assert_eq!(parsed[0].len(), APPRAISAL_COLUMNS.len());
        assert!(
            parsed.len() > 1,
            "answers are still exported without a definition"
        );
        let first = &parsed[1];
        assert_eq!(first[6], "export-fixture", "tool_id");
        assert_eq!(
            first[7], "export-fixture",
            "tool_name falls back to the stored id"
        );
        assert_eq!(first[8], "1", "tool_version");
        Ok(())
    }

    #[test]
    fn appraisal_quotes_with_commas_and_newlines_round_trip() {
        let rows = assessment("deepref-rct-generic", 1);
        let evidence = vec![EvidenceRow {
            assessment_id: rows.assessment_id,
            question_id: "allocation_description".to_owned(),
            page: Some(2),
            quote: Some("Random, \"concealed\"\nallocation.".to_owned()),
        }];
        let parsed = csv_rows(&render_appraisal_csv(
            std::slice::from_ref(&rows),
            &evidence,
        ));
        let quotes_index = APPRAISAL_COLUMNS
            .iter()
            .position(|column| *column == "evidence_quotes");
        let quotes = quotes_index
            .and_then(|index| parsed.get(1).and_then(|row| row.get(index)))
            .map(String::as_str);
        assert_eq!(quotes, Some("p. 2: Random, \"concealed\"\nallocation."));
    }

    #[test]
    fn included_studies_header_and_rows_cover_grouped_and_ungrouped_reports() {
        assert_eq!(
            render_included_csv(&[]),
            csv_output::header(INCLUDED_COLUMNS)
        );
        let grouped = IncludedRow {
            study_id: Some(study_id()),
            study_title: Some("Cadmus-Bertram 2015".to_owned()),
            study_design: Some("rct".to_owned()),
            relationship: Some("report_of_study".to_owned()),
            report_id: Uuid::from_u128(0x30),
            report_title: Some("Effects of \"self-monitoring\"".to_owned()),
            doi: Some("10.1000/x".to_owned()),
            publication_year: Some(2015),
            title_abstract_decision: "include".to_owned(),
            full_text_decision: "include".to_owned(),
            screening_status: "include".to_owned(),
        };
        let ungrouped = IncludedRow {
            study_id: None,
            study_title: None,
            study_design: None,
            relationship: None,
            report_id: Uuid::from_u128(0x31),
            report_title: Some("Lone report".to_owned()),
            doi: None,
            publication_year: None,
            title_abstract_decision: "include".to_owned(),
            full_text_decision: "include".to_owned(),
            screening_status: "include".to_owned(),
        };
        let parsed = csv_rows(&render_included_csv(&[grouped, ungrouped]));
        assert_eq!(parsed.len(), 3);
        assert_eq!(parsed[0], INCLUDED_COLUMNS);
        assert_eq!(parsed[1][0], study_id().to_string());
        assert_eq!(parsed[1][3], "report_of_study");
        assert_eq!(parsed[1][5], "Effects of \"self-monitoring\"");
        assert_eq!(parsed[1][7], "2015");
        assert_eq!(parsed[2][0], "", "an ungrouped report has no study");
        assert_eq!(parsed[2][5], "Lone report");
    }
}
