use chrono::{DateTime, Utc};
use deepref_application::AutomationDomainEvent;
use deepref_documents::{PARSER_VERSION, ParsedDocument};
use deepref_domain::{Actor, ActorKind, ProjectId};
use serde_json::Value;
use sqlx::{PgPool, Postgres, Row, Transaction};
use uuid::Uuid;

#[derive(Debug, Clone)]
pub struct DocumentRecord {
    pub id: Uuid,
    pub project_id: Uuid,
    pub report_id: Uuid,
    pub original_filename: Option<String>,
    pub external_url: Option<String>,
    pub source: String,
    pub status: String,
    pub mime_type: String,
    pub byte_size: i64,
    pub content_hash: Option<String>,
    pub object_key: Option<String>,
    pub parser_version: Option<String>,
    pub active_parser_version: Option<String>,
    pub parser_error: Option<String>,
    pub ocr_required: bool,
    /// Whether the PDF's title and DOI agree with the report (set after each parse).
    pub identity_check: Option<deepref_documents::IdentityCheck>,
    /// When the researcher chose to keep a PDF that was flagged as a mismatch.
    pub identity_acknowledged_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub struct DocumentBlockRecord {
    pub id: Uuid,
    pub document_id: Uuid,
    pub parser_version: String,
    pub page_number: i32,
    pub page_width: Option<f64>,
    pub page_height: Option<f64>,
    pub page_ocr_required: bool,
    pub kind: String,
    pub section_path: Vec<String>,
    pub ordinal: i32,
    pub text: String,
    pub bbox: Option<Value>,
    pub content_hash: String,
}

#[derive(Debug, Clone)]
pub struct DocumentPageRecord {
    pub document_id: Uuid,
    pub parser_version: String,
    pub page_number: i32,
    pub width: f64,
    pub height: f64,
    pub ocr_required: bool,
}

#[derive(Debug, Clone)]
pub struct MissingFullTextRecord {
    pub report_id: Uuid,
    pub title: Option<String>,
    pub abstract_text: Option<String>,
    pub doi: Option<String>,
    pub status: String,
}

#[derive(Debug, Clone)]
pub struct FullTextQueueRecord {
    pub report_id: Uuid,
    pub title: Option<String>,
    pub abstract_text: Option<String>,
    pub doi: Option<String>,
    pub publication_year: Option<i32>,
    pub full_text_status: String,
    pub revision: i64,
    pub document: Option<DocumentRecord>,
}

#[derive(Debug, Clone)]
pub struct ExclusionReasonRecord {
    pub id: Uuid,
    pub code: String,
    pub label: String,
    pub stage: String,
}

#[derive(Debug, Clone, Copy)]
pub struct NewDocument<'a> {
    pub project_id: Uuid,
    pub report_id: Uuid,
    pub id: Uuid,
    pub source: &'a str,
    pub status: &'a str,
    pub original_filename: Option<&'a str>,
    pub external_url: Option<&'a str>,
    pub mime_type: &'a str,
    pub byte_size: i64,
    pub content_hash: Option<&'a str>,
    pub object_key: Option<&'a str>,
    pub actor_kind: &'a str,
    pub actor_id: &'a str,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompleteDocumentRetrievalOutcome {
    Applied,
    AlreadyCompleted,
}

pub async fn list_full_text_reasons(
    pool: &PgPool,
    project_id: Uuid,
) -> anyhow::Result<Vec<ExclusionReasonRecord>> {
    let rows = sqlx::query(
        "SELECT id,code,label,stage FROM exclusion_reasons
         WHERE project_id=$1 AND stage='full_text' ORDER BY code,id",
    )
    .bind(project_id)
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|row| ExclusionReasonRecord {
            id: row.get("id"),
            code: row.get("code"),
            label: row.get("label"),
            stage: row.get("stage"),
        })
        .collect())
}

pub async fn list_documents(
    pool: &PgPool,
    project_id: Uuid,
    report_id: Option<Uuid>,
    limit: i64,
) -> anyhow::Result<Vec<DocumentRecord>> {
    let rows = sqlx::query(
        "SELECT id,project_id,report_id,original_filename,external_url,source,status,mime_type,byte_size,
                content_hash,object_key,parser_version,active_parser_version,parser_error,ocr_required,identity_check,identity_acknowledged_at,created_at,updated_at
         FROM documents
         WHERE project_id=$1 AND ($2::uuid IS NULL OR report_id=$2)
         ORDER BY created_at DESC,id DESC LIMIT $3",
    )
    .bind(project_id)
    .bind(report_id)
    .bind(limit.clamp(1, 100))
    .fetch_all(pool)
    .await?;
    Ok(rows.into_iter().map(document_from_row).collect())
}

pub async fn get_document(
    pool: &PgPool,
    project_id: Uuid,
    report_id: Uuid,
    document_id: Uuid,
) -> anyhow::Result<DocumentRecord> {
    let row = sqlx::query(
        "SELECT id,project_id,report_id,original_filename,external_url,source,status,mime_type,byte_size,
                content_hash,object_key,parser_version,active_parser_version,parser_error,ocr_required,identity_check,identity_acknowledged_at,created_at,updated_at
         FROM documents
         WHERE id=$1 AND project_id=$2 AND report_id=$3",
    )
    .bind(document_id)
    .bind(project_id)
    .bind(report_id)
    .fetch_one(pool)
    .await?;
    Ok(document_from_row(row))
}

