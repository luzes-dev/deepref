use std::time::Duration;

use deepref_application::{AutomationDomainEvent, FetchedWork, jobs::JobLease};
use deepref_domain::events::{
    EntityType, EventEnvelope, SUBJECT_WORK_FETCH_REQUESTED, WorkFetchRequested,
};
use deepref_domain::{IngestionItemStatus, ProviderKind, Reference, WorkWithReferences};
use sqlx::{PgPool, Postgres, Row, Transaction};
use uuid::Uuid;

#[derive(Debug, Clone)]
pub struct RuntimeSettings {
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
    sqlx::query("INSERT INTO settings (id) VALUES (1) ON CONFLICT DO NOTHING")
        .execute(pool)
        .await?;
    let row =
        sqlx::query("SELECT rate_limit_per_second, retry_attempts FROM settings WHERE id = 1")
            .fetch_one(pool)
            .await?;
    Ok(RuntimeSettings {
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
    Ok(if acquired {
        ClaimState::Acquired
    } else {
        ClaimState::Busy
    })
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
    Ok(sqlx::query("SELECT 1 FROM report_fetch_state f JOIN report_identifiers i ON i.report_id=f.report_id JOIN works w ON w.canonical_doi=f.canonical_doi WHERE i.scheme='doi' AND i.normalized_value=$1 AND f.status='fetched'")
        .bind(doi).fetch_optional(pool).await?.is_some())
}

/// Locks the run and marks the item fetching. Returns the metadata chain the run
/// stored, or `None` when the item is no longer queued and the job should ack.
pub async fn mark_fetching(
    pool: &PgPool,
    event: &EventEnvelope<WorkFetchRequested>,
    doi: &str,
    lease: &JobLease,
) -> anyhow::Result<Option<Vec<ProviderKind>>> {
    let mut tx = pool.begin().await?;
    validate_event_ownership(&mut tx, lease).await?;
    lock_run(&mut tx, event).await?;
    let config: serde_json::Value =
        sqlx::query_scalar("SELECT config FROM acquisition_runs WHERE id=$1 AND project_id=$2")
            .bind(event.payload.acquisition_id)
            .bind(event.payload.project_id)
            .fetch_one(&mut *tx)
            .await?;
    let changed = sqlx::query("UPDATE acquisition_items SET status='fetching',attempts=attempts+1 WHERE acquisition_id=$1 AND project_id=$2 AND canonical_doi=$3 AND status IN ('queued','fetching')")
        .bind(event.payload.acquisition_id).bind(event.payload.project_id).bind(doi).execute(&mut *tx).await?.rows_affected() == 1;
    tx.commit().await?;
    Ok(changed.then(|| stored_metadata_chain(&config)))
}

/// The chain a run stored under `metadata_providers`. Lenient: a name that is not a
/// provider is dropped. A run in the legacy shape, `metadata_provider` and `citation_provider`
/// both `"crossref"`, ran on Crossref alone, so it stays on `[Crossref]` and a deployment that
/// now prefers OpenAlex does not move it partway through a traversal. A config in neither shape
/// has no chain, and the router falls back to the deployment chain.
fn stored_metadata_chain(config: &serde_json::Value) -> Vec<ProviderKind> {
    if let Some(providers) = config
        .get("metadata_providers")
        .and_then(serde_json::Value::as_array)
    {
        return providers
            .iter()
            .filter_map(serde_json::Value::as_str)
            .filter_map(|name| name.parse::<ProviderKind>().ok())
            .collect();
    }
    let legacy = |key: &str| config.get(key).and_then(serde_json::Value::as_str);
    if legacy("metadata_provider") == Some("crossref")
        && legacy("citation_provider") == Some("crossref")
    {
        return vec![ProviderKind::Crossref];
    }
    Vec::new()
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
        .bind(acquisition_id)
        .fetch_one(pool)
        .await?;
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
    )
    .bind(source_report_id)
    .fetch_one(&mut *tx)
    .await?;
    attach_project_work(&mut tx, &event.payload, doi).await?;
    sqlx::query(
        "INSERT INTO project_unresolved_references \
         (id, project_id, source_report_id, raw_unstructured, article_title, author, year, volume, first_page) \
         SELECT id, $1, $3, raw_unstructured, article_title, author, year, volume, first_page \
         FROM fetched_unresolved_reference_facts WHERE source_doi = $2 ON CONFLICT DO NOTHING",
    ).bind(event.payload.project_id).bind(&cached_doi).bind(source_report_id).execute(&mut *tx).await?;
    if event.payload.depth < event.payload.max_depth {
        let targets = sqlx::query(
            "SELECT target_doi, source FROM fetched_citation_facts WHERE source_doi=$1 ORDER BY target_doi",
        )
        .bind(&cached_doi)
        .fetch_all(&mut *tx)
        .await?;
        for fact in targets {
            let target = fact.get::<String, _>("target_doi");
            // The project citation carries the lane label the cached fact was stored with.
            let label = fact.get::<String, _>("source");
            persist_citation(&mut tx, event, doi, &target, &label).await?;
            enqueue_child(&mut tx, event, doi, &target).await?;
        }
    }
    emit_metrics_request(&mut tx, event).await?;
    complete_item_and_claim(&mut tx, event, doi, IngestionItemStatus::Fetched, None).await?;
    tx.commit().await?;
    Ok(())
}

pub async fn finalize_success(
    pool: &PgPool,
    event: &EventEnvelope<WorkFetchRequested>,
    doi: &str,
    owner: Uuid,
    lease: &JobLease,
    fetched: &FetchedWork,
) -> anyhow::Result<()> {
    let mut tx = pool.begin().await?;
    validate_ownership(&mut tx, lease, doi, owner).await?;
    lock_run(&mut tx, event).await?;
    persist_work(&mut tx, &fetched.work).await?;
    attach_project_work(&mut tx, &event.payload, doi).await?;

    for reference in &fetched.work.references {
        match &reference.doi {
            Some(target) => {
                persist_citation(&mut tx, event, doi, target, NEW_EDGE_LABEL).await?;
            }
            None => persist_unresolved(&mut tx, event, doi, reference).await?,
        }
    }
    // Child items and their deterministic durable jobs are inserted before the
    // terminal counts are evaluated.
    if event.payload.depth < event.payload.max_depth {
        for target in fetched
            .work
            .references
            .iter()
            .filter_map(|reference| reference.doi.as_deref())
        {
            enqueue_child(&mut tx, event, doi, target).await?;
        }
    }
    emit_metrics_request(&mut tx, event).await?;
    complete_item_and_claim(&mut tx, event, doi, IngestionItemStatus::Fetched, None).await?;
    sqlx::query(
        "UPDATE report_fetch_state SET canonical_doi=$1, status = 'fetched', provider = $3, fetched_at = now(), owner_token = NULL, \
         lease_expires_at = NULL, heartbeat_at = now(), updated_at = now() \
         WHERE report_id = (SELECT report_id FROM report_identifiers WHERE scheme='doi' AND normalized_value=$1) AND owner_token = $2",
    )
    .bind(doi)
    .bind(owner)
    .bind(fetched.provider.as_str())
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub async fn finalize_terminal_failure(
    pool: &PgPool,
    event: &EventEnvelope<WorkFetchRequested>,
    doi: &str,
    owner: Uuid,
    lease: &JobLease,
    status: IngestionItemStatus,
    error: &str,
    provider: Option<ProviderKind>,
) -> anyhow::Result<()> {
    let mut tx = pool.begin().await?;
    validate_ownership(&mut tx, lease, doi, owner).await?;
    lock_run(&mut tx, event).await?;
    complete_item_and_claim(&mut tx, event, doi, status, Some(error)).await?;
    sqlx::query(
        "UPDATE report_fetch_state SET status = $3, last_error = $4, provider = COALESCE($5, provider), owner_token = NULL, \
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
    .bind(provider.map(ProviderKind::as_str))
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
        "SELECT id FROM jobs WHERE id=$1 AND state='running' AND lease_owner=$2 AND claim_seq=$3 AND leased_until>clock_timestamp() FOR UPDATE",
    ).bind(lease.id).bind(&lease.owner).bind(lease.claim_seq).fetch_optional(&mut **tx).await?;
    if valid.is_none() {
        anyhow::bail!("job lease ownership was lost before persistence");
    }
    Ok(())
}

async fn lock_run(
    tx: &mut Transaction<'_, Postgres>,
    event: &EventEnvelope<WorkFetchRequested>,
) -> anyhow::Result<()> {
    sqlx::query("SELECT id FROM acquisition_runs WHERE id=$1 AND project_id=$2 FOR UPDATE")
        .bind(event.payload.acquisition_id)
        .bind(event.payload.project_id)
        .fetch_one(&mut **tx)
        .await?;
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
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1,0))")
        .bind(doi)
        .execute(&mut **tx)
        .await?;
    sqlx::query("INSERT INTO works (canonical_doi) VALUES ($1) ON CONFLICT DO NOTHING")
        .bind(doi)
        .execute(&mut **tx)
        .await?;
    let report_id: Uuid = sqlx::query_scalar(
        "SELECT COALESCE((SELECT report_id FROM report_identifiers WHERE scheme='doi' AND normalized_value=$1),format('%s-%s-%s-%s-%s',substr(md5('deepref:report:'||$1),1,8),substr(md5('deepref:report:'||$1),9,4),substr(md5('deepref:report:'||$1),13,4),substr(md5('deepref:report:'||$1),17,4),substr(md5('deepref:report:'||$1),21,12))::uuid)",
    )
    .bind(doi)
    .fetch_one(&mut **tx)
    .await?;
    let previous: Option<(Option<i32>,i64,i64)> = sqlx::query_as(
        "SELECT publication_year,total_citations,references_count FROM reports WHERE id=$1 FOR UPDATE",
    ).bind(report_id).fetch_optional(&mut **tx).await?;
    sqlx::query(
        "INSERT INTO reports (id,title,abstract_text,publication_year,journal,url,work_type,publisher,container_title,total_citations,references_count,raw) SELECT $1,title,abstract_text,COALESCE(published_year,issued_year),container_title,url,work_type,publisher,container_title,total_citations,references_count,raw FROM works WHERE canonical_doi=$2 ON CONFLICT (id) DO UPDATE SET title=EXCLUDED.title,abstract_text=EXCLUDED.abstract_text,publication_year=EXCLUDED.publication_year,journal=EXCLUDED.journal,url=EXCLUDED.url,work_type=EXCLUDED.work_type,publisher=EXCLUDED.publisher,container_title=EXCLUDED.container_title,total_citations=EXCLUDED.total_citations,references_count=EXCLUDED.references_count,raw=EXCLUDED.raw,updated_at=now()",
    )
    .bind(report_id)
    .bind(doi)
    .execute(&mut **tx)
    .await?;
    let current: (Option<i32>, i64, i64) = sqlx::query_as(
        "SELECT publication_year,total_citations,references_count FROM reports WHERE id=$1",
    )
    .bind(report_id)
    .fetch_one(&mut **tx)
    .await?;
    if previous.is_some_and(|previous| previous != current) {
        let projects: Vec<Uuid> = sqlx::query_scalar(
            "SELECT project_id FROM project_reports WHERE report_id=$1 ORDER BY project_id",
        )
        .bind(report_id)
        .fetch_all(&mut **tx)
        .await?;
        for project in projects {
            deepref_app::enqueue_metrics_recompute(tx, project).await?;
        }
    }
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

/// The lane label of a DOI-traversal edge written now. It names how the edge was found, not
/// which provider found it: the answering providers are recorded per work. Edges written before
/// routing keep their truthful `crossref-reference` label.
const NEW_EDGE_LABEL: &str = "doi-reference";

/// Writes the citation fact and the project citation with the lane `label`. A fact or citation
/// that already exists keeps the label it was stored with.
async fn persist_citation(
    tx: &mut Transaction<'_, Postgres>,
    event: &EventEnvelope<WorkFetchRequested>,
    source: &str,
    target: &str,
    label: &str,
) -> anyhow::Result<()> {
    sqlx::query("INSERT INTO works (canonical_doi) VALUES ($1) ON CONFLICT DO NOTHING")
        .bind(target)
        .execute(&mut **tx)
        .await?;
    sqlx::query(
        "INSERT INTO fetched_citation_facts (source_doi,target_doi,source) VALUES ($1,$2,$3) ON CONFLICT DO NOTHING",
    ).bind(source).bind(target).bind(label).execute(&mut **tx).await?;
    let source_report_id = ensure_v2_report(tx, source).await?;
    let target_report_id = ensure_v2_report(tx, target).await?;
    sqlx::query(
        "INSERT INTO citations (project_id,source_report_id,target_report_id,source,acquisition_run_id) VALUES ($1,$2,$3,$5,$4) ON CONFLICT (project_id,source_report_id,target_report_id) DO UPDATE SET acquisition_run_id=COALESCE(citations.acquisition_run_id,EXCLUDED.acquisition_run_id)",
    )
    .bind(event.payload.project_id)
    .bind(source_report_id)
    .bind(target_report_id)
    .bind(event.payload.acquisition_id)
    .bind(label)
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
        sqlx::query("UPDATE acquisition_runs SET queued_count=queued_count+1 WHERE id=$1")
            .bind(event.payload.acquisition_id)
            .execute(&mut **tx)
            .await?;
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
    deepref_app::enqueue_metrics_recompute(tx, event.payload.project_id).await?;
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
    )
    .bind(event.payload.acquisition_id)
    .bind(event.payload.project_id)
    .fetch_one(&mut **tx)
    .await?;
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
            acquisition_status.as_str(),
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
    use std::time::Duration;

