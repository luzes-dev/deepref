#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::panic,
    clippy::panic_in_result_fn
)]
//! Typed job scope: workspace- and platform-scoped work runs without a dummy
//! project, and the jobs scope columns accept only consistent rows.
//! Skipped unless `DATABASE_URL` points at a reachable PostgreSQL.

use std::time::Duration;

use anyhow::Result;
use deepref_app::{PostgresDurableExecutionStore, scoped_job};
use deepref_application::jobs::{DurableExecutionStore, JobRetry, JobScope};
use deepref_domain::{JobKind, WorkspaceId};
use sqlx::{PgPool, Row};
use uuid::Uuid;

async fn database() -> Option<PgPool> {
    deepref_app::test_isolation::isolated_pool_from_env("DATABASE_URL", 8).await
}

/// Every scope column agrees with the scope kind: a project row carries its
/// project (and no workspace), a workspace row carries its workspace (and no
/// project), a platform row carries neither. No row may fabricate a project to
/// carry workspace- or platform-level work.
#[tokio::test]
async fn the_scope_check_rejects_inconsistent_rows() -> Result<()> {
    let Some(pool) = database().await else {
        return Ok(());
    };
    let project_id = Uuid::new_v4();
    sqlx::query("INSERT INTO projects (id,name) VALUES ($1,'scope check fixture')")
        .bind(project_id)
        .execute(&pool)
        .await?;
    let workspace_id = Uuid::new_v4();

    // Platform work with a workspace attached: inconsistent.
    let error = sqlx::query(
        "INSERT INTO jobs (id,scope,workspace_id,kind,workload_class,payload,dedupe_key)
         VALUES ($1,'platform',$2,'recompute_metrics','projection','{}',$3)",
    )
    .bind(Uuid::new_v4())
    .bind(workspace_id)
    .bind(format!("scope-check:platform-workspace:{}", Uuid::new_v4()))
    .execute(&pool)
    .await
    .expect_err("platform rows carry neither tenant reference");
    assert!(error.to_string().contains("jobs_scope_check"), "{error}");

    // Workspace work on a project: inconsistent.
    let error = sqlx::query(
        "INSERT INTO jobs (id,scope,project_id,workspace_id,kind,workload_class,payload,dedupe_key)
         VALUES ($1,'workspace',$2,$3,'recompute_metrics','projection','{}',$4)",
    )
    .bind(Uuid::new_v4())
    .bind(project_id)
    .bind(workspace_id)
    .bind(format!("scope-check:workspace-project:{}", Uuid::new_v4()))
    .execute(&pool)
    .await
    .expect_err("workspace rows carry no project");
    assert!(error.to_string().contains("jobs_scope_check"), "{error}");

    // Workspace work without its workspace: inconsistent.
    let error = sqlx::query(
        "INSERT INTO jobs (id,scope,kind,workload_class,payload,dedupe_key)
         VALUES ($1,'workspace','recompute_metrics','projection','{}',$2)",
    )
    .bind(Uuid::new_v4())
    .bind(format!(
        "scope-check:workspace-tenantless:{}",
        Uuid::new_v4()
    ))
    .execute(&pool)
    .await
    .expect_err("workspace rows carry their workspace");
    assert!(error.to_string().contains("jobs_scope_check"), "{error}");

    // Active project work without a project: the historical rule, folded in.
    let error = sqlx::query(
        "INSERT INTO jobs (id,kind,workload_class,payload,state,dedupe_key)
         VALUES ($1,'recompute_metrics','projection','{}','queued',$2)",
    )
    .bind(Uuid::new_v4())
    .bind(format!("scope-check:projectless-active:{}", Uuid::new_v4()))
    .execute(&pool)
    .await
    .expect_err("active project rows require their project");
    assert!(error.to_string().contains("jobs_scope_check"), "{error}");

    // Consistent rows of every scope insert cleanly.
    sqlx::query(
        "INSERT INTO jobs (id,scope,project_id,kind,workload_class,payload,dedupe_key)
         VALUES ($1,'project',$2,'recompute_metrics','projection','{}',$3)",
    )
    .bind(Uuid::new_v4())
    .bind(project_id)
    .bind(format!("scope-check:project:{}", Uuid::new_v4()))
    .execute(&pool)
    .await?;
    sqlx::query(
        "INSERT INTO jobs (id,scope,workspace_id,kind,workload_class,payload,dedupe_key)
         VALUES ($1,'workspace',$2,'recompute_metrics','projection','{}',$3)",
    )
    .bind(Uuid::new_v4())
    .bind(workspace_id)
    .bind(format!("scope-check:workspace:{}", Uuid::new_v4()))
    .execute(&pool)
    .await?;
    sqlx::query(
        "INSERT INTO jobs (id,scope,kind,workload_class,payload,dedupe_key)
         VALUES ($1,'platform','recompute_metrics','projection','{}',$2)",
    )
    .bind(Uuid::new_v4())
    .bind(format!("scope-check:platform:{}", Uuid::new_v4()))
    .execute(&pool)
    .await?;

    // Terminal history predating explicit scope keeps its provenance: a
    // completed row without a project stays legal as project history.
    sqlx::query(
        "INSERT INTO jobs (id,kind,workload_class,payload,state,dedupe_key,completed_at)
         VALUES ($1,'recompute_metrics','projection','{}','completed',$2,now())",
    )
    .bind(Uuid::new_v4())
    .bind(format!("scope-check:legacy-terminal:{}", Uuid::new_v4()))
    .execute(&pool)
    .await?;
    Ok(())
}

