//! Project-scoped reads used by the controlled agent tool adapter.
//!
//! The queries in this module deliberately accept a project scope on every
//! lookup. The HTTP application owns `AppState` and may call this scoped
//! adapter; the agent runtime and tool request never receive SQL, a `PgPool`,
//! or an unrestricted repository.

use chrono::{DateTime, Utc};
use serde::Serialize;
use serde_json::Value;
use sqlx::{PgPool, Row};
use thiserror::Error;
use uuid::Uuid;

#[derive(Debug, Error)]
pub enum AgentReadError {
    #[error("database operation failed")]
    Database(#[from] sqlx::Error),
    #[error("scoped resource was not found")]
    NotFound,
    #[error("stored agent read data is invalid")]
    InvalidData,
}

#[derive(Debug, Clone, Serialize)]
pub struct AgentReportIdentifier {
    pub scheme: String,
    pub value: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct AgentReportRecord {
    pub id: Uuid,
    pub project_id: Uuid,
    pub title: Option<String>,
    pub abstract_text: Option<String>,
    pub publication_year: Option<i32>,
    pub journal: Option<String>,
    pub url: Option<String>,
    pub identifiers: Vec<AgentReportIdentifier>,
    /// The report's documents. Only `get_agent_report` fills this in; search
    /// results leave it out to stay small.
    pub documents: Option<Vec<AgentDocumentSummary>>,
}

#[derive(Debug, Clone, Serialize)]
pub struct AgentDocumentSummary {
    pub id: Uuid,
    /// `missing`, `external`, `uploaded`, `retrieving`, `available` or `failed`.
    /// Only `available` documents have readable full text.
    pub status: String,
    pub source: String,
    pub original_filename: Option<String>,
    /// Active parsed blocks; zero until the document has been parsed.
    pub block_count: i64,
    /// Outline of the parsed document, in reading order (empty when not parsed).
    pub sections: Vec<AgentSectionSummary>,
}

#[derive(Debug, Clone, Serialize)]
pub struct AgentSectionSummary {
    pub number: Option<String>,
    pub title: String,
    pub depth: i32,
}

#[derive(Debug, Clone, Serialize)]
pub struct AgentDocumentBlockRecord {
    pub id: Uuid,
    pub document_id: Uuid,
    pub page_number: i32,
    pub kind: String,
    pub section_path: Vec<String>,
    pub ordinal: i32,
    pub text: String,
    pub content_hash: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct AgentAppraisalRecord {
    pub id: Uuid,
    pub project_id: Uuid,
    pub report_id: Uuid,
    pub definition_id: String,
    pub definition_version: i32,
    pub responses: Value,
    pub judgments: Value,
    pub evidence: Vec<AgentAppraisalEvidence>,
    pub completed_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize)]
pub struct AgentAppraisalEvidence {
    pub question_id: String,
    pub document_id: Uuid,
    pub block_id: Uuid,
}

pub async fn project_exists(pool: &PgPool, project_id: Uuid) -> Result<bool, sqlx::Error> {
    sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM projects WHERE id=$1)")
        .bind(project_id)
        .fetch_one(pool)
        .await
}

pub async fn get_agent_report(
    pool: &PgPool,
    project_id: Uuid,
    report_id: Uuid,
) -> Result<AgentReportRecord, AgentReadError> {
    let row = sqlx::query(
        "SELECT r.id,pr.project_id,r.title,r.abstract_text,r.publication_year,r.journal,r.url
         FROM project_reports pr
         JOIN reports r ON r.id=pr.report_id
         WHERE pr.project_id=$1 AND pr.report_id=$2",
    )
    .bind(project_id)
    .bind(report_id)
    .fetch_optional(pool)
    .await?
    .ok_or(AgentReadError::NotFound)?;
    let identifiers = sqlx::query(
        "SELECT scheme,value FROM report_identifiers
         WHERE report_id=$1 ORDER BY scheme,value LIMIT 32",
    )
    .bind(report_id)
    .fetch_all(pool)
    .await?
    .into_iter()
    .map(|identifier| AgentReportIdentifier {
        scheme: identifier.get("scheme"),
        value: identifier.get("value"),
    })
    .collect();
    let documents = list_agent_report_documents(pool, project_id, report_id).await?;
    Ok(AgentReportRecord {
        id: row.get("id"),
        project_id: row.get("project_id"),
        title: decoded(row.get("title")),
        abstract_text: decoded(row.get("abstract_text")),
        publication_year: row.get("publication_year"),
        journal: row.get("journal"),
        url: row.get("url"),
        identifiers,
        documents: Some(documents),
    })
}

/// The documents of one report with their parse status, block count and
/// section outline. Bounded: at most ten documents and forty sections each.
async fn list_agent_report_documents(
    pool: &PgPool,
    project_id: Uuid,
    report_id: Uuid,
) -> Result<Vec<AgentDocumentSummary>, AgentReadError> {
    let rows = sqlx::query(
        "SELECT d.id,d.status,d.source,d.original_filename,d.active_parser_version,
                (SELECT count(*) FROM document_blocks b
                  WHERE b.document_id=d.id AND b.active AND b.parser_version=d.active_parser_version)
                  AS block_count
         FROM documents d
         WHERE d.project_id=$1 AND d.report_id=$2
         ORDER BY d.created_at DESC,d.id DESC LIMIT 10",
    )
    .bind(project_id)
    .bind(report_id)
    .fetch_all(pool)
    .await?;
    let mut documents = Vec::with_capacity(rows.len());
    for row in rows {
        let id: Uuid = row.get("id");
        let sections = match row.get::<Option<String>, _>("active_parser_version") {
            Some(parser_version) => sqlx::query(
                "SELECT number,title,depth FROM document_sections
                 WHERE document_id=$1 AND parser_version=$2 ORDER BY ordinal LIMIT 40",
            )
            .bind(id)
            .bind(parser_version)
            .fetch_all(pool)
            .await?
            .into_iter()
            .map(|section| AgentSectionSummary {
                number: section.get("number"),
                title: decoded(Some(section.get::<String, _>("title"))).unwrap_or_default(),
                depth: section.get("depth"),
            })
            .collect(),
            None => Vec::new(),
        };
        documents.push(AgentDocumentSummary {
            id,
            status: row.get("status"),
            source: row.get("source"),
            original_filename: row.get("original_filename"),
            block_count: row.get("block_count"),
            sections,
        });
    }
    Ok(documents)
}

/// Turns a question into OR-ed key terms for full-text search, the same way
/// the retrieval path does: short and common words are dropped, duplicates are
/// removed and at most twelve words are kept. Each word also matches its other
/// spelling (-ise and -ize), because sources mix the two. `None` when nothing
/// searchable is left.
fn report_search_query(raw: &str) -> Option<String> {
    const STOP_WORDS: &[&str] = &[
        "the", "and", "for", "with", "from", "that", "this", "are", "was", "were", "how", "many",
        "what", "which", "does", "did", "have", "has", "into", "over", "about", "per", "para",
        "com", "uma", "dos", "das", "que", "por", "como", "sobre", "nos", "nas", "n\u{e3}o",
    ];
    let words: Vec<String> = raw
        .split(|character: char| !character.is_alphanumeric())
        .map(str::to_lowercase)
        .filter(|term| term.chars().count() >= 3 && !STOP_WORDS.contains(&term.as_str()))
        .take(12)
        .collect();
    let mut seen = std::collections::BTreeSet::new();
    let mut terms = Vec::new();
    for word in &words {
        for variant in spelling_variants(word) {
            if seen.insert(variant.clone()) {
                terms.push(variant);
            }
        }
    }
    (!terms.is_empty()).then(|| terms.join(" or "))
}

/// The word itself and its other common spelling: -ise/-ize, -isation/-ization
/// and their inflections.
fn spelling_variants(word: &str) -> Vec<String> {
    const PAIRS: &[(&str, &str)] = &[
        ("isation", "ization"),
        ("ising", "izing"),
        ("ised", "ized"),
        ("ises", "izes"),
        ("ise", "ize"),
    ];
    let mut variants = vec![word.to_owned()];
    for (british, american) in PAIRS {
        let other = word
            .strip_suffix(british)
            .map(|stem| format!("{stem}{american}"))
            .or_else(|| {
                word.strip_suffix(american)
                    .map(|stem| format!("{stem}{british}"))
            });
        if let Some(other) = other {
            variants.push(other);
            break;
        }
    }
    variants
}

/// Finds reports whose title, abstract or authors contain any of the key terms
/// in `query`, best matches first. Multi-word questions therefore match.
pub async fn search_agent_reports(
    pool: &PgPool,
    project_id: Uuid,
    query: &str,
    limit: i64,
) -> Result<Vec<AgentReportRecord>, AgentReadError> {
    let Some(terms) = report_search_query(query) else {
        return Ok(Vec::new());
    };
    let rows = sqlx::query(
        "WITH q AS (SELECT websearch_to_tsquery('simple',$2) AS query),
         docs AS (
           SELECT r.id,pr.project_id,r.title,r.abstract_text,r.publication_year,r.journal,r.url,
                  r.updated_at,
                  setweight(to_tsvector('simple',coalesce(r.title,'')),'A')
                  || setweight(to_tsvector('simple',coalesce(r.abstract_text,'')),'B')
                  || setweight(to_tsvector('simple',coalesce((
                       SELECT string_agg(concat_ws(' ',a->>'given',a->>'family',a->>'literal'),' ')
                       FROM jsonb_array_elements(
                         CASE WHEN jsonb_typeof(r.authors)='array' THEN r.authors ELSE '[]'::jsonb END) a
                     ),'')),'C') AS doc
           FROM project_reports pr
           JOIN reports r ON r.id=pr.report_id
           WHERE pr.project_id=$1
         )
         SELECT d.id,d.project_id,d.title,d.abstract_text,d.publication_year,d.journal,d.url,
                ts_rank_cd(d.doc,q.query) AS score
         FROM docs d CROSS JOIN q
         WHERE d.doc @@ q.query
         ORDER BY score DESC,d.updated_at DESC,d.id DESC LIMIT $3",
    )
    .bind(project_id)
    .bind(terms)
    .bind(limit.clamp(1, 100))
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|row| AgentReportRecord {
            id: row.get("id"),
            project_id: row.get("project_id"),
            title: decoded(row.get("title")),
            abstract_text: decoded(row.get("abstract_text")),
            publication_year: row.get("publication_year"),
            journal: row.get("journal"),
            url: row.get("url"),
            identifiers: Vec::new(),
            documents: None,
        })
        .collect())
}

