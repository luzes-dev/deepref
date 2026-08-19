use std::collections::HashMap;

use anyhow::{Context, anyhow};
use chrono::{DateTime, Utc};
use deepref_domain::normalize_doi;
use sqlx::{PgPool, Postgres, Row, Transaction};
use uuid::Uuid;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct LegacyImportCounts {
    pub projects_seen: u64,
    pub works_seen: u64,
    pub reports_created: u64,
    pub reports_reused: u64,
    pub report_identifiers_created: u64,
    pub acquisition_runs_created: u64,
    pub acquisition_runs_reused: u64,
    pub records_created: u64,
    pub records_repaired: u64,
    pub provenance_rows_created: u64,
    pub project_reports_created: u64,
    pub citations_created: u64,
    pub citations_repaired: u64,
}

struct LegacyReport {
    doi: String,
    title: Option<String>,
    abstract_text: Option<String>,
    issued_year: Option<i32>,
    published_year: Option<i32>,
    journal: Option<String>,
    url: Option<String>,
    raw: serde_json::Value,
}

/// Import the DOI-era tables into the UUID evidence model in one transaction.
/// The operation is deterministic and safe to run repeatedly.
pub async fn import_legacy(pool: &PgPool) -> anyhow::Result<LegacyImportCounts> {
    let mut transaction = pool.begin().await.context("begin legacy import")?;
    let result = import_in_transaction(&mut transaction).await;
    match result {
        Ok(counts) => {
            transaction.commit().await.context("commit legacy import")?;
            Ok(counts)
        }
        Err(error) => {
            let _ = transaction.rollback().await;
            Err(error)
        }
    }
}