pub async fn get_document_by_id(
    pool: &PgPool,
    document_id: Uuid,
) -> anyhow::Result<DocumentRecord> {
    let row = sqlx::query(
        "SELECT id,project_id,report_id,original_filename,external_url,source,status,mime_type,byte_size,
                content_hash,object_key,parser_version,active_parser_version,parser_error,ocr_required,identity_check,identity_acknowledged_at,created_at,updated_at
         FROM documents WHERE id=$1",
    )
    .bind(document_id)
    .fetch_one(pool)
    .await?;
    Ok(document_from_row(row))
}

pub async fn get_document_blocks(
    pool: &PgPool,
    project_id: Uuid,
    report_id: Uuid,
    document_id: Uuid,
) -> anyhow::Result<Vec<DocumentBlockRecord>> {
    let rows = sqlx::query(
        "SELECT b.id,b.document_id,b.parser_version,b.page_number,b.page_width,b.page_height,p.ocr_required AS page_ocr_required,b.kind,b.section_path,
                b.ordinal,b.text,b.bbox,b.content_hash
         FROM document_blocks b
         JOIN documents d ON d.id=b.document_id
         JOIN document_pages p ON p.document_id=b.document_id AND p.parser_version=b.parser_version
           AND p.page_number=b.page_number AND p.active
         WHERE b.document_id=$1 AND d.project_id=$2 AND d.report_id=$3 AND b.active
         ORDER BY b.page_number,b.ordinal,b.id",
    )
    .bind(document_id)
    .bind(project_id)
    .bind(report_id)
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|row| DocumentBlockRecord {
            id: row.get("id"),
            document_id: row.get("document_id"),
            parser_version: row.get("parser_version"),
            page_number: row.get("page_number"),
            page_width: row.get("page_width"),
            page_height: row.get("page_height"),
            page_ocr_required: row.get("page_ocr_required"),
            kind: row.get("kind"),
            section_path: row.get("section_path"),
            ordinal: row.get("ordinal"),
            text: row.get("text"),
            bbox: row.get("bbox"),
            content_hash: row.get("content_hash"),
        })
        .collect())
}

pub async fn get_document_pages(
    pool: &PgPool,
    project_id: Uuid,
    report_id: Uuid,
    document_id: Uuid,
) -> anyhow::Result<Vec<DocumentPageRecord>> {
    let rows = sqlx::query(
        "SELECT p.document_id,p.parser_version,p.page_number,p.width,p.height,p.ocr_required
         FROM document_pages p JOIN documents d ON d.id=p.document_id
         WHERE p.document_id=$1 AND d.project_id=$2 AND d.report_id=$3 AND p.active
         ORDER BY p.page_number",
    )
    .bind(document_id)
    .bind(project_id)
    .bind(report_id)
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|row| DocumentPageRecord {
            document_id: row.get("document_id"),
            parser_version: row.get("parser_version"),
            page_number: row.get("page_number"),
            width: row.get("width"),
            height: row.get("height"),
            ocr_required: row.get("ocr_required"),
        })
        .collect())
}

pub async fn search_document_blocks(
    pool: &PgPool,
    project_id: Uuid,
    report_id: Uuid,
    document_id: Uuid,
    query: &str,
    limit: i64,
) -> anyhow::Result<Vec<DocumentBlockRecord>> {
    let rows = sqlx::query(
        "SELECT b.id,b.document_id,b.parser_version,b.page_number,b.page_width,b.page_height,p.ocr_required AS page_ocr_required,b.kind,b.section_path,
                b.ordinal,b.text,b.bbox,b.content_hash
         FROM document_blocks b
         JOIN documents d ON d.id=b.document_id
         JOIN document_pages p ON p.document_id=b.document_id AND p.parser_version=b.parser_version
           AND p.page_number=b.page_number AND p.active
         WHERE b.document_id=$1 AND d.project_id=$2 AND d.report_id=$3 AND b.active
           AND b.search_vector @@ websearch_to_tsquery('simple',$4)
         ORDER BY ts_rank_cd(b.search_vector,websearch_to_tsquery('simple',$4)) DESC,
                  b.page_number,b.ordinal,b.id LIMIT $5",
    )
    .bind(document_id)
    .bind(project_id)
    .bind(report_id)
    .bind(query)
    .bind(limit.clamp(1, 100))
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|row| DocumentBlockRecord {
            id: row.get("id"),
            document_id: row.get("document_id"),
            parser_version: row.get("parser_version"),
            page_number: row.get("page_number"),
            page_width: row.get("page_width"),
            page_height: row.get("page_height"),
            page_ocr_required: row.get("page_ocr_required"),
            kind: row.get("kind"),
            section_path: row.get("section_path"),
            ordinal: row.get("ordinal"),
            text: row.get("text"),
            bbox: row.get("bbox"),
            content_hash: row.get("content_hash"),
        })
        .collect())
}