fn decoded(value: Option<String>) -> Option<String> {
    value.map(|text| decode_html_entities(&text))
}

/// Decodes the HTML entities that bibliographic sources leave in titles (for
/// example `&amp;`), so the model and the user see the plain character.
pub fn decode_html_entities(value: &str) -> String {
    if !value.contains('&') {
        return value.to_owned();
    }
    let mut output = String::with_capacity(value.len());
    let mut rest = value;
    while let Some((before, after)) = rest.split_once('&') {
        output.push_str(before);
        let decoded = after
            .split_once(';')
            .filter(|(name, _)| name.len() <= 11)
            .and_then(|(name, tail)| decode_entity(name).map(|character| (character, tail)));
        match decoded {
            Some((character, tail)) => {
                output.push(character);
                rest = tail;
            }
            None => {
                output.push('&');
                rest = after;
            }
        }
    }
    output.push_str(rest);
    output
}

fn decode_entity(name: &str) -> Option<char> {
    match name {
        "amp" => Some('&'),
        "lt" => Some('<'),
        "gt" => Some('>'),
        "quot" => Some('"'),
        "apos" => Some('\''),
        "nbsp" => Some(' '),
        _ => {
            let number = match name.strip_prefix("#x").or_else(|| name.strip_prefix("#X")) {
                Some(hex) => u32::from_str_radix(hex, 16).ok()?,
                None => name.strip_prefix('#')?.parse::<u32>().ok()?,
            };
            char::from_u32(number)
        }
    }
}