async fn import_in_transaction(
    transaction: &mut Transaction<'_, Postgres>,
) -> anyhow::Result<LegacyImportCounts> {
    let mut counts = LegacyImportCounts::default();
    let project_count: i64 = sqlx::query_scalar("SELECT count(*)::bigint FROM projects")
        .fetch_one(&mut **transaction)
        .await?;
    counts.projects_seen = project_count as u64;

    let work_rows = sqlx::query(
        "SELECT canonical_doi,title,abstract_text,issued_year,published_year,container_title,url,raw \
         FROM works ORDER BY canonical_doi",
    )
    .fetch_all(&mut **transaction)
    .await?;
    let mut reports = HashMap::with_capacity(work_rows.len());
    for row in work_rows {
        let legacy_report = LegacyReport {
            doi: row.get("canonical_doi"),
            title: row.get("title"),
            abstract_text: row.get("abstract_text"),
            issued_year: row.get("issued_year"),
            published_year: row.get("published_year"),
            journal: row.get("container_title"),
            url: row.get("url"),
            raw: row.get("raw"),
        };
        let normalized = normalize(&legacy_report.doi)?;
        let report_id = ensure_report(transaction, legacy_report, &mut counts, true).await?;
        reports.insert(normalized, report_id);
        counts.works_seen += 1;
    }

    let ingestion_rows = sqlx::query(
        "SELECT id,project_id,status,max_depth,seed_count,queued_count,fetched_count,failed_count,\
                metadata_provider,citation_provider,created_at,started_at,completed_at \
         FROM ingestions ORDER BY id",
    )
    .fetch_all(&mut **transaction)
    .await?;
    for row in ingestion_rows {
        let ingestion_id: Uuid = row.get("id");
        let existed: bool = sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM acquisition_runs WHERE legacy_ingestion_id=$1)",
        )
        .bind(ingestion_id)
        .fetch_one(&mut **transaction)
        .await?;
        let inserted = sqlx::query(
            "INSERT INTO acquisition_runs \
             (id,project_id,legacy_ingestion_id,status,max_depth,seed_count,queued_count,fetched_count,failed_count,\
              metadata_provider,citation_provider,created_at,started_at,completed_at) \
             VALUES ($1,$2,$1,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13) \
             ON CONFLICT (legacy_ingestion_id) DO UPDATE SET \
               project_id=EXCLUDED.project_id,status=EXCLUDED.status,max_depth=EXCLUDED.max_depth,\
               seed_count=EXCLUDED.seed_count,queued_count=EXCLUDED.queued_count,\
               fetched_count=EXCLUDED.fetched_count,failed_count=EXCLUDED.failed_count,\
               metadata_provider=EXCLUDED.metadata_provider,citation_provider=EXCLUDED.citation_provider,\
               created_at=EXCLUDED.created_at,started_at=EXCLUDED.started_at,completed_at=EXCLUDED.completed_at",
        )
        .bind(ingestion_id)
        .bind(row.get::<Uuid, _>("project_id"))
        .bind(row.get::<String, _>("status"))
        .bind(row.get::<i32, _>("max_depth"))
        .bind(row.get::<i32, _>("seed_count"))
        .bind(row.get::<i32, _>("queued_count"))
        .bind(row.get::<i32, _>("fetched_count"))
        .bind(row.get::<i32, _>("failed_count"))
        .bind(row.get::<String, _>("metadata_provider"))
        .bind(row.get::<String, _>("citation_provider"))
        .bind(row.get::<DateTime<Utc>, _>("created_at"))
        .bind(row.get::<Option<DateTime<Utc>>, _>("started_at"))
        .bind(row.get::<Option<DateTime<Utc>>, _>("completed_at"))
        .execute(&mut **transaction)
        .await?;
        if inserted.rows_affected() == 1 && existed {
            counts.acquisition_runs_reused += 1;
        } else if inserted.rows_affected() == 1 {
            counts.acquisition_runs_created += 1;
        }
    }

    let project_work_rows = sqlx::query(
        "SELECT project_id,canonical_doi,first_seen_ingestion_id,seed,min_depth \
         FROM project_works ORDER BY project_id,canonical_doi",
    )
    .fetch_all(&mut **transaction)
    .await?;
    for row in project_work_rows {
        let project_id: Uuid = row.get("project_id");
        let doi: String = row.get("canonical_doi");
        let report_id = report_for_doi(transaction, &mut reports, &doi, &mut counts).await?;
        let source = "legacy_project_works";
        let source_key = doi.clone();
        let existing = sqlx::query_scalar::<_, Uuid>(
            "SELECT id FROM records WHERE project_id=$1 AND source=$2 AND source_key=$3",
        )
        .bind(project_id)
        .bind(source)
        .bind(&source_key)
        .fetch_optional(&mut **transaction)
        .await?;
        let record_id = existing.unwrap_or_else(|| {
            stable_uuid("record", &format!("{project_id}:{source}:{source_key}"))
        });
        if let Some(existing_id) = existing {
            sqlx::query(
                "UPDATE records SET report_id=$4,acquisition_run_id=COALESCE($5,acquisition_run_id),\
                 title=(SELECT title FROM works WHERE canonical_doi=$2),abstract_text=(SELECT abstract_text FROM works WHERE canonical_doi=$2),\
                 publication_year=(SELECT COALESCE(published_year,issued_year) FROM works WHERE canonical_doi=$2),\
                 raw=jsonb_build_object('legacy_doi',$2,'seed',$6,'min_depth',$7) WHERE id=$1",
            )
            .bind(existing_id)
            .bind(&doi)
            .bind(project_id)
            .bind(report_id)
            .bind(row.get::<Option<Uuid>, _>("first_seen_ingestion_id"))
            .bind(row.get::<bool, _>("seed"))
            .bind(row.get::<i32, _>("min_depth"))
            .execute(&mut **transaction)
            .await?;
            counts.records_repaired += 1;
        } else {
            sqlx::query(
                "INSERT INTO records (id,project_id,report_id,acquisition_run_id,source,source_key,title,abstract_text,publication_year,raw) \
                 SELECT $1,$2,$3,$4,'legacy_project_works',w.canonical_doi, w.title,w.abstract_text,COALESCE(w.published_year,w.issued_year),\
                   jsonb_build_object('legacy_doi',w.canonical_doi,'seed',$5,'min_depth',$6) \
                 FROM works w WHERE w.canonical_doi=$7",
            )
                .bind(record_id)
                .bind(project_id)
                .bind(report_id)
                .bind(row.get::<Option<Uuid>, _>("first_seen_ingestion_id"))
                .bind(row.get::<bool, _>("seed"))
                .bind(row.get::<i32, _>("min_depth"))
                .bind(&doi)
            .execute(&mut **transaction)
            .await?;
            counts.records_created += 1;
        }
        upsert_project_report(transaction, project_id, report_id, record_id, &mut counts).await?;
    }

    let item_rows = sqlx::query(
        "SELECT ingestion_id,project_id,canonical_doi,depth,parent_doi,status,attempts,queued_at,fetched_at,\
                last_error,work_event_id \
         FROM ingestion_items ORDER BY ingestion_id,canonical_doi",
    )
    .fetch_all(&mut **transaction)
    .await?;
    for row in item_rows {
        let ingestion_id: Uuid = row.get("ingestion_id");
        let project_id: Uuid = row.get("project_id");
        let doi: String = row.get("canonical_doi");
        let report_id = report_for_doi(transaction, &mut reports, &doi, &mut counts).await?;
        let source = "legacy_ingestion_item";
        let source_key = format!("{ingestion_id}:{doi}");
        let existing = sqlx::query_scalar::<_, Uuid>(
            "SELECT id FROM records WHERE project_id=$1 AND source=$2 AND source_key=$3",
        )
        .bind(project_id)
        .bind(source)
        .bind(&source_key)
        .fetch_optional(&mut **transaction)
        .await?;
        let record_id = existing.unwrap_or_else(|| stable_uuid("record", &source_key));
        if existing.is_some() {
            sqlx::query(
                "UPDATE records SET report_id=$4,acquisition_run_id=$5,title=(SELECT title FROM works WHERE canonical_doi=$2),\
                 abstract_text=(SELECT abstract_text FROM works WHERE canonical_doi=$2),\
                 publication_year=(SELECT COALESCE(published_year,issued_year) FROM works WHERE canonical_doi=$2),\
                 raw=jsonb_build_object('legacy_ingestion_id',$1,'legacy_doi',$2,'depth',$6,'parent_doi',$7,'status',$8) WHERE id=$3",
            )
            .bind(ingestion_id)
            .bind(&doi)
            .bind(record_id)
            .bind(report_id)
            .bind(ingestion_id)
            .bind(row.get::<i32, _>("depth"))
            .bind(row.get::<Option<String>, _>("parent_doi"))
            .bind(row.get::<String, _>("status"))
            .execute(&mut **transaction)
            .await?;
            counts.records_repaired += 1;
        } else {
            sqlx::query(
                "INSERT INTO records (id,project_id,report_id,acquisition_run_id,source,source_key,title,abstract_text,publication_year,raw) \
                 SELECT $1,$2,$3,$4,'legacy_ingestion_item',$5,w.title,w.abstract_text,COALESCE(w.published_year,w.issued_year),\
                   jsonb_build_object('legacy_ingestion_id',$6,'legacy_doi',$7,'depth',$8,'parent_doi',$9,'status',$10) \
                 FROM works w WHERE w.canonical_doi=$7 \
                 UNION ALL SELECT $1,$2,$3,$4,'legacy_ingestion_item',$5,NULL,NULL,NULL,\
                   jsonb_build_object('legacy_ingestion_id',$6,'legacy_doi',$7,'depth',$8,'parent_doi',$9,'status',$10) \
                 WHERE NOT EXISTS (SELECT 1 FROM works WHERE canonical_doi=$7)",
            )
                .bind(record_id)
                .bind(project_id)
                .bind(report_id)
                .bind(ingestion_id)
                .bind(&source_key)
                .bind(ingestion_id)
            .bind(&doi)
            .bind(row.get::<i32, _>("depth"))
            .bind(row.get::<Option<String>, _>("parent_doi"))
            .bind(row.get::<String, _>("status"))
            .execute(&mut **transaction)
            .await?;
            counts.records_created += 1;
        }
        upsert_project_report(transaction, project_id, report_id, record_id, &mut counts).await?;

        let provenance_exists: bool = sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM record_provenance WHERE acquisition_run_id=$1 AND canonical_doi=$2)",
        )
        .bind(ingestion_id)
        .bind(&doi)
        .fetch_one(&mut **transaction)
        .await?;
        sqlx::query(
            "INSERT INTO record_provenance \
             (record_id,acquisition_run_id,canonical_doi,depth,parent_doi,status,attempts,queued_at,fetched_at,last_error,work_event_id) \
             VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11) \
             ON CONFLICT (acquisition_run_id,canonical_doi) DO UPDATE SET record_id=EXCLUDED.record_id,depth=EXCLUDED.depth,\
               parent_doi=EXCLUDED.parent_doi,status=EXCLUDED.status,attempts=EXCLUDED.attempts,queued_at=EXCLUDED.queued_at,\
               fetched_at=EXCLUDED.fetched_at,last_error=EXCLUDED.last_error,work_event_id=EXCLUDED.work_event_id",
        )
        .bind(record_id)
        .bind(ingestion_id)
        .bind(&doi)
        .bind(row.get::<i32, _>("depth"))
        .bind(row.get::<Option<String>, _>("parent_doi"))
        .bind(row.get::<String, _>("status"))
        .bind(row.get::<i32, _>("attempts"))
        .bind(row.get::<DateTime<Utc>, _>("queued_at"))
        .bind(row.get::<Option<DateTime<Utc>>, _>("fetched_at"))
        .bind(row.get::<Option<String>, _>("last_error"))
        .bind(row.get::<Option<Uuid>, _>("work_event_id"))
        .execute(&mut **transaction)
        .await?;
        if !provenance_exists {
            counts.provenance_rows_created += 1;
        }
    }

    let citation_rows = sqlx::query(
        "SELECT project_id,source_doi,target_doi,source,first_seen_ingestion_id,created_at \
         FROM legacy_citations ORDER BY project_id,source_doi,target_doi",
    )
    .fetch_all(&mut **transaction)
    .await?;
    for row in citation_rows {
        let project_id: Uuid = row.get("project_id");
        let source_doi: String = row.get("source_doi");
        let target_doi: String = row.get("target_doi");
        let source_report_id =
            report_for_doi(transaction, &mut reports, &source_doi, &mut counts).await?;
        let target_report_id =
            report_for_doi(transaction, &mut reports, &target_doi, &mut counts).await?;
        let exists: bool = sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM citations WHERE project_id=$1 AND source_report_id=$2 AND target_report_id=$3)",
        )
        .bind(project_id)
        .bind(source_report_id)
        .bind(target_report_id)
        .fetch_one(&mut **transaction)
        .await?;
        sqlx::query(
            "INSERT INTO citations (project_id,source_report_id,target_report_id,source,first_seen_ingestion_id,legacy_source_doi,legacy_target_doi,created_at) \
             VALUES ($1,$2,$3,$4,$5,$6,$7,$8) \
             ON CONFLICT (project_id,source_report_id,target_report_id) DO UPDATE SET source=EXCLUDED.source,\
               first_seen_ingestion_id=EXCLUDED.first_seen_ingestion_id,legacy_source_doi=EXCLUDED.legacy_source_doi,\
               legacy_target_doi=EXCLUDED.legacy_target_doi",
        )
        .bind(project_id)
        .bind(source_report_id)
        .bind(target_report_id)
        .bind(row.get::<String, _>("source"))
        .bind(row.get::<Option<Uuid>, _>("first_seen_ingestion_id"))
        .bind(&source_doi)
        .bind(&target_doi)
        .bind(row.get::<DateTime<Utc>, _>("created_at"))
        .execute(&mut **transaction)
        .await?;
        if exists {
            counts.citations_repaired += 1;
        } else {
            counts.citations_created += 1;
        }
    }

    Ok(counts)
}

