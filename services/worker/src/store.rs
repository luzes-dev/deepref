use deepref_core::{
    FetchStatus, IngestionItemStatus, IngestionStatus, Reference, WorkWithReferences,
};
use deepref_events::WorkFetchRequested;
use sqlx::{PgPool, Row};

pub(crate) struct RuntimeSettings {
    pub(crate) crossref_mailto: String,
    pub(crate) rate_limit_per_second: u32,
    pub(crate) retry_attempts: usize,
}

pub(crate) async fn ingestion_cancelled(
    pool: &PgPool,
    ingestion_id: uuid::Uuid,
) -> anyhow::Result<bool> {
    let row = sqlx::query("SELECT status FROM ingestions WHERE id = $1")
        .bind(ingestion_id)
        .fetch_one(pool)
        .await?;
    Ok(row.get::<String, _>("status") == IngestionStatus::Cancelled.as_str())
}

pub(crate) async fn load_runtime_settings(pool: &PgPool) -> anyhow::Result<RuntimeSettings> {
    sqlx::query(
        r#"
        INSERT INTO settings (id, crossref_mailto)
        VALUES (1, '')
        ON CONFLICT (id) DO NOTHING
        "#,
    )
    .execute(pool)
    .await?;

    let row = sqlx::query(
        r#"
        SELECT crossref_mailto, rate_limit_per_second, retry_attempts
        FROM settings WHERE id = 1
        "#,
    )
    .fetch_one(pool)
    .await?;
    Ok(RuntimeSettings {
        crossref_mailto: row.get("crossref_mailto"),
        rate_limit_per_second: row.get::<i32, _>("rate_limit_per_second").max(1) as u32,
        retry_attempts: row.get::<i32, _>("retry_attempts").max(1) as usize,
    })
}

pub(crate) async fn event_processed(pool: &PgPool, event_id: uuid::Uuid) -> anyhow::Result<bool> {
    let exists = sqlx::query("SELECT 1 FROM processed_events WHERE event_id = $1")
        .bind(event_id)
        .fetch_optional(pool)
        .await?;
    Ok(exists.is_some())
}

pub(crate) async fn mark_event_processed(
    pool: &PgPool,
    event_id: uuid::Uuid,
) -> anyhow::Result<()> {
    sqlx::query(
        r#"
        INSERT INTO processed_events (event_id)
        VALUES ($1)
        ON CONFLICT DO NOTHING
        "#,
    )
    .bind(event_id)
    .execute(pool)
    .await?;
    Ok(())
}

pub(crate) async fn claim_item(
    pool: &PgPool,
    payload: &WorkFetchRequested,
    doi: &str,
) -> anyhow::Result<bool> {
    let row = sqlx::query(
        r#"
        INSERT INTO ingestion_items (ingestion_id, project_id, canonical_doi, depth, parent_doi, status)
        VALUES ($1, $2, $3, $4, $5, 'fetching')
        ON CONFLICT (ingestion_id, canonical_doi)
        DO UPDATE SET
          status = CASE
            WHEN ingestion_items.status IN ('fetched', 'failed', 'not_found', 'skipped') THEN ingestion_items.status
            ELSE 'fetching'
          END,
          attempts = ingestion_items.attempts + 1
        RETURNING status
        "#,
    )
    .bind(payload.ingestion_id)
    .bind(payload.project_id)
    .bind(doi)
    .bind(payload.depth)
    .bind(&payload.parent_doi)
    .fetch_one(pool)
    .await?;
    Ok(row.get::<String, _>("status") == IngestionItemStatus::Fetching.as_str())
}

pub(crate) async fn claim_global_fetch(pool: &PgPool, doi: &str) -> anyhow::Result<bool> {
    let row = sqlx::query(
        r#"
        INSERT INTO doi_fetch_state (canonical_doi, status, locked_at)
        VALUES ($1, 'fetching', now())
        ON CONFLICT (canonical_doi)
        DO UPDATE SET
          status = CASE
            WHEN doi_fetch_state.status IN ('fetched', 'fetching') THEN doi_fetch_state.status
            ELSE 'fetching'
          END,
          locked_at = CASE
            WHEN doi_fetch_state.status IN ('fetched', 'fetching') THEN doi_fetch_state.locked_at
            ELSE now()
          END
        RETURNING status
        "#,
    )
    .bind(doi)
    .fetch_one(pool)
    .await?;
    Ok(row.get::<String, _>("status") == IngestionItemStatus::Fetching.as_str())
}

pub(crate) async fn persist_work(
    pool: &PgPool,
    payload: &WorkFetchRequested,
    work: &WorkWithReferences,
) -> anyhow::Result<()> {
    sqlx::query(
        r#"
        INSERT INTO works
          (canonical_doi, title, abstract_text, work_type, publisher, container_title,
           issued_year, published_year, url, total_citations, references_count,
           metadata_provider, citation_provider, fetch_status, fetched_at, raw)
        VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,now(),$15)
        ON CONFLICT (canonical_doi) DO UPDATE SET
          title = EXCLUDED.title,
          abstract_text = EXCLUDED.abstract_text,
          work_type = EXCLUDED.work_type,
          publisher = EXCLUDED.publisher,
          container_title = EXCLUDED.container_title,
          issued_year = EXCLUDED.issued_year,
          published_year = EXCLUDED.published_year,
          url = EXCLUDED.url,
          total_citations = EXCLUDED.total_citations,
          references_count = EXCLUDED.references_count,
          metadata_provider = EXCLUDED.metadata_provider,
          citation_provider = EXCLUDED.citation_provider,
          fetch_status = EXCLUDED.fetch_status,
          fetched_at = now(),
          raw = EXCLUDED.raw
        "#,
    )
    .bind(&work.work.doi)
    .bind(&work.work.title)
    .bind(&work.work.abstract_text)
    .bind(&work.work.work_type)
    .bind(&work.work.publisher)
    .bind(&work.work.container_title)
    .bind(work.work.issued_year)
    .bind(work.work.published_year)
    .bind(&work.work.url)
    .bind(work.work.total_citations)
    .bind(work.work.references_count)
    .bind(&work.work.metadata_provider)
    .bind(&work.work.citation_provider)
    .bind(FetchStatus::Fetched.as_str())
    .bind(&work.raw)
    .execute(pool)
    .await?;

    sqlx::query(
        r#"
        INSERT INTO project_works (project_id, canonical_doi, first_seen_ingestion_id, seed, min_depth)
        VALUES ($1, $2, $3, $4, $5)
        ON CONFLICT (project_id, canonical_doi) DO UPDATE SET
          min_depth = LEAST(project_works.min_depth, EXCLUDED.min_depth)
        "#,
    )
    .bind(payload.project_id)
    .bind(&work.work.doi)
    .bind(payload.ingestion_id)
    .bind(payload.depth == 0)
    .bind(payload.depth)
    .execute(pool)
    .await?;

    Ok(())
}