pub async fn create_document(
    tx: &mut Transaction<'_, Postgres>,
    document: NewDocument<'_>,
) -> anyhow::Result<DocumentRecord> {
    let row = sqlx::query(
        "INSERT INTO documents
           (id,project_id,report_id,object_key,content_hash,mime_type,byte_size,original_filename,
            source,status,external_url,actor_kind,actor_id,content_available_at,updated_at)
         VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,
                 CASE WHEN $4::text IS NULL THEN NULL ELSE now() END,now())
         RETURNING id,project_id,report_id,original_filename,external_url,source,status,mime_type,byte_size,
           content_hash,object_key,parser_version,active_parser_version,parser_error,ocr_required,identity_check,identity_acknowledged_at,created_at,updated_at",
    )
    .bind(document.id)
    .bind(document.project_id)
    .bind(document.report_id)
    .bind(document.object_key)
    .bind(document.content_hash)
    .bind(document.mime_type)
    .bind(document.byte_size)
    .bind(document.original_filename)
    .bind(document.source)
    .bind(document.status)
    .bind(document.external_url)
    .bind(document.actor_kind)
    .bind(document.actor_id)
    .fetch_one(&mut **tx)
    .await?;
    if matches!(document.status, "uploaded" | "available") {
        let actor_kind = ActorKind::parse(document.actor_kind)
            .ok_or_else(|| anyhow::anyhow!("document actor kind is invalid"))?;
        let actor = Actor::new(actor_kind, document.actor_id)?;
        crate::dispatch_automation_domain_event(
            tx,
            &AutomationDomainEvent::FullTextAttached {
                project_id: ProjectId::new(document.project_id),
                document_id: document.id,
                actor,
            },
        )
        .await?;
    }
    Ok(document_from_row(row))
}

pub async fn enqueue_parse(
    tx: &mut Transaction<'_, Postgres>,
    project_id: ProjectId,
    document_id: Uuid,
    _content_hash: &str,
) -> anyhow::Result<Uuid> {
    let job_id = Uuid::new_v4();
    let dedupe = format!("parse_document:{document_id}:{PARSER_VERSION}");
    let payload =
        serde_json::json!({ "document_id": document_id, "parser_version": PARSER_VERSION });
    crate::enqueue_job(
        tx,
        &crate::job(job_id, project_id, "parse_document", payload, dedupe),
    )
    .await
}

/// Queues a forced parse of a stored PDF. Every request gets its own dedupe key,
/// so the job runs even when the active parser version is already current. The
/// returned key is the job's dedupe key.
pub async fn enqueue_reparse(
    tx: &mut Transaction<'_, Postgres>,
    project_id: ProjectId,
    document_id: Uuid,
) -> anyhow::Result<String> {
    let job_id = Uuid::new_v4();
    let dedupe = format!("parse_document:{document_id}:{PARSER_VERSION}:reparse:{job_id}");
    let payload = serde_json::json!({
        "document_id": document_id,
        "parser_version": PARSER_VERSION,
        "force": true,
    });
    crate::enqueue_job(
        tx,
        &crate::job(
            job_id,
            project_id,
            "parse_document",
            payload,
            dedupe.clone(),
        ),
    )
    .await?;
    Ok(dedupe)
}

pub async fn enqueue_retrieve(
    tx: &mut Transaction<'_, Postgres>,
    project_id: ProjectId,
    document_id: Uuid,
) -> anyhow::Result<Uuid> {
    let job_id = Uuid::new_v4();
    let dedupe = format!("retrieve_document:{document_id}");
    let payload = serde_json::json!({ "document_id": document_id });
    crate::enqueue_job(
        tx,
        &crate::job(job_id, project_id, "retrieve_document", payload, dedupe),
    )
    .await
}

/// Returns the failed document of a report with identical content, if any, so
/// a repeated upload can retry it instead of colliding with the unique hash.
pub async fn find_failed_document_by_hash(
    pool: &PgPool,
    project_id: Uuid,
    report_id: Uuid,
    content_hash: &str,
) -> anyhow::Result<Option<Uuid>> {
    Ok(sqlx::query_scalar(
        "SELECT id FROM documents
         WHERE project_id=$1 AND report_id=$2 AND content_hash=$3 AND status='failed'",
    )
    .bind(project_id)
    .bind(report_id)
    .bind(content_hash)
    .fetch_optional(pool)
    .await?)
}

