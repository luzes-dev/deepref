//! Primary identities and canonical count fallback around a disposable cache.
//! Callers still authorize through the central PDP before reading any counts.

use std::collections::HashMap;

use deepref_application::project_count_cache::{
    ArticleCountSnapshot, COUNT_CACHE_MAX_BATCH, ProjectCountCache, ProjectCountKey,
};
use deepref_domain::{ProjectId, WorkspaceId};
use sqlx::{PgPool, Row};
use uuid::Uuid;

// Optional cache work has its own small budget; a slow adapter must leave time
// for the authoritative query rather than consuming its entire outer deadline.
const CACHE_OPERATION_BUDGET: std::time::Duration = std::time::Duration::from_millis(100);

pub async fn article_counts(
    pool: &PgPool,
    requested: &[(WorkspaceId, ProjectId)],
    cache: Option<&dyn ProjectCountCache>,
    primary_only: bool,
) -> Result<HashMap<ProjectId, ArticleCountSnapshot>, sqlx::Error> {
    tokio::time::timeout(
        std::time::Duration::from_secs(2),
        article_counts_once(pool, requested, cache, primary_only),
    )
    .await
    .map_err(|_| sqlx::Error::Protocol("project_count_deadline_exceeded".into()))?
}

/// Resolve live tenant/project identities on primary before cache access.
/// Missing, deleted or moved projects never become accessible through cache.
async fn article_counts_once(
    pool: &PgPool,
    requested: &[(WorkspaceId, ProjectId)],
    cache: Option<&dyn ProjectCountCache>,
    primary_only: bool,
) -> Result<HashMap<ProjectId, ArticleCountSnapshot>, sqlx::Error> {
    if requested.len() > COUNT_CACHE_MAX_BATCH {
        return Err(sqlx::Error::Protocol(
            "project count batch exceeds limit".into(),
        ));
    }
    if requested.is_empty() {
        return Ok(HashMap::new());
    }
    let mut tx = pool.begin().await?;
    sqlx::raw_sql("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ READ ONLY; SET LOCAL statement_timeout='1500ms'; SET LOCAL lock_timeout='250ms'; SET LOCAL idle_in_transaction_session_timeout='3s'")
        .execute(&mut *tx).await?;
    let ids = requested
        .iter()
        .map(|(_, project)| project.as_uuid())
        .collect::<Vec<_>>();
    let rows = sqlx::query(
        "SELECT id,workspace_id,metric_requested_revision FROM projects WHERE id=ANY($1)",
    )
    .bind(&ids)
    .fetch_all(&mut *tx)
    .await?;
    let mut keys = Vec::new();
    for row in rows {
        let project = ProjectId::new(row.try_get("id")?);
        let workspace = WorkspaceId::new(row.try_get("workspace_id")?);
        if requested.contains(&(workspace, project)) {
            keys.push(ProjectCountKey {
                workspace,
                project,
                revision: row.try_get("metric_requested_revision")?,
            });
        }
    }
    let cached = if !primary_only {
        match cache {
            Some(cache) => tokio::time::timeout(CACHE_OPERATION_BUDGET, cache.get(&keys))
                .await
                .ok()
                .and_then(Result::ok)
                .filter(|values| values.len() == keys.len()),
            None => None,
        }
    } else {
        None
    };
    let mut counts = HashMap::new();
    let mut missing = Vec::new();
    for (index, key) in keys.iter().enumerate() {
        if let Some(Some(snapshot)) = cached.as_ref().and_then(|values| values.get(index))
            && snapshot.matches_at(*key, chrono::Utc::now())
        {
            counts.insert(key.project, snapshot.clone());
        } else {
            missing.push(key.project.as_uuid());
        }
    }
    if missing.is_empty() {
        tx.rollback().await?;
        return Ok(counts);
    }
    // Count and observed revision share one SQL snapshot. A write racing the
    // earlier identity read can never populate the old key with a new count.
    let rows = sqlx::query(
        "SELECT p.id,p.workspace_id,p.metric_requested_revision,transaction_timestamp() AS observed_at,
           (SELECT count(*) FROM project_reports pr WHERE pr.project_id=p.id)::bigint AS count
         FROM projects p WHERE p.id=ANY($1)"
    ).bind(&missing).fetch_all(&mut *tx).await?;
    let mut snapshots = Vec::new();
    for row in rows {
        let project = ProjectId::new(row.try_get::<Uuid, _>("id")?);
        let workspace = WorkspaceId::new(row.try_get("workspace_id")?);
        if !requested.contains(&(workspace, project)) {
            continue;
        }
        let snapshot = ArticleCountSnapshot {
            key: ProjectCountKey {
                workspace,
                project,
                revision: row.try_get("metric_requested_revision")?,
            },
            count: row.try_get("count")?,
            observed_at: row.try_get("observed_at")?,
        };
        counts.insert(project, snapshot.clone());
        snapshots.push(snapshot);
    }
    tx.rollback().await?;
    if !primary_only && let Some(cache) = cache {
        let _ = tokio::time::timeout(CACHE_OPERATION_BUDGET, cache.put(&snapshots)).await;
    }
    Ok(counts)
}