pub async fn read_agent_document_blocks(
    pool: &PgPool,
    project_id: Uuid,
    document_id: Uuid,
    block_ids: &[Uuid],
) -> Result<Vec<AgentDocumentBlockRecord>, AgentReadError> {
    let document_exists = sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS(SELECT 1 FROM documents WHERE project_id=$1 AND id=$2)",
    )
    .bind(project_id)
    .bind(document_id)
    .fetch_one(pool)
    .await?;
    if !document_exists {
        return Err(AgentReadError::NotFound);
    }
    let rows = sqlx::query(
        "SELECT b.id,b.document_id,b.page_number,b.kind,b.section_path,b.ordinal,b.text,b.content_hash
         FROM document_blocks b
         JOIN documents d ON d.id=b.document_id
         WHERE d.project_id=$1 AND b.document_id=$2 AND b.id=ANY($3::uuid[])
           AND b.active AND b.parser_version=d.active_parser_version
         ORDER BY b.page_number,b.ordinal,b.id",
    )
    .bind(project_id)
    .bind(document_id)
    .bind(block_ids)
    .fetch_all(pool)
    .await?;
    if rows.len() != block_ids.len() {
        return Err(AgentReadError::NotFound);
    }
    Ok(rows
        .into_iter()
        .map(|row| AgentDocumentBlockRecord {
            id: row.get("id"),
            document_id: row.get("document_id"),
            page_number: row.get("page_number"),
            kind: row.get("kind"),
            section_path: row.get("section_path"),
            ordinal: row.get("ordinal"),
            text: row.get("text"),
            content_hash: row.get("content_hash"),
        })
        .collect())
}

