use std::time::Duration;

use deepref_application::{AutomationDomainEvent, jobs::JobLease};
use deepref_domain::{IngestionItemStatus, Reference, WorkWithReferences};
use deepref_domain::events::{EventEnvelope, EntityType, WorkFetchRequested, SUBJECT_WORK_FETCH_REQUESTED};
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


pub async fn claim_doi(
    pool: &PgPool,
    doi: &str,
    owner: Uuid,
    lease: Duration,
) -> anyhow::Result<ClaimState> {
    let mut tx = pool.begin().await?;
    let report_id = ensure_v2_report(&mut tx, doi).await?;
    let acquired = sqlx::query(
        "INSERT INTO report_fetch_state (report_id,canonical_doi,provider,status,owner_token,lease_expires_at,heartbeat_at,attempts,locked_at) VALUES ($1,$2,'crossref','fetching',$3,now()+($4*interval '1 millisecond'),now(),1,now()) ON CONFLICT (report_id) DO UPDATE SET status='fetching',owner_token=EXCLUDED.owner_token,lease_expires_at=EXCLUDED.lease_expires_at,heartbeat_at=now(),locked_at=now(),attempts=report_fetch_state.attempts+1,last_error=NULL WHERE report_fetch_state.status<>'fetched' AND (report_fetch_state.lease_expires_at IS NULL OR report_fetch_state.lease_expires_at<now() OR report_fetch_state.owner_token=EXCLUDED.owner_token)",
    ).bind(report_id).bind(doi).bind(owner).bind(lease.as_millis() as i64).execute(&mut *tx).await?.rows_affected() == 1;
    tx.commit().await?;
    Ok(if acquired { ClaimState::Acquired } else { ClaimState::Busy })
}

pub async fn renew_claims(
    pool: &PgPool,
    lease: &JobLease,
    doi: &str,
    owner: Uuid,
    duration: Duration,
) -> anyhow::Result<bool> {
    let mut tx = pool.begin().await?;
    validate_event_ownership(&mut tx, lease).await?;
    let renewed = sqlx::query(
        "UPDATE report_fetch_state SET lease_expires_at=now()+($3*interval '1 millisecond'),heartbeat_at=now(),updated_at=now() WHERE report_id=(SELECT report_id FROM report_identifiers WHERE scheme='doi' AND normalized_value=$1) AND owner_token=$2 AND status='fetching' AND lease_expires_at>now()",
    ).bind(doi).bind(owner).bind(duration.as_millis() as i64).execute(&mut *tx).await?.rows_affected() == 1;
    tx.commit().await?;
    Ok(renewed)
}

pub async fn is_cached(pool: &PgPool, doi: &str) -> anyhow::Result<bool> {
    Ok(sqlx::query("SELECT 1 FROM report_fetch_state f JOIN report_identifiers i ON i.report_id=f.report_id JOIN works w ON w.canonical_doi=f.canonical_doi WHERE i.scheme='doi' AND i.normalized_value=$1 AND f.provider='crossref' AND f.status='fetched'")
        .bind(doi).fetch_optional(pool).await?.is_some())
}

pub async fn mark_fetching(
    pool: &PgPool,
    event: &EventEnvelope<WorkFetchRequested>,
    doi: &str,
    lease: &JobLease,
) -> anyhow::Result<bool> {
    let mut tx = pool.begin().await?;
    validate_event_ownership(&mut tx, lease).await?;
    lock_run(&mut tx, event).await?;
    let changed = sqlx::query("UPDATE acquisition_items SET status='fetching',attempts=attempts+1 WHERE acquisition_id=$1 AND project_id=$2 AND canonical_doi=$3 AND status IN ('queued','fetching')")
        .bind(event.payload.acquisition_id).bind(event.payload.project_id).bind(doi).execute(&mut *tx).await?.rows_affected() == 1;
    tx.commit().await?;
    Ok(changed)
}