    use chrono::Utc;
    use deepref_app::NotificationSeverity;
    use deepref_application::FetchedWork;
    use deepref_application::jobs::JobLease;
    use deepref_domain::events::{
        EntityType, EventEnvelope, SUBJECT_WORK_FETCH_REQUESTED, WorkFetchRequested,
    };
    use deepref_domain::{FetchStatus, ProviderKind, Work, WorkWithReferences};
    use serde_json::json;
    use sqlx::{PgPool, Row};
    use uuid::Uuid;

    use super::{
        ClaimState, acquisition_notification, attach_cached, claim_doi, finalize_success,
        finalize_terminal_failure, is_cached, settled_run_status, stored_metadata_chain,
    };
    use deepref_domain::IngestionItemStatus;

    const OWNER_LEASE: Duration = Duration::from_secs(600);

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
        let Some(pool) =
            deepref_app::test_isolation::isolated_pool_from_env("DATABASE_URL", 5).await
        else {
            return Ok(());
        };
        let project_id = Uuid::new_v4();
        let acquisition_id = Uuid::new_v4();
        let report_id = Uuid::new_v4();
        sqlx::query("INSERT INTO projects(id,name) VALUES($1,'DOI fence regression')")
            .bind(project_id)
            .execute(&pool)
            .await?;
        sqlx::query("INSERT INTO reports(id) VALUES($1)")
            .bind(report_id)
            .execute(&pool)
            .await?;
        sqlx::query("INSERT INTO acquisition_runs(id,project_id,source,strategy,status,seed_count,queued_count,created_at) VALUES($1,$2,'crossref','citation_traversal','running',1,1,now())")
            .bind(acquisition_id).bind(project_id).execute(&pool).await?;
        let event = deepref_domain::events::EventEnvelope::v1(
            deepref_domain::events::SUBJECT_WORK_FETCH_REQUESTED,
            "test",
            deepref_domain::events::EntityType::Work,
            format!("{acquisition_id}|10.5555/fence"),
            0,
            acquisition_id,
            None,
            deepref_domain::events::WorkFetchRequested {
                project_id,
                acquisition_id,
                doi: "10.5555/fence".to_owned(),
                depth: 0,
                max_depth: 0,
                parent_doi: None,
            },
        );
        sqlx::query("INSERT INTO acquisition_items(acquisition_id,project_id,report_id,canonical_doi,depth,work_event_id) VALUES($1,$2,$3,'10.5555/fence',0,$4)")
            .bind(acquisition_id).bind(project_id).bind(report_id).bind(event.event_id).execute(&pool).await?;
        sqlx::query("INSERT INTO jobs(id,project_id,kind,workload_class,state,attempts,lease_owner,leased_until) VALUES($1,$2,'work_fetch_requested','integration','running',2,'current-owner',now()+interval '1 minute')")
            .bind(event.event_id).bind(project_id).execute(&pool).await?;
        let mut lease = deepref_application::jobs::JobLease {
            id: event.event_id,
            owner: "current-owner".to_owned(),
            claim_seq: 1,
            attempt: 1,
        };
        anyhow::ensure!(
            super::complete_without_doi(
                &pool,
                &event,
                &event.payload.doi,
                &lease,
                deepref_domain::IngestionItemStatus::Fetched,
                None
            )
            .await
            .is_err(),
            "a stale claim identity must not settle the acquisition item"
        );
        let queued: i32 =
            sqlx::query_scalar("SELECT queued_count FROM acquisition_runs WHERE id=$1")
                .bind(acquisition_id)
                .fetch_one(&pool)
                .await?;
        anyhow::ensure!(queued == 1, "stale completion changed the queued count");
        lease.claim_seq = 0;
        for _ in 0..2 {
            super::complete_without_doi(
                &pool,
                &event,
                &event.payload.doi,
                &lease,
                deepref_domain::IngestionItemStatus::Fetched,
                None,
            )
            .await?;
        }
        let counts: (String, i32, i32, i32) = sqlx::query_as("SELECT status,queued_count,fetched_count,failed_count FROM acquisition_runs WHERE id=$1")
            .bind(acquisition_id).fetch_one(&pool).await?;
        anyhow::ensure!(
            counts == ("completed".to_owned(), 0, 1, 0),
            "current completion must settle exactly once"
        );
        Ok(())
    }

    #[test]
    fn a_stored_chain_is_read_leniently() {
        assert_eq!(
            stored_metadata_chain(&json!({"metadata_providers": ["openalex", "crossref"]})),
            vec![ProviderKind::OpenAlex, ProviderKind::Crossref]
        );
        assert_eq!(
            stored_metadata_chain(
                &json!({"metadata_provider": "crossref", "citation_provider": "crossref"})
            ),
            vec![ProviderKind::Crossref],
            "the legacy shape ran on Crossref alone"
        );
        assert!(
            stored_metadata_chain(
                &json!({"metadata_provider": "pubmed", "citation_provider": "crossref"})
            )
            .is_empty(),
            "an invalid legacy shape has no chain"
        );
        assert!(stored_metadata_chain(&json!({})).is_empty());
        assert_eq!(
            stored_metadata_chain(&json!({"metadata_providers": ["pubmed", "crossref", 7]})),
            vec![ProviderKind::Crossref],
            "names that are not providers are dropped"
        );
    }

    fn fetched(doi: &str, provider: ProviderKind) -> FetchedWork {
        FetchedWork {
            work: WorkWithReferences {
                work: Work {
                    doi: doi.to_owned(),
                    title: Some("A title".to_owned()),
                    abstract_text: None,
                    work_type: None,
                    publisher: None,
                    container_title: None,
                    issued_year: Some(2024),
                    published_year: None,
                    url: None,
                    total_citations: 0,
                    references_count: 0,
                    metadata_provider: provider.as_str().to_owned(),
                    citation_provider: provider.as_str().to_owned(),
                    fetched_at: Utc::now(),
                    fetch_status: FetchStatus::Fetched.as_str().to_owned(),
                },
                references: Vec::new(),
                raw: json!({ "title": ["A title"] }),
            },
            provider,
            references: deepref_application::ReferenceReport::default(),
        }
    }

    /// A project with one queued item for `doi` under a running job, so the store
    /// writes can take the lease. The DOI is not claimed yet.
    async fn seeded_run(
        pool: &PgPool,
        doi: &str,
    ) -> anyhow::Result<(EventEnvelope<WorkFetchRequested>, JobLease)> {
        let project_id = Uuid::new_v4();
        let acquisition_id = Uuid::new_v4();
        let report_id = Uuid::new_v4();
        let event = EventEnvelope::v1(
            SUBJECT_WORK_FETCH_REQUESTED,
            "test",
            EntityType::Work,
            format!("{acquisition_id}|{doi}"),
            0,
            acquisition_id,
            None,
            WorkFetchRequested {
                project_id,
                acquisition_id,
                doi: doi.to_owned(),
                depth: 0,
                max_depth: 0,
                parent_doi: None,
            },
        );
        sqlx::query("INSERT INTO projects(id,name) VALUES($1,'store provider fixture')")
            .bind(project_id)
            .execute(pool)
            .await?;
        sqlx::query("INSERT INTO reports(id) VALUES($1)")
            .bind(report_id)
            .execute(pool)
            .await?;
        sqlx::query("INSERT INTO acquisition_runs(id,project_id,source,strategy,status,seed_count,queued_count,created_at) VALUES($1,$2,'crossref','citation_traversal','running',1,1,now())")
            .bind(acquisition_id).bind(project_id).execute(pool).await?;
        sqlx::query("INSERT INTO acquisition_items(acquisition_id,project_id,report_id,canonical_doi,depth,work_event_id) VALUES($1,$2,$3,$4,0,$5)")
            .bind(acquisition_id).bind(project_id).bind(report_id).bind(doi).bind(event.event_id)
            .execute(pool).await?;
        sqlx::query("INSERT INTO jobs(id,project_id,kind,workload_class,state,attempts,lease_owner,leased_until) VALUES($1,$2,'work_fetch_requested','integration','running',1,'worker-fixture',now()+interval '10 minutes')")
            .bind(event.event_id).bind(project_id).execute(pool).await?;
        let lease = JobLease {
            id: event.event_id,
            owner: "worker-fixture".to_owned(),
            claim_seq: 0,
            attempt: 1,
        };
        Ok((event, lease))
    }

    /// The DOI's cache row as `(provider, status)`.
    async fn cache_row(pool: &PgPool, doi: &str) -> anyhow::Result<(String, String)> {
        let row = sqlx::query(
            "SELECT f.provider, f.status FROM report_fetch_state f JOIN report_identifiers i ON i.report_id=f.report_id WHERE i.scheme='doi' AND i.normalized_value=$1",
        )
        .bind(doi)
        .fetch_one(pool)
        .await?;
        Ok((row.get("provider"), row.get("status")))
    }

    #[tokio::test]
    async fn a_crossref_era_cache_row_still_attaches_after_the_provider_filter_is_dropped()
    -> anyhow::Result<()> {
        let Some(pool) =
            deepref_app::test_isolation::isolated_pool_from_env("DATABASE_URL", 5).await
        else {
            return Ok(());
        };
        let doi = "10.5555/crossref-era";
        let first_owner = Uuid::new_v4();
        let (first_event, first_lease) = seeded_run(&pool, doi).await?;
        anyhow::ensure!(
            claim_doi(&pool, doi, first_owner, OWNER_LEASE).await? == ClaimState::Acquired,
            "the first run should claim the DOI"
        );
        finalize_success(
            &pool,
            &first_event,
            doi,
            first_owner,
            &first_lease,
            &fetched(doi, ProviderKind::Crossref),
        )
        .await?;
        anyhow::ensure!(
            cache_row(&pool, doi).await? == ("crossref".to_owned(), "fetched".to_owned()),
            "a Crossref answer leaves a fetched Crossref-era row"
        );

        let (second_event, second_lease) = seeded_run(&pool, doi).await?;
        anyhow::ensure!(
            is_cached(&pool, doi).await?,
            "the Crossref-era row is still a cache hit"
        );
        attach_cached(&pool, &second_event, doi, &second_lease).await?;

        let item_status: String =
            sqlx::query_scalar("SELECT status FROM acquisition_items WHERE acquisition_id=$1")
                .bind(second_event.payload.acquisition_id)
                .fetch_one(&pool)
                .await?;
        anyhow::ensure!(
            item_status == "fetched",
            "the second run's item attaches from the cache, got {item_status}"
        );
        let record_source: String =
            sqlx::query_scalar("SELECT source FROM records WHERE project_id=$1")
                .bind(second_event.payload.project_id)
                .fetch_one(&pool)
                .await?;
        anyhow::ensure!(
            record_source == "crossref",
            "the attached record keeps the DOI lane label"
        );
        Ok(())
    }

    #[tokio::test]
    async fn finalize_success_records_the_answering_provider_and_the_cache_still_hits()
    -> anyhow::Result<()> {
        let Some(pool) =
            deepref_app::test_isolation::isolated_pool_from_env("DATABASE_URL", 5).await
        else {
            return Ok(());
        };
        let doi = "10.5555/openalex-answer";
        let owner = Uuid::new_v4();
        let (event, lease) = seeded_run(&pool, doi).await?;
        anyhow::ensure!(
            claim_doi(&pool, doi, owner, OWNER_LEASE).await? == ClaimState::Acquired,
            "the run should claim the DOI"
        );
        anyhow::ensure!(
            cache_row(&pool, doi).await?.0 == "crossref",
            "the claim writes the lane placeholder"
        );

        finalize_success(
            &pool,
            &event,
            doi,
            owner,
            &lease,
            &fetched(doi, ProviderKind::OpenAlex),
        )
        .await?;

        anyhow::ensure!(
            cache_row(&pool, doi).await? == ("openalex".to_owned(), "fetched".to_owned()),
            "the cache row names the provider that answered"
        );
        let work_provider: String =
            sqlx::query_scalar("SELECT metadata_provider FROM works WHERE canonical_doi=$1")
                .bind(doi)
                .fetch_one(&pool)
                .await?;
        anyhow::ensure!(
            work_provider == "openalex",
            "the work names the answering provider, got {work_provider}"
        );
        anyhow::ensure!(
            is_cached(&pool, doi).await?,
            "a row fetched by a provider other than Crossref is a cache hit"
        );
        Ok(())
    }

    #[tokio::test]
    async fn a_terminal_failure_records_the_last_attempted_provider() -> anyhow::Result<()> {
        let Some(pool) =
            deepref_app::test_isolation::isolated_pool_from_env("DATABASE_URL", 5).await
        else {
            return Ok(());
        };
        let doi = "10.5555/attempted-miss";
        let owner = Uuid::new_v4();
        let (event, lease) = seeded_run(&pool, doi).await?;
        anyhow::ensure!(
            claim_doi(&pool, doi, owner, OWNER_LEASE).await? == ClaimState::Acquired,
            "the run should claim the DOI"
        );

        finalize_terminal_failure(
            &pool,
            &event,
            doi,
            owner,
            &lease,
            IngestionItemStatus::NotFound,
            "No metadata provider has a record for 10.5555/attempted-miss.",
            Some(ProviderKind::OpenAlex),
        )
        .await?;

        let (provider, status) = cache_row(&pool, doi).await?;
        anyhow::ensure!(
            provider == "openalex" && status == "not_found",
            "the failure names the last provider attempted, got {provider}/{status}"
        );
        anyhow::ensure!(
            !is_cached(&pool, doi).await?,
            "a not-found row is not a cache hit"
        );
        Ok(())
    }

    #[tokio::test]
    async fn a_failure_before_any_attempt_keeps_the_claim_placeholder() -> anyhow::Result<()> {
        let Some(pool) =
            deepref_app::test_isolation::isolated_pool_from_env("DATABASE_URL", 5).await
        else {
            return Ok(());
        };
        let doi = "10.5555/no-lane";
        let owner = Uuid::new_v4();
        let (event, lease) = seeded_run(&pool, doi).await?;
        anyhow::ensure!(
            claim_doi(&pool, doi, owner, OWNER_LEASE).await? == ClaimState::Acquired,
            "the run should claim the DOI"
        );

        finalize_terminal_failure(
            &pool,
            &event,
            doi,
            owner,
            &lease,
            IngestionItemStatus::Failed,
            "a contact e-mail address is required",
            None,
        )
        .await?;

        anyhow::ensure!(
            cache_row(&pool, doi).await? == ("crossref".to_owned(), "failed".to_owned()),
            "with no attempt the placeholder stays"
        );
        Ok(())
    }
    #[tokio::test]
    async fn graph_shared_report_refresh_invalidates_every_project_and_rolls_back_atomically()
    -> anyhow::Result<()> {
        let Some(pool) =
            deepref_app::test_isolation::isolated_pool_from_env("DATABASE_URL", 8).await
        else {
            return Ok(());
        };
        let doi = "10.5555/shared-refresh";
        let owner = Uuid::new_v4();
        let (first, first_lease) = seeded_run(&pool, doi).await?;
        claim_doi(&pool, doi, owner, OWNER_LEASE).await?;
        finalize_success(
            &pool,
            &first,
            doi,
            owner,
            &first_lease,
            &fetched(doi, ProviderKind::Crossref),
        )
        .await?;
        let (second, second_lease) = seeded_run(&pool, doi).await?;
        attach_cached(&pool, &second, doi, &second_lease).await?;
        for project in [first.payload.project_id, second.payload.project_id] {
            deepref_app::recompute_project_metrics(&pool, project).await?;
        }
        let before = graph_refresh_state(&pool).await?;
        let (refresh, lease) = seeded_run(&pool, doi).await?;
        sqlx::query("UPDATE report_fetch_state SET status='failed' WHERE canonical_doi=$1")
            .bind(doi)
            .execute(&pool)
            .await?;
        claim_doi(&pool, doi, owner, OWNER_LEASE).await?;
        let mut changed = fetched(doi, ProviderKind::OpenAlex);
        changed.work.work.total_citations = 123;
        changed.work.work.issued_year = Some(2020);
        // A late canonical-command failure happens after report writes and
        // all projection events. The entire command must roll back.
        sqlx::raw_sql("CREATE FUNCTION reject_graph_settle() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN RAISE EXCEPTION 'graph command rollback'; END $$; CREATE TRIGGER reject_graph_settle BEFORE UPDATE ON acquisition_items FOR EACH ROW EXECUTE FUNCTION reject_graph_settle();").execute(&pool).await?;
        assert!(
            finalize_success(&pool, &refresh, doi, owner, &lease, &changed)
                .await
                .is_err()
        );
        let after = graph_refresh_state(&pool).await?;
        for project in [first.payload.project_id, second.payload.project_id] {
            assert_eq!(
                before.iter().find(|row| row.0 == project),
                after.iter().find(|row| row.0 == project)
            );
            assert_eq!(
                deepref_app::get_projection_status(&pool, project)
                    .await?
                    .unwrap()
                    .state,
                deepref_domain::graph::ProjectionState::Ready
            );
        }
        let canonical: (Option<i32>,i64) = sqlx::query_as("SELECT publication_year,total_citations FROM reports WHERE id=(SELECT report_id FROM report_identifiers WHERE normalized_value=$1 AND scheme='doi')").bind(doi).fetch_one(&pool).await?;
        assert_eq!(canonical, (Some(2024), 0));
        sqlx::query("DROP TRIGGER reject_graph_settle ON acquisition_items")
            .execute(&pool)
            .await?;
        finalize_success(&pool, &refresh, doi, owner, &lease, &changed).await?;
        let after = graph_refresh_state(&pool).await?;
        for project in [first.payload.project_id, second.payload.project_id] {
            let old = before.iter().find(|row| row.0 == project).unwrap();
            let new = after.iter().find(|row| row.0 == project).unwrap();
            assert_eq!(new.1, old.1 + 1);
            assert_eq!(new.2, old.2 + 1);
            let status = deepref_app::get_projection_status(&pool, project)
                .await?
                .unwrap();
            assert_eq!(
                status.state,
                deepref_domain::graph::ProjectionState::Lagging
            );
            assert_eq!(status.lag, 1);
        }
        Ok(())
    }

    async fn graph_refresh_state(pool: &PgPool) -> anyhow::Result<Vec<(Uuid, i64, i64)>> {
        Ok(sqlx::query_as("SELECT p.id,p.metric_requested_revision,(SELECT count(*) FROM outbox_events e WHERE e.message_type='graph.metrics_recompute_requested' AND e.payload->>'project_id'=p.id::text) FROM projects p ORDER BY p.id").fetch_all(pool).await?)
    }

    #[tokio::test]
    async fn graph_production_command_outbox_consumer_handler_failure_restart_and_redelivery()
    -> anyhow::Result<()> {
        use deepref_application::{
            jobs::JobRetry,
            outbox::{DeliveryContext, OutboxRegistry},
        };
        use deepref_domain::{WorkloadClass, graph::ProjectionState};
        let Some(pool) =
            deepref_app::test_isolation::isolated_pool_from_env("DATABASE_URL", 8).await
        else {
            return Ok(());
        };
        let doi = "10.5555/graph-delivery";
        let owner = Uuid::new_v4();
        let (event, lease) = seeded_run(&pool, doi).await?;
        claim_doi(&pool, doi, owner, OWNER_LEASE).await?;
        finalize_success(
            &pool,
            &event,
            doi,
            owner,
            &lease,
            &fetched(doi, ProviderKind::Crossref),
        )
        .await?;
        let project = event.payload.project_id;
        let count: i64 =
            sqlx::query_scalar("SELECT count(*) FROM jobs WHERE kind='recompute_metrics'")
                .fetch_one(&pool)
                .await?;
        assert_eq!(
            count, 0,
            "command emits outbox; dispatcher owns queue effect"
        );
        assert_eq!(
            deepref_app::get_projection_status(&pool, project)
                .await?
                .unwrap()
                .state,
            ProjectionState::Lagging
        );
        // Repair is another real producer. With dispatcher/worker stopped,
        // both requests remain durable. Deliver the newer request first.
        deepref_app::rebuild_project_projection(&pool, project).await?;
        let mut registry = OutboxRegistry::new();
        deepref_app::register_metrics_projection(&pool, &mut registry)?;
        let mut events = deepref_app::outbox::claim_outbox_batch(
            &pool,
            &registry,
            "before-restart",
            OWNER_LEASE,
            10,
        )
        .await?;
        assert_eq!(events.len(), 2);
        events.sort_by_key(|e| std::cmp::Reverse(e.payload["revision"].as_i64().unwrap()));
        let latest = events[0].clone();
        // The actual registered consumer must roll its receipt back when
        // enqueue fails, then succeed under a fresh dispatcher generation.
        sqlx::raw_sql("CREATE FUNCTION reject_graph_job() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN IF NEW.kind='recompute_metrics' THEN RAISE EXCEPTION 'graph enqueue failure'; END IF; RETURN NEW; END $$; CREATE TRIGGER reject_graph_job BEFORE INSERT ON jobs FOR EACH ROW EXECUTE FUNCTION reject_graph_job();").execute(&pool).await?;
        deepref_app::outbox::dispatch_outbox_event(&pool, &registry, OWNER_LEASE, &latest).await?;
        let delivered: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM outbox_receipts WHERE event_id=$1 AND state='delivered')",
        )
        .bind(latest.event_id)
        .fetch_one(&pool)
        .await?;
        assert!(!delivered, "receipt cannot outlive failed effect");
        sqlx::query("DROP TRIGGER reject_graph_job ON jobs")
            .execute(&pool)
            .await?;
        // Simulate restart/reclaim without waiting through operational backoff.
        sqlx::query("UPDATE outbox_events SET lease_expires_at=CASE WHEN lease_owner IS NOT NULL THEN now()-interval '1 second' ELSE NULL END,next_attempt_at=now() WHERE delivery_state='pending'").execute(&pool).await?;
        sqlx::query("UPDATE outbox_receipts SET next_attempt_at=now() WHERE state='retrying'")
            .execute(&pool)
            .await?;
        let mut restarted = OutboxRegistry::new();
        deepref_app::register_metrics_projection(&pool, &mut restarted)?;
        let mut events = deepref_app::outbox::claim_outbox_batch(
            &pool,
            &restarted,
            "after-restart",
            OWNER_LEASE,
            10,
        )
        .await?;
        // Recovery charges the abandoned predecessor and schedules its own
        // backoff. Age that backoff only after the real recovery has run;
        // preserve any claim accepted by the first intake.
        sqlx::query("UPDATE outbox_events SET next_attempt_at=now() WHERE delivery_state='pending' AND lease_owner IS NULL")
            .execute(&pool).await?;
        sqlx::query("UPDATE outbox_receipts SET next_attempt_at=now() WHERE state='retrying'")
            .execute(&pool)
            .await?;
        events.extend(
            deepref_app::outbox::claim_outbox_batch(
                &pool,
                &restarted,
                "after-restart",
                OWNER_LEASE,
                10,
            )
            .await?,
        );
        events.sort_by_key(|e| std::cmp::Reverse(e.payload["revision"].as_i64().unwrap()));
        assert_eq!(events.len(), 2);
        for e in &events {
            deepref_app::outbox::dispatch_outbox_event(&pool, &restarted, OWNER_LEASE, e).await?;
            let consumer = &e.expected_consumers[0];
            let handler = restarted
                .handler(&e.message_type, e.message_version, consumer)
                .unwrap();
            handler(
                DeliveryContext {
                    event_id: e.event_id,
                    consumer: consumer.clone(),
                    message_type: e.message_type.clone(),
                    message_version: e.message_version,
                    claim_fence: e.claim_fence.clone(),
                },
                e.payload.clone(),
            )
            .await?;
        }
        let count: i64 =
            sqlx::query_scalar("SELECT count(*) FROM jobs WHERE kind='recompute_metrics'")
                .fetch_one(&pool)
                .await?;
        let diagnostics: Vec<(Uuid,serde_json::Value,String,Option<String>)> = sqlx::query_as("SELECT event_id,payload,delivery_state,last_error FROM outbox_events ORDER BY event_id").fetch_all(&pool).await?;
        assert_eq!(
            count, 2,
            "consumer redelivery cannot duplicate queue effects: {diagnostics:?}"
        );
        let non_projection = deepref_app::claim_jobs(
            &pool,
            "other-pool",
            OWNER_LEASE,
            &[
                WorkloadClass::Ai,
                WorkloadClass::Workflow,
                WorkloadClass::Documents,
            ],
            10,
        )
        .await?;
        assert!(non_projection.is_empty(), "projection work stays isolated");
        let mut jobs = deepref_app::claim_jobs(
            &pool,
            "projection-before-restart",
            OWNER_LEASE,
            &[WorkloadClass::Projection],
            10,
        )
        .await?;
        jobs.sort_by_key(|j| std::cmp::Reverse(j.payload["revision"].as_i64().unwrap()));
        assert_eq!(jobs.len(), 2);
        sqlx::raw_sql("CREATE FUNCTION reject_graph_metrics() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN RAISE EXCEPTION 'graph compute failure'; END $$; CREATE TRIGGER reject_graph_metrics BEFORE UPDATE ON project_reports FOR EACH ROW EXECUTE FUNCTION reject_graph_metrics();").execute(&pool).await?;
        let error = crate::processor::handle_recompute_metrics_job(&pool, jobs[0].payload.clone())
            .await
            .unwrap_err();
        let failed = deepref_app::get_projection_status(&pool, project)
            .await?
            .unwrap();
        assert_eq!(failed.state, ProjectionState::Failed);
        assert!(failed.last_error.unwrap().contains("graph compute failure"));
        assert!(failed.rebuild_state.is_some());
        let snapshots: i64 =
            sqlx::query_scalar("SELECT count(*) FROM metric_snapshots WHERE project_id=$1")
                .bind(project)
                .fetch_one(&pool)
                .await?;
        assert_eq!(
            snapshots, 0,
            "failed compute rolls back all projection output"
        );
        deepref_app::fail_job(
            &pool,
            &jobs[0].lease("projection-before-restart"),
            &error.to_string(),
            JobRetry::After(Duration::ZERO),
        )
        .await?;
        sqlx::query("DROP TRIGGER reject_graph_metrics ON project_reports")
            .execute(&pool)
            .await?;
        let resumed = deepref_app::claim_jobs(
            &pool,
            "projection-after-restart",
            OWNER_LEASE,
            &[WorkloadClass::Projection],
            10,
        )
        .await?;
        assert_eq!(resumed.len(), 1);
        crate::processor::handle_recompute_metrics_job(&pool, resumed[0].payload.clone()).await?;
        assert!(
            deepref_app::complete_job(&pool, &resumed[0].lease("projection-after-restart")).await?
        );
        let expected = graph_metric_values(&pool, project).await?;
        // Old event, then exact redelivery after success: every deterministic
        // output must remain identical, even though execution time changes.
        for payload in [jobs[1].payload.clone(), resumed[0].payload.clone()] {
            crate::processor::handle_recompute_metrics_job(&pool, payload).await?;
            assert_eq!(graph_metric_values(&pool, project).await?, expected);
        }
        assert!(
            deepref_app::complete_job(&pool, &jobs[1].lease("projection-before-restart")).await?
        );
        let ready = deepref_app::get_projection_status(&pool, project)
            .await?
            .unwrap();
        assert_eq!(ready.state, ProjectionState::Ready);
        assert_eq!(
            (ready.requested_revision, ready.applied_revision, ready.lag),
            (2, 2, 0)
        );
        Ok(())
    }

    async fn graph_metric_values(
        pool: &PgPool,
        project: Uuid,
    ) -> anyhow::Result<Vec<(Uuid, i64, i64, i64, i64, f64)>> {
        Ok(sqlx::query_as("SELECT report_id,total_citations,references_count,internal_citations,outbound_internal_references,rank_score FROM project_reports WHERE project_id=$1 ORDER BY report_id").bind(project).fetch_all(pool).await?)
    }
}