pub async fn search_agent_document(
    pool: &PgPool,
    project_id: Uuid,
    document_id: Uuid,
    query: &str,
    limit: i64,
) -> Result<Vec<AgentDocumentBlockRecord>, AgentReadError> {
    let document_exists = sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS(SELECT 1 FROM documents WHERE project_id=$1 AND id=$2)",
    )
    .bind(project_id)
    .bind(document_id)
    .fetch_one(pool)
    .await?;
    if !document_exists {
        return Err(AgentReadError::NotFound);
    }
    // Any of the key terms can match and the best-matching blocks come first,
    // so a question with several words still finds the passage that has most of them.
    let Some(terms) = report_search_query(query) else {
        return Ok(Vec::new());
    };
    let rows = sqlx::query(
        "SELECT b.id,b.document_id,b.page_number,b.kind,b.section_path,b.ordinal,b.text,b.content_hash
         FROM document_blocks b
         JOIN documents d ON d.id=b.document_id
         WHERE d.project_id=$1 AND b.document_id=$2
           AND b.active AND b.parser_version=d.active_parser_version
           AND b.search_vector @@ websearch_to_tsquery('simple',$3)
         ORDER BY ts_rank_cd(b.search_vector,websearch_to_tsquery('simple',$3)) DESC,
                  b.page_number,b.ordinal,b.id LIMIT $4",
    )
    .bind(project_id)
    .bind(document_id)
    .bind(terms)
    .bind(limit.clamp(1, 100))
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|row| AgentDocumentBlockRecord {
            id: row.get("id"),
            document_id: row.get("document_id"),
            page_number: row.get("page_number"),
            kind: row.get("kind"),
            section_path: row.get("section_path"),
            ordinal: row.get("ordinal"),
            text: row.get("text"),
            content_hash: row.get("content_hash"),
        })
        .collect())
}