pub(crate) async fn persist_citation(
    pool: &PgPool,
    payload: &WorkFetchRequested,
    source_doi: &str,
    target_doi: &str,
) -> anyhow::Result<()> {
    sqlx::query("INSERT INTO works (canonical_doi, fetch_status) VALUES ($1, 'stub') ON CONFLICT DO NOTHING")
        .bind(target_doi)
        .execute(pool)
        .await?;
    sqlx::query(
        r#"
        INSERT INTO citations (project_id, source_doi, target_doi, first_seen_ingestion_id)
        VALUES ($1, $2, $3, $4)
        ON CONFLICT DO NOTHING
        "#,
    )
    .bind(payload.project_id)
    .bind(source_doi)
    .bind(target_doi)
    .bind(payload.ingestion_id)
    .execute(pool)
    .await?;
    Ok(())
}

pub(crate) async fn persist_unresolved_reference(
    pool: &PgPool,
    payload: &WorkFetchRequested,
    source_doi: &str,
    reference: &Reference,
) -> anyhow::Result<()> {
    let id = deepref_graph::unresolved_reference_id(source_doi, reference);
    sqlx::query(
        r#"
        INSERT INTO unresolved_references
          (id, project_id, source_doi, raw_unstructured, article_title, author, year, volume, first_page)
        VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9)
        ON CONFLICT DO NOTHING
        "#,
    )
    .bind(id)
    .bind(payload.project_id)
    .bind(source_doi)
    .bind(&reference.raw_unstructured)
    .bind(&reference.article_title)
    .bind(&reference.author)
    .bind(&reference.year)
    .bind(&reference.volume)
    .bind(&reference.first_page)
    .execute(pool)
    .await?;
    Ok(())
}

pub(crate) async fn mark_item(
    pool: &PgPool,
    payload: &WorkFetchRequested,
    doi: &str,
    status: IngestionItemStatus,
    error: Option<&str>,
) -> anyhow::Result<()> {
    sqlx::query(
        r#"
        UPDATE ingestion_items
        SET status = $4, last_error = $5, fetched_at = CASE WHEN $4 IN ('fetched','failed','not_found','skipped') THEN now() ELSE fetched_at END
        WHERE ingestion_id = $1 AND project_id = $2 AND canonical_doi = $3
        "#,
    )
    .bind(payload.ingestion_id)
    .bind(payload.project_id)
    .bind(doi)
    .bind(status.as_str())
    .bind(error)
    .execute(pool)
    .await?;

    sqlx::query(
        r#"
        UPDATE ingestions SET
          status = CASE
            WHEN status = 'cancelled' THEN status
            WHEN (
              SELECT count(*) FROM ingestion_items
              WHERE ingestion_id = $1 AND status IN ('queued','fetching')
            ) = 0
              THEN CASE
                WHEN (
                  SELECT count(*) FROM ingestion_items
                  WHERE ingestion_id = $1 AND status IN ('failed','not_found')
                ) > 0 THEN 'failed'
                ELSE 'completed'
              END
            WHEN status = 'queued' THEN 'running'
            ELSE status
          END,
          started_at = COALESCE(started_at, now()),
          completed_at = CASE
            WHEN status = 'cancelled' THEN completed_at
            WHEN (
              SELECT count(*) FROM ingestion_items
              WHERE ingestion_id = $1 AND status IN ('queued','fetching')
            ) = 0 THEN now()
            ELSE completed_at
          END,
          fetched_count = (SELECT count(*)::int FROM ingestion_items WHERE ingestion_id = $1 AND status = 'fetched'),
          failed_count = (SELECT count(*)::int FROM ingestion_items WHERE ingestion_id = $1 AND status IN ('failed','not_found')),
          queued_count = (SELECT count(*)::int FROM ingestion_items WHERE ingestion_id = $1 AND status IN ('queued','fetching'))
        WHERE id = $1
        "#,
    )
    .bind(payload.ingestion_id)
    .execute(pool)
    .await?;
    Ok(())
}

pub(crate) async fn mark_global_fetch(
    pool: &PgPool,
    doi: &str,
    status: FetchStatus,
    error: Option<&str>,
) -> anyhow::Result<()> {
    sqlx::query(
        r#"
        UPDATE doi_fetch_state
        SET status = $2, last_error = $3, fetched_at = CASE WHEN $2 = 'fetched' THEN now() ELSE fetched_at END
        WHERE canonical_doi = $1
        "#,
    )
    .bind(doi)
    .bind(status.as_str())
    .bind(error)
    .execute(pool)
    .await?;
    Ok(())
}