/// Re-queues processing for a failed document (or re-parses an available one).
/// Resets the failure state and revives the deduplicated job so the worker
/// picks it up again. Returns `None` when the document is not retryable.
pub async fn requeue_document_processing(
    tx: &mut Transaction<'_, Postgres>,
    project_id: ProjectId,
    report_id: Uuid,
    document_id: Uuid,
) -> anyhow::Result<Option<DocumentRecord>> {
    let Some(current) = sqlx::query(
        "SELECT status,source,object_key,content_hash FROM documents
         WHERE id=$1 AND project_id=$2 AND report_id=$3 FOR UPDATE",
    )
    .bind(document_id)
    .bind(project_id.as_uuid())
    .bind(report_id)
    .fetch_optional(&mut **tx)
    .await?
    else {
        return Err(sqlx::Error::RowNotFound.into());
    };
    let status: String = current.get("status");
    let source: String = current.get("source");
    let object_key: Option<String> = current.get("object_key");
    let content_hash: Option<String> = current.get("content_hash");
    let dedupe = match (status.as_str(), object_key.is_some()) {
        ("failed", true) => {
            sqlx::query(
                "UPDATE documents SET status='uploaded',parser_error=NULL,failed_at=NULL,updated_at=now()
                 WHERE id=$1",
            )
            .bind(document_id)
            .execute(&mut **tx)
            .await?;
            enqueue_parse(
                tx,
                project_id,
                document_id,
                content_hash.as_deref().unwrap_or_default(),
            )
            .await?;
            format!("parse_document:{document_id}:{PARSER_VERSION}")
        }
        ("failed", false) if source == "external_url" => {
            sqlx::query(
                "UPDATE documents SET status='external',parser_error=NULL,failed_at=NULL,updated_at=now()
                 WHERE id=$1",
            )
            .bind(document_id)
            .execute(&mut **tx)
            .await?;
            enqueue_retrieve(tx, project_id, document_id).await?;
            format!("retrieve_document:{document_id}")
        }
        ("available", true) => {
            // A reparse must run even when the stored parse is current, so it
            // gets its own forced job and bumps `updated_at` for the UI.
            let dedupe = enqueue_reparse(tx, project_id, document_id).await?;
            sqlx::query("UPDATE documents SET updated_at=now() WHERE id=$1")
                .bind(document_id)
                .execute(&mut **tx)
                .await?;
            dedupe
        }
        _ => return Ok(None),
    };
    sqlx::query(
        "UPDATE jobs SET state='queued',attempts=0,last_error=NULL,available_at=now(),
             completed_at=NULL,lease_owner=NULL,leased_until=NULL,lease_renewed_at=NULL
         WHERE dedupe_key=$1 AND state IN ('completed','failed','dead')",
    )
    .bind(dedupe)
    .execute(&mut **tx)
    .await?;
    let row = sqlx::query(
        "SELECT id,project_id,report_id,original_filename,external_url,source,status,mime_type,byte_size,
                content_hash,object_key,parser_version,active_parser_version,parser_error,ocr_required,identity_check,identity_acknowledged_at,created_at,updated_at
         FROM documents WHERE id=$1",
    )
    .bind(document_id)
    .fetch_one(&mut **tx)
    .await?;
    Ok(Some(document_from_row(row)))
}

pub async fn mark_document_retrieving(pool: &PgPool, document_id: Uuid) -> anyhow::Result<bool> {
    Ok(sqlx::query!(
        "UPDATE documents SET status='retrieving',parser_error=NULL,failed_at=NULL,updated_at=now()
         WHERE id=$1 AND source='external_url' AND status IN ('external','failed','retrieving')",
        document_id,
    )
    .execute(pool)
    .await?
    .rows_affected()
        == 1)
}

pub async fn complete_document_retrieval(
    tx: &mut Transaction<'_, Postgres>,
    project_id: ProjectId,
    document_id: Uuid,
    object_key: &str,
    content_hash: &str,
    byte_size: i64,
) -> anyhow::Result<CompleteDocumentRetrievalOutcome> {
    let updated = sqlx::query!(
        "UPDATE documents SET status='uploaded',object_key=$2,content_hash=$3,byte_size=$4,
             mime_type='application/pdf',content_available_at=now(),failed_at=NULL,
             parser_error=NULL,updated_at=now()
         WHERE id=$1 AND status='retrieving'
         RETURNING actor_kind, actor_id",
        document_id,
        object_key,
        content_hash,
        byte_size,
    )
    .fetch_optional(&mut **tx)
    .await?;
    if let Some(updated) = updated {
        enqueue_parse(tx, project_id, document_id, content_hash).await?;
        let actor_kind = ActorKind::parse(&updated.actor_kind)
            .ok_or_else(|| anyhow::anyhow!("document actor kind is invalid"))?;
        let actor = Actor::new(actor_kind, updated.actor_id)?;
        crate::dispatch_automation_domain_event(
            tx,
            &AutomationDomainEvent::FullTextAttached {
                project_id,
                document_id,
                actor,
            },
        )
        .await?;
        return Ok(CompleteDocumentRetrievalOutcome::Applied);
    }

    let already_completed = sqlx::query_scalar!(
        r#"SELECT (status IN ('uploaded','available') AND object_key IS NOT NULL) as "completed!"
         FROM documents WHERE id=$1"#,
        document_id,
    )
    .fetch_optional(&mut **tx)
    .await?
    .unwrap_or(false);
    if already_completed {
        Ok(CompleteDocumentRetrievalOutcome::AlreadyCompleted)
    } else {
        anyhow::bail!("document retrieval completion lost ownership before content was available")
    }
}

