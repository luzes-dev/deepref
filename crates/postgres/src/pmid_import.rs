//! PubMed ID imports. A submitted list of PMIDs becomes one acquisition run (strategy
//! `pmid_import`) with one item per PMID, so the run inspector can say what happened to
//! each. The worker fetches the queued items from PubMed, saves the articles it finds
//! through the same record path as a file import, and settles the run.

use std::collections::HashSet;

use deepref_application::{AutomationDomainEvent, RawRecord};
use deepref_domain::ProjectId;
use serde_json::json;
use sqlx::{PgPool, Postgres, Row, Transaction};
use uuid::Uuid;

use crate::acquisition::{AcquisitionError, insert_raw_record};

/// Durable job kind that fetches the queued PMIDs of one run.
pub const PMID_IMPORT_JOB_KIND: &str = "pmid_import";
/// Acquisition strategy of a PubMed ID run.
pub const PMID_IMPORT_STRATEGY: &str = "pmid_import";
/// Source of the records a PubMed ID run saves. File imports use `import:<format>`.
pub const PMID_IMPORT_SOURCE: &str = "import:pmid";

/// Where one PMID stands in its run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PmidItemStatus {
    Queued,
    Imported,
    AlreadyInProject,
    NotFound,
    Failed,
}

impl PmidItemStatus {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Imported => "imported",
            Self::AlreadyInProject => "already_in_project",
            Self::NotFound => "not_found",
            Self::Failed => "failed",
        }
    }
}

