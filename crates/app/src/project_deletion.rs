use sqlx::{Postgres, Transaction};
use uuid::Uuid;

/// Rows that protect data during normal operation. They reference document
/// blocks, eligibility criteria, field definitions or exclusion reasons with
/// RESTRICT or NO ACTION, so the cascade from `projects` cannot pass those
/// constraints when the parent rows are removed in the same statement. A
/// project delete removes them first, children before parents. Every other
/// project-owned row cascades (migration 0041).
const PROTECTED_CHILDREN_FIRST: [&str; 7] = [
    "DELETE FROM extraction_values WHERE project_id = $1",
    "DELETE FROM ai_proposal_evidence WHERE project_id = $1",
    "DELETE FROM ai_run_evidence WHERE project_id = $1",
    "DELETE FROM ai_proposal_criterion_judgments WHERE project_id = $1",
    "DELETE FROM screening_state WHERE project_id = $1",
    "DELETE FROM screening_events WHERE project_id = $1",
    // projection_state has no foreign key to projects.
    "DELETE FROM projection_state WHERE project_id = $1",
];

/// Deletes a project and every row that belongs to it, inside the caller's
/// transaction. Returns `false` when the project does not exist.
pub async fn delete_project_in_transaction(
    tx: &mut Transaction<'_, Postgres>,
    project_id: Uuid,
) -> Result<bool, sqlx::Error> {
    let workspace_id: Option<Uuid> =
        sqlx::query_scalar("SELECT workspace_id FROM projects WHERE id = $1 FOR UPDATE")
            .bind(project_id)
            .fetch_optional(&mut **tx)
            .await?;
    let Some(workspace_id) = workspace_id else {
        return Ok(false);
    };
    let has_open_liability: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM ai_usage_reservations
         WHERE project_id = $1 AND state IN ('open', 'observed', 'unresolved'))",
    )
    .bind(project_id)
    .fetch_one(&mut **tx)
    .await?;
    if has_open_liability {
        sqlx::query(
            "INSERT INTO ai_usage_detached_projects (project_id, workspace_id)
             VALUES ($1, $2) ON CONFLICT (project_id) DO NOTHING",
        )
        .bind(project_id)
        .bind(workspace_id)
        .execute(&mut **tx)
        .await?;
    } else {
        sqlx::query(
            "DELETE FROM ai_usage_period_buckets
             WHERE project_id = $1 AND scope_kind IN ('project', 'run')",
        )
        .bind(project_id)
        .execute(&mut **tx)
        .await?;
        sqlx::query("DELETE FROM ai_usage_limits WHERE scope_kind = 'project' AND scope_id = $1")
            .bind(project_id)
            .execute(&mut **tx)
            .await?;
    }
    for statement in PROTECTED_CHILDREN_FIRST {
        sqlx::query(statement)
            .bind(project_id)
            .execute(&mut **tx)
            .await?;
    }
    let result = sqlx::query("DELETE FROM projects WHERE id = $1")
        .bind(project_id)
        .execute(&mut **tx)
        .await?;
    Ok(result.rows_affected() > 0)
}