pub async fn mark_document_retrieval_failed(
    pool: &PgPool,
    document_id: Uuid,
    error: &str,
) -> anyhow::Result<()> {
    let truncated_err = error.chars().take(1000).collect::<String>();
    sqlx::query!(
        "UPDATE documents SET status='failed',parser_error=$2,failed_at=now(),updated_at=now()
         WHERE id=$1 AND status='retrieving' AND active_parser_version IS NULL",
        document_id,
        truncated_err,
    )
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn list_full_text_queue(
    pool: &PgPool,
    project_id: Uuid,
    status: Option<&str>,
    search: Option<&str>,
    after_report_id: Option<Uuid>,
    limit: i64,
) -> anyhow::Result<Vec<FullTextQueueRecord>> {
    let rows = sqlx::query(
        "SELECT pr.report_id,r.title,r.abstract_text,r.publication_year,
                (SELECT ri.normalized_value FROM report_identifiers ri
                 WHERE ri.report_id=r.id AND ri.scheme='doi' ORDER BY ri.id LIMIT 1) AS doi,
                ss.full_text_status,ss.revision,
                d.id AS document_id,d.project_id AS document_project_id,d.report_id AS document_report_id,
                d.original_filename,d.external_url,d.source,d.status AS document_status,d.mime_type,d.byte_size,
                d.content_hash,d.object_key,d.parser_version,d.active_parser_version,d.parser_error,
                d.ocr_required,d.identity_check AS document_identity_check,
                d.identity_acknowledged_at AS document_identity_acknowledged_at,
                d.created_at AS document_created_at,d.updated_at AS document_updated_at
         FROM project_reports pr
         JOIN reports r ON r.id=pr.report_id
         JOIN screening_state ss ON ss.project_id=pr.project_id AND ss.report_id=pr.report_id
         LEFT JOIN LATERAL (
           SELECT * FROM documents candidate
           WHERE candidate.project_id=pr.project_id AND candidate.report_id=pr.report_id
           ORDER BY (candidate.status='available') DESC,candidate.updated_at DESC,candidate.id DESC LIMIT 1
         ) d ON true
         WHERE pr.project_id=$1 AND NOT ai_first_audit_masked(pr.project_id,pr.report_id) AND ss.title_abstract_status='include'
           AND ($2::text IS NULL OR ss.full_text_status=$2)
           AND ($3::text IS NULL OR lower(coalesce(r.title,'') || ' ' || coalesce(r.abstract_text,'')) LIKE '%' || lower($3) || '%')
           AND ($4::uuid IS NULL OR pr.report_id > $4)
         ORDER BY pr.report_id LIMIT $5",
    )
    .bind(project_id)
    .bind(status)
    .bind(search)
    .bind(after_report_id)
    .bind(limit.clamp(1, 101))
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|row| FullTextQueueRecord {
            report_id: row.get("report_id"),
            title: row.get("title"),
            abstract_text: row.get("abstract_text"),
            doi: row.get("doi"),
            publication_year: row.get("publication_year"),
            full_text_status: row.get("full_text_status"),
            revision: row.get("revision"),
            document: row
                .try_get::<Uuid, _>("document_id")
                .ok()
                .map(|id| DocumentRecord {
                    id,
                    project_id: row.get("document_project_id"),
                    report_id: row.get("document_report_id"),
                    original_filename: row.get("original_filename"),
                    external_url: row.get("external_url"),
                    source: row.get("source"),
                    status: row.get("document_status"),
                    mime_type: row.get("mime_type"),
                    byte_size: row.get("byte_size"),
                    content_hash: row.get("content_hash"),
                    object_key: row.get("object_key"),
                    parser_version: row.get("parser_version"),
                    active_parser_version: row.get("active_parser_version"),
                    parser_error: row.get("parser_error"),
                    ocr_required: row.get("ocr_required"),
                    identity_check: identity_check_from_column(row.get("document_identity_check")),
                    identity_acknowledged_at: row.get("document_identity_acknowledged_at"),
                    created_at: row.get("document_created_at"),
                    updated_at: row.get("document_updated_at"),
                }),
        })
        .collect())
}

pub async fn list_missing_full_text(
    pool: &PgPool,
    project_id: Uuid,
    limit: i64,
) -> anyhow::Result<Vec<MissingFullTextRecord>> {
    let rows = sqlx::query(
        "SELECT pr.report_id,r.title,r.abstract_text,COALESCE(d.status,'missing') AS status,
                (SELECT ri.normalized_value FROM report_identifiers ri
                 WHERE ri.report_id=r.id AND ri.scheme='doi' ORDER BY ri.id LIMIT 1) AS doi
         FROM project_reports pr
         JOIN reports r ON r.id=pr.report_id
         LEFT JOIN LATERAL (
           SELECT candidate.status FROM documents candidate
           WHERE candidate.project_id=pr.project_id AND candidate.report_id=pr.report_id
           ORDER BY (candidate.status NOT IN ('missing','failed')) DESC,
                    candidate.updated_at DESC,candidate.id DESC LIMIT 1
         ) d ON true
         LEFT JOIN screening_state ss
           ON ss.project_id=pr.project_id AND ss.report_id=pr.report_id
         WHERE pr.project_id=$1 AND NOT ai_first_audit_masked(pr.project_id,pr.report_id) AND ss.title_abstract_status='include'
           AND (d.status IS NULL OR d.status IN ('missing','failed'))
         ORDER BY r.created_at,pr.report_id LIMIT $2",
    )
    .bind(project_id)
    .bind(limit.clamp(1, 100))
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|row| MissingFullTextRecord {
            report_id: row.get("report_id"),
            title: row.get("title"),
            abstract_text: row.get("abstract_text"),
            doi: row.get("doi"),
            status: row.get("status"),
        })
        .collect())
}