pub async fn complete_without_doi(
    pool: &PgPool,
    event: &EventEnvelope<WorkFetchRequested>,
    doi: &str,
    lease: &JobLease,
    status: IngestionItemStatus,
    error: Option<&str>,
) -> anyhow::Result<()> {
    let mut tx = pool.begin().await?;
    validate_event_ownership(&mut tx, lease).await?;
    lock_run(&mut tx, event).await?;
    complete_item_and_claim(&mut tx, event, doi, status, error).await?;
    tx.commit().await?;
    Ok(())
}

pub async fn acquisition_cancelled(pool: &PgPool, acquisition_id: Uuid) -> anyhow::Result<bool> {
    let status: String = sqlx::query_scalar("SELECT status FROM acquisition_runs WHERE id=$1")
        .bind(acquisition_id).fetch_one(pool).await?;
    Ok(status == "cancelled")
}

pub async fn attach_cached(
    pool: &PgPool,
    event: &EventEnvelope<WorkFetchRequested>,
    doi: &str,
    lease: &JobLease,
) -> anyhow::Result<()> {
    let mut tx = pool.begin().await?;
    validate_event_ownership(&mut tx, lease).await?;
    lock_run(&mut tx, event).await?;
    let source_report_id = ensure_v2_report(&mut tx, doi).await?;
    let cached_doi: String = sqlx::query_scalar(
        "SELECT canonical_doi FROM report_fetch_state WHERE report_id=$1 AND status='fetched'",
    ).bind(source_report_id).fetch_one(&mut *tx).await?;
    attach_project_work(&mut tx, &event.payload, doi).await?;
    sqlx::query(
        "INSERT INTO project_unresolved_references \
         (id, project_id, source_report_id, raw_unstructured, article_title, author, year, volume, first_page) \
         SELECT id, $1, $3, raw_unstructured, article_title, author, year, volume, first_page \
         FROM fetched_unresolved_reference_facts WHERE source_doi = $2 ON CONFLICT DO NOTHING",
    ).bind(event.payload.project_id).bind(&cached_doi).bind(source_report_id).execute(&mut *tx).await?;
    if event.payload.depth < event.payload.max_depth {
        let targets = sqlx::query(
            "SELECT target_doi FROM fetched_citation_facts WHERE source_doi=$1 ORDER BY target_doi",
        )
        .bind(&cached_doi)
        .fetch_all(&mut *tx)
        .await?;
        for target in targets {
            let target = target.get::<String, _>("target_doi");
            persist_citation(&mut tx, event, doi, &target).await?;
            enqueue_child(&mut tx, event, doi, &target).await?;
        }
    }
    emit_metrics_request(&mut tx, event).await?;
    complete_item_and_claim(
        &mut tx,
        event,
        doi,
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
    lease: &JobLease,
    work: &WorkWithReferences,
) -> anyhow::Result<()> {
    let mut tx = pool.begin().await?;
    validate_ownership(&mut tx, lease, doi, owner).await?;
    lock_run(&mut tx, event).await?;
    persist_work(&mut tx, work).await?;
    attach_project_work(&mut tx, &event.payload, doi).await?;

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
        IngestionItemStatus::Fetched,
        None,
    )
    .await?;
    sqlx::query(
        "UPDATE report_fetch_state SET canonical_doi=$1, status = 'fetched', fetched_at = now(), owner_token = NULL, \
         lease_expires_at = NULL, heartbeat_at = now(), updated_at = now() \
         WHERE report_id = (SELECT report_id FROM report_identifiers WHERE scheme='doi' AND normalized_value=$1) AND owner_token = $2",
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
    lease: &JobLease,
    status: IngestionItemStatus,
    error: &str,
) -> anyhow::Result<()> {
    let mut tx = pool.begin().await?;
    validate_ownership(&mut tx, lease, doi, owner).await?;
    lock_run(&mut tx, event).await?;
    complete_item_and_claim(&mut tx, event, doi, status, Some(error)).await?;
    sqlx::query(
        "UPDATE report_fetch_state SET status = $3, last_error = $4, owner_token = NULL, \
         lease_expires_at = NULL, updated_at = now() WHERE report_id=(SELECT report_id FROM report_identifiers WHERE scheme='doi' AND normalized_value=$1) AND owner_token = $2 AND status<>'fetched'",
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
    lease: &JobLease,
    doi: &str,
    owner: Uuid,
    error: &str,
) -> anyhow::Result<()> {
    let mut tx = pool.begin().await?;
    validate_event_ownership(&mut tx, lease).await?;
    sqlx::query(
        "UPDATE report_fetch_state SET status = 'failed', owner_token = NULL, lease_expires_at = now(), \
         last_error = $3, updated_at = now() WHERE report_id=(SELECT report_id FROM report_identifiers WHERE scheme='doi' AND normalized_value=$1) AND owner_token = $2 AND status<>'fetched'",
    ).bind(doi).bind(owner).bind(error).execute(&mut *tx).await?;
    tx.commit().await?;
    Ok(())
}


async fn validate_ownership(
    tx: &mut Transaction<'_, Postgres>,
    lease: &JobLease,
    doi: &str,
    owner: Uuid,
) -> anyhow::Result<()> {
    validate_event_ownership(tx, lease).await?;
    let valid: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM report_fetch_state WHERE report_id=(SELECT report_id FROM report_identifiers WHERE scheme='doi' AND normalized_value=$1) AND owner_token=$2 AND lease_expires_at>now())",
    ).bind(doi).bind(owner).fetch_one(&mut **tx).await?;
    if !valid {
        anyhow::bail!("lease ownership was lost before commit");
    }
    Ok(())
}

