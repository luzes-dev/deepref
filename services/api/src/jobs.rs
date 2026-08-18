use std::time::Duration;

use serde_json::Value;
use sqlx::{PgPool, Row};
use uuid::Uuid;

const LEASE: &str = r#"
WITH candidate AS (
  SELECT id
  FROM jobs
  WHERE state = 'queued'
    AND available_at <= now()
    AND (leased_until IS NULL OR leased_until < now())
  ORDER BY priority DESC, available_at ASC
  FOR UPDATE SKIP LOCKED
  LIMIT 1
)
UPDATE jobs j
SET state = 'running', lease_owner = $1, leased_until = now() + interval '60 seconds', attempts = attempts + 1
FROM candidate
WHERE j.id = candidate.id
RETURNING j.id, j.kind, j.payload, j.attempts, j.max_attempts
"#;

pub async fn run(pool: PgPool) {
    let owner = format!("api-job-runner-{}", Uuid::new_v4());
    let _ = sqlx::query(
        "UPDATE jobs SET state='queued', lease_owner=NULL, leased_until=NULL WHERE state='running' AND leased_until < now()",
    )
    .execute(&pool)
    .await;

    loop {
        match claim(&pool, &owner).await {
            Ok(Some(job)) => {
                let result = process(&pool, &job.kind, &job.payload).await;
                if let Err(error) = result {
                    tracing::warn!(job_id = %job.id, kind = %job.kind, %error, "durable job failed");
                    fail(&pool, &owner, &job, &error.to_string()).await;
                } else {
                    complete(&pool, &owner, job.id).await;
                }
            }
            Ok(None) => tokio::time::sleep(Duration::from_millis(500)).await,
            Err(error) => {
                tracing::warn!(%error, "durable job claim failed");
                tokio::time::sleep(Duration::from_secs(2)).await;
            }
        }
    }
}

struct Job {
    id: Uuid,
    kind: String,
    payload: Value,
    attempts: i32,
    max_attempts: i32,
}

async fn claim(pool: &PgPool, owner: &str) -> Result<Option<Job>, sqlx::Error> {
    let row = sqlx::query(LEASE).bind(owner).fetch_optional(pool).await?;
    Ok(row.map(|row| Job {
        id: row.get("id"),
        kind: row.get("kind"),
        payload: row.get("payload"),
        attempts: row.get("attempts"),
        max_attempts: row.get("max_attempts"),
    }))
}

async fn process(pool: &PgPool, kind: &str, payload: &Value) -> anyhow::Result<()> {
    match kind {
        "recompute_prisma" => recompute_prisma(pool, payload).await,
        other => anyhow::bail!("unsupported job kind: {other}"),
    }
}

async fn recompute_prisma(pool: &PgPool, payload: &Value) -> anyhow::Result<()> {
    let project_id = payload
        .get("project_id")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow::anyhow!("recompute_prisma payload is missing project_id"))?
        .parse::<Uuid>()?;
    let row = sqlx::query(
        r#"
        SELECT
          count(*)::bigint AS records_identified,
          count(*) FILTER (WHERE pr.report_id IS NOT NULL)::bigint AS records_deduplicated,
          count(*) FILTER (WHERE coalesce(ss.title_abstract_status, 'unscreened') = 'unscreened')::bigint AS title_abstract_pending,
          count(*) FILTER (WHERE ss.title_abstract_status = 'include')::bigint AS title_abstract_included,
          count(*) FILTER (WHERE ss.title_abstract_status = 'exclude')::bigint AS title_abstract_excluded,
          count(*) FILTER (WHERE coalesce(ss.final_status, 'unscreened') = 'pending_full_text')::bigint AS full_text_pending,
          count(*) FILTER (WHERE ss.full_text_status = 'include')::bigint AS full_text_included,
          count(*) FILTER (WHERE ss.full_text_status = 'exclude')::bigint AS full_text_excluded,
          coalesce(max(ss.revision), 0)::bigint AS revision
        FROM records rec
        LEFT JOIN project_reports pr ON pr.project_id = rec.project_id AND pr.report_id = rec.report_id
        LEFT JOIN screening_state ss ON ss.project_id = rec.project_id AND ss.report_id = rec.report_id
        WHERE rec.project_id = $1
        "#,
    )
    .bind(project_id)
    .fetch_one(pool)
    .await?;
    sqlx::query(
        r#"
        INSERT INTO prisma_snapshots (
          project_id, records_identified, records_deduplicated,
          title_abstract_pending, title_abstract_included, title_abstract_excluded,
          full_text_pending, full_text_included, full_text_excluded, revision, updated_at
        ) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,now())
        ON CONFLICT (project_id) DO UPDATE SET
          records_identified = EXCLUDED.records_identified,
          records_deduplicated = EXCLUDED.records_deduplicated,
          title_abstract_pending = EXCLUDED.title_abstract_pending,
          title_abstract_included = EXCLUDED.title_abstract_included,
          title_abstract_excluded = EXCLUDED.title_abstract_excluded,
          full_text_pending = EXCLUDED.full_text_pending,
          full_text_included = EXCLUDED.full_text_included,
          full_text_excluded = EXCLUDED.full_text_excluded,
          revision = EXCLUDED.revision,
          updated_at = now()
        "#,
    )
    .bind(project_id)
    .bind(row.get::<i64, _>("records_identified"))
    .bind(row.get::<i64, _>("records_deduplicated"))
    .bind(row.get::<i64, _>("title_abstract_pending"))
    .bind(row.get::<i64, _>("title_abstract_included"))
    .bind(row.get::<i64, _>("title_abstract_excluded"))
    .bind(row.get::<i64, _>("full_text_pending"))
    .bind(row.get::<i64, _>("full_text_included"))
    .bind(row.get::<i64, _>("full_text_excluded"))
    .bind(row.get::<i64, _>("revision"))
    .execute(pool)
    .await?;
    Ok(())
}

async fn complete(pool: &PgPool, owner: &str, id: Uuid) {
    if let Err(error) = sqlx::query(
        "UPDATE jobs SET state='completed', completed_at=now(), lease_owner=NULL, leased_until=NULL WHERE id=$1 AND lease_owner=$2",
    )
    .bind(id)
    .bind(owner)
    .execute(pool)
    .await
    {
        tracing::warn!(job_id = %id, %error, "durable job completion update failed");
    }
}

async fn fail(pool: &PgPool, owner: &str, job: &Job, error: &str) {
    let state = if job.attempts >= job.max_attempts {
        "dead"
    } else {
        "queued"
    };
    if let Err(update_error) = sqlx::query(
        "UPDATE jobs SET state=$3, last_error=$4, available_at=now()+interval '10 seconds', lease_owner=NULL, leased_until=NULL WHERE id=$1 AND lease_owner=$2",
    )
    .bind(job.id)
    .bind(owner)
    .bind(state)
    .bind(error)
    .execute(pool)
    .await
    {
        tracing::warn!(job_id = %job.id, %update_error, "durable job failure update failed");
    }
}