pub async fn mark_document_parsing(pool: &PgPool, document_id: Uuid) -> anyhow::Result<()> {
    sqlx::query("UPDATE documents SET parser_error=NULL,parser_version=$2,updated_at=now() WHERE id=$1 AND active_parser_version IS DISTINCT FROM $2")
        .bind(document_id)
        .bind(PARSER_VERSION)
        .execute(pool)
        .await?;
    Ok(())
}

/// Structure recovered for a parsed document beyond the raw blocks: section
/// paths per block ordinal, the section outline and the bibliography.
#[derive(Debug, Clone, Default)]
pub struct DocumentStructure {
    /// Overrides for `document_blocks.section_path`, keyed by block ordinal.
    pub section_paths: std::collections::HashMap<u32, Vec<String>>,
    pub sections: Vec<NewDocumentSection>,
    pub references: Vec<NewDocumentReference>,
}

#[derive(Debug, Clone)]
pub struct NewDocumentSection {
    pub number: Option<String>,
    pub title: String,
    pub depth: i32,
    pub path: Vec<String>,
    pub source: String,
}

#[derive(Debug, Clone)]
pub struct NewDocumentReference {
    pub raw: String,
    pub title: Option<String>,
    pub authors: Vec<String>,
    pub year: Option<i32>,
    pub venue: Option<String>,
    pub doi: Option<String>,
    pub source: String,
}

#[derive(Debug, Clone)]
pub struct DocumentReferenceRecord {
    pub id: Uuid,
    pub ordinal: i32,
    pub raw: String,
    pub title: Option<String>,
    pub authors: Vec<String>,
    pub year: Option<i32>,
    pub venue: Option<String>,
    pub doi: Option<String>,
    pub source: String,
}

#[derive(Debug, Clone)]
pub struct DocumentSectionRecord {
    pub id: Uuid,
    pub ordinal: i32,
    pub number: Option<String>,
    pub title: String,
    pub depth: i32,
    pub path: Vec<String>,
    pub source: String,
}

pub async fn persist_parsed_document(
    pool: &PgPool,
    document_id: Uuid,
    parsed: &ParsedDocument,
    parser_version: &str,
) -> anyhow::Result<()> {
    persist_parsed_document_with_structure(
        pool,
        document_id,
        parsed,
        parser_version,
        &DocumentStructure::default(),
    )
    .await
}

