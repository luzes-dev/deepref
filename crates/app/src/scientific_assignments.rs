use chrono::{DateTime, Utc};
use deepref_domain::{ProjectId, ScientificAssignmentRole, UserId};
use sqlx::{PgPool, Row};
use thiserror::Error;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScientificAssignment {
    pub project_id: ProjectId,
    pub user_id: UserId,
    pub role: ScientificAssignmentRole,
    pub assigned_by: UserId,
    pub created_at: DateTime<Utc>,
    pub revoked_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Error)]
pub enum ScientificAssignmentError {
    #[error(transparent)]
    Database(#[from] sqlx::Error),
    #[error("scientific assignment not found")]
    NotFound,
    #[error("database contains unknown scientific assignment role {0:?}")]
    UnknownStoredRole(String),
}

pub async fn list_scientific_assignments(
    pool: &PgPool,
    project_id: ProjectId,
) -> Result<Vec<ScientificAssignment>, ScientificAssignmentError> {
    let rows = sqlx::query(
        "SELECT project_id,user_id,role,assigned_by,created_at,revoked_at \
         FROM scientific_assignments WHERE project_id=$1 AND revoked_at IS NULL \
         ORDER BY role,user_id,created_at",
    )
    .bind(project_id.as_uuid())
    .fetch_all(pool)
    .await?;
    rows.into_iter().map(assignment_from_row).collect()
}

/// Assign a user to a methodology role. Repeating an active assignment is
/// idempotent; a revoked assignment creates a new historical row.
pub async fn assign_scientific_reviewer(
    pool: &PgPool,
    project_id: ProjectId,
    user_id: UserId,
    role: ScientificAssignmentRole,
    assigned_by: UserId,
) -> Result<ScientificAssignment, ScientificAssignmentError> {
    let inserted = sqlx::query(
        "INSERT INTO scientific_assignments(project_id,user_id,role,assigned_by) \
         SELECT $1,$2,$3,$4 \
         WHERE EXISTS (SELECT 1 FROM users WHERE id=$2 AND status='active') \
           AND EXISTS (SELECT 1 FROM users WHERE id=$4 AND status='active') \
         ON CONFLICT (project_id,user_id,role) WHERE revoked_at IS NULL DO NOTHING \
         RETURNING project_id,user_id,role,assigned_by,created_at,revoked_at",
    )
    .bind(project_id.as_uuid())
    .bind(user_id.as_uuid())
    .bind(role.as_str())
    .bind(assigned_by.as_uuid())
    .fetch_optional(pool)
    .await?;

    if let Some(row) = inserted {
        return assignment_from_row(row);
    }
    sqlx::query(
        "SELECT project_id,user_id,role,assigned_by,created_at,revoked_at \
         FROM scientific_assignments WHERE project_id=$1 AND user_id=$2 \
           AND role=$3 AND revoked_at IS NULL",
    )
    .bind(project_id.as_uuid())
    .bind(user_id.as_uuid())
    .bind(role.as_str())
    .fetch_optional(pool)
    .await?
    .map(assignment_from_row)
    .transpose()?
    .ok_or(ScientificAssignmentError::NotFound)
}

pub async fn revoke_scientific_assignment(
    pool: &PgPool,
    project_id: ProjectId,
    user_id: UserId,
    role: ScientificAssignmentRole,
) -> Result<ScientificAssignment, ScientificAssignmentError> {
    let row = sqlx::query(
        "UPDATE scientific_assignments SET revoked_at=now() \
         WHERE project_id=$1 AND user_id=$2 AND role=$3 AND revoked_at IS NULL \
         RETURNING project_id,user_id,role,assigned_by,created_at,revoked_at",
    )
    .bind(project_id.as_uuid())
    .bind(user_id.as_uuid())
    .bind(role.as_str())
    .fetch_optional(pool)
    .await?
    .ok_or(ScientificAssignmentError::NotFound)?;
    assignment_from_row(row)
}

fn assignment_from_row(
    row: sqlx::postgres::PgRow,
) -> Result<ScientificAssignment, ScientificAssignmentError> {
    let role_text: String = row.try_get("role")?;
    let role = ScientificAssignmentRole::parse(&role_text)
        .ok_or_else(|| ScientificAssignmentError::UnknownStoredRole(role_text))?;
    Ok(ScientificAssignment {
        project_id: ProjectId::new(row.try_get("project_id")?),
        user_id: UserId::new(row.try_get("user_id")?),
        role,
        assigned_by: UserId::new(row.try_get("assigned_by")?),
        created_at: row.try_get("created_at")?,
        revoked_at: row.try_get("revoked_at")?,
    })
}
