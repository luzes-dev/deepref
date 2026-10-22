//! Append-only audit rows for the project lifecycle (handoff 11 Part B).
//!
//! Creating and deleting a project are administrative actions, and the delete
//! removes content. Both are recorded in the same transaction as the change,
//! naming the workspace and the project; the actor is the principal (the
//! system when an operator command acts, as in an account completion).

use deepref_domain::{ProjectId, UserId, WorkspaceId};
use serde_json::json;
use sqlx::{Postgres, Transaction};

use crate::identity::{AuditPrincipal, AuditRequest, insert_admin_audit};

fn principal(actor: Option<UserId>) -> AuditPrincipal {
    actor.map_or(AuditPrincipal::System, AuditPrincipal::User)
}

/// Records that `actor` created `project` in `workspace`.
pub async fn record_project_created(
    tx: &mut Transaction<'_, Postgres>,
    actor: Option<UserId>,
    workspace: WorkspaceId,
    project: ProjectId,
    audit: AuditRequest<'_>,
) -> Result<(), sqlx::Error> {
    insert_admin_audit(
        tx,
        "project.created",
        principal(actor),
        None,
        Some(workspace.as_uuid()),
        Some(project.as_uuid()),
        json!({}),
        audit,
    )
    .await
}

/// Records that `actor` deleted `project` and its content from `workspace`.
pub async fn record_project_deleted(
    tx: &mut Transaction<'_, Postgres>,
    actor: Option<UserId>,
    workspace: WorkspaceId,
    project: ProjectId,
    audit: AuditRequest<'_>,
) -> Result<(), sqlx::Error> {
    insert_admin_audit(
        tx,
        "project.deleted",
        principal(actor),
        None,
        Some(workspace.as_uuid()),
        Some(project.as_uuid()),
        json!({}),
        audit,
    )
    .await
}
