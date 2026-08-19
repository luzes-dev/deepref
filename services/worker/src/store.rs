use std::time::Duration;

use deepref_core::{IngestionItemStatus, IngestionStatus, Reference, WorkWithReferences};
use deepref_events::{
    CitationUpserted, DeadLetterRecord, DomainPayload, EntityType, EventEnvelope,
    MetricsRecomputeRequested, ProjectMembershipUpserted, SUBJECT_CITATION_UPSERTED, SUBJECT_DLQ,
    SUBJECT_METRICS_RECOMPUTE_REQUESTED, SUBJECT_PROJECT_MEMBERSHIP_UPSERTED,
    SUBJECT_UNRESOLVED_REFERENCE_UPSERTED, SUBJECT_WORK_FETCH_REQUESTED, SUBJECT_WORK_UPSERTED,
    UnresolvedReferenceUpserted, WorkFetchRequested, WorkUpserted, deterministic_event_id,
};
use sqlx::{PgPool, Postgres, Row, Transaction};
use uuid::Uuid;

#[derive(Debug, Clone)]
pub struct RuntimeSettings {
    pub crossref_mailto: String,
    pub rate_limit_per_second: u32,
    pub retry_attempts: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClaimState {
    Acquired,
    Busy,
    Completed,
}

pub async fn load_runtime_settings(pool: &PgPool) -> anyhow::Result<RuntimeSettings> {
    sqlx::query("INSERT INTO settings (id, crossref_mailto) VALUES (1, '') ON CONFLICT DO NOTHING")
        .execute(pool)
        .await?;
    let row = sqlx::query(
        "SELECT crossref_mailto, rate_limit_per_second, retry_attempts FROM settings WHERE id = 1",
    )
    .fetch_one(pool)
    .await?;
    Ok(RuntimeSettings {
        crossref_mailto: row.get("crossref_mailto"),
        rate_limit_per_second: row.get::<i32, _>("rate_limit_per_second").max(1) as u32,
        retry_attempts: row.get::<i32, _>("retry_attempts").max(1) as usize,
    })
}

pub async fn claim_event(
    pool: &PgPool,
    event_id: Uuid,
    owner: Uuid,
    lease: Duration,
) -> anyhow::Result<ClaimState> {
    let row = sqlx::query(
        r#"
        INSERT INTO processed_events
          (event_id, owner_token, lease_expires_at, attempts, processed_at, completed_at)
        VALUES ($1, $2, now() + ($3 * interval '1 millisecond'), 1, NULL, NULL)
        ON CONFLICT (event_id) DO UPDATE SET
          owner_token = EXCLUDED.owner_token,
          lease_expires_at = EXCLUDED.lease_expires_at,
          attempts = processed_events.attempts + 1,
          last_error = NULL
        WHERE processed_events.completed_at IS NULL
          AND (processed_events.lease_expires_at < now() OR processed_events.owner_token = EXCLUDED.owner_token)
        RETURNING event_id
        "#,
    ).bind(event_id).bind(owner).bind(lease.as_millis() as i64)
        .fetch_optional(pool).await?;
    if row.is_some() {
        return Ok(ClaimState::Acquired);
    }
    let completed: bool = sqlx::query_scalar(
        "SELECT completed_at IS NOT NULL FROM processed_events WHERE event_id=$1",
    )
    .bind(event_id)
    .fetch_one(pool)
    .await?;
    Ok(if completed {
        ClaimState::Completed
    } else {
        ClaimState::Busy
    })
}

pub async fn claim_doi(
    pool: &PgPool,
    doi: &str,
    owner: Uuid,
    lease: Duration,
) -> anyhow::Result<ClaimState> {
    let row = sqlx::query(
        r#"
        INSERT INTO doi_fetch_state
          (canonical_doi, status, owner_token, lease_expires_at, heartbeat_at, attempts, locked_at)
        VALUES ($1, 'fetching', $2, now() + ($3 * interval '1 millisecond'), now(), 1, now())
        ON CONFLICT (canonical_doi) DO UPDATE SET
          status = 'fetching', owner_token = EXCLUDED.owner_token,
          lease_expires_at = EXCLUDED.lease_expires_at, heartbeat_at = now(), locked_at = now(),
          attempts = doi_fetch_state.attempts + 1, last_error = NULL
        WHERE doi_fetch_state.status <> 'fetched'
          AND (doi_fetch_state.lease_expires_at IS NULL OR doi_fetch_state.lease_expires_at < now()
               OR doi_fetch_state.owner_token = EXCLUDED.owner_token)
        RETURNING canonical_doi
        "#,
    )
    .bind(doi)
    .bind(owner)
    .bind(lease.as_millis() as i64)
    .fetch_optional(pool)
    .await?;
    Ok(if row.is_some() {
        ClaimState::Acquired
    } else {
        ClaimState::Busy
    })
}

pub async fn renew_claims(
    pool: &PgPool,
    event_id: Uuid,
    doi: &str,
    owner: Uuid,
    lease: Duration,
) -> anyhow::Result<bool> {
    let mut tx = pool.begin().await?;
    let event = sqlx::query(
        "UPDATE processed_events SET lease_expires_at = now() + ($3 * interval '1 millisecond') \
         WHERE event_id = $1 AND owner_token = $2 AND completed_at IS NULL",
    )
    .bind(event_id)
    .bind(owner)
    .bind(lease.as_millis() as i64)
    .execute(&mut *tx)
    .await?;
    let doi = sqlx::query(
        "UPDATE doi_fetch_state SET lease_expires_at = now() + ($4 * interval '1 millisecond'), \
         heartbeat_at = now(), updated_at = now() WHERE canonical_doi = $1 AND owner_token = $2 \
         AND status = 'fetching' AND lease_expires_at > now() - ($3 * interval '1 millisecond')",
    )
    .bind(doi)
    .bind(owner)
    .bind(lease.as_millis() as i64)
    .bind(lease.as_millis() as i64)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(event.rows_affected() == 1 && doi.rows_affected() == 1)
}

pub async fn is_cached(pool: &PgPool, doi: &str) -> anyhow::Result<bool> {
    Ok(
        sqlx::query("SELECT 1 FROM works WHERE canonical_doi = $1 AND fetch_status = 'fetched'")
            .bind(doi)
            .fetch_optional(pool)
            .await?
            .is_some(),
    )
}

pub async fn mark_fetching(
    pool: &PgPool,
    event: &EventEnvelope<WorkFetchRequested>,
    doi: &str,
) -> anyhow::Result<()> {
    sqlx::query(
        "INSERT INTO ingestion_items (ingestion_id,project_id,canonical_doi,depth,parent_doi,status,attempts) \
         VALUES ($1,$2,$3,$4,$5,'fetching',1) ON CONFLICT (ingestion_id,canonical_doi) DO UPDATE SET \
         status = CASE WHEN ingestion_items.status IN ('fetched','not_found') THEN ingestion_items.status ELSE 'fetching' END, \
         attempts = ingestion_items.attempts + 1",
    ).bind(event.payload.ingestion_id).bind(event.payload.project_id).bind(doi)
        .bind(event.payload.depth).bind(&event.payload.parent_doi).execute(pool).await?;
    Ok(())
}

pub async fn release_event_claim(
    pool: &PgPool,
    event_id: Uuid,
    owner: Uuid,
    error: &str,
) -> anyhow::Result<()> {
    sqlx::query(
        "UPDATE processed_events SET owner_token=NULL,lease_expires_at=now(),last_error=$3 \
         WHERE event_id=$1 AND owner_token=$2 AND completed_at IS NULL",
    )
    .bind(event_id)
    .bind(owner)
    .bind(error)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn complete_without_doi(
    pool: &PgPool,
    event: &EventEnvelope<WorkFetchRequested>,
    doi: &str,
    owner: Uuid,
    status: IngestionItemStatus,
    error: Option<&str>,
    dead_letter: Option<&DeadLetterRecord>,
) -> anyhow::Result<()> {
    let mut tx = pool.begin().await?;
    validate_event_ownership(&mut tx, event.event_id, owner).await?;
    if let Some(record) = dead_letter {
        persist_dead_letter(&mut tx, record, Some(event)).await?;
    }
    complete_item_and_claim(&mut tx, event, doi, owner, status, error).await?;
    tx.commit().await?;
    Ok(())
}

pub async fn ingestion_cancelled(pool: &PgPool, ingestion_id: Uuid) -> anyhow::Result<bool> {
    let status: String = sqlx::query_scalar("SELECT status FROM ingestions WHERE id = $1")
        .bind(ingestion_id)
        .fetch_one(pool)
        .await?;
    Ok(status == IngestionStatus::Cancelled.as_str())
}

pub async fn attach_cached(
    pool: &PgPool,
    event: &EventEnvelope<WorkFetchRequested>,
    doi: &str,
    owner: Uuid,
) -> anyhow::Result<()> {
    let mut tx = pool.begin().await?;
    validate_event_ownership(&mut tx, event.event_id, owner).await?;
    attach_project_work(&mut tx, &event.payload, doi).await?;
    sqlx::query(
        "INSERT INTO unresolved_references \
         (id, project_id, source_doi, raw_unstructured, article_title, author, year, volume, first_page) \
         SELECT id, $1, source_doi, raw_unstructured, article_title, author, year, volume, first_page \
         FROM fetched_unresolved_reference_facts WHERE source_doi = $2 ON CONFLICT DO NOTHING",
    ).bind(event.payload.project_id).bind(doi).execute(&mut *tx).await?;
    if event.payload.depth < event.payload.max_depth {
        let targets = sqlx::query(
            "SELECT target_doi FROM fetched_citation_facts WHERE source_doi=$1 ORDER BY target_doi",
        )
        .bind(doi)
        .fetch_all(&mut *tx)
        .await?;
        for target in targets {
            let target = target.get::<String, _>("target_doi");
            persist_citation(&mut tx, event, doi, &target).await?;
            enqueue_child(&mut tx, event, doi, &target).await?;
        }
    }
    emit_membership(&mut tx, event, doi).await?;
    emit_metrics_request(&mut tx, event).await?;
    complete_item_and_claim(
        &mut tx,
        event,
        doi,
        owner,
        IngestionItemStatus::Fetched,
        None,
    )
    .await?;
    tx.commit().await?;
    Ok(())
}

pub async fn finalize_success(
    pool: &PgPool,
    event: &EventEnvelope<WorkFetchRequested>,
    doi: &str,
    owner: Uuid,
    work: &WorkWithReferences,
) -> anyhow::Result<()> {
    let mut tx = pool.begin().await?;
    validate_ownership(&mut tx, event.event_id, doi, owner).await?;
    persist_work(&mut tx, work).await?;
    attach_project_work(&mut tx, &event.payload, doi).await?;
    emit_work(&mut tx, event, work).await?;
    emit_membership(&mut tx, event, doi).await?;

    for reference in &work.references {
        match &reference.doi {
            Some(target) => persist_citation(&mut tx, event, doi, target).await?,
            None => persist_unresolved(&mut tx, event, doi, reference).await?,
        }
    }
    // Child items and their deterministic durable jobs are inserted before the
    // terminal counts are evaluated.
    if event.payload.depth < event.payload.max_depth {
        for target in work
            .references
            .iter()
            .filter_map(|reference| reference.doi.as_deref())
        {
            enqueue_child(&mut tx, event, doi, target).await?;
        }
    }
    emit_metrics_request(&mut tx, event).await?;
    complete_item_and_claim(
        &mut tx,
        event,
        doi,
        owner,
        IngestionItemStatus::Fetched,
        None,
    )
    .await?;
    sqlx::query(
        "UPDATE doi_fetch_state SET status = 'fetched', fetched_at = now(), owner_token = NULL, \
         lease_expires_at = NULL, heartbeat_at = now(), updated_at = now() \
         WHERE canonical_doi = $1 AND owner_token = $2",
    )
    .bind(doi)
    .bind(owner)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(())
}

pub async fn finalize_terminal_failure(
    pool: &PgPool,
    event: &EventEnvelope<WorkFetchRequested>,
    doi: &str,
    owner: Uuid,
    status: IngestionItemStatus,
    error: &str,
    dead_letter: Option<&DeadLetterRecord>,
) -> anyhow::Result<()> {
    let mut tx = pool.begin().await?;
    validate_ownership(&mut tx, event.event_id, doi, owner).await?;
    if let Some(record) = dead_letter {
        persist_dead_letter(&mut tx, record, Some(event)).await?;
    }
    complete_item_and_claim(&mut tx, event, doi, owner, status, Some(error)).await?;
    sqlx::query(
        "UPDATE doi_fetch_state SET status = $3, last_error = $4, owner_token = NULL, \
         lease_expires_at = NULL, updated_at = now() WHERE canonical_doi = $1 AND owner_token = $2",
    )
    .bind(doi)
    .bind(owner)
    .bind(if status == IngestionItemStatus::NotFound {
        "not_found"
    } else {
        "failed"
    })
    .bind(error)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(())
}

pub async fn release_retryable(
    pool: &PgPool,
    event_id: Uuid,
    doi: &str,
    owner: Uuid,
    error: &str,
) -> anyhow::Result<()> {
    let mut tx = pool.begin().await?;
    sqlx::query(
        "UPDATE processed_events SET owner_token = NULL, lease_expires_at = now(), last_error = $3 \
         WHERE event_id = $1 AND owner_token = $2 AND completed_at IS NULL",
    )
    .bind(event_id)
    .bind(owner)
    .bind(error)
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        "UPDATE doi_fetch_state SET status = 'failed', owner_token = NULL, lease_expires_at = now(), \
         last_error = $3, updated_at = now() WHERE canonical_doi = $1 AND owner_token = $2",
    ).bind(doi).bind(owner).bind(error).execute(&mut *tx).await?;
    tx.commit().await?;
    Ok(())
}

pub async fn persist_malformed_dead_letter(
    pool: &PgPool,
    record: &DeadLetterRecord,
    raw: serde_json::Value,
) -> anyhow::Result<()> {
    let mut tx = pool.begin().await?;
    let event_id = deterministic_event_id(
        1,
        SUBJECT_DLQ,
        EntityType::DeadLetter,
        &record.identity,
        record.delivery_count as i64,
    );
    sqlx::query(
        "INSERT INTO dead_letter_records \
         (identity, source_subject, source_event_id, delivery_count, reason_code, payload_sha256, payload, job_event_id) \
         VALUES ($1,$2,$3,$4,$5,$6,$7,$8) ON CONFLICT (identity) DO UPDATE SET \
         delivery_count = GREATEST(dead_letter_records.delivery_count, EXCLUDED.delivery_count), last_seen_at = now()",
    ).bind(&record.identity).bind(&record.source_subject).bind(record.source_event_id)
        .bind(record.delivery_count as i64).bind(&record.reason_code).bind(&record.payload_sha256)
        .bind(raw).bind(event_id).execute(&mut *tx).await?;
    tx.commit().await?;
    Ok(())
}

async fn validate_ownership(
    tx: &mut Transaction<'_, Postgres>,
    event_id: Uuid,
    doi: &str,
    owner: Uuid,
) -> anyhow::Result<()> {
    let valid: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM processed_events e JOIN doi_fetch_state d ON d.canonical_doi = $2 \
         WHERE e.event_id = $1 AND e.owner_token = $3 AND e.completed_at IS NULL \
         AND e.lease_expires_at > now() AND d.owner_token = $3 AND d.lease_expires_at > now())",
    ).bind(event_id).bind(doi).bind(owner).fetch_one(&mut **tx).await?;
    if !valid {
        anyhow::bail!("lease ownership was lost before commit");
    }
    Ok(())
}