pub async fn persist_parsed_document_with_structure(
    pool: &PgPool,
    document_id: Uuid,
    parsed: &ParsedDocument,
    parser_version: &str,
    structure: &DocumentStructure,
) -> anyhow::Result<()> {
    let mut transaction = pool.begin().await?;
    sqlx::query(
        "DELETE FROM document_blocks WHERE document_id=$1 AND parser_version=$2 AND NOT active",
    )
    .bind(document_id)
    .bind(parser_version)
    .execute(&mut *transaction)
    .await?;
    sqlx::query(
        "DELETE FROM document_pages WHERE document_id=$1 AND parser_version=$2 AND NOT active",
    )
    .bind(document_id)
    .bind(parser_version)
    .execute(&mut *transaction)
    .await?;
    for page in &parsed.pages {
        sqlx::query(
            "INSERT INTO document_pages
               (document_id,parser_version,page_number,width,height,ocr_required,active)
             VALUES ($1,$2,$3,$4,$5,$6,false)
             ON CONFLICT (document_id,parser_version,page_number) DO UPDATE
               SET width=EXCLUDED.width,height=EXCLUDED.height,
                   ocr_required=EXCLUDED.ocr_required",
        )
        .bind(document_id)
        .bind(parser_version)
        .bind(i32::try_from(page.page_number)?)
        .bind(f64::from(page.width))
        .bind(f64::from(page.height))
        .bind(page.ocr_required)
        .execute(&mut *transaction)
        .await?;
    }
    for block in &parsed.blocks {
        sqlx::query(
            "INSERT INTO document_blocks
               (id,document_id,parser_version,page_number,page_width,page_height,kind,section_path,
                ordinal,text,bbox,content_hash,active)
             VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,false)
             ON CONFLICT (document_id,parser_version,ordinal) DO UPDATE
               SET page_number=EXCLUDED.page_number,page_width=EXCLUDED.page_width,
                   page_height=EXCLUDED.page_height,kind=EXCLUDED.kind,
                   section_path=EXCLUDED.section_path,text=EXCLUDED.text,
                   content_hash=EXCLUDED.content_hash,bbox=EXCLUDED.bbox",
        )
        .bind(Uuid::new_v4())
        .bind(document_id)
        .bind(parser_version)
        .bind(i32::try_from(block.page_number)?)
        .bind(f64::from(block.page_width))
        .bind(f64::from(block.page_height))
        .bind(&block.kind)
        .bind(
            structure
                .section_paths
                .get(&block.ordinal)
                .cloned()
                .unwrap_or_default(),
        )
        .bind(i32::try_from(block.ordinal)?)
        .bind(&block.text)
        .bind(block.bbox.as_ref().map(serde_json::to_value).transpose()?)
        .bind(&block.content_hash)
        .execute(&mut *transaction)
        .await?;
    }
    sqlx::query("DELETE FROM document_references WHERE document_id=$1 AND parser_version=$2")
        .bind(document_id)
        .bind(parser_version)
        .execute(&mut *transaction)
        .await?;
    sqlx::query("DELETE FROM document_sections WHERE document_id=$1 AND parser_version=$2")
        .bind(document_id)
        .bind(parser_version)
        .execute(&mut *transaction)
        .await?;
    for (index, reference) in structure.references.iter().enumerate() {
        sqlx::query(
            "INSERT INTO document_references
               (id,document_id,parser_version,ordinal,raw,title,authors,year,venue,doi,source)
             VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11)",
        )
        .bind(Uuid::new_v4())
        .bind(document_id)
        .bind(parser_version)
        .bind(i32::try_from(index + 1)?)
        .bind(&reference.raw)
        .bind(&reference.title)
        .bind(serde_json::to_value(&reference.authors)?)
        .bind(reference.year)
        .bind(&reference.venue)
        .bind(&reference.doi)
        .bind(&reference.source)
        .execute(&mut *transaction)
        .await?;
    }
    for (index, section) in structure.sections.iter().enumerate() {
        sqlx::query(
            "INSERT INTO document_sections
               (id,document_id,parser_version,ordinal,number,title,depth,path,source)
             VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9)",
        )
        .bind(Uuid::new_v4())
        .bind(document_id)
        .bind(parser_version)
        .bind(i32::try_from(index + 1)?)
        .bind(&section.number)
        .bind(&section.title)
        .bind(section.depth)
        .bind(&section.path)
        .bind(&section.source)
        .execute(&mut *transaction)
        .await?;
    }
    sqlx::query("UPDATE document_blocks SET active=false WHERE document_id=$1 AND active")
        .bind(document_id)
        .execute(&mut *transaction)
        .await?;
    sqlx::query(
        "UPDATE document_blocks SET active=true WHERE document_id=$1 AND parser_version=$2",
    )
    .bind(document_id)
    .bind(parser_version)
    .execute(&mut *transaction)
    .await?;
    sqlx::query("UPDATE document_pages SET active=false WHERE document_id=$1 AND active")
        .bind(document_id)
        .execute(&mut *transaction)
        .await?;
    sqlx::query("UPDATE document_pages SET active=true WHERE document_id=$1 AND parser_version=$2")
        .bind(document_id)
        .bind(parser_version)
        .execute(&mut *transaction)
        .await?;
    sqlx::query(
        "UPDATE documents
         SET status='available',
             parser_version=$2,active_parser_version=$2,ocr_required=$3,parser_error=NULL,
             parsed_at=now(),failed_at=NULL,updated_at=now()
         WHERE id=$1",
    )
    .bind(document_id)
    .bind(parser_version)
    .bind(parsed.ocr_required)
    .execute(&mut *transaction)
    .await?;
    let parsed_project: Option<Uuid> =
        sqlx::query_scalar("SELECT project_id FROM documents WHERE id=$1")
            .bind(document_id)
            .fetch_optional(&mut *transaction)
            .await?;
    if let Some(project_id) = parsed_project {
        crate::dispatch_automation_domain_event(
            &mut transaction,
            &deepref_application::AutomationDomainEvent::DocumentParsed {
                project_id: ProjectId::new(project_id),
                document_id,
            },
        )
        .await?;
    }
    transaction.commit().await?;
    Ok(())
}

pub async fn list_document_references(
    pool: &PgPool,
    project_id: Uuid,
    report_id: Uuid,
    document_id: Uuid,
) -> anyhow::Result<Vec<DocumentReferenceRecord>> {
    let rows = sqlx::query(
        "SELECT r.id,r.ordinal,r.raw,r.title,r.authors,r.year,r.venue,r.doi,r.source
         FROM document_references r JOIN documents d ON d.id=r.document_id
         WHERE r.document_id=$1 AND d.project_id=$2 AND d.report_id=$3
           AND r.parser_version=d.active_parser_version
         ORDER BY r.ordinal",
    )
    .bind(document_id)
    .bind(project_id)
    .bind(report_id)
    .fetch_all(pool)
    .await?;
    rows.into_iter()
        .map(|row| {
            let authors: Value = row.get("authors");
            Ok(DocumentReferenceRecord {
                id: row.get("id"),
                ordinal: row.get("ordinal"),
                raw: row.get("raw"),
                title: row.get("title"),
                authors: serde_json::from_value(authors)?,
                year: row.get("year"),
                venue: row.get("venue"),
                doi: row.get("doi"),
                source: row.get("source"),
            })
        })
        .collect()
}