async fn validate_event_ownership(
    tx: &mut Transaction<'_, Postgres>,
    lease: &JobLease,
) -> anyhow::Result<()> {
    let valid = sqlx::query(
        "SELECT id FROM jobs WHERE id=$1 AND state='running' AND lease_owner=$2 AND attempts=$3 AND leased_until>clock_timestamp() FOR UPDATE",
    ).bind(lease.id).bind(&lease.owner).bind(lease.attempt).fetch_optional(&mut **tx).await?;
    if valid.is_none() {
        anyhow::bail!("job lease ownership was lost before persistence");
    }
    Ok(())
}

async fn lock_run(tx: &mut Transaction<'_, Postgres>, event: &EventEnvelope<WorkFetchRequested>) -> anyhow::Result<()> {
    sqlx::query("SELECT id FROM acquisition_runs WHERE id=$1 AND project_id=$2 FOR UPDATE")
        .bind(event.payload.acquisition_id).bind(event.payload.project_id).fetch_one(&mut **tx).await?;
    Ok(())
}

async fn persist_work(
    tx: &mut Transaction<'_, Postgres>,
    work: &WorkWithReferences,
) -> anyhow::Result<()> {
    sqlx::query(
        r#"INSERT INTO works
        (canonical_doi,title,abstract_text,work_type,publisher,container_title,issued_year,published_year,
         url,total_citations,references_count,metadata_provider,citation_provider,fetched_at,raw)
        VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,now(),$14)
        ON CONFLICT (canonical_doi) DO UPDATE SET title=EXCLUDED.title, abstract_text=EXCLUDED.abstract_text,
        work_type=EXCLUDED.work_type,publisher=EXCLUDED.publisher,container_title=EXCLUDED.container_title,
        issued_year=EXCLUDED.issued_year,published_year=EXCLUDED.published_year,url=EXCLUDED.url,
        total_citations=EXCLUDED.total_citations,references_count=EXCLUDED.references_count,
        metadata_provider=EXCLUDED.metadata_provider,citation_provider=EXCLUDED.citation_provider,
        fetched_at=now(),raw=EXCLUDED.raw"#,
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
    let report_id = ensure_v2_report(tx, doi).await?;
    let record_id = Uuid::new_v5(
        &Uuid::NAMESPACE_URL,
        format!(
            "deepref:record:{}:{}:{}",
            payload.project_id, payload.acquisition_id, doi
        )
        .as_bytes(),
    );
    let record_id: Uuid = sqlx::query_scalar(
        "INSERT INTO records (id,project_id,report_id,acquisition_run_id,source,source_key,title,abstract_text,publication_year,raw) SELECT $1,$2,$3,$4,'crossref',$5,r.title,r.abstract_text,r.publication_year,jsonb_build_object('doi',$6,'seed',$7,'min_depth',$8) FROM reports r WHERE r.id=$3 ON CONFLICT (project_id,source,source_key) DO UPDATE SET report_id=EXCLUDED.report_id,title=EXCLUDED.title,abstract_text=EXCLUDED.abstract_text,publication_year=EXCLUDED.publication_year,raw=records.raw || EXCLUDED.raw RETURNING id",
    )
    .bind(record_id)
    .bind(payload.project_id)
    .bind(report_id)
    .bind(payload.acquisition_id)
    .bind(format!("{}|{doi}", payload.acquisition_id))
    .bind(doi)
    .bind(payload.depth == 0)
    .bind(payload.depth)
    .fetch_one(&mut **tx)
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
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1,0))").bind(doi).execute(&mut **tx).await?;
    sqlx::query("INSERT INTO works (canonical_doi) VALUES ($1) ON CONFLICT DO NOTHING").bind(doi).execute(&mut **tx).await?;
    let report_id: Uuid = sqlx::query_scalar(
        "SELECT COALESCE((SELECT report_id FROM report_identifiers WHERE scheme='doi' AND normalized_value=$1),format('%s-%s-%s-%s-%s',substr(md5('deepref:report:'||$1),1,8),substr(md5('deepref:report:'||$1),9,4),substr(md5('deepref:report:'||$1),13,4),substr(md5('deepref:report:'||$1),17,4),substr(md5('deepref:report:'||$1),21,12))::uuid)",
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
        "INSERT INTO report_identifiers (id,report_id,scheme,value,normalized_value) VALUES ($1,$2,'doi',$3,$3) ON CONFLICT (scheme,normalized_value) DO NOTHING",
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
        "INSERT INTO works (canonical_doi) VALUES ($1) ON CONFLICT DO NOTHING",
    )
    .bind(target)
    .execute(&mut **tx)
    .await?;
    sqlx::query(
        "INSERT INTO fetched_citation_facts (source_doi,target_doi) VALUES ($1,$2) ON CONFLICT DO NOTHING",
    ).bind(source).bind(target).execute(&mut **tx).await?;
    let source_report_id = ensure_v2_report(tx, source).await?;
    let target_report_id = ensure_v2_report(tx, target).await?;
    sqlx::query(
        "INSERT INTO citations (project_id,source_report_id,target_report_id,source,acquisition_run_id) VALUES ($1,$2,$3,'crossref-reference',$4) ON CONFLICT (project_id,source_report_id,target_report_id) DO UPDATE SET acquisition_run_id=COALESCE(citations.acquisition_run_id,EXCLUDED.acquisition_run_id)",
    )
    .bind(event.payload.project_id)
    .bind(source_report_id)
    .bind(target_report_id)
    .bind(event.payload.acquisition_id)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

