use deepref_application::RawRecord;
use deepref_domain::{ImportFormat, normalize_bibliography_title};
use serde_json::Value;
use sqlx::{PgPool, Postgres, Row, Transaction};
use thiserror::Error;
use uuid::Uuid;

#[derive(Debug, Error)]
pub enum AcquisitionError {
    #[error("database operation failed")]
    Database(#[from] sqlx::Error),
    #[error("project not found")]
    ProjectNotFound,
    #[error("idempotency key already belongs to acquisition run {run_id} with different input")]
    IdempotencyConflict { run_id: Uuid },
    #[error("failed to serialize acquisition metadata")]
    Serialization(#[from] serde_json::Error),
}

#[derive(Debug, Clone)]
pub struct ImportPersistRequest {
    pub project_id: Uuid,
    pub source: String,
    pub strategy: String,
    pub format: ImportFormat,
    pub idempotency_key: Option<String>,
    pub config: Value,
    pub metadata: Value,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ImportPersistResult {
    pub run_id: Uuid,
    pub created: bool,
    pub records_created: i64,
}

pub async fn persist_import(
    pool: &PgPool,
    request: &ImportPersistRequest,
    records: &[RawRecord],
) -> Result<ImportPersistResult, AcquisitionError> {
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

    let inserted_id = if request.idempotency_key.is_some() {
        sqlx::query_scalar::<_, Uuid>(
            "INSERT INTO acquisition_runs
             (id,project_id,legacy_ingestion_id,source,strategy,format,idempotency_key,config,metadata,status,
              max_depth,seed_count,queued_count,fetched_count,failed_count,metadata_provider,citation_provider,
              created_at,started_at,completed_at)
             VALUES ($1,$2,NULL,$3,$4,$5,$6,$7,$8,'completed',0,0,$9,0,0,'','',now(),now(),now())
             ON CONFLICT (project_id,idempotency_key) WHERE idempotency_key IS NOT NULL DO NOTHING
             RETURNING id",
        )
        .bind(run_id)
        .bind(request.project_id)
        .bind(&request.source)
        .bind(&request.strategy)
        .bind(request.format.as_str())
        .bind(request.idempotency_key.as_deref())
        .bind(&request.config)
        .bind(&request.metadata)
        .bind(i32::try_from(records.len()).unwrap_or(i32::MAX))
        .fetch_optional(&mut *tx)
        .await?
    } else {
        Some(
            sqlx::query_scalar::<_, Uuid>(
                "INSERT INTO acquisition_runs
                 (id,project_id,legacy_ingestion_id,source,strategy,format,idempotency_key,config,metadata,status,
                  max_depth,seed_count,queued_count,fetched_count,failed_count,metadata_provider,citation_provider,
                  created_at,started_at,completed_at)
                 VALUES ($1,$2,NULL,$3,$4,$5,$6,$7,$8,'completed',0,0,$9,0,0,'','',now(),now(),now())
                 RETURNING id",
            )
                .bind(run_id)
                .bind(request.project_id)
                .bind(&request.source)
                .bind(&request.strategy)
                .bind(request.format.as_str())
                .bind(request.idempotency_key.as_deref())
                .bind(&request.config)
                .bind(&request.metadata)
                .bind(i32::try_from(records.len()).unwrap_or(i32::MAX))
                .fetch_one(&mut *tx)
                .await?,
        )
    };

    let Some(run_id) = inserted_id else {
        let existing = sqlx::query(
            "SELECT id, config FROM acquisition_runs WHERE project_id=$1 AND idempotency_key=$2 FOR UPDATE",
        )
        .bind(request.project_id)
        .bind(request.idempotency_key.as_deref())
        .fetch_one(&mut *tx)
        .await?;
        let existing_id: Uuid = existing.get("id");
        let existing_config: Value = existing.get("config");
        if existing_config != request.config {
            return Err(AcquisitionError::IdempotencyConflict {
                run_id: existing_id,
            });
        }
        tx.commit().await?;
        return Ok(ImportPersistResult {
            run_id: existing_id,
            created: false,
            records_created: 0,
        });
    };

    let mut records_created = 0_i64;
    for (index, record) in records.iter().enumerate() {
        let record_id = Uuid::new_v4();
        let source_key = format!("{run_id}:{index}");
        let inserted = sqlx::query(
            "INSERT INTO records
             (id,project_id,report_id,acquisition_run_id,source,source_key,title,abstract_text,
              publication_year,journal,authors,source_identifiers,normalized_title,raw)
             VALUES ($1,$2,NULL,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13)
             ON CONFLICT (project_id,source,source_key) DO NOTHING",
        )
        .bind(record_id)
        .bind(request.project_id)
        .bind(run_id)
        .bind(&request.source)
        .bind(source_key)
        .bind(&record.title)
        .bind(&record.abstract_text)
        .bind(record.publication_year)
        .bind(&record.journal)
        .bind(serde_json::to_value(&record.authors)?)
        .bind(serde_json::to_value(&record.source_identifiers)?)
        .bind(record.title.as_deref().map(normalize_bibliography_title))
        .bind(&record.raw)
        .execute(&mut *tx)
        .await?;
        if inserted.rows_affected() == 0 {
            continue;
        }
        records_created += 1;
        for identifier in &record.source_identifiers {
            sqlx::query(
                "INSERT INTO record_identifiers
                 (id,record_id,scheme,value,normalized_value)
                 VALUES ($1,$2,$3,$4,$5)
                 ON CONFLICT (record_id,scheme,normalized_value) DO NOTHING",
            )
            .bind(Uuid::new_v4())
            .bind(record_id)
            .bind(identifier.scheme.as_str())
            .bind(&identifier.value)
            .bind(&identifier.normalized_value)
            .execute(&mut *tx)
            .await?;
        }
    }
    tx.commit().await?;
    Ok(ImportPersistResult {
        run_id,
        created: true,
        records_created,
    })
}

pub async fn ensure_legacy_acquisition_run(
    tx: &mut Transaction<'_, Postgres>,
    ingestion_id: Uuid,
    project_id: Uuid,
    max_depth: i32,
    seed_count: i32,
    metadata_provider: &str,
    citation_provider: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO acquisition_runs
         (id,project_id,legacy_ingestion_id,source,strategy,status,max_depth,seed_count,queued_count,
          fetched_count,failed_count,metadata_provider,citation_provider,created_at)
         VALUES ($1,$2,$1,'legacy-ingestion','citation_traversal','queued',$3,$4,$4,0,0,$5,$6,now())
         ON CONFLICT (id) DO UPDATE SET
           project_id=EXCLUDED.project_id,
           max_depth=EXCLUDED.max_depth,
           seed_count=EXCLUDED.seed_count,
           queued_count=EXCLUDED.queued_count,
           metadata_provider=EXCLUDED.metadata_provider,
           citation_provider=EXCLUDED.citation_provider",
    )
    .bind(ingestion_id)
    .bind(project_id)
    .bind(max_depth)
    .bind(seed_count)
    .bind(metadata_provider)
    .bind(citation_provider)
    .execute(&mut **tx)
    .await?;
    Ok(())
}