pub async fn list_document_sections(
    pool: &PgPool,
    project_id: Uuid,
    report_id: Uuid,
    document_id: Uuid,
) -> anyhow::Result<Vec<DocumentSectionRecord>> {
    let rows = sqlx::query(
        "SELECT s.id,s.ordinal,s.number,s.title,s.depth,s.path,s.source
         FROM document_sections s JOIN documents d ON d.id=s.document_id
         WHERE s.document_id=$1 AND d.project_id=$2 AND d.report_id=$3
           AND s.parser_version=d.active_parser_version
         ORDER BY s.ordinal",
    )
    .bind(document_id)
    .bind(project_id)
    .bind(report_id)
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|row| DocumentSectionRecord {
            id: row.get("id"),
            ordinal: row.get("ordinal"),
            number: row.get("number"),
            title: row.get("title"),
            depth: row.get("depth"),
            path: row.get("path"),
            source: row.get("source"),
        })
        .collect())
}

pub async fn insert_document_blocks(
    pool: &PgPool,
    document_id: Uuid,
    parsed: &ParsedDocument,
) -> anyhow::Result<()> {
    persist_parsed_document(pool, document_id, parsed, PARSER_VERSION).await
}

pub async fn mark_document_failed(
    pool: &PgPool,
    document_id: Uuid,
    error: &str,
) -> anyhow::Result<()> {
    sqlx::query(
        "UPDATE documents
         SET status=CASE WHEN active_parser_version IS NULL THEN 'failed' ELSE 'available' END,
             parser_error=$2,failed_at=now(),updated_at=now()
         WHERE id=$1",
    )
    .bind(document_id)
    .bind(error.chars().take(1000).collect::<String>())
    .execute(pool)
    .await?;
    Ok(())
}

/// The report's title and DOI, which the identity check compares the PDF with.
pub async fn report_identity(
    pool: &PgPool,
    report_id: Uuid,
) -> anyhow::Result<(Option<String>, Option<String>)> {
    let row = sqlx::query(
        "SELECT r.title,
                (SELECT ri.normalized_value FROM report_identifiers ri
                 WHERE ri.report_id=r.id AND ri.scheme='doi' ORDER BY ri.id LIMIT 1) AS doi
         FROM reports r WHERE r.id=$1",
    )
    .bind(report_id)
    .fetch_one(pool)
    .await?;
    Ok((row.get("title"), row.get("doi")))
}

/// Stores the identity verdict of the latest parse.
pub async fn record_identity_check(
    pool: &PgPool,
    document_id: Uuid,
    check: &deepref_documents::IdentityCheck,
) -> anyhow::Result<()> {
    sqlx::query("UPDATE documents SET identity_check=$2 WHERE id=$1")
        .bind(document_id)
        .bind(serde_json::to_value(check)?)
        .execute(pool)
        .await?;
    Ok(())
}

/// Marks a flagged PDF as kept by the researcher (the first acknowledgement time is kept).
/// Returns false when the document is not flagged.
pub async fn acknowledge_identity_mismatch(
    pool: &PgPool,
    project_id: Uuid,
    report_id: Uuid,
    document_id: Uuid,
) -> anyhow::Result<bool> {
    Ok(sqlx::query(
        "UPDATE documents SET identity_acknowledged_at=COALESCE(identity_acknowledged_at,now())
         WHERE id=$1 AND project_id=$2 AND report_id=$3 AND identity_check->>'verdict'='mismatch'",
    )
    .bind(document_id)
    .bind(project_id)
    .bind(report_id)
    .execute(pool)
    .await?
    .rows_affected()
        == 1)
}

/// Removes a document row (its blocks, pages, sections and references go with it). Returns the
/// stored object key so the caller can delete the file, or `None` when no such document exists.
pub async fn delete_document(
    pool: &PgPool,
    project_id: Uuid,
    report_id: Uuid,
    document_id: Uuid,
) -> anyhow::Result<Option<Option<String>>> {
    let row = sqlx::query(
        "DELETE FROM documents WHERE id=$1 AND project_id=$2 AND report_id=$3 RETURNING object_key",
    )
    .bind(document_id)
    .bind(project_id)
    .bind(report_id)
    .fetch_optional(pool)
    .await?;
    Ok(row.map(|row| row.get("object_key")))
}

fn document_from_row(row: sqlx::postgres::PgRow) -> DocumentRecord {
    DocumentRecord {
        id: row.get("id"),
        project_id: row.get("project_id"),
        report_id: row.get("report_id"),
        original_filename: row.get("original_filename"),
        external_url: row.get("external_url"),
        source: row.get("source"),
        status: row.get("status"),
        mime_type: row.get("mime_type"),
        byte_size: row.get("byte_size"),
        content_hash: row.get("content_hash"),
        object_key: row.get("object_key"),
        parser_version: row.get("parser_version"),
        active_parser_version: row.get("active_parser_version"),
        parser_error: row.get("parser_error"),
        ocr_required: row.get("ocr_required"),
        identity_check: identity_check_from_column(row.get("identity_check")),
        identity_acknowledged_at: row.get("identity_acknowledged_at"),
        created_at: row.get("created_at"),
        updated_at: row.get("updated_at"),
    }
}

/// The stored verdict; a value this build cannot read is treated as no verdict.
fn identity_check_from_column(value: Option<Value>) -> Option<deepref_documents::IdentityCheck> {
    value.and_then(|value| serde_json::from_value(value).ok())
}