async fn persist_unresolved(
    tx: &mut Transaction<'_, Postgres>,
    event: &EventEnvelope<WorkFetchRequested>,
    source: &str,
    reference: &Reference,
) -> anyhow::Result<()> {
    let id = deepref_domain::graph::unresolved_reference_id(source, reference);
    let source_report_id = ensure_v2_report(tx, source).await?;
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
        "INSERT INTO project_unresolved_references \
         (id,project_id,source_report_id,raw_unstructured,article_title,author,year,volume,first_page) \
         VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9) ON CONFLICT DO NOTHING",
    )
    .bind(&id)
    .bind(event.payload.project_id)
    .bind(source_report_id)
    .bind(&reference.raw_unstructured)
    .bind(&reference.article_title)
    .bind(&reference.author)
    .bind(&reference.year)
    .bind(&reference.volume)
    .bind(&reference.first_page)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

async fn enqueue_child(
    tx: &mut Transaction<'_, Postgres>,
    event: &EventEnvelope<WorkFetchRequested>,
    parent_doi: &str,
    target: &str,
) -> anyhow::Result<()> {
    let report_id = ensure_v2_report(tx, target).await?;
    let inserted = sqlx::query(
        "INSERT INTO acquisition_items (acquisition_id,project_id,report_id,canonical_doi,depth,parent_doi,status) VALUES ($1,$2,$3,$4,$5,$6,'queued') ON CONFLICT DO NOTHING",
    ).bind(event.payload.acquisition_id).bind(event.payload.project_id).bind(report_id).bind(target)
        .bind(event.payload.depth + 1).bind(parent_doi).execute(&mut **tx).await?;
    if inserted.rows_affected() == 1 {
        sqlx::query("UPDATE acquisition_runs SET queued_count=queued_count+1 WHERE id=$1").bind(event.payload.acquisition_id).execute(&mut **tx).await?;
        let child = EventEnvelope::v1(
            SUBJECT_WORK_FETCH_REQUESTED,
            "deepref.worker",
            EntityType::Work,
            format!("{}|{target}", event.payload.acquisition_id),
            0,
            event.correlation_id,
            Some(event.event_id),
            WorkFetchRequested {
                project_id: event.payload.project_id,
                acquisition_id: event.payload.acquisition_id,
                doi: target.to_owned(),
                depth: event.payload.depth + 1,
                max_depth: event.payload.max_depth,
                parent_doi: Some(parent_doi.to_owned()),
            },
        );
        sqlx::query(
            "UPDATE acquisition_items SET work_event_id=$3 WHERE acquisition_id=$1 AND canonical_doi=$2",
        )
        .bind(event.payload.acquisition_id)
        .bind(target)
        .bind(child.event_id)
        .execute(&mut **tx)
        .await?;
        deepref_app::enqueue_job(
            tx,
            &deepref_app::job(
                child.event_id,
                event.payload.project_id.into(),
                deepref_domain::JobKind::WorkFetchRequested,
                serde_json::to_value(&child)?,
                format!("work_fetch:{}", child.event_id),
            ),
        )
        .await?;
    }
    Ok(())
}