pub async fn get_agent_screening_state(
    pool: &PgPool,
    project_id: Uuid,
    report_id: Uuid,
) -> Result<crate::screening::ScreeningStateSnapshot, AgentReadError> {
    let row = sqlx::query(
        "SELECT pr.project_id,pr.report_id,CASE WHEN ai_first_audit_masked(pr.project_id,pr.report_id) THEN 'unscreened' ELSE coalesce(ss.title_abstract_status,'unscreened') END AS title_abstract_status,
                CASE WHEN ai_first_audit_masked(pr.project_id,pr.report_id) THEN 'not_required' ELSE coalesce(ss.full_text_status,'not_required') END AS full_text_status,
                CASE WHEN ai_first_audit_masked(pr.project_id,pr.report_id) THEN NULL ELSE ss.full_text_exclusion_reason_id END AS full_text_exclusion_reason_id,
                CASE WHEN ai_first_audit_masked(pr.project_id,pr.report_id) THEN 'unscreened' ELSE coalesce(ss.final_status,'unscreened') END AS final_status,
                CASE WHEN ai_first_audit_masked(pr.project_id,pr.report_id) THEN 0 ELSE coalesce(ss.revision,0) END::bigint AS revision, CASE WHEN ai_first_audit_masked(pr.project_id,pr.report_id) THEN NULL ELSE ss.last_event_id END AS last_event_id, CASE WHEN ai_first_audit_masked(pr.project_id,pr.report_id) THEN NULL ELSE ss.updated_at END AS updated_at
         FROM project_reports pr
         LEFT JOIN screening_state ss ON ss.project_id=pr.project_id AND ss.report_id=pr.report_id
         WHERE pr.project_id=$1 AND pr.report_id=$2",
    )
    .bind(project_id)
    .bind(report_id)
    .fetch_optional(pool)
    .await?
    .ok_or(AgentReadError::NotFound)?;
    Ok(crate::screening::ScreeningStateSnapshot {
        project_id: row.get("project_id"),
        report_id: row.get("report_id"),
        title_abstract_status: row.get("title_abstract_status"),
        full_text_status: row.get("full_text_status"),
        full_text_exclusion_reason_id: row.get("full_text_exclusion_reason_id"),
        final_status: row.get("final_status"),
        revision: row.get("revision"),
        last_event_id: row.get("last_event_id"),
        updated_at: row.get("updated_at"),
    })
}

pub async fn get_latest_agent_appraisal(
    pool: &PgPool,
    project_id: Uuid,
    report_id: Uuid,
    definition_id: &str,
    definition_version: i32,
) -> Result<AgentAppraisalRecord, AgentReadError> {
    let row = sqlx::query(
        "SELECT a.id,a.project_id,a.report_id,a.definition_id,a.definition_version,
                a.responses,a.judgments,a.completed_at
         FROM appraisal_assessments a
         JOIN project_reports pr ON pr.project_id=a.project_id AND pr.report_id=a.report_id
         WHERE a.project_id=$1 AND a.report_id=$2 AND a.definition_id=$3
           AND a.definition_version=$4 AND a.completed_at IS NOT NULL
         ORDER BY a.completed_at DESC,a.id DESC LIMIT 1",
    )
    .bind(project_id)
    .bind(report_id)
    .bind(definition_id)
    .bind(definition_version)
    .fetch_optional(pool)
    .await?
    .ok_or(AgentReadError::NotFound)?;
    let assessment_id: Uuid = row.get("id");
    let evidence = sqlx::query(
        "SELECT question_id,document_id,block_id
         FROM appraisal_assessment_evidence WHERE assessment_id=$1 ORDER BY question_id",
    )
    .bind(assessment_id)
    .fetch_all(pool)
    .await?
    .into_iter()
    .map(|evidence| AgentAppraisalEvidence {
        question_id: evidence.get("question_id"),
        document_id: evidence.get("document_id"),
        block_id: evidence.get("block_id"),
    })
    .collect();
    Ok(AgentAppraisalRecord {
        id: assessment_id,
        project_id: row.get("project_id"),
        report_id: row.get("report_id"),
        definition_id: row.get("definition_id"),
        definition_version: row.get("definition_version"),
        responses: row.get("responses"),
        judgments: row.get("judgments"),
        evidence,
        completed_at: row.get("completed_at"),
    })
}