/// Workspace- and platform-scoped jobs enqueue, claim, retry and complete with
/// no project row at all.
#[tokio::test]
async fn workspace_and_platform_jobs_run_without_a_project() -> Result<()> {
    let Some(pool) = database().await else {
        return Ok(());
    };
    let store = PostgresDurableExecutionStore { pool: pool.clone() };
    let workspace_id = WorkspaceId::new(Uuid::new_v4());
    let workspace_job = scoped_job(
        Uuid::new_v4(),
        JobScope::Workspace(workspace_id),
        JobKind::RecomputeMetrics,
        serde_json::json!({"fixture": "workspace"}),
        format!("scope-workspace:{}", Uuid::new_v4()),
    );
    let platform_job = scoped_job(
        Uuid::new_v4(),
        JobScope::Platform,
        JobKind::RecomputeMetrics,
        serde_json::json!({"fixture": "platform"}),
        format!("scope-platform:{}", Uuid::new_v4()),
    );
    let workspace_job_id = store.enqueue(workspace_job).await?;
    let platform_job_id = store.enqueue(platform_job).await?;

    // Claiming filters on scope, never on `project_id IS NOT NULL`: both are
    // claimable, and their scopes round-trip untouched.
    let mut claimed = store
        .claim("scope-test", Duration::from_secs(30), &[], 10)
        .await?;
    claimed.sort_by_key(|job| job.id);
    assert_eq!(claimed.len(), 2, "both scoped jobs are claimable");
    let mut expected = [
        (workspace_job_id, JobScope::Workspace(workspace_id)),
        (platform_job_id, JobScope::Platform),
    ];
    expected.sort_by_key(|(id, _)| *id);
    for (job, (id, scope)) in claimed.iter().zip(&expected) {
        assert_eq!(job.id, *id);
        assert_eq!(job.scope, *scope);
        assert_eq!(job.project_id(), None, "no project is fabricated");
        assert!(job.require_project().is_err());
    }

    // The workspace job retries once with a persisted due time, then
    // completes; the platform job completes outright.
    let workspace_claim = claimed
        .iter()
        .find(|job| job.id == workspace_job_id)
        .unwrap()
        .clone();
    assert!(
        store
            .fail(
                &workspace_claim.lease("scope-test"),
                "retry fixture",
                JobRetry::After(Duration::ZERO),
            )
            .await?,
        "the workspace attempt is retried"
    );
    let retried = store
        .claim("scope-test", Duration::from_secs(30), &[], 10)
        .await?;
    let retried = retried
        .into_iter()
        .find(|job| job.id == workspace_job_id)
        .expect("the workspace job is claimable again when due");
    assert_eq!(retried.attempts, 2, "each accepted claim is one attempt");
    assert!(store.complete(&retried.lease("scope-test")).await?);

    let platform_claim = claimed
        .iter()
        .find(|job| job.id == platform_job_id)
        .unwrap()
        .clone();
    assert!(store.complete(&platform_claim.lease("scope-test")).await?);

    for (id, scope) in [
        (workspace_job_id, "workspace"),
        (platform_job_id, "platform"),
    ] {
        let row = sqlx::query(
            "SELECT state,attempts,project_id,workspace_id,scope FROM jobs WHERE id=$1",
        )
        .bind(id)
        .fetch_one(&pool)
        .await?;
        assert_eq!(row.get::<String, _>("state"), "completed");
        assert_eq!(row.get::<String, _>("scope"), scope);
        assert!(row.get::<Option<Uuid>, _>("project_id").is_none());
    }
    let workspace_row = sqlx::query("SELECT workspace_id FROM jobs WHERE id=$1")
        .bind(workspace_job_id)
        .fetch_one(&pool)
        .await?;
    assert_eq!(
        workspace_row.get::<Option<Uuid>, _>("workspace_id"),
        Some(workspace_id.as_uuid())
    );
    // No dummy project was ever created for this work.
    let projects: i64 = sqlx::query_scalar("SELECT count(*) FROM projects")
        .fetch_one(&pool)
        .await?;
    assert_eq!(projects, 0, "scoped jobs must not fabricate projects");
    Ok(())
}