async fn emit_metrics_request(
    tx: &mut Transaction<'_, Postgres>,
    event: &EventEnvelope<WorkFetchRequested>,
) -> anyhow::Result<()> {
    let revision: i64 = sqlx::query_scalar("UPDATE projects SET metric_requested_revision=metric_requested_revision+1 WHERE id=$1 RETURNING metric_requested_revision")
        .bind(event.payload.project_id).fetch_one(&mut **tx).await?;
    deepref_app::enqueue_job(tx, &deepref_app::job(
        Uuid::new_v4(), event.payload.project_id.into(), deepref_domain::JobKind::RecomputeMetrics,
        serde_json::json!({"project_id":event.payload.project_id,"revision":revision}),
        format!("recompute_metrics:{}:{revision}",event.payload.project_id),
    )).await?;
    Ok(())
}

pub(crate) fn acquisition_notification(
    status: &str,
    project_id: Uuid,
    acquisition_id: Uuid,
    fetched_count: i32,
    failed_count: i32,
) -> deepref_app::NotificationDraft {
    let plural = |count: i32| if count == 1 { "article" } else { "articles" };
    let project_id = Some(project_id);
    let payload = serde_json::json!({
        "acquisition_id": acquisition_id,
        "fetched_count": fetched_count,
        "failed_count": failed_count,
    });
    match status {
        "completed" if failed_count > 0 => deepref_app::NotificationDraft::warning(
            "acquisition.completed",
            project_id,
            "Import completed with problems",
            Some(format!(
                "{} {} imported. {} {} could not be fetched; open the run to see which and why.",
                fetched_count,
                plural(fetched_count),
                failed_count,
                plural(failed_count)
            )),
            payload,
        ),
        "completed" => deepref_app::NotificationDraft::success(
            "acquisition.completed",
            project_id,
            "Import completed",
            Some(format!(
                "{} {} imported.",
                fetched_count,
                plural(fetched_count)
            )),
            payload,
        ),
        _ => deepref_app::NotificationDraft::error(
            "acquisition.failed",
            project_id,
            "Import failed",
            Some(format!(
                "None of the {} {} could be fetched; open the run to see why.",
                failed_count,
                plural(failed_count)
            )),
            payload,
        ),
    }
}