/// A PubMed ID import as the API receives it. The PMIDs are already normalised and unique.
#[derive(Debug, Clone)]
pub struct PmidImportRequest {
    pub project_id: Uuid,
    pub idempotency_key: Option<String>,
    pub pmids: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PmidImportResult {
    pub run_id: Uuid,
    pub created: bool,
}

/// Creates the run, its queued items and the job that fetches them, in one transaction. A
/// repeated idempotency key returns the existing run when its input is the same.
pub async fn create_pmid_import(
    pool: &PgPool,
    request: &PmidImportRequest,
) -> Result<PmidImportResult, AcquisitionError> {
    let mut tx = pool.begin().await?;
    sqlx::query_scalar::<_, Uuid>("SELECT id FROM projects WHERE id=$1 FOR KEY SHARE")
        .bind(request.project_id)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or(AcquisitionError::ProjectNotFound)?;

    let run_id = request
        .idempotency_key
        .as_deref()
        .map_or_else(Uuid::new_v4, |key| {
            Uuid::new_v5(
                &Uuid::NAMESPACE_URL,
                format!("deepref:acquisition:{}:{key}", request.project_id).as_bytes(),
            )
        });
    let config = json!({ "format": "pmid", "pmids": request.pmids });
    let count = i32::try_from(request.pmids.len()).unwrap_or(i32::MAX);
    let inserted = sqlx::query_scalar::<_, Uuid>(
        "INSERT INTO acquisition_runs
         (id,project_id,legacy_ingestion_id,source,strategy,format,idempotency_key,config,metadata,status,
          max_depth,seed_count,queued_count,fetched_count,failed_count,metadata_provider,citation_provider,created_at)
         VALUES ($1,$2,NULL,'pubmed',$3,'pmid',$4,$5,$6,'queued',0,$7,$7,0,0,'','',now())
         ON CONFLICT (project_id,idempotency_key) WHERE idempotency_key IS NOT NULL DO NOTHING
         RETURNING id",
    )
    .bind(run_id)
    .bind(request.project_id)
    .bind(PMID_IMPORT_STRATEGY)
    .bind(request.idempotency_key.as_deref())
    .bind(&config)
    .bind(json!({ "pmid_count": count }))
    .bind(count)
    .fetch_optional(&mut *tx)
    .await?;

    let Some(run_id) = inserted else {
        let existing = sqlx::query(
            "SELECT id, config FROM acquisition_runs WHERE project_id=$1 AND idempotency_key=$2 FOR UPDATE",
        )
        .bind(request.project_id)
        .bind(request.idempotency_key.as_deref())
        .fetch_one(&mut *tx)
        .await?;
        let existing_id: Uuid = existing.get("id");
        let existing_config: serde_json::Value = existing.get("config");
        if existing_config != config {
            return Err(AcquisitionError::IdempotencyConflict {
                run_id: existing_id,
            });
        }
        tx.commit().await?;
        return Ok(PmidImportResult {
            run_id: existing_id,
            created: false,
        });
    };

    for (position, pmid) in request.pmids.iter().enumerate() {
        sqlx::query(
            "INSERT INTO pmid_import_items (acquisition_run_id,pmid,position,status)
             VALUES ($1,$2,$3,'queued')",
        )
        .bind(run_id)
        .bind(pmid)
        .bind(i32::try_from(position).unwrap_or(i32::MAX))
        .execute(&mut *tx)
        .await?;
    }
    crate::jobs::enqueue_job(
        &mut tx,
        &crate::jobs::job(
            Uuid::new_v4(),
            ProjectId::new(request.project_id),
            PMID_IMPORT_JOB_KIND,
            json!({ "project_id": request.project_id, "acquisition_id": run_id }),
            format!("{PMID_IMPORT_JOB_KIND}:{run_id}"),
        ),
    )
    .await
    .map_err(AcquisitionError::Queue)?;
    tx.commit().await?;
    Ok(PmidImportResult {
        run_id,
        created: true,
    })
}

/// A PubMed ID run as the worker reads it.
#[derive(Debug, Clone)]
pub struct PmidRun {
    pub run_id: Uuid,
    pub project_id: Uuid,
    pub status: String,
}

pub async fn load_pmid_run(pool: &PgPool, run_id: Uuid) -> Result<Option<PmidRun>, sqlx::Error> {
    let row = sqlx::query(
        "SELECT id, project_id, status FROM acquisition_runs WHERE id=$1 AND strategy=$2",
    )
    .bind(run_id)
    .bind(PMID_IMPORT_STRATEGY)
    .fetch_optional(pool)
    .await?;
    Ok(row.map(|row| PmidRun {
        run_id: row.get("id"),
        project_id: row.get("project_id"),
        status: row.get("status"),
    }))
}

/// Marks a queued run as running. A run that is already running is left as it is.
pub async fn start_pmid_run(pool: &PgPool, run_id: Uuid) -> Result<(), sqlx::Error> {
    sqlx::query(
        "UPDATE acquisition_runs SET status='running', started_at=COALESCE(started_at, now())
         WHERE id=$1 AND status IN ('queued', 'running')",
    )
    .bind(run_id)
    .execute(pool)
    .await?;
    Ok(())
}

/// The PMIDs of a run that still wait for a fetch, in the order they were given.
pub async fn queued_pmids(
    pool: &PgPool,
    run_id: Uuid,
    limit: i64,
) -> Result<Vec<String>, sqlx::Error> {
    sqlx::query_scalar(
        "SELECT pmid FROM pmid_import_items
         WHERE acquisition_run_id=$1 AND status='queued'
         ORDER BY position LIMIT $2",
    )
    .bind(run_id)
    .bind(limit)
    .fetch_all(pool)
    .await
}

/// The PMIDs among `pmids` that the project already holds, either as a report or as a
/// record that the duplicate check has not reached yet.
pub async fn pmids_in_project(
    pool: &PgPool,
    project_id: Uuid,
    pmids: &[String],
) -> Result<HashSet<String>, sqlx::Error> {
    if pmids.is_empty() {
        return Ok(HashSet::new());
    }
    let found: Vec<String> = sqlx::query_scalar(
        "SELECT i.normalized_value FROM report_identifiers i
           JOIN project_reports pr ON pr.report_id = i.report_id
          WHERE pr.project_id=$1 AND i.scheme='pmid' AND i.normalized_value = ANY($2)
         UNION
         SELECT ri.normalized_value FROM record_identifiers ri
           JOIN records r ON r.id = ri.record_id
          WHERE r.project_id=$1 AND ri.scheme='pmid' AND ri.normalized_value = ANY($2)",
    )
    .bind(project_id)
    .bind(pmids)
    .fetch_all(pool)
    .await?;
    Ok(found.into_iter().collect())
}

/// What one fetch decided for one PMID.
#[derive(Debug, Clone)]
pub enum PmidOutcome {
    /// A new record was saved for the article.
    Imported {
        pmid: String,
        title: Option<String>,
        record: RawRecord,
    },
    /// The project already holds this PMID, so nothing new was saved.
    AlreadyInProject { pmid: String, title: Option<String> },
    /// PubMed does not know this PMID.
    NotFound { pmid: String },
    /// The fetch for this PMID failed for good.
    Failed { pmid: String, error: String },
}

/// Saves the outcomes of one fetch, the new records included, in one transaction, and
/// refreshes the run's counters. Returns how many records were saved.
pub async fn save_pmid_outcomes(
    pool: &PgPool,
    project_id: Uuid,
    run_id: Uuid,
    outcomes: &[PmidOutcome],
) -> Result<i64, AcquisitionError> {
    let mut tx = pool.begin().await?;
    let mut saved = 0_i64;
    for outcome in outcomes {
        match outcome {
            PmidOutcome::Imported {
                pmid,
                title,
                record,
            } => {
                let source_key = format!("{run_id}:{pmid}");
                let record_id = match insert_raw_record(
                    &mut tx,
                    project_id,
                    run_id,
                    PMID_IMPORT_SOURCE,
                    &source_key,
                    record,
                )
                .await?
                {
                    Some(record_id) => {
                        saved += 1;
                        Some(record_id)
                    }
                    // An earlier attempt already saved this article: keep the item linked to it.
                    None => existing_record(&mut tx, project_id, &source_key).await?,
                };
                set_item(
                    &mut tx,
                    run_id,
                    pmid,
                    PmidItemStatus::Imported,
                    title.as_deref(),
                    record_id,
                    None,
                )
                .await?;
            }
            PmidOutcome::AlreadyInProject { pmid, title } => {
                set_item(
                    &mut tx,
                    run_id,
                    pmid,
                    PmidItemStatus::AlreadyInProject,
                    title.as_deref(),
                    None,
                    None,
                )
                .await?;
            }
            PmidOutcome::NotFound { pmid } => {
                set_item(
                    &mut tx,
                    run_id,
                    pmid,
                    PmidItemStatus::NotFound,
                    None,
                    None,
                    None,
                )
                .await?;
            }
            PmidOutcome::Failed { pmid, error } => {
                set_item(
                    &mut tx,
                    run_id,
                    pmid,
                    PmidItemStatus::Failed,
                    None,
                    None,
                    Some(error),
                )
                .await?;
            }
        }
    }
    sqlx::query(
        "UPDATE acquisition_runs SET
           queued_count=(SELECT count(*)::int FROM pmid_import_items WHERE acquisition_run_id=$1 AND status='queued'),
           fetched_count=(SELECT count(*)::int FROM pmid_import_items WHERE acquisition_run_id=$1 AND status IN ('imported','already_in_project')),
           failed_count=(SELECT count(*)::int FROM pmid_import_items WHERE acquisition_run_id=$1 AND status IN ('not_found','failed'))
         WHERE id=$1",
    )
    .bind(run_id)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(saved)
}

async fn existing_record(
    tx: &mut Transaction<'_, Postgres>,
    project_id: Uuid,
    source_key: &str,
) -> Result<Option<Uuid>, sqlx::Error> {
    sqlx::query_scalar("SELECT id FROM records WHERE project_id=$1 AND source=$2 AND source_key=$3")
        .bind(project_id)
        .bind(PMID_IMPORT_SOURCE)
        .bind(source_key)
        .fetch_optional(&mut **tx)
        .await
}

async fn set_item(
    tx: &mut Transaction<'_, Postgres>,
    run_id: Uuid,
    pmid: &str,
    status: PmidItemStatus,
    title: Option<&str>,
    record_id: Option<Uuid>,
    last_error: Option<&str>,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "UPDATE pmid_import_items
         SET status=$3, title=$4, record_id=$5, last_error=$6, processed_at=now()
         WHERE acquisition_run_id=$1 AND pmid=$2",
    )
    .bind(run_id)
    .bind(pmid)
    .bind(status.as_str())
    .bind(title)
    .bind(record_id)
    .bind(last_error)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

/// How many items of a run are in each state.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PmidRunCounts {
    pub queued: i64,
    pub imported: i64,
    pub already_in_project: i64,
    pub not_found: i64,
    pub failed: i64,
}

