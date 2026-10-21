#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
//! Upgrade compatibility for pre-0086 active jobs whose project was lost.

use anyhow::{Result, ensure};
use deepref_app::MIGRATOR;
use sqlx::{PgPool, Row, migrate::Migrator, postgres::PgPoolOptions};
use uuid::Uuid;

async fn pre_0086_pool() -> Option<PgPool> {
    let url = deepref_app::test_isolation::isolated_empty_url_from_env("DATABASE_URL").await?;
    let pool = PgPoolOptions::new()
        .max_connections(4)
        .connect(&url)
        .await
        .expect("pre-0086 database");
    Migrator::with_migrations(
        MIGRATOR
            .iter()
            .filter(|migration| migration.version < 86)
            .cloned()
            .collect(),
    )
    .run(&pool)
    .await
    .expect("pre-0086 migrations");
    Some(pool)
}

#[tokio::test]
async fn active_orphan_is_quarantined_before_workload_class_backfill() -> Result<()> {
    let Some(pool) = pre_0086_pool().await else {
        return Ok(());
    };
    let job_id = Uuid::new_v4();
    sqlx::query("ALTER TABLE jobs DROP CONSTRAINT jobs_active_project_id_check")
        .execute(&pool)
        .await?;
    sqlx::query(
        "INSERT INTO jobs (id,kind,payload,state,dedupe_key)
         VALUES ($1,'parse_document','{}','queued',$2)",
    )
    .bind(job_id)
    .bind(format!("pre-0086-active-orphan:{job_id}"))
    .execute(&pool)
    .await?;
    sqlx::query(
        "ALTER TABLE jobs ADD CONSTRAINT jobs_active_project_id_check
         CHECK (project_id IS NOT NULL OR state IN ('completed','failed','dead')) NOT VALID",
    )
    .execute(&pool)
    .await?;

    MIGRATOR.run(&pool).await.expect("full migration chain");

    let row = sqlx::query("SELECT state,last_error,workload_class,claim_seq FROM jobs WHERE id=$1")
        .bind(job_id)
        .fetch_one(&pool)
        .await?;
    ensure!(row.get::<String, _>("state") == "dead");
    ensure!(
        row.get::<String, _>("last_error") == "quarantined by 0086: active job without project"
    );
    ensure!(row.get::<Option<String>, _>("workload_class").is_none());
    ensure!(row.get::<i64, _>("claim_seq") == 0);

    let index_valid: bool = sqlx::query_scalar(
        "SELECT indisvalid FROM pg_index WHERE indexrelid='jobs_claim_pool_idx'::regclass",
    )
    .fetch_one(&pool)
    .await?;
    ensure!(index_valid, "the no-transaction concurrent index is valid");
    Ok(())
}