async fn ensure_report(
    transaction: &mut Transaction<'_, Postgres>,
    legacy_report: LegacyReport,
    counts: &mut LegacyImportCounts,
    repair_existing: bool,
) -> anyhow::Result<Uuid> {
    let normalized = normalize(&legacy_report.doi)?;
    if let Some(report_id) = sqlx::query_scalar::<_, Uuid>(
        "SELECT report_id FROM report_identifiers WHERE scheme='doi' AND normalized_value=$1",
    )
    .bind(&normalized)
    .fetch_optional(&mut **transaction)
    .await?
    {
        counts.reports_reused += 1;
        if repair_existing {
            sqlx::query(
                "UPDATE reports SET title=$2,abstract_text=$3,publication_year=COALESCE($4,$5),\
                 journal=$6,url=$7,raw=$8,updated_at=now() WHERE id=$1",
            )
            .bind(report_id)
            .bind(legacy_report.title.as_deref())
            .bind(legacy_report.abstract_text.as_deref())
            .bind(legacy_report.published_year)
            .bind(legacy_report.issued_year)
            .bind(legacy_report.journal.as_deref())
            .bind(legacy_report.url.as_deref())
            .bind(&legacy_report.raw)
            .execute(&mut **transaction)
            .await?;
        }
        return Ok(report_id);
    }

    let report_id = stable_uuid("report", &normalized);
    let inserted = sqlx::query(
        "INSERT INTO reports (id,title,abstract_text,publication_year,journal,url,raw) \
         VALUES ($1,$2,$3,COALESCE($4,$5),$6,$7,$8) ON CONFLICT (id) DO UPDATE SET title=EXCLUDED.title,\
           abstract_text=EXCLUDED.abstract_text,publication_year=EXCLUDED.publication_year,journal=EXCLUDED.journal,url=EXCLUDED.url,raw=EXCLUDED.raw,updated_at=now()",
    )
    .bind(report_id)
    .bind(legacy_report.title)
    .bind(legacy_report.abstract_text)
    .bind(legacy_report.published_year)
    .bind(legacy_report.issued_year)
    .bind(legacy_report.journal)
    .bind(legacy_report.url)
    .bind(legacy_report.raw)
    .execute(&mut **transaction)
    .await?;
    if inserted.rows_affected() == 1 {
        counts.reports_created += 1;
    }
    let identifier_id = stable_uuid("report-identifier", &format!("doi:{normalized}"));
    let identifier = sqlx::query(
        "INSERT INTO report_identifiers (id,report_id,scheme,value,normalized_value) VALUES ($1,$2,'doi',$3,$4) \
         ON CONFLICT (scheme,normalized_value) DO NOTHING",
    )
    .bind(identifier_id)
    .bind(report_id)
    .bind(legacy_report.doi)
    .bind(&normalized)
    .execute(&mut **transaction)
    .await?;
    if identifier.rows_affected() == 1 {
        counts.report_identifiers_created += 1;
    }
    let mapped = sqlx::query_scalar::<_, Uuid>(
        "SELECT report_id FROM report_identifiers WHERE scheme='doi' AND normalized_value=$1",
    )
    .bind(normalized)
    .fetch_one(&mut **transaction)
    .await?;
    Ok(mapped)
}