impl PmidRunCounts {
    /// PMIDs PubMed returned: new records plus those the project already held.
    pub fn found(&self) -> i64 {
        self.imported + self.already_in_project
    }

    /// PMIDs that were not fetched: unknown to PubMed, or failed.
    pub fn missed(&self) -> i64 {
        self.not_found + self.failed
    }
}

pub async fn pmid_run_counts(
    tx: &mut Transaction<'_, Postgres>,
    run_id: Uuid,
) -> Result<PmidRunCounts, sqlx::Error> {
    let row = sqlx::query(
        "SELECT count(*) FILTER (WHERE status='queued') AS queued,
                count(*) FILTER (WHERE status='imported') AS imported,
                count(*) FILTER (WHERE status='already_in_project') AS already_in_project,
                count(*) FILTER (WHERE status='not_found') AS not_found,
                count(*) FILTER (WHERE status='failed') AS failed
         FROM pmid_import_items WHERE acquisition_run_id=$1",
    )
    .bind(run_id)
    .fetch_one(&mut **tx)
    .await?;
    Ok(PmidRunCounts {
        queued: row.get("queued"),
        imported: row.get("imported"),
        already_in_project: row.get("already_in_project"),
        not_found: row.get("not_found"),
        failed: row.get("failed"),
    })
}

/// Sets a run's terminal status and counters. A completed run also records its domain
/// event. The caller holds the transaction, so the run's notification commits with it.
pub async fn complete_pmid_run(
    tx: &mut Transaction<'_, Postgres>,
    project_id: Uuid,
    run_id: Uuid,
    status: &str,
    counts: &PmidRunCounts,
) -> Result<(), AcquisitionError> {
    sqlx::query(
        "UPDATE acquisition_runs SET status=$2, fetched_count=$3, failed_count=$4, queued_count=0,
           started_at=COALESCE(started_at, now()), completed_at=now()
         WHERE id=$1",
    )
    .bind(run_id)
    .bind(status)
    .bind(i32::try_from(counts.found()).unwrap_or(i32::MAX))
    .bind(i32::try_from(counts.missed()).unwrap_or(i32::MAX))
    .execute(&mut **tx)
    .await?;
    if status == "completed" {
        crate::dispatch_automation_domain_event(
            tx,
            &AutomationDomainEvent::AcquisitionCompleted {
                project_id: ProjectId::new(project_id),
                acquisition_id: run_id,
            },
        )
        .await?;
    }
    Ok(())
}