/// Counts the assistant uses to orient itself in a project.
pub async fn get_agent_project_overview(
    pool: &PgPool,
    project_id: Uuid,
) -> Result<Value, AgentReadError> {
    if !project_exists(pool, project_id).await? {
        return Err(AgentReadError::NotFound);
    }
    // Every report is counted once in the title/abstract buckets, which add up
    // to the report total. Full-text counts are a subset of `include`. A report
    // hidden by an active blind audit is counted through the same masked status
    // the queue and reader show, so one projection drives every bucket and a
    // masked record can never land in two of them.
    let row = sqlx::query(
        "SELECT
           count(*) AS reports,
           count(*) FILTER (WHERE m.title_abstract_status='unscreened') AS unscreened,
           count(*) FILTER (WHERE m.title_abstract_status='include') AS ta_include,
           count(*) FILTER (WHERE m.title_abstract_status='exclude') AS ta_exclude,
           count(*) FILTER (WHERE m.title_abstract_status='maybe') AS ta_maybe,
           count(*) FILTER (WHERE m.final_status='pending_full_text') AS awaiting_full_text,
           count(*) FILTER (WHERE m.final_status='pending_full_text' AND EXISTS (
             SELECT 1 FROM documents d
             WHERE d.project_id=pr.project_id AND d.report_id=pr.report_id AND d.status='available'
           )) AS awaiting_with_full_text,
           (SELECT count(*) FROM studies WHERE project_id=$1) AS studies,
           EXISTS(SELECT 1 FROM protocol_versions WHERE project_id=$1 AND status='published') AS protocol_published
         FROM project_reports pr
         LEFT JOIN screening_state s ON s.project_id=pr.project_id AND s.report_id=pr.report_id
         LEFT JOIN LATERAL (
           SELECT CASE WHEN ai_first_audit_masked(pr.project_id,pr.report_id)
                       THEN 'unscreened' ELSE COALESCE(s.title_abstract_status,'unscreened') END AS title_abstract_status,
                  CASE WHEN ai_first_audit_masked(pr.project_id,pr.report_id)
                       THEN 'unscreened' ELSE COALESCE(s.final_status,'unscreened') END AS final_status
         ) m ON true
         WHERE pr.project_id=$1",
    )
    .bind(project_id)
    .fetch_one(pool)
    .await?;
    let awaiting = row.get::<i64, _>("awaiting_full_text");
    let with_full_text = row.get::<i64, _>("awaiting_with_full_text");
    Ok(serde_json::json!({
        "reports": row.get::<i64, _>("reports"),
        "studies": row.get::<i64, _>("studies"),
        "protocol_published": row.get::<bool, _>("protocol_published"),
        "title_abstract": {
            "unscreened": row.get::<i64, _>("unscreened"),
            "include": row.get::<i64, _>("ta_include"),
            "exclude": row.get::<i64, _>("ta_exclude"),
            "maybe": row.get::<i64, _>("ta_maybe"),
        },
        "full_text": {
            "awaiting_decision": awaiting,
            "awaiting_decision_with_full_text": with_full_text,
            "awaiting_decision_without_full_text": awaiting - with_full_text,
        },
        "notes": "The title_abstract buckets are disjoint and add up to reports. full_text counts are a subset of title_abstract.include: included records still waiting for a full-text decision.",
    }))
}

/// Lists the project's studies with their design and how many reports each one
/// groups, oldest first.
pub async fn list_agent_studies(
    pool: &PgPool,
    project_id: Uuid,
    limit: i64,
) -> Result<Value, AgentReadError> {
    let rows = sqlx::query(
        "SELECT s.id,s.title,s.design,s.updated_at,count(sr.report_id) AS report_count
         FROM studies s
         LEFT JOIN study_reports sr ON sr.project_id=s.project_id AND sr.study_id=s.id
         WHERE s.project_id=$1
         GROUP BY s.id
         ORDER BY s.created_at,s.id LIMIT $2",
    )
    .bind(project_id)
    .bind(limit.clamp(1, 50))
    .fetch_all(pool)
    .await?;
    Ok(Value::Array(
        rows.into_iter()
            .map(|row| {
                serde_json::json!({
                    "study_id": row.get::<Uuid, _>("id"),
                    "title": decode_html_entities(&row.get::<String, _>("title")),
                    "design": row.get::<Option<String>, _>("design"),
                    "report_count": row.get::<i64, _>("report_count"),
                    "updated_at": row.get::<DateTime<Utc>, _>("updated_at"),
                })
            })
            .collect(),
    ))
}