async fn validate_event_ownership(
    tx: &mut Transaction<'_, Postgres>,
    event_id: Uuid,
    owner: Uuid,
) -> anyhow::Result<()> {
    let valid: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM processed_events WHERE event_id=$1 AND owner_token=$2 \
         AND completed_at IS NULL AND lease_expires_at > now())",
    )
    .bind(event_id)
    .bind(owner)
    .fetch_one(&mut **tx)
    .await?;
    if !valid {
        anyhow::bail!("event claim ownership was lost before commit");
    }
    Ok(())
}

async fn persist_work(
    tx: &mut Transaction<'_, Postgres>,
    work: &WorkWithReferences,
) -> anyhow::Result<()> {
    sqlx::query(
        r#"INSERT INTO works
        (canonical_doi,title,abstract_text,work_type,publisher,container_title,issued_year,published_year,
         url,total_citations,references_count,metadata_provider,citation_provider,fetch_status,fetched_at,raw)
        VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,'fetched',now(),$14)
        ON CONFLICT (canonical_doi) DO UPDATE SET title=EXCLUDED.title, abstract_text=EXCLUDED.abstract_text,
        work_type=EXCLUDED.work_type,publisher=EXCLUDED.publisher,container_title=EXCLUDED.container_title,
        issued_year=EXCLUDED.issued_year,published_year=EXCLUDED.published_year,url=EXCLUDED.url,
        total_citations=EXCLUDED.total_citations,references_count=EXCLUDED.references_count,
        metadata_provider=EXCLUDED.metadata_provider,citation_provider=EXCLUDED.citation_provider,
        fetch_status='fetched',fetched_at=now(),raw=EXCLUDED.raw"#,
    ).bind(&work.work.doi).bind(&work.work.title).bind(&work.work.abstract_text)
        .bind(&work.work.work_type).bind(&work.work.publisher).bind(&work.work.container_title)
        .bind(work.work.issued_year).bind(work.work.published_year).bind(&work.work.url)
        .bind(work.work.total_citations).bind(work.work.references_count)
        .bind(&work.work.metadata_provider).bind(&work.work.citation_provider).bind(&work.raw)
        .execute(&mut **tx).await?;
    ensure_v2_report(tx, &work.work.doi).await?;
    Ok(())
}