async fn report_for_doi(
    transaction: &mut Transaction<'_, Postgres>,
    reports: &mut HashMap<String, Uuid>,
    doi: &str,
    counts: &mut LegacyImportCounts,
) -> anyhow::Result<Uuid> {
    let normalized = normalize(doi)?;
    if let Some(report_id) = reports.get(&normalized) {
        return Ok(*report_id);
    }
    let report_id = ensure_report(
        transaction,
        LegacyReport {
            doi: doi.to_owned(),
            title: None,
            abstract_text: None,
            issued_year: None,
            published_year: None,
            journal: None,
            url: None,
            raw: serde_json::json!({"legacy_doi": doi}),
        },
        counts,
        false,
    )
    .await?;
    reports.insert(normalized, report_id);
    Ok(report_id)
}

async fn upsert_project_report(
    transaction: &mut Transaction<'_, Postgres>,
    project_id: Uuid,
    report_id: Uuid,
    record_id: Uuid,
    counts: &mut LegacyImportCounts,
) -> anyhow::Result<()> {
    let exists: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM project_reports WHERE project_id=$1 AND report_id=$2)",
    )
    .bind(project_id)
    .bind(report_id)
    .fetch_one(&mut **transaction)
    .await?;
    sqlx::query(
        "INSERT INTO project_reports (project_id,report_id,first_seen_record_id) VALUES ($1,$2,$3) \
         ON CONFLICT (project_id,report_id) DO UPDATE SET first_seen_record_id=COALESCE(project_reports.first_seen_record_id,EXCLUDED.first_seen_record_id)",
    )
    .bind(project_id)
    .bind(report_id)
    .bind(record_id)
    .execute(&mut **transaction)
    .await?;
    if !exists {
        counts.project_reports_created += 1;
    }
    Ok(())
}

fn normalize(doi: &str) -> anyhow::Result<String> {
    normalize_doi(doi).map_err(|error| anyhow!(error).context("normalize legacy DOI"))
}

fn stable_uuid(kind: &str, key: &str) -> Uuid {
    Uuid::new_v5(
        &Uuid::NAMESPACE_URL,
        format!("deepref:{kind}:{key}").as_bytes(),
    )
}
