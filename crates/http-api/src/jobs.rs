use std::{future::Future, time::Duration};

use serde_json::Value;
use sqlx::{PgPool, Row};
use uuid::Uuid;

const LEASE_RENEWAL_INTERVAL: Duration = Duration::from_secs(20);

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

const RECOVER_EXPIRED: &str = r#"
UPDATE jobs
SET state = 'queued', lease_owner = NULL, leased_until = NULL
WHERE state = 'running' AND leased_until < now()
"#;

const RENEW_LEASE: &str = r#"
UPDATE jobs
SET leased_until = now() + interval '60 seconds'
WHERE id = $1
  AND state = 'running'
  AND lease_owner = $2
  AND leased_until >= now()
"#;

pub(crate) fn recompute_prisma_dedupe_key(project_id: Uuid, event_id: Uuid) -> String {
    format!("recompute_prisma:{project_id}:{event_id}")
}

pub async fn run(pool: PgPool) {
    let owner = format!("api-job-runner-{}", Uuid::new_v4());
    if let Err(error) = recover_expired_jobs(&pool).await {
        tracing::warn!(%error, "durable job lease recovery failed");
    }

    loop {
        if let Err(error) = recover_expired_jobs(&pool).await {
            tracing::warn!(%error, "durable job lease recovery failed");
        }
        match claim(&pool, &owner).await {
            Ok(Some(job)) => {
                let result = process_with_lease_renewal(&pool, &owner, &job).await;
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

async fn recover_expired_jobs(pool: &PgPool) -> Result<u64, sqlx::Error> {
    Ok(sqlx::query(RECOVER_EXPIRED)
        .execute(pool)
        .await?
        .rows_affected())
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

async fn process_with_lease_renewal(pool: &PgPool, owner: &str, job: &Job) -> anyhow::Result<()> {
    run_with_lease_renewal(
        pool,
        owner,
        job.id,
        LEASE_RENEWAL_INTERVAL,
        process(pool, &job.kind, &job.payload),
    )
    .await
}

async fn run_with_lease_renewal<F>(
    pool: &PgPool,
    owner: &str,
    job_id: Uuid,
    renewal_interval: Duration,
    work: F,
) -> anyhow::Result<()>
where
    F: Future<Output = anyhow::Result<()>>,
{
    if renewal_interval.is_zero() {
        anyhow::bail!("job lease renewal interval must be greater than zero");
    }
    tokio::pin!(work);
    let mut renewal = tokio::time::interval(renewal_interval);
    renewal.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    renewal.tick().await;

    loop {
        tokio::select! {
            result = &mut work => return result,
            _ = renewal.tick() => match renew_lease(pool, owner, job_id).await {
                Ok(true) => {}
                Ok(false) => anyhow::bail!("job lease was lost before renewal"),
                Err(error) => anyhow::bail!("job lease renewal failed: {error}"),
            },
        }
    }
}

async fn renew_lease(pool: &PgPool, owner: &str, id: Uuid) -> Result<bool, sqlx::Error> {
    Ok(sqlx::query(RENEW_LEASE)
        .bind(id)
        .bind(owner)
        .execute(pool)
        .await?
        .rows_affected()
        == 1)
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
    match sqlx::query(
        "UPDATE jobs SET state='completed', completed_at=now(), lease_owner=NULL, leased_until=NULL WHERE id=$1 AND state='running' AND lease_owner=$2 AND leased_until >= now()",
    )
    .bind(id)
    .bind(owner)
    .execute(pool)
    .await
    {
        Ok(result) if result.rows_affected() == 1 => {}
        Ok(_) => tracing::warn!(job_id = %id, "durable job completion rejected because lease is no longer owned"),
        Err(error) => tracing::warn!(job_id = %id, %error, "durable job completion update failed"),
    }
}

async fn fail(pool: &PgPool, owner: &str, job: &Job, error: &str) {
    let state = failure_state(job.attempts, job.max_attempts);
    match sqlx::query(
        "UPDATE jobs SET state=$3, last_error=$4, available_at=now()+interval '10 seconds', lease_owner=NULL, leased_until=NULL WHERE id=$1 AND state='running' AND lease_owner=$2 AND leased_until >= now()",
    )
    .bind(job.id)
    .bind(owner)
    .bind(state)
    .bind(error)
    .execute(pool)
    .await
    {
        Ok(result) if result.rows_affected() == 1 => {}
        Ok(_) => tracing::warn!(job_id = %job.id, "durable job failure update rejected because lease is no longer owned"),
        Err(update_error) => {
            tracing::warn!(job_id = %job.id, %update_error, "durable job failure update failed");
        }
    }
}

fn failure_state(attempts: i32, max_attempts: i32) -> &'static str {
    if attempts >= max_attempts {
        "dead"
    } else {
        "queued"
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    };

    use super::*;
    use sqlx::{PgPool, postgres::PgPoolOptions};

    static DATABASE_TEST_MUTEX: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

    async fn database_test_guard() -> tokio::sync::MutexGuard<'static, ()> {
        DATABASE_TEST_MUTEX.lock().await
    }

    async fn database() -> Option<PgPool> {
        let url = match std::env::var("DATABASE_URL") {
            Ok(url) => url,
            Err(_) => return None,
        };
        let pool = PgPoolOptions::new()
            .max_connections(5)
            .connect(&url)
            .await
            .unwrap_or_else(|error| {
                panic!("DATABASE_URL is set but PostgreSQL is unavailable: {error}")
            });
        deepref_postgres::migrate(&pool)
            .await
            .unwrap_or_else(|error| panic!("failed to apply PostgreSQL migrations: {error}"));
        Some(pool)
    }

    #[test]
    fn prisma_dedupe_key_uses_the_immutable_event_id() {
        let project_id = Uuid::from_u128(1);
        let first_event_id = Uuid::from_u128(2);
        let second_event_id = Uuid::from_u128(3);

        assert_ne!(
            recompute_prisma_dedupe_key(project_id, first_event_id),
            recompute_prisma_dedupe_key(project_id, second_event_id)
        );
    }

    #[test]
    fn failure_state_preserves_retry_and_dead_attempt_boundaries() {
        assert_eq!(failure_state(1, 3), "queued");
        assert_eq!(failure_state(3, 3), "dead");
        assert_eq!(failure_state(4, 3), "dead");
    }

    #[tokio::test]
    async fn independent_report_events_enqueue_independent_jobs() {
        let _guard = database_test_guard().await;
        let Some(pool) = database().await else {
            return;
        };
        let project_id = Uuid::new_v4();
        let first_event_id = Uuid::new_v4();
        let second_event_id = Uuid::new_v4();
        let first_job_id = Uuid::new_v4();
        let second_job_id = Uuid::new_v4();

        for (job_id, event_id) in [
            (first_job_id, first_event_id),
            (second_job_id, second_event_id),
        ] {
            sqlx::query(
                "INSERT INTO jobs (id,kind,payload,priority,max_attempts,dedupe_key) VALUES ($1,'recompute_prisma',$2,10,5,$3)",
            )
            .bind(job_id)
            .bind(serde_json::json!({ "project_id": project_id }))
            .bind(recompute_prisma_dedupe_key(project_id, event_id))
            .execute(&pool)
            .await
            .unwrap_or_else(|error| panic!("failed to enqueue test job: {error}"));
        }

        let count: i64 = sqlx::query_scalar("SELECT count(*)::bigint FROM jobs WHERE id = ANY($1)")
            .bind(vec![first_job_id, second_job_id])
            .fetch_one(&pool)
            .await
            .unwrap_or_else(|error| panic!("failed to count test jobs: {error}"));
        assert_eq!(count, 2);

        sqlx::query("DELETE FROM jobs WHERE id = ANY($1)")
            .bind(vec![first_job_id, second_job_id])
            .execute(&pool)
            .await
            .unwrap_or_else(|error| panic!("failed to clean test jobs: {error}"));
    }

    #[tokio::test]
    async fn expired_running_jobs_are_reclaimed() {
        let _guard = database_test_guard().await;
        let Some(pool) = database().await else {
            return;
        };
        let job_id = Uuid::new_v4();
        sqlx::query(
            "INSERT INTO jobs (id,kind,state,lease_owner,leased_until) VALUES ($1,'recompute_prisma','running','expired-owner',now()-interval '1 second')",
        )
        .bind(job_id)
        .execute(&pool)
        .await
        .unwrap_or_else(|error| panic!("failed to insert expired test job: {error}"));

        assert_eq!(recover_expired_jobs(&pool).await.unwrap(), 1);
        let row = sqlx::query("SELECT state, lease_owner, leased_until FROM jobs WHERE id=$1")
            .bind(job_id)
            .fetch_one(&pool)
            .await
            .unwrap_or_else(|error| panic!("failed to fetch reclaimed test job: {error}"));
        assert_eq!(row.get::<String, _>("state"), "queued");
        assert!(row.get::<Option<String>, _>("lease_owner").is_none());
        assert!(
            row.get::<Option<chrono::DateTime<chrono::Utc>>, _>("leased_until")
                .is_none()
        );

        sqlx::query("DELETE FROM jobs WHERE id=$1")
            .bind(job_id)
            .execute(&pool)
            .await
            .unwrap_or_else(|error| panic!("failed to clean reclaimed test job: {error}"));
    }

    #[tokio::test]
    async fn claimed_jobs_can_renew_their_lease() {
        let _guard = database_test_guard().await;
        let Some(pool) = database().await else {
            return;
        };
        let job_id = Uuid::new_v4();
        let owner = "renewing-owner";
        sqlx::query(
            "INSERT INTO jobs (id,kind,state,lease_owner,leased_until) VALUES ($1,'recompute_prisma','running',$2,now()+interval '5 seconds')",
        )
        .bind(job_id)
        .bind(owner)
        .execute(&pool)
        .await
        .unwrap_or_else(|error| panic!("failed to insert renewal test job: {error}"));
        let before: chrono::DateTime<chrono::Utc> =
            sqlx::query_scalar("SELECT leased_until FROM jobs WHERE id=$1")
                .bind(job_id)
                .fetch_one(&pool)
                .await
                .unwrap_or_else(|error| panic!("failed to read lease before renewal: {error}"));

        assert!(renew_lease(&pool, owner, job_id).await.unwrap());
        let after: chrono::DateTime<chrono::Utc> =
            sqlx::query_scalar("SELECT leased_until FROM jobs WHERE id=$1")
                .bind(job_id)
                .fetch_one(&pool)
                .await
                .unwrap_or_else(|error| panic!("failed to read lease after renewal: {error}"));
        assert!(after > before);

        sqlx::query("DELETE FROM jobs WHERE id=$1")
            .bind(job_id)
            .execute(&pool)
            .await
            .unwrap_or_else(|error| panic!("failed to clean renewal test job: {error}"));
    }

    #[tokio::test]
    async fn wrong_owner_cannot_complete_a_claimed_job() {
        let _guard = database_test_guard().await;
        let Some(pool) = database().await else {
            return;
        };
        let job_id = Uuid::new_v4();
        sqlx::query(
            "INSERT INTO jobs (id,kind,state,lease_owner,leased_until) VALUES ($1,'recompute_prisma','running','actual-owner',now()+interval '60 seconds')",
        )
        .bind(job_id)
        .execute(&pool)
        .await
        .unwrap_or_else(|error| panic!("failed to insert owner test job: {error}"));

        complete(&pool, "wrong-owner", job_id).await;
        let row = sqlx::query("SELECT state, lease_owner FROM jobs WHERE id=$1")
            .bind(job_id)
            .fetch_one(&pool)
            .await
            .unwrap_or_else(|error| panic!("failed to fetch owner test job: {error}"));
        assert_eq!(row.get::<String, _>("state"), "running");
        assert_eq!(row.get::<String, _>("lease_owner"), "actual-owner");

        sqlx::query("DELETE FROM jobs WHERE id=$1")
            .bind(job_id)
            .execute(&pool)
            .await
            .unwrap_or_else(|error| panic!("failed to clean owner test job: {error}"));
    }

    #[tokio::test]
    async fn expired_same_owner_cannot_complete_a_claimed_job() {
        let _guard = database_test_guard().await;
        let Some(pool) = database().await else {
            return;
        };
        let job_id = Uuid::new_v4();
        let owner = "expired-owner";
        sqlx::query(
            "INSERT INTO jobs (id,kind,state,lease_owner,leased_until) VALUES ($1,'recompute_prisma','running',$2,now()-interval '1 second')",
        )
        .bind(job_id)
        .bind(owner)
        .execute(&pool)
        .await
        .unwrap_or_else(|error| panic!("failed to insert expired completion job: {error}"));

        complete(&pool, owner, job_id).await;
        let row = sqlx::query("SELECT state, lease_owner FROM jobs WHERE id=$1")
            .bind(job_id)
            .fetch_one(&pool)
            .await
            .unwrap_or_else(|error| panic!("failed to fetch expired completion job: {error}"));
        assert_eq!(row.get::<String, _>("state"), "running");
        assert_eq!(row.get::<String, _>("lease_owner"), owner);

        sqlx::query("DELETE FROM jobs WHERE id=$1")
            .bind(job_id)
            .execute(&pool)
            .await
            .unwrap_or_else(|error| panic!("failed to clean expired completion job: {error}"));
    }

    #[tokio::test]
    async fn lease_loss_aborts_work_before_its_side_effect() {
        let _guard = database_test_guard().await;
        let Some(pool) = database().await else {
            return;
        };
        let job_id = Uuid::new_v4();
        let owner = "lease-loss-owner";
        sqlx::query(
            "INSERT INTO jobs (id,kind,state,lease_owner,leased_until) VALUES ($1,'test','running',$2,now()+interval '1 second')",
        )
        .bind(job_id)
        .bind(owner)
        .execute(&pool)
        .await
        .unwrap_or_else(|error| panic!("failed to insert lease-loss job: {error}"));

        let side_effect_ran = Arc::new(AtomicBool::new(false));
        let side_effect_marker = Arc::clone(&side_effect_ran);
        let task_pool = pool.clone();
        let task = tokio::spawn(async move {
            run_with_lease_renewal(
                &task_pool,
                owner,
                job_id,
                Duration::from_millis(5),
                async move {
                    tokio::time::sleep(Duration::from_millis(100)).await;
                    side_effect_marker.store(true, Ordering::SeqCst);
                    Ok(())
                },
            )
            .await
        });

        tokio::time::sleep(Duration::from_millis(10)).await;
        sqlx::query("UPDATE jobs SET leased_until=now()-interval '1 second' WHERE id=$1")
            .bind(job_id)
            .execute(&pool)
            .await
            .unwrap_or_else(|error| panic!("failed to expire lease-loss job: {error}"));

        let result = task
            .await
            .unwrap_or_else(|error| panic!("lease-loss task should join: {error}"));
        assert!(result.is_err(), "lease loss must abort processing");
        assert!(
            !side_effect_ran.load(Ordering::SeqCst),
            "work must not continue after lease loss"
        );

        sqlx::query("DELETE FROM jobs WHERE id=$1")
            .bind(job_id)
            .execute(&pool)
            .await
            .unwrap_or_else(|error| panic!("failed to clean lease-loss job: {error}"));
    }

    #[tokio::test]
    async fn prisma_projection_converges_after_independent_screening_changes() {
        let _guard = database_test_guard().await;
        let Some(pool) = database().await else {
            return;
        };
        let project_id = Uuid::new_v4();
        let protocol_id = Uuid::new_v4();
        let first_report_id = Uuid::new_v4();
        let second_report_id = Uuid::new_v4();
        let first_record_id = Uuid::new_v4();
        let second_record_id = Uuid::new_v4();
        let first_event_id = Uuid::new_v4();
        let second_event_id = Uuid::new_v4();
        let third_event_id = Uuid::new_v4();

        sqlx::query("INSERT INTO projects (id,name) VALUES ($1,'job test project')")
            .bind(project_id)
            .execute(&pool)
            .await
            .unwrap_or_else(|error| panic!("failed to insert test project: {error}"));
        sqlx::query(
            "INSERT INTO protocol_versions (id,project_id,version,name,status,criteria) VALUES ($1,$2,1,'test','published','[]')",
        )
        .bind(protocol_id)
        .bind(project_id)
        .execute(&pool)
        .await
        .unwrap_or_else(|error| panic!("failed to insert test protocol: {error}"));
        for report_id in [first_report_id, second_report_id] {
            sqlx::query("INSERT INTO reports (id,title) VALUES ($1,'test report')")
                .bind(report_id)
                .execute(&pool)
                .await
                .unwrap_or_else(|error| panic!("failed to insert test report: {error}"));
        }
        for (record_id, report_id) in [
            (first_record_id, first_report_id),
            (second_record_id, second_report_id),
        ] {
            sqlx::query(
                "INSERT INTO records (id,project_id,report_id,source) VALUES ($1,$2,$3,'test')",
            )
            .bind(record_id)
            .bind(project_id)
            .bind(report_id)
            .execute(&pool)
            .await
            .unwrap_or_else(|error| panic!("failed to insert test record: {error}"));
            sqlx::query("INSERT INTO project_reports (project_id,report_id) VALUES ($1,$2)")
                .bind(project_id)
                .bind(report_id)
                .execute(&pool)
                .await
                .unwrap_or_else(|error| panic!("failed to link test report: {error}"));
        }
        for (event_id, report_id, decision) in [
            (first_event_id, first_report_id, "include"),
            (second_event_id, second_report_id, "exclude"),
        ] {
            sqlx::query(
                "INSERT INTO screening_events (id,project_id,report_id,stage,decision,protocol_version_id,actor_kind,actor_id) VALUES ($1,$2,$3,'title_abstract',$4,$5,'system','job-test')",
            )
            .bind(event_id)
            .bind(project_id)
            .bind(report_id)
            .bind(decision)
            .bind(protocol_id)
            .execute(&pool)
            .await
            .unwrap_or_else(|error| panic!("failed to insert test screening event: {error}"));
        }
        sqlx::query(
            "INSERT INTO screening_state (project_id,report_id,title_abstract_status,final_status,revision,last_event_id) VALUES ($1,$2,'include','pending_full_text',1,$3),($1,$4,'exclude','exclude',1,$5)",
        )
        .bind(project_id)
        .bind(first_report_id)
        .bind(first_event_id)
        .bind(second_report_id)
        .bind(second_event_id)
        .execute(&pool)
        .await
        .unwrap_or_else(|error| panic!("failed to insert test screening state: {error}"));

        recompute_prisma(&pool, &serde_json::json!({ "project_id": project_id }))
            .await
            .unwrap_or_else(|error| panic!("failed to compute initial PRISMA snapshot: {error}"));
        let initial: (i64, i64, i64) = sqlx::query_as(
            "SELECT title_abstract_included,title_abstract_excluded,revision FROM prisma_snapshots WHERE project_id=$1",
        )
        .bind(project_id)
        .fetch_one(&pool)
        .await
        .unwrap_or_else(|error| panic!("failed to fetch initial PRISMA snapshot: {error}"));
        assert_eq!(initial, (1, 1, 1));

        sqlx::query(
            "INSERT INTO screening_events (id,project_id,report_id,stage,decision,protocol_version_id,actor_kind,actor_id,supersedes_event_id) VALUES ($1,$2,$3,'title_abstract','include',$4,'system','job-test',$5)",
        )
        .bind(third_event_id)
        .bind(project_id)
        .bind(second_report_id)
        .bind(protocol_id)
        .bind(second_event_id)
        .execute(&pool)
        .await
        .unwrap_or_else(|error| panic!("failed to insert converged screening event: {error}"));
        sqlx::query(
            "UPDATE screening_state SET title_abstract_status='include',final_status='pending_full_text',revision=2,last_event_id=$3 WHERE project_id=$1 AND report_id=$2",
        )
        .bind(project_id)
        .bind(second_report_id)
        .bind(third_event_id)
        .execute(&pool)
        .await
        .unwrap_or_else(|error| panic!("failed to update converged screening state: {error}"));

        recompute_prisma(&pool, &serde_json::json!({ "project_id": project_id }))
            .await
            .unwrap_or_else(|error| panic!("failed to compute converged PRISMA snapshot: {error}"));
        let converged: (i64, i64, i64) = sqlx::query_as(
            "SELECT title_abstract_included,title_abstract_excluded,revision FROM prisma_snapshots WHERE project_id=$1",
        )
        .bind(project_id)
        .fetch_one(&pool)
        .await
        .unwrap_or_else(|error| panic!("failed to fetch converged PRISMA snapshot: {error}"));
        assert_eq!(converged, (2, 0, 2));

        sqlx::query("DELETE FROM projects WHERE id=$1")
            .bind(project_id)
            .execute(&pool)
            .await
            .unwrap_or_else(|error| panic!("failed to clean convergence test project: {error}"));
    }
}