async fn attach_project_work(
    tx: &mut Transaction<'_, Postgres>,
    payload: &WorkFetchRequested,
    doi: &str,
) -> anyhow::Result<()> {
    sqlx::query(
        "INSERT INTO project_works (project_id,canonical_doi,first_seen_ingestion_id,seed,min_depth) \
         VALUES ($1,$2,$3,$4,$5) ON CONFLICT (project_id,canonical_doi) DO UPDATE SET \
         seed = project_works.seed OR EXCLUDED.seed, min_depth = LEAST(project_works.min_depth,EXCLUDED.min_depth)",
    ).bind(payload.project_id).bind(doi).bind(payload.ingestion_id).bind(payload.depth == 0)
        .bind(payload.depth).execute(&mut **tx).await?;
    let report_id = ensure_v2_report(tx, doi).await?;
    let record_id = Uuid::new_v5(
        &Uuid::NAMESPACE_URL,
        format!(
            "deepref:record:{}:{}:{}",
            payload.project_id, payload.ingestion_id, doi
        )
        .as_bytes(),
    );
    sqlx::query(
        "INSERT INTO records (id,project_id,report_id,acquisition_run_id,source,source_key,title,abstract_text,publication_year,raw) SELECT $1,$2,$3,$4,'worker_ingestion',$5,r.title,r.abstract_text,r.publication_year,jsonb_build_object('doi',$5) FROM reports r WHERE r.id=$3 ON CONFLICT (project_id,source,source_key) DO UPDATE SET report_id=EXCLUDED.report_id,acquisition_run_id=EXCLUDED.acquisition_run_id,title=EXCLUDED.title,abstract_text=EXCLUDED.abstract_text,publication_year=EXCLUDED.publication_year",
    )
    .bind(record_id)
    .bind(payload.project_id)
    .bind(report_id)
    .bind(payload.ingestion_id)
    .bind(doi)
    .execute(&mut **tx)
    .await?;
    sqlx::query(
        "INSERT INTO project_reports (project_id,report_id,first_seen_record_id) VALUES ($1,$2,$3) ON CONFLICT (project_id,report_id) DO UPDATE SET first_seen_record_id=COALESCE(project_reports.first_seen_record_id,EXCLUDED.first_seen_record_id)",
    )
    .bind(payload.project_id)
    .bind(report_id)
    .bind(record_id)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

async fn ensure_v2_report(tx: &mut Transaction<'_, Postgres>, doi: &str) -> anyhow::Result<Uuid> {
    let report_id: Uuid = sqlx::query_scalar(
        "SELECT format('%s-%s-%s-%s-%s',substr(md5('deepref:report:'||$1),1,8),substr(md5('deepref:report:'||$1),9,4),substr(md5('deepref:report:'||$1),13,4),substr(md5('deepref:report:'||$1),17,4),substr(md5('deepref:report:'||$1),21,12))::uuid",
    )
    .bind(doi)
    .fetch_one(&mut **tx)
    .await?;
    sqlx::query(
        "INSERT INTO reports (id,title,abstract_text,publication_year,journal,url,work_type,publisher,container_title,total_citations,references_count,raw) SELECT $1,title,abstract_text,COALESCE(published_year,issued_year),container_title,url,work_type,publisher,container_title,total_citations,references_count,raw FROM works WHERE canonical_doi=$2 ON CONFLICT (id) DO UPDATE SET title=EXCLUDED.title,abstract_text=EXCLUDED.abstract_text,publication_year=EXCLUDED.publication_year,journal=EXCLUDED.journal,url=EXCLUDED.url,work_type=EXCLUDED.work_type,publisher=EXCLUDED.publisher,container_title=EXCLUDED.container_title,total_citations=EXCLUDED.total_citations,references_count=EXCLUDED.references_count,raw=EXCLUDED.raw,updated_at=now()",
    )
    .bind(report_id)
    .bind(doi)
    .execute(&mut **tx)
    .await?;
    let identifier_id = Uuid::new_v5(
        &Uuid::NAMESPACE_URL,
        format!("deepref:doi:{doi}").as_bytes(),
    );
    sqlx::query(
        "INSERT INTO report_identifiers (id,report_id,scheme,value,normalized_value) VALUES ($1,$2,'doi',$3,$3) ON CONFLICT (scheme,normalized_value) DO UPDATE SET report_id=EXCLUDED.report_id,value=EXCLUDED.value",
    )
    .bind(identifier_id)
    .bind(report_id)
    .bind(doi)
    .execute(&mut **tx)
    .await?;
    Ok(report_id)
}

async fn persist_citation(
    tx: &mut Transaction<'_, Postgres>,
    event: &EventEnvelope<WorkFetchRequested>,
    source: &str,
    target: &str,
) -> anyhow::Result<()> {
    sqlx::query(
        "INSERT INTO works (canonical_doi,fetch_status) VALUES ($1,'stub') ON CONFLICT DO NOTHING",
    )
    .bind(target)
    .execute(&mut **tx)
    .await?;
    sqlx::query(
        "INSERT INTO fetched_citation_facts (source_doi,target_doi) VALUES ($1,$2) ON CONFLICT DO NOTHING",
    ).bind(source).bind(target).execute(&mut **tx).await?;
    sqlx::query(
        "INSERT INTO legacy_citations (project_id,source_doi,target_doi,first_seen_ingestion_id) VALUES ($1,$2,$3,$4) ON CONFLICT DO NOTHING",
    ).bind(event.payload.project_id).bind(source).bind(target).bind(event.payload.ingestion_id)
        .execute(&mut **tx).await?;
    let source_report_id = ensure_v2_report(tx, source).await?;
    let target_report_id = ensure_v2_report(tx, target).await?;
    sqlx::query(
        "INSERT INTO citations (project_id,source_report_id,target_report_id,source,first_seen_ingestion_id,legacy_source_doi,legacy_target_doi) VALUES ($1,$2,$3,'crossref-reference',$4,$5,$6) ON CONFLICT (project_id,source_report_id,target_report_id) DO UPDATE SET first_seen_ingestion_id=COALESCE(citations.first_seen_ingestion_id,EXCLUDED.first_seen_ingestion_id)",
    )
    .bind(event.payload.project_id)
    .bind(source_report_id)
    .bind(target_report_id)
    .bind(event.payload.ingestion_id)
    .bind(source)
    .bind(target)
    .execute(&mut **tx)
    .await?;
    let payload = DomainPayload::CitationUpserted(CitationUpserted {
        project_id: event.payload.project_id,
        source_doi: source.to_owned(),
        target_doi: target.to_owned(),
    });
    emit_domain(
        tx,
        SUBJECT_CITATION_UPSERTED,
        EntityType::Citation,
        &format!("{}|{source}|{target}", event.payload.project_id),
        event,
        payload,
    )
    .await?;
    Ok(())
}

async fn persist_unresolved(
    tx: &mut Transaction<'_, Postgres>,
    event: &EventEnvelope<WorkFetchRequested>,
    source: &str,
    reference: &Reference,
) -> anyhow::Result<()> {
    let id = deepref_graph::unresolved_reference_id(source, reference);
    sqlx::query(
        "INSERT INTO fetched_unresolved_reference_facts \
         (id,source_doi,raw_unstructured,article_title,author,year,volume,first_page) \
         VALUES ($1,$2,$3,$4,$5,$6,$7,$8) ON CONFLICT DO NOTHING",
    )
    .bind(&id)
    .bind(source)
    .bind(&reference.raw_unstructured)
    .bind(&reference.article_title)
    .bind(&reference.author)
    .bind(&reference.year)
    .bind(&reference.volume)
    .bind(&reference.first_page)
    .execute(&mut **tx)
    .await?;
    sqlx::query(
        "INSERT INTO unresolved_references \
         (id,project_id,source_doi,raw_unstructured,article_title,author,year,volume,first_page) \
         VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9) ON CONFLICT DO NOTHING",
    )
    .bind(&id)
    .bind(event.payload.project_id)
    .bind(source)
    .bind(&reference.raw_unstructured)
    .bind(&reference.article_title)
    .bind(&reference.author)
    .bind(&reference.year)
    .bind(&reference.volume)
    .bind(&reference.first_page)
    .execute(&mut **tx)
    .await?;
    let payload = DomainPayload::UnresolvedReferenceUpserted(UnresolvedReferenceUpserted {
        id: id.clone(),
        project_id: event.payload.project_id,
        source_doi: source.to_owned(),
        raw_unstructured: reference.raw_unstructured.clone(),
    });
    emit_domain(
        tx,
        SUBJECT_UNRESOLVED_REFERENCE_UPSERTED,
        EntityType::UnresolvedReference,
        &format!("{}|{id}", event.payload.project_id),
        event,
        payload,
    )
    .await?;
    Ok(())
}

async fn enqueue_child(
    tx: &mut Transaction<'_, Postgres>,
    event: &EventEnvelope<WorkFetchRequested>,
    parent_doi: &str,
    target: &str,
) -> anyhow::Result<()> {
    let inserted = sqlx::query(
        "INSERT INTO ingestion_items (ingestion_id,project_id,canonical_doi,depth,parent_doi,status) \
         VALUES ($1,$2,$3,$4,$5,'queued') ON CONFLICT DO NOTHING",
    ).bind(event.payload.ingestion_id).bind(event.payload.project_id).bind(target)
        .bind(event.payload.depth + 1).bind(parent_doi).execute(&mut **tx).await?;
    if inserted.rows_affected() == 1 {
        let revision: i64 = sqlx::query_scalar("SELECT nextval('graph_domain_revision_seq')")
            .fetch_one(&mut **tx)
            .await?;
        let child = EventEnvelope::v1(
            SUBJECT_WORK_FETCH_REQUESTED,
            "deepref.worker",
            EntityType::Work,
            format!("{}|{target}", event.payload.ingestion_id),
            revision,
            event.correlation_id,
            Some(event.event_id),
            WorkFetchRequested {
                project_id: event.payload.project_id,
                ingestion_id: event.payload.ingestion_id,
                doi: target.to_owned(),
                depth: event.payload.depth + 1,
                max_depth: event.payload.max_depth,
                parent_doi: Some(parent_doi.to_owned()),
            },
        );
        sqlx::query(
            "UPDATE ingestion_items SET work_event_id=$3 WHERE ingestion_id=$1 AND canonical_doi=$2",
        )
        .bind(event.payload.ingestion_id)
        .bind(target)
        .bind(child.event_id)
        .execute(&mut **tx)
        .await?;
        deepref_postgres::enqueue_job(
            tx,
            &deepref_postgres::job(
                child.event_id,
                "work_fetch_requested",
                serde_json::to_value(&child)?,
                format!("work_fetch:{}", child.event_id),
            ),
        )
        .await?;
    }
    Ok(())
}

async fn emit_work(
    tx: &mut Transaction<'_, Postgres>,
    event: &EventEnvelope<WorkFetchRequested>,
    work: &WorkWithReferences,
) -> anyhow::Result<()> {
    emit_domain(
        tx,
        SUBJECT_WORK_UPSERTED,
        EntityType::Work,
        &work.work.doi,
        event,
        DomainPayload::WorkUpserted(WorkUpserted {
            doi: work.work.doi.clone(),
            title: work.work.title.clone(),
            issued_year: work.work.issued_year,
            total_citations: work.work.total_citations,
        }),
    )
    .await?;
    Ok(())
}

async fn emit_membership(
    tx: &mut Transaction<'_, Postgres>,
    event: &EventEnvelope<WorkFetchRequested>,
    doi: &str,
) -> anyhow::Result<()> {
    emit_domain(
        tx,
        SUBJECT_PROJECT_MEMBERSHIP_UPSERTED,
        EntityType::ProjectMembership,
        &format!("{}|{doi}", event.payload.project_id),
        event,
        DomainPayload::ProjectMembershipUpserted(ProjectMembershipUpserted {
            project_id: event.payload.project_id,
            doi: doi.to_owned(),
            seed: event.payload.depth == 0,
            min_depth: event.payload.depth,
        }),
    )
    .await?;
    Ok(())
}

async fn emit_metrics_request(
    tx: &mut Transaction<'_, Postgres>,
    event: &EventEnvelope<WorkFetchRequested>,
) -> anyhow::Result<()> {
    let envelope = emit_domain(
        tx,
        SUBJECT_METRICS_RECOMPUTE_REQUESTED,
        EntityType::Metric,
        &event.payload.project_id.to_string(),
        event,
        DomainPayload::MetricsRecomputeRequested(MetricsRecomputeRequested {
            project_id: event.payload.project_id,
            ingestion_id: Some(event.payload.ingestion_id),
        }),
    )
    .await?;
    deepref_postgres::enqueue_job(
        tx,
        &deepref_postgres::job(
            envelope.event_id,
            "recompute_metrics",
            serde_json::to_value(&envelope)?,
            format!(
                "recompute_metrics:{}:{}",
                event.payload.project_id, envelope.event_id
            ),
        ),
    )
    .await?;
    Ok(())
}

async fn emit_domain(
    tx: &mut Transaction<'_, Postgres>,
    subject: &str,
    entity_type: EntityType,
    entity_key: &str,
    cause: &EventEnvelope<WorkFetchRequested>,
    payload: DomainPayload,
) -> anyhow::Result<EventEnvelope<DomainPayload>> {
    let revision: i64 = sqlx::query_scalar("SELECT nextval('graph_domain_revision_seq')")
        .fetch_one(&mut **tx)
        .await?;
    let envelope = EventEnvelope::v1(
        subject,
        "deepref.worker",
        entity_type,
        entity_key.to_owned(),
        revision,
        cause.correlation_id,
        Some(cause.event_id),
        payload,
    );
    sqlx::query(
        "INSERT INTO domain_events (event_id,schema_version,event_type,entity_type,entity_key,revision,payload,correlation_id,causation_id,created_at) \
         VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10) ON CONFLICT (event_id) DO NOTHING",
    ).bind(envelope.event_id).bind(envelope.schema_version as i16).bind(&envelope.event_type)
        .bind(envelope.entity_type.as_str()).bind(&envelope.entity_key).bind(envelope.revision)
        .bind(serde_json::to_value(&envelope.payload)?).bind(envelope.correlation_id)
        .bind(envelope.causation_id).bind(envelope.occurred_at).execute(&mut **tx).await?;
    Ok(envelope)
}

async fn persist_dead_letter(
    tx: &mut Transaction<'_, Postgres>,
    record: &DeadLetterRecord,
    _cause: Option<&EventEnvelope<WorkFetchRequested>>,
) -> anyhow::Result<()> {
    let event_id = deterministic_event_id(
        1,
        SUBJECT_DLQ,
        EntityType::DeadLetter,
        &record.identity,
        record.delivery_count as i64,
    );
    sqlx::query(
        "INSERT INTO dead_letter_records \
         (identity,source_subject,source_event_id,delivery_count,reason_code,payload_sha256,job_event_id) \
         VALUES ($1,$2,$3,$4,$5,$6,$7) ON CONFLICT (identity) DO UPDATE SET \
         delivery_count=GREATEST(dead_letter_records.delivery_count,EXCLUDED.delivery_count),last_seen_at=now()",
    ).bind(&record.identity).bind(&record.source_subject).bind(record.source_event_id)
        .bind(record.delivery_count as i64).bind(&record.reason_code).bind(&record.payload_sha256)
        .bind(event_id).execute(&mut **tx).await?;
    Ok(())
}

async fn complete_item_and_claim(
    tx: &mut Transaction<'_, Postgres>,
    event: &EventEnvelope<WorkFetchRequested>,
    doi: &str,
    owner: Uuid,
    status: IngestionItemStatus,
    error: Option<&str>,
) -> anyhow::Result<()> {
    sqlx::query(
        "UPDATE ingestion_items SET status=$4,last_error=$5,fetched_at=now() \
         WHERE ingestion_id=$1 AND project_id=$2 AND canonical_doi=$3",
    )
    .bind(event.payload.ingestion_id)
    .bind(event.payload.project_id)
    .bind(doi)
    .bind(status.as_str())
    .bind(error)
    .execute(&mut **tx)
    .await?;
    // Counts are deliberately updated after all child items have been inserted.
    sqlx::query(
        r#"UPDATE ingestions SET
        status = CASE WHEN status='cancelled' THEN status
          WHEN NOT EXISTS (SELECT 1 FROM ingestion_items WHERE ingestion_id=$1 AND status IN ('queued','fetching'))
          THEN CASE WHEN EXISTS (SELECT 1 FROM ingestion_items WHERE ingestion_id=$1 AND status IN ('failed','not_found')) THEN 'failed' ELSE 'completed' END
          WHEN status='queued' THEN 'running' ELSE status END,
        started_at=COALESCE(started_at,now()),
        completed_at=CASE WHEN NOT EXISTS (SELECT 1 FROM ingestion_items WHERE ingestion_id=$1 AND status IN ('queued','fetching')) THEN now() ELSE completed_at END,
        fetched_count=(SELECT count(*)::int FROM ingestion_items WHERE ingestion_id=$1 AND status='fetched'),
        failed_count=(SELECT count(*)::int FROM ingestion_items WHERE ingestion_id=$1 AND status IN ('failed','not_found')),
        queued_count=(SELECT count(*)::int FROM ingestion_items WHERE ingestion_id=$1 AND status IN ('queued','fetching'))
        WHERE id=$1"#,
    ).bind(event.payload.ingestion_id).execute(&mut **tx).await?;
    let completed = sqlx::query(
        "UPDATE processed_events SET completed_at=now(),processed_at=now(),owner_token=NULL,lease_expires_at=NULL,last_error=$3 \
         WHERE event_id=$1 AND owner_token=$2 AND completed_at IS NULL",
    ).bind(event.event_id).bind(owner).bind(error).execute(&mut **tx).await?;
    if completed.rows_affected() != 1 {
        anyhow::bail!("event claim ownership was lost before completion");
    }
    Ok(())
}