/// Lists reports with their title/abstract screening status. `status` is one
/// of `unscreened`, `include`, `exclude`, `maybe`, or `None` for all.
pub async fn list_agent_reports_by_screening(
    pool: &PgPool,
    project_id: Uuid,
    status: Option<&str>,
    limit: i64,
) -> Result<Value, AgentReadError> {
    if !matches!(
        status,
        None | Some("unscreened" | "include" | "exclude" | "maybe")
    ) {
        return Err(AgentReadError::InvalidData);
    }
    let rows = sqlx::query(
        "SELECT r.id, r.title, r.publication_year, r.journal,
                CASE WHEN ai_first_audit_masked(pr.project_id,pr.report_id) THEN 'unscreened' ELSE COALESCE(s.title_abstract_status,'unscreened') END AS status,
                CASE WHEN ai_first_audit_masked(pr.project_id,pr.report_id) THEN 'unscreened' ELSE COALESCE(s.final_status,'unscreened') END AS final_status
         FROM project_reports pr
         JOIN reports r ON r.id=pr.report_id
         LEFT JOIN screening_state s ON s.project_id=pr.project_id AND s.report_id=pr.report_id
         WHERE pr.project_id=$1
           AND ($2::text IS NULL OR CASE WHEN ai_first_audit_masked(pr.project_id,pr.report_id) THEN 'unscreened' ELSE COALESCE(s.title_abstract_status,'unscreened') END=$2)
         ORDER BY pr.created_at, r.id LIMIT $3",
    )
    .bind(project_id)
    .bind(status)
    .bind(limit.clamp(1, 50))
    .fetch_all(pool)
    .await?;
    Ok(Value::Array(
        rows.into_iter()
            .map(|row| {
                serde_json::json!({
                    "report_id": row.get::<Uuid, _>("id"),
                    "title": decoded(row.get::<Option<String>, _>("title")),
                    "year": row.get::<Option<i32>, _>("publication_year"),
                    "journal": row.get::<Option<String>, _>("journal"),
                    "title_abstract_status": row.get::<String, _>("status"),
                    "final_status": row.get::<String, _>("final_status"),
                })
            })
            .collect(),
    ))
}

#[cfg(test)]
mod tests {
    use super::{decode_html_entities, report_search_query};

    #[test]
    fn titles_lose_their_html_entities_once() {
        assert_eq!(
            decode_html_entities("Lancet Diabetes &amp; Endocrinology"),
            "Lancet Diabetes & Endocrinology"
        );
        assert_eq!(
            decode_html_entities("Cardio&#x2013;vascular &lt;risk&gt; &quot;x&quot; &#39;y&#39;"),
            "Cardio\u{2013}vascular <risk> \"x\" 'y'"
        );
        assert_eq!(decode_html_entities("AT&T and a &b; c"), "AT&T and a &b; c");
        assert_eq!(decode_html_entities("&amp;amp;"), "&amp;");
    }

    #[test]
    fn questions_become_key_terms_that_match_any_word() {
        assert_eq!(
            report_search_query("steps per day"),
            Some("steps or day".to_owned())
        );
        assert_eq!(
            report_search_query("Fitbit, fitbit and the women!"),
            Some("fitbit or women".to_owned())
        );
        assert_eq!(report_search_query("the of an"), None);
    }

    #[test]
    fn key_terms_match_both_spellings_of_ise_and_ize_words() {
        assert_eq!(
            super::spelling_variants("randomised"),
            vec!["randomised".to_owned(), "randomized".to_owned()]
        );
        assert_eq!(
            super::spelling_variants("organization"),
            vec!["organization".to_owned(), "organisation".to_owned()]
        );
        assert_eq!(super::spelling_variants("trial"), vec!["trial".to_owned()]);
        assert_eq!(
            report_search_query("how many participants were randomised?"),
            Some("participants or randomised or randomized".to_owned())
        );
    }
}