/// Terminal status for an import run once no item is queued or fetching.
///
/// A run is `failed` only when nothing was fetched and at least one item failed or was not
/// found. A run that fetched some articles and missed others is `completed`; the UI reports
/// those misses as "completed with problems".
pub(crate) fn settled_run_status(fetched: i64, failed: i64) -> &'static str {
    if fetched == 0 && failed > 0 {
        "failed"
    } else {
        "completed"
    }
}

async fn complete_item_and_claim(
    tx: &mut Transaction<'_, Postgres>,
    event: &EventEnvelope<WorkFetchRequested>,
    doi: &str,
    status: IngestionItemStatus,
    error: Option<&str>,
) -> anyhow::Result<()> {
    let previous_acquisition_status = sqlx::query_scalar::<_, String>(
        "SELECT status FROM acquisition_runs WHERE id=$1 AND project_id=$2 FOR UPDATE",
    ).bind(event.payload.acquisition_id).bind(event.payload.project_id).fetch_one(&mut **tx).await?;
    let transitioned = sqlx::query(
        "UPDATE acquisition_items SET status=$4,last_error=$5,fetched_at=CASE WHEN $4='fetched' THEN now() ELSE fetched_at END WHERE acquisition_id=$1 AND project_id=$2 AND canonical_doi=$3 AND status IN ('queued','fetching')",
    ).bind(event.payload.acquisition_id).bind(event.payload.project_id).bind(doi).bind(status.as_str()).bind(error).execute(&mut **tx).await?.rows_affected();
    if transitioned == 0 {
        return Ok(());
    }
    sqlx::query("UPDATE acquisition_runs SET queued_count=queued_count-1,fetched_count=fetched_count+$2,failed_count=failed_count+$3 WHERE id=$1")
        .bind(event.payload.acquisition_id).bind(i32::from(status == IngestionItemStatus::Fetched))
        .bind(i32::from(matches!(status, IngestionItemStatus::Failed | IngestionItemStatus::NotFound)))
        .execute(&mut **tx).await?;
    let (acquisition_status, fetched_count, failed_count) = sqlx::query_as::<_, (String, i32, i32)>(
        "UPDATE acquisition_runs SET status=CASE WHEN status='cancelled' THEN status WHEN queued_count=0 THEN CASE WHEN fetched_count=0 AND failed_count>0 THEN 'failed' ELSE 'completed' END WHEN status='queued' THEN 'running' ELSE status END,started_at=COALESCE(started_at,now()),completed_at=CASE WHEN queued_count=0 THEN COALESCE(completed_at,now()) ELSE completed_at END WHERE id=$1 RETURNING status,fetched_count,failed_count",
    ).bind(event.payload.acquisition_id)
    .fetch_one(&mut **tx).await?;
    if previous_acquisition_status != "completed" && acquisition_status == "completed" {
        deepref_app::dispatch_automation_domain_event(
            tx,
            &AutomationDomainEvent::AcquisitionCompleted {
                project_id: event.payload.project_id.into(),
                acquisition_id: event.payload.acquisition_id,
            },
        )
        .await?;
    }
    if previous_acquisition_status != acquisition_status
        && matches!(acquisition_status.as_str(), "completed" | "failed")
    {
        let draft = acquisition_notification(
            &acquisition_status,
            event.payload.project_id,
            event.payload.acquisition_id,
            fetched_count,
            failed_count,
        );
        deepref_app::record_notification_in_transaction(tx, &draft).await?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{acquisition_notification, settled_run_status};
    use deepref_app::NotificationSeverity;
    use uuid::Uuid;

    #[test]
    fn a_run_that_fetched_some_articles_completes_instead_of_failing() {
        assert_eq!(settled_run_status(2, 1), "completed");
    }

    #[test]
    fn a_run_that_fetched_nothing_fails() {
        assert_eq!(settled_run_status(0, 3), "failed");
    }

    #[test]
    fn a_run_without_misses_completes() {
        assert_eq!(settled_run_status(3, 0), "completed");
    }

    #[test]
    fn partial_success_notifies_with_a_warning_that_does_not_say_the_run_stopped() {
        let draft = acquisition_notification("completed", Uuid::nil(), Uuid::nil(), 2, 1);
        assert_eq!(draft.severity, NotificationSeverity::Warning);
        assert_eq!(draft.title, "Import completed with problems");
        let body = draft.body.unwrap_or_default();
        assert!(body.contains("2 articles imported"), "{body}");
        assert!(!body.contains("stopped"), "{body}");
    }

    #[tokio::test]
    async fn live_job_fence_and_item_transition_settle_a_run_exactly_once() -> anyhow::Result<()> {
        let Some(pool) = deepref_app::test_isolation::isolated_pool_from_env("DATABASE_URL", 5).await else {
            return Ok(());
        };
        let project_id = Uuid::new_v4();
        let acquisition_id = Uuid::new_v4();
        let report_id = Uuid::new_v4();
        sqlx::query("INSERT INTO projects(id,name) VALUES($1,'DOI fence regression')")
            .bind(project_id).execute(&pool).await?;
        sqlx::query("INSERT INTO reports(id) VALUES($1)").bind(report_id).execute(&pool).await?;
        sqlx::query("INSERT INTO acquisition_runs(id,project_id,source,strategy,status,seed_count,queued_count) VALUES($1,$2,'crossref','citation_traversal','running',1,1)")
            .bind(acquisition_id).bind(project_id).execute(&pool).await?;
        let event = deepref_domain::events::EventEnvelope::v1(
            deepref_domain::events::SUBJECT_WORK_FETCH_REQUESTED, "test",
            deepref_domain::events::EntityType::Work, format!("{acquisition_id}|10.5555/fence"),
            0, acquisition_id, None,
            deepref_domain::events::WorkFetchRequested {
                project_id, acquisition_id, doi: "10.5555/fence".to_owned(),
                depth: 0, max_depth: 0, parent_doi: None,
            },
        );
        sqlx::query("INSERT INTO acquisition_items(acquisition_id,project_id,report_id,canonical_doi,depth,work_event_id) VALUES($1,$2,$3,'10.5555/fence',0,$4)")
            .bind(acquisition_id).bind(project_id).bind(report_id).bind(event.event_id).execute(&pool).await?;
        sqlx::query("INSERT INTO jobs(id,project_id,kind,state,attempts,lease_owner,leased_until) VALUES($1,$2,'work_fetch_requested','running',2,'current-owner',now()+interval '1 minute')")
            .bind(event.event_id).bind(project_id).execute(&pool).await?;
        let mut lease = deepref_application::jobs::JobLease {
            id: event.event_id, owner: "current-owner".to_owned(), attempt: 1,
        };
        assert!(super::complete_without_doi(&pool, &event, &event.payload.doi, &lease,
            deepref_domain::IngestionItemStatus::Fetched, None).await.is_err());
        let queued: i32 = sqlx::query_scalar("SELECT queued_count FROM acquisition_runs WHERE id=$1")
            .bind(acquisition_id).fetch_one(&pool).await?;
        assert_eq!(queued, 1);
        lease.attempt = 2;
        for _ in 0..2 {
            super::complete_without_doi(&pool, &event, &event.payload.doi, &lease,
                deepref_domain::IngestionItemStatus::Fetched, None).await?;
        }
        let counts: (String, i32, i32, i32) = sqlx::query_as("SELECT status,queued_count,fetched_count,failed_count FROM acquisition_runs WHERE id=$1")
            .bind(acquisition_id).fetch_one(&pool).await?;
        assert_eq!(counts, ("completed".to_owned(), 0, 1, 0));
        Ok(())
    }
}
