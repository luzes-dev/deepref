//! Workspaces, memberships, project access and WorkOS reconciliation.
//!
//! DeepRef PostgreSQL is authoritative for runtime access decisions: nothing
//! here is cached, so a membership removal or role downgrade takes effect on
//! the next request. Every administrative change writes its
//! [`crate::identity::AuditRequest`]-scoped `admin_audit_events` row in the
//! same transaction as the change itself.
//!
//! WorkOS is authoritative for organization provisioning only.
//! [`apply_organization_snapshot`] moves its state into local projections:
//! absence at the provider deactivates `workos_mirrored` grants and nothing
//! else, `local` grants are preserved, and every disagreement is returned as
//! a [`ReconciliationFinding`] instead of being resolved silently.

use std::collections::{HashMap, HashSet};

use deepref_application::workspaces::{
    DirectoryError, DirectoryMembership, DirectoryMembershipStatus, DirectoryOrganization,
    OrganizationDirectory, ReconciliationFinding, map_directory_role, normalize_invitation_email,
    personal_workspace_name, workspace_slug,
};
use deepref_domain::{
    ExternalOrganizationId, ExternalUserId, GrantSource, MembershipStatus, ProjectId, ProjectRole,
    UserId, WorkspaceId, WorkspaceKind, WorkspaceRole,
};
use serde_json::{Value, json};
use sqlx::{PgPool, Postgres, Row, Transaction, postgres::PgRow};
use thiserror::Error;
use uuid::Uuid;

use crate::identity::{AuditPrincipal, AuditRequest, insert_admin_audit};

/// The deterministic migration workspace that holds every project created
/// before workspaces (migration 0087, stage 2). Fixed across reruns and
/// releases so the staged backfill, the trigger that serves old binaries and
/// every runbook mean the same row.
pub const MIGRATION_WORKSPACE_ID: WorkspaceId =
    WorkspaceId::new(Uuid::from_u128(0xf45d_8021_6271_556e_a59a_fd2e_0a73_9f95));

#[derive(Debug, Error)]
pub enum WorkspaceError {
    #[error("workspace storage failed")]
    Database(#[from] sqlx::Error),
    #[error("stored workspace row is invalid: {0}")]
    Corrupt(&'static str),
    #[error("no such workspace")]
    UnknownWorkspace,
    #[error("no such membership")]
    UnknownMembership,
    #[error("no such project")]
    UnknownProject,
    #[error("the user is already a member of this workspace")]
    AlreadyMember,
    #[error("the user already has access to this project")]
    AccessAlreadyGranted,
    #[error("the workspace name must be between 1 and 200 characters")]
    InvalidName,
    #[error("this change removes the last owner of the workspace; transfer ownership before it")]
    LastOwner,
    #[error("the migration workspace cannot be deleted")]
    MigrationWorkspaceProtected,
    #[error("a workspace that still owns projects cannot be deleted")]
    WorkspaceHasProjects,
    #[error("only an active owner or admin of the workspace may do this")]
    NotPermitted,
    #[error("an invitation cannot grant a role above the inviter's own")]
    RoleAboveInviter,
    #[error("the invitation address or expiry is invalid")]
    InvalidInvitation,
    #[error("a pending invitation for this address already exists")]
    InvitationAlreadyPending,
    #[error("no such invitation")]
    UnknownInvitation,
    #[error("the invitation is no longer pending")]
    InvitationUnavailable,
    #[error("the invitation has expired")]
    InvitationExpired,
    #[error("the invitation was issued to a different address")]
    InvitationEmailMismatch,
    #[error(transparent)]
    Directory(#[from] DirectoryError),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceRecord {
    pub id: WorkspaceId,
    pub kind: WorkspaceKind,
    pub name: String,
    pub slug: String,
    pub workos_organization_id: Option<ExternalOrganizationId>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MembershipRecord {
    pub id: Uuid,
    pub workspace_id: WorkspaceId,
    pub user_id: UserId,
    pub role: WorkspaceRole,
    pub status: MembershipStatus,
    pub grant_source: GrantSource,
    pub workos_membership_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectAccessRecord {
    pub id: Uuid,
    pub project_id: ProjectId,
    pub user_id: UserId,
    pub role: ProjectRole,
    pub grant_source: GrantSource,
}

/// One WorkOS Organization as reconciliation sees it: the organization and
/// the memberships the provider lists for it right now.
#[derive(Debug, Clone)]
pub struct OrganizationSnapshot {
    pub organization: DirectoryOrganization,
    pub memberships: Vec<DirectoryMembership>,
}

/// What one reconciliation of one organization did. Counts are audit rows
/// written; `findings` is everything the run refused to resolve silently.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReconciliationReport {
    pub workspace_id: WorkspaceId,
    pub created: usize,
    pub role_changes: usize,
    pub status_changes: usize,
    pub findings: Vec<ReconciliationFinding>,
}

/// The closed administrative events of handoff 03a. Each one fixes its
/// metadata shape; the database refuses every other shape.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TenancyEvent {
    WorkspaceCreated(WorkspaceKind),
    WorkspaceRenamed,
    WorkspaceDeleted,
    MembershipCreated(WorkspaceRole, GrantSource),
    MembershipRoleChanged(WorkspaceRole),
    MembershipStatusChanged(MembershipStatus),
    MembershipRemoved(WorkspaceRole, GrantSource),
    ProjectAccessGranted(ProjectRole, GrantSource),
    ProjectAccessRoleChanged(ProjectRole),
    ProjectAccessRemoved(ProjectRole, GrantSource),
    InvitationCreated(WorkspaceRole),
    InvitationAccepted(WorkspaceRole),
    InvitationRevoked(WorkspaceRole),
    InvitationExpired(WorkspaceRole),
}

impl TenancyEvent {
    const fn as_str(self) -> &'static str {
        match self {
            Self::WorkspaceCreated(_) => "workspace.created",
            Self::WorkspaceRenamed => "workspace.renamed",
            Self::WorkspaceDeleted => "workspace.deleted",
            Self::MembershipCreated(_, _) => "membership.created",
            Self::MembershipRoleChanged(_) => "membership.role_changed",
            Self::MembershipStatusChanged(_) => "membership.status_changed",
            Self::MembershipRemoved(_, _) => "membership.removed",
            Self::ProjectAccessGranted(_, _) => "project_access.granted",
            Self::ProjectAccessRoleChanged(_) => "project_access.role_changed",
            Self::ProjectAccessRemoved(_, _) => "project_access.removed",
            Self::InvitationCreated(_) => "invitation.created",
            Self::InvitationAccepted(_) => "invitation.accepted",
            Self::InvitationRevoked(_) => "invitation.revoked",
            Self::InvitationExpired(_) => "invitation.expired",
        }
    }

    fn metadata(self) -> Value {
        match self {
            Self::WorkspaceCreated(kind) => json!({ "workspace_kind": kind.as_str() }),
            Self::MembershipCreated(role, source) | Self::MembershipRemoved(role, source) => {
                json!({ "role": role.as_str(), "grant_source": source.as_str() })
            }
            Self::MembershipRoleChanged(role) => json!({ "role": role.as_str() }),
            Self::MembershipStatusChanged(status) => json!({ "status": status.as_str() }),
            Self::ProjectAccessGranted(role, source) | Self::ProjectAccessRemoved(role, source) => {
                json!({ "role": role.as_str(), "grant_source": source.as_str() })
            }
            Self::ProjectAccessRoleChanged(role) => json!({ "role": role.as_str() }),
            Self::InvitationCreated(role)
            | Self::InvitationAccepted(role)
            | Self::InvitationRevoked(role)
            | Self::InvitationExpired(role) => json!({ "role": role.as_str() }),
            Self::WorkspaceRenamed | Self::WorkspaceDeleted => json!({}),
        }
    }
}

async fn write_tenancy_audit(
    tx: &mut Transaction<'_, Postgres>,
    event: TenancyEvent,
    principal: AuditPrincipal,
    target: Option<UserId>,
    workspace_id: Option<WorkspaceId>,
    project_id: Option<ProjectId>,
    audit: AuditRequest<'_>,
) -> Result<(), WorkspaceError> {
    insert_admin_audit(
        tx,
        event.as_str(),
        principal,
        target,
        workspace_id.map(WorkspaceId::as_uuid),
        project_id.map(ProjectId::as_uuid),
        event.metadata(),
        audit,
    )
    .await?;
    Ok(())
}

fn membership_record(row: &PgRow) -> Result<MembershipRecord, WorkspaceError> {
    let role: String = row.try_get("role")?;
    let status: String = row.try_get("status")?;
    let grant_source: String = row.try_get("grant_source")?;
    Ok(MembershipRecord {
        id: row.try_get("id")?,
        workspace_id: WorkspaceId::new(row.try_get("workspace_id")?),
        user_id: UserId::new(row.try_get("user_id")?),
        role: WorkspaceRole::parse(&role).ok_or(WorkspaceError::Corrupt("role"))?,
        status: MembershipStatus::parse(&status).ok_or(WorkspaceError::Corrupt("status"))?,
        grant_source: GrantSource::parse(&grant_source)
            .ok_or(WorkspaceError::Corrupt("grant_source"))?,
        workos_membership_id: row.try_get("workos_membership_id")?,
    })
}

fn workspace_record(row: &PgRow) -> Result<WorkspaceRecord, WorkspaceError> {
    let kind: String = row.try_get("kind")?;
    let organization: Option<String> = row.try_get("workos_organization_id")?;
    Ok(WorkspaceRecord {
        id: WorkspaceId::new(row.try_get("id")?),
        kind: WorkspaceKind::parse(&kind).ok_or(WorkspaceError::Corrupt("kind"))?,
        name: row.try_get("name")?,
        slug: row.try_get("slug")?,
        workos_organization_id: organization
            .map(ExternalOrganizationId::parse)
            .transpose()
            .map_err(|_| WorkspaceError::Corrupt("workos_organization_id"))?,
    })
}

fn project_access_record(row: &PgRow) -> Result<ProjectAccessRecord, WorkspaceError> {
    let role: String = row.try_get("role")?;
    let grant_source: String = row.try_get("grant_source")?;
    Ok(ProjectAccessRecord {
        id: row.try_get("id")?,
        project_id: ProjectId::new(row.try_get("project_id")?),
        user_id: UserId::new(row.try_get("user_id")?),
        role: ProjectRole::parse(&role).ok_or(WorkspaceError::Corrupt("role"))?,
        grant_source: GrantSource::parse(&grant_source)
            .ok_or(WorkspaceError::Corrupt("grant_source"))?,
    })
}

/// The workspace row insert every creation path shares. The name is bounded
/// here because a provider name is caller-adjacent text; the slug is derived
/// from the workspace ID and never moves.
async fn insert_workspace(
    tx: &mut Transaction<'_, Postgres>,
    id: WorkspaceId,
    kind: WorkspaceKind,
    name: &str,
    workos_organization_id: Option<&ExternalOrganizationId>,
) -> Result<WorkspaceRecord, WorkspaceError> {
    let name = bounded_name(name);
    let slug = workspace_slug(&name, id.as_uuid());
    sqlx::query(
        "INSERT INTO workspaces (id, kind, name, slug, workos_organization_id)
         VALUES ($1, $2, $3, $4, $5)",
    )
    .bind(id.as_uuid())
    .bind(kind.as_str())
    .bind(&name)
    .bind(&slug)
    .bind(workos_organization_id.map(ExternalOrganizationId::as_str))
    .execute(&mut **tx)
    .await?;
    Ok(WorkspaceRecord {
        id,
        kind,
        name,
        slug,
        workos_organization_id: workos_organization_id.cloned(),
    })
}

/// Provider names are unbounded text; a workspace name is not.
fn bounded_name(raw: &str) -> String {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return "Workspace".to_owned();
    }
    trimmed.chars().take(200).collect()
}

/// Onboarding (03a): the first sign-in of a new user creates one personal
/// workspace with the user as its owner, in the same transaction that
/// provisions the user. A retried callback provisions no second user and so
/// creates no second workspace. No request depends on a WorkOS Organization
/// here: mapping happens when reconciliation sees one.
pub async fn create_personal_workspace(
    tx: &mut Transaction<'_, Postgres>,
    user_id: UserId,
    display_name: Option<&str>,
    email: &str,
    audit: AuditRequest<'_>,
) -> Result<WorkspaceRecord, WorkspaceError> {
    let id = WorkspaceId::new(Uuid::new_v4());
    let name = personal_workspace_name(display_name, email);
    let workspace = insert_workspace(tx, id, WorkspaceKind::Personal, &name, None).await?;
    insert_membership_row(
        tx,
        workspace.id,
        user_id,
        WorkspaceRole::Owner,
        MembershipStatus::Active,
        GrantSource::Local,
        None,
    )
    .await?;
    write_tenancy_audit(
        &mut *tx,
        TenancyEvent::WorkspaceCreated(WorkspaceKind::Personal),
        AuditPrincipal::User(user_id),
        None,
        Some(workspace.id),
        None,
        audit,
    )
    .await?;
    write_tenancy_audit(
        &mut *tx,
        TenancyEvent::MembershipCreated(WorkspaceRole::Owner, GrantSource::Local),
        AuditPrincipal::User(user_id),
        Some(user_id),
        Some(workspace.id),
        None,
        audit,
    )
    .await?;
    Ok(workspace)
}

#[allow(clippy::too_many_arguments)]
async fn insert_membership_row(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: WorkspaceId,
    user_id: UserId,
    role: WorkspaceRole,
    status: MembershipStatus,
    grant_source: GrantSource,
    workos_membership_id: Option<&str>,
) -> Result<MembershipRecord, WorkspaceError> {
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO workspace_memberships
           (id, workspace_id, user_id, workos_membership_id, role, status, grant_source)
         VALUES ($1, $2, $3, $4, $5, $6, $7)",
    )
    .bind(id)
    .bind(workspace_id.as_uuid())
    .bind(user_id.as_uuid())
    .bind(workos_membership_id)
    .bind(role.as_str())
    .bind(status.as_str())
    .bind(grant_source.as_str())
    .execute(&mut **tx)
    .await?;
    Ok(MembershipRecord {
        id,
        workspace_id,
        user_id,
        role,
        status,
        grant_source,
        workos_membership_id: workos_membership_id.map(str::to_owned),
    })
}

async fn load_membership_for_update(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: WorkspaceId,
    user_id: UserId,
) -> Result<MembershipRecord, WorkspaceError> {
    let row = sqlx::query(
        "SELECT id, workspace_id, user_id, role, status, grant_source, workos_membership_id
         FROM workspace_memberships
         WHERE workspace_id = $1 AND user_id = $2 FOR UPDATE",
    )
    .bind(workspace_id.as_uuid())
    .bind(user_id.as_uuid())
    .fetch_optional(&mut **tx)
    .await?;
    row.as_ref()
        .map(membership_record)
        .transpose()?
        .ok_or(WorkspaceError::UnknownMembership)
}

/// Serializes membership changes inside one workspace, so two concurrent
/// changes cannot both decide they are not the last owner.
async fn lock_workspace(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: WorkspaceId,
) -> Result<(), WorkspaceError> {
    let found: Option<Uuid> =
        sqlx::query_scalar("SELECT id FROM workspaces WHERE id = $1 FOR UPDATE")
            .bind(workspace_id.as_uuid())
            .fetch_optional(&mut **tx)
            .await?;
    found.map(|_| ()).ok_or(WorkspaceError::UnknownWorkspace)
}

/// The last-owner guard (03a). It applies to voluntary membership changes
/// only: removing, deactivating or demoting the last active `owner` is refused
/// until ownership is transferred. Global user deactivation (02) never passes
/// through here and proceeds regardless.
async fn refuse_last_owner_loss(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: WorkspaceId,
    membership: &MembershipRecord,
) -> Result<(), WorkspaceError> {
    if membership.role != WorkspaceRole::Owner || membership.status != MembershipStatus::Active {
        return Ok(());
    }
    let other_owners: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM workspace_memberships
         WHERE workspace_id = $1 AND user_id <> $2 AND role = 'owner' AND status = 'active'",
    )
    .bind(workspace_id.as_uuid())
    .bind(membership.user_id.as_uuid())
    .fetch_one(&mut **tx)
    .await?;
    if other_owners == 0 {
        return Err(WorkspaceError::LastOwner);
    }
    Ok(())
}

/// Adds a DeepRef-only (`local`) membership. Provider-mirrored memberships are
/// created by reconciliation, never by hand.
pub async fn add_workspace_member(
    pool: &PgPool,
    actor: UserId,
    workspace_id: WorkspaceId,
    user_id: UserId,
    role: WorkspaceRole,
    audit: AuditRequest<'_>,
) -> Result<MembershipRecord, WorkspaceError> {
    let mut tx = pool.begin().await?;
    lock_workspace(&mut tx, workspace_id).await?;
    let membership = insert_membership_row(
        &mut tx,
        workspace_id,
        user_id,
        role,
        MembershipStatus::Active,
        GrantSource::Local,
        None,
    )
    .await?;
    write_tenancy_audit(
        &mut tx,
        TenancyEvent::MembershipCreated(role, GrantSource::Local),
        AuditPrincipal::User(actor),
        Some(user_id),
        Some(workspace_id),
        None,
        audit,
    )
    .await?;
    tx.commit().await?;
    Ok(membership)
}

/// Changes a membership's role. A no-op change records nothing.
pub async fn change_membership_role(
    pool: &PgPool,
    actor: UserId,
    workspace_id: WorkspaceId,
    user_id: UserId,
    role: WorkspaceRole,
    audit: AuditRequest<'_>,
) -> Result<MembershipRecord, WorkspaceError> {
    let mut tx = pool.begin().await?;
    lock_workspace(&mut tx, workspace_id).await?;
    let membership = load_membership_for_update(&mut tx, workspace_id, user_id).await?;
    if membership.role == role {
        tx.commit().await?;
        return Ok(membership);
    }
    let demoting_owner = membership.role == WorkspaceRole::Owner && role != WorkspaceRole::Owner;
    if demoting_owner {
        refuse_last_owner_loss(&mut tx, workspace_id, &membership).await?;
    }
    sqlx::query(
        "UPDATE workspace_memberships SET role = $3, updated_at = now()
         WHERE workspace_id = $1 AND user_id = $2",
    )
    .bind(workspace_id.as_uuid())
    .bind(user_id.as_uuid())
    .bind(role.as_str())
    .execute(&mut *tx)
    .await?;
    write_tenancy_audit(
        &mut tx,
        TenancyEvent::MembershipRoleChanged(role),
        AuditPrincipal::User(actor),
        Some(user_id),
        Some(workspace_id),
        None,
        audit,
    )
    .await?;
    tx.commit().await?;
    Ok(MembershipRecord { role, ..membership })
}

/// Activates or deactivates a membership. Deactivation keeps the row and its
/// history; it only withdraws access.
pub async fn set_membership_status(
    pool: &PgPool,
    actor: UserId,
    workspace_id: WorkspaceId,
    user_id: UserId,
    status: MembershipStatus,
    audit: AuditRequest<'_>,
) -> Result<MembershipRecord, WorkspaceError> {
    let mut tx = pool.begin().await?;
    lock_workspace(&mut tx, workspace_id).await?;
    let membership = load_membership_for_update(&mut tx, workspace_id, user_id).await?;
    if membership.status == status {
        tx.commit().await?;
        return Ok(membership);
    }
    if status == MembershipStatus::Deactivated {
        refuse_last_owner_loss(&mut tx, workspace_id, &membership).await?;
    }
    sqlx::query(
        "UPDATE workspace_memberships SET status = $3, updated_at = now()
         WHERE workspace_id = $1 AND user_id = $2",
    )
    .bind(workspace_id.as_uuid())
    .bind(user_id.as_uuid())
    .bind(status.as_str())
    .execute(&mut *tx)
    .await?;
    write_tenancy_audit(
        &mut tx,
        TenancyEvent::MembershipStatusChanged(status),
        AuditPrincipal::User(actor),
        Some(user_id),
        Some(workspace_id),
        None,
        audit,
    )
    .await?;
    tx.commit().await?;
    Ok(MembershipRecord {
        status,
        ..membership
    })
}

/// Removes a membership: the user loses this workspace and keeps their account
/// and every other workspace. The row is deleted; its history lives in the
/// audit trail.
pub async fn remove_membership(
    pool: &PgPool,
    actor: UserId,
    workspace_id: WorkspaceId,
    user_id: UserId,
    audit: AuditRequest<'_>,
) -> Result<MembershipRecord, WorkspaceError> {
    let mut tx = pool.begin().await?;
    lock_workspace(&mut tx, workspace_id).await?;
    let membership = load_membership_for_update(&mut tx, workspace_id, user_id).await?;
    refuse_last_owner_loss(&mut tx, workspace_id, &membership).await?;
    sqlx::query("DELETE FROM workspace_memberships WHERE workspace_id = $1 AND user_id = $2")
        .bind(workspace_id.as_uuid())
        .bind(user_id.as_uuid())
        .execute(&mut *tx)
        .await?;
    write_tenancy_audit(
        &mut tx,
        TenancyEvent::MembershipRemoved(membership.role, membership.grant_source),
        AuditPrincipal::User(actor),
        Some(user_id),
        Some(workspace_id),
        None,
        audit,
    )
    .await?;
    tx.commit().await?;
    Ok(membership)
}

async fn load_project_access(
    tx: &mut Transaction<'_, Postgres>,
    project_id: ProjectId,
    user_id: UserId,
) -> Result<ProjectAccessRecord, WorkspaceError> {
    let row = sqlx::query(
        "SELECT id, project_id, user_id, role, grant_source
         FROM project_access WHERE project_id = $1 AND user_id = $2 FOR UPDATE",
    )
    .bind(project_id.as_uuid())
    .bind(user_id.as_uuid())
    .fetch_optional(&mut **tx)
    .await?;
    row.as_ref()
        .map(project_access_record)
        .transpose()?
        .ok_or(WorkspaceError::UnknownMembership)
}

async fn require_project(
    tx: &mut Transaction<'_, Postgres>,
    project_id: ProjectId,
) -> Result<(), WorkspaceError> {
    let found: Option<Uuid> = sqlx::query_scalar("SELECT id FROM projects WHERE id = $1")
        .bind(project_id.as_uuid())
        .fetch_optional(&mut **tx)
        .await?;
    found.map(|_| ()).ok_or(WorkspaceError::UnknownProject)
}

/// Grants project access narrower than workspace membership. All such grants
/// are `local`: WorkOS knows organizations, not DeepRef projects.
pub async fn grant_project_access(
    pool: &PgPool,
    actor: UserId,
    project_id: ProjectId,
    user_id: UserId,
    role: ProjectRole,
    audit: AuditRequest<'_>,
) -> Result<ProjectAccessRecord, WorkspaceError> {
    let mut tx = pool.begin().await?;
    require_project(&mut tx, project_id).await?;
    let id = Uuid::new_v4();
    let inserted = sqlx::query(
        "INSERT INTO project_access (id, project_id, user_id, role, grant_source)
         VALUES ($1, $2, $3, $4, 'local')
         ON CONFLICT (project_id, user_id) DO NOTHING",
    )
    .bind(id)
    .bind(project_id.as_uuid())
    .bind(user_id.as_uuid())
    .bind(role.as_str())
    .execute(&mut *tx)
    .await?
    .rows_affected()
        == 1;
    if !inserted {
        return Err(WorkspaceError::AccessAlreadyGranted);
    }
    let workspace_id = workspace_of_project(&mut tx, project_id).await?;
    write_tenancy_audit(
        &mut tx,
        TenancyEvent::ProjectAccessGranted(role, GrantSource::Local),
        AuditPrincipal::User(actor),
        Some(user_id),
        Some(workspace_id),
        Some(project_id),
        audit,
    )
    .await?;
    tx.commit().await?;
    Ok(ProjectAccessRecord {
        id,
        project_id,
        user_id,
        role,
        grant_source: GrantSource::Local,
    })
}

async fn workspace_of_project(
    tx: &mut Transaction<'_, Postgres>,
    project_id: ProjectId,
) -> Result<WorkspaceId, WorkspaceError> {
    let workspace: Option<Uuid> =
        sqlx::query_scalar("SELECT workspace_id FROM projects WHERE id = $1")
            .bind(project_id.as_uuid())
            .fetch_optional(&mut **tx)
            .await?;
    workspace
        .map(WorkspaceId::new)
        .ok_or(WorkspaceError::UnknownProject)
}

/// Changes one project-access role. A no-op change records nothing.
pub async fn change_project_access_role(
    pool: &PgPool,
    actor: UserId,
    project_id: ProjectId,
    user_id: UserId,
    role: ProjectRole,
    audit: AuditRequest<'_>,
) -> Result<ProjectAccessRecord, WorkspaceError> {
    let mut tx = pool.begin().await?;
    let access = load_project_access(&mut tx, project_id, user_id).await?;
    if access.role == role {
        tx.commit().await?;
        return Ok(access);
    }
    sqlx::query(
        "UPDATE project_access SET role = $3, updated_at = now()
         WHERE project_id = $1 AND user_id = $2",
    )
    .bind(project_id.as_uuid())
    .bind(user_id.as_uuid())
    .bind(role.as_str())
    .execute(&mut *tx)
    .await?;
    let workspace_id = workspace_of_project(&mut tx, project_id).await?;
    write_tenancy_audit(
        &mut tx,
        TenancyEvent::ProjectAccessRoleChanged(role),
        AuditPrincipal::User(actor),
        Some(user_id),
        Some(workspace_id),
        Some(project_id),
        audit,
    )
    .await?;
    tx.commit().await?;
    Ok(ProjectAccessRecord { role, ..access })
}

/// Revokes one project-access grant.
pub async fn revoke_project_access(
    pool: &PgPool,
    actor: UserId,
    project_id: ProjectId,
    user_id: UserId,
    audit: AuditRequest<'_>,
) -> Result<ProjectAccessRecord, WorkspaceError> {
    let mut tx = pool.begin().await?;
    let access = load_project_access(&mut tx, project_id, user_id).await?;
    sqlx::query("DELETE FROM project_access WHERE project_id = $1 AND user_id = $2")
        .bind(project_id.as_uuid())
        .bind(user_id.as_uuid())
        .execute(&mut *tx)
        .await?;
    let workspace_id = workspace_of_project(&mut tx, project_id).await?;
    write_tenancy_audit(
        &mut tx,
        TenancyEvent::ProjectAccessRemoved(access.role, access.grant_source),
        AuditPrincipal::User(actor),
        Some(user_id),
        Some(workspace_id),
        Some(project_id),
        audit,
    )
    .await?;
    tx.commit().await?;
    Ok(access)
}

/// Renames a workspace. The slug is identity and never moves.
pub async fn rename_workspace(
    pool: &PgPool,
    actor: UserId,
    workspace_id: WorkspaceId,
    name: &str,
    audit: AuditRequest<'_>,
) -> Result<WorkspaceRecord, WorkspaceError> {
    let trimmed = name.trim();
    if trimmed.is_empty() || trimmed.chars().count() > 200 {
        return Err(WorkspaceError::InvalidName);
    }
    let mut tx = pool.begin().await?;
    lock_workspace(&mut tx, workspace_id).await?;
    sqlx::query("UPDATE workspaces SET name = $2, updated_at = now() WHERE id = $1")
        .bind(workspace_id.as_uuid())
        .bind(trimmed)
        .execute(&mut *tx)
        .await?;
    write_tenancy_audit(
        &mut tx,
        TenancyEvent::WorkspaceRenamed,
        AuditPrincipal::User(actor),
        None,
        Some(workspace_id),
        None,
        audit,
    )
    .await?;
    tx.commit().await?;
    let row = sqlx::query(
        "SELECT id, kind, name, slug, workos_organization_id FROM workspaces WHERE id = $1",
    )
    .bind(workspace_id.as_uuid())
    .fetch_one(pool)
    .await?;
    workspace_record(&row)
}

/// Deletes a workspace that owns no projects. The migration workspace and any
/// workspace still holding projects are refused.
pub async fn delete_workspace(
    pool: &PgPool,
    actor: UserId,
    workspace_id: WorkspaceId,
    audit: AuditRequest<'_>,
) -> Result<(), WorkspaceError> {
    if workspace_id == MIGRATION_WORKSPACE_ID {
        return Err(WorkspaceError::MigrationWorkspaceProtected);
    }
    let mut tx = pool.begin().await?;
    lock_workspace(&mut tx, workspace_id).await?;
    let projects: i64 = sqlx::query_scalar("SELECT count(*) FROM projects WHERE workspace_id = $1")
        .bind(workspace_id.as_uuid())
        .fetch_one(&mut *tx)
        .await?;
    if projects > 0 {
        return Err(WorkspaceError::WorkspaceHasProjects);
    }
    write_tenancy_audit(
        &mut tx,
        TenancyEvent::WorkspaceDeleted,
        AuditPrincipal::User(actor),
        None,
        Some(workspace_id),
        None,
        audit,
    )
    .await?;
    sqlx::query("DELETE FROM workspace_memberships WHERE workspace_id = $1")
        .bind(workspace_id.as_uuid())
        .execute(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM workspaces WHERE id = $1")
        .bind(workspace_id.as_uuid())
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(())
}

/// Applies one WorkOS Organization snapshot to local projections, in one
/// transaction per organization:
///
/// - an Organization with no local workspace is created through the same path
///   onboarding uses;
/// - a `workos_mirrored` membership the provider lists is created or moved
///   forward (role, status, provider membership ID);
/// - a `workos_mirrored` membership the provider no longer lists is
///   deactivated, never deleted;
/// - a `local` grant is never touched. Disagreements with provider state are
///   returned as findings, not resolved;
/// - a provider role slug that maps to no workspace role changes nothing and
///   is a finding (fail closed).
pub async fn apply_organization_snapshot(
    pool: &PgPool,
    snapshot: &OrganizationSnapshot,
    audit: AuditRequest<'_>,
) -> Result<ReconciliationReport, WorkspaceError> {
    let mut tx = pool.begin().await?;
    let workspace = match find_workspace_by_organization(
        &mut tx,
        &snapshot.organization.external_organization_id,
    )
    .await?
    {
        Some(workspace) => workspace,
        None => {
            let workspace = insert_workspace(
                &mut tx,
                WorkspaceId::new(Uuid::new_v4()),
                WorkspaceKind::Organization,
                &snapshot.organization.name,
                Some(&snapshot.organization.external_organization_id),
            )
            .await?;
            write_tenancy_audit(
                &mut tx,
                TenancyEvent::WorkspaceCreated(WorkspaceKind::Organization),
                AuditPrincipal::System,
                None,
                Some(workspace.id),
                None,
                audit,
            )
            .await?;
            workspace
        }
    };
    let mut report = ReconciliationReport {
        workspace_id: workspace.id,
        created: 0,
        role_changes: 0,
        status_changes: 0,
        findings: Vec::new(),
    };

    let local_rows = sqlx::query(
        "SELECT id, workspace_id, user_id, role, status, grant_source, workos_membership_id
         FROM workspace_memberships WHERE workspace_id = $1 FOR UPDATE",
    )
    .bind(workspace.id.as_uuid())
    .fetch_all(&mut *tx)
    .await?;
    let mut local: HashMap<Uuid, MembershipRecord> = HashMap::new();
    for row in &local_rows {
        let record = membership_record(row)?;
        local.insert(record.user_id.as_uuid(), record);
    }

    let owners_before = count_active_owners(&mut tx, workspace.id).await?;
    let mut provider_users: HashSet<Uuid> = HashSet::new();

    for remote in &snapshot.memberships {
        let Some(user_id) = find_user_id(&mut tx, &remote.external_user_id).await? else {
            report
                .findings
                .push(ReconciliationFinding::ProviderMemberWithoutLocalUser {
                    external_user_id: remote.external_user_id.clone(),
                });
            continue;
        };
        if remote.status == DirectoryMembershipStatus::Pending {
            report
                .findings
                .push(ReconciliationFinding::ProviderMembershipPending {
                    external_user_id: remote.external_user_id.clone(),
                });
            continue;
        }
        if remote.status == DirectoryMembershipStatus::Active {
            provider_users.insert(user_id.as_uuid());
        }
        let Some(provider_role) = map_directory_role(remote.role_slug.as_deref()) else {
            report
                .findings
                .push(ReconciliationFinding::ProviderRoleUnknown {
                    external_user_id: remote.external_user_id.clone(),
                    role_slug: remote.role_slug.clone(),
                });
            continue;
        };
        match local.get(&user_id.as_uuid()) {
            // A DeepRef-only grant: preserved. A role disagreement is
            // surfaced here; an absence from the provider is surfaced in the
            // sweep below, once every provider member is known.
            Some(existing) if existing.grant_source == GrantSource::Local => {
                if remote.status == DirectoryMembershipStatus::Active
                    && existing.role != provider_role
                {
                    report
                        .findings
                        .push(ReconciliationFinding::LocalGrantRoleConflict {
                            user_id,
                            local_role: existing.role,
                            provider_role,
                        });
                }
            }
            Some(existing) => {
                let desired_status = if remote.status == DirectoryMembershipStatus::Active {
                    MembershipStatus::Active
                } else {
                    MembershipStatus::Deactivated
                };
                let membership_id = remote.external_membership_id.as_str();
                if existing.workos_membership_id.as_deref() != Some(membership_id) {
                    sqlx::query(
                        "UPDATE workspace_memberships
                         SET workos_membership_id = $3, updated_at = now()
                         WHERE workspace_id = $1 AND user_id = $2",
                    )
                    .bind(workspace.id.as_uuid())
                    .bind(user_id.as_uuid())
                    .bind(membership_id)
                    .execute(&mut *tx)
                    .await?;
                }
                if existing.role != provider_role {
                    sqlx::query(
                        "UPDATE workspace_memberships SET role = $3, updated_at = now()
                         WHERE workspace_id = $1 AND user_id = $2",
                    )
                    .bind(workspace.id.as_uuid())
                    .bind(user_id.as_uuid())
                    .bind(provider_role.as_str())
                    .execute(&mut *tx)
                    .await?;
                    write_tenancy_audit(
                        &mut tx,
                        TenancyEvent::MembershipRoleChanged(provider_role),
                        AuditPrincipal::System,
                        Some(user_id),
                        Some(workspace.id),
                        None,
                        audit,
                    )
                    .await?;
                    report.role_changes += 1;
                }
                if existing.status != desired_status {
                    sqlx::query(
                        "UPDATE workspace_memberships SET status = $3, updated_at = now()
                         WHERE workspace_id = $1 AND user_id = $2",
                    )
                    .bind(workspace.id.as_uuid())
                    .bind(user_id.as_uuid())
                    .bind(desired_status.as_str())
                    .execute(&mut *tx)
                    .await?;
                    write_tenancy_audit(
                        &mut tx,
                        TenancyEvent::MembershipStatusChanged(desired_status),
                        AuditPrincipal::System,
                        Some(user_id),
                        Some(workspace.id),
                        None,
                        audit,
                    )
                    .await?;
                    report.status_changes += 1;
                }
            }
            // A missed addition: the provider lists a member with no local row.
            None => {
                if remote.status == DirectoryMembershipStatus::Active {
                    insert_membership_row(
                        &mut tx,
                        workspace.id,
                        user_id,
                        provider_role,
                        MembershipStatus::Active,
                        GrantSource::WorkosMirrored,
                        Some(remote.external_membership_id.as_str()),
                    )
                    .await?;
                    write_tenancy_audit(
                        &mut tx,
                        TenancyEvent::MembershipCreated(provider_role, GrantSource::WorkosMirrored),
                        AuditPrincipal::System,
                        Some(user_id),
                        Some(workspace.id),
                        None,
                        audit,
                    )
                    .await?;
                    report.created += 1;
                }
            }
        }
    }

    // A missed removal: mirrored grants the provider no longer lists (or lists
    // as inactive) are deactivated, never deleted. `local` grants are left as
    // they are; a local grant the provider does not confirm is surfaced here.
    for (user_id, existing) in &local {
        if provider_users.contains(user_id) {
            continue;
        }
        match existing.grant_source {
            GrantSource::WorkosMirrored => {
                if existing.status != MembershipStatus::Active {
                    continue;
                }
                sqlx::query(
                    "UPDATE workspace_memberships SET status = 'deactivated', updated_at = now()
                     WHERE workspace_id = $1 AND user_id = $2",
                )
                .bind(workspace.id.as_uuid())
                .bind(user_id)
                .execute(&mut *tx)
                .await?;
                write_tenancy_audit(
                    &mut tx,
                    TenancyEvent::MembershipStatusChanged(MembershipStatus::Deactivated),
                    AuditPrincipal::System,
                    Some(UserId::new(*user_id)),
                    Some(workspace.id),
                    None,
                    audit,
                )
                .await?;
                report.status_changes += 1;
            }
            GrantSource::Local => {
                report
                    .findings
                    .push(ReconciliationFinding::LocalGrantWithoutProviderMembership {
                        user_id: UserId::new(*user_id),
                    });
            }
        }
    }

    // Provider state (not a voluntary change) may remove the last owner. The
    // state is mirrored as it stands; the loss of the last owner is surfaced.
    if owners_before > 0 && count_active_owners(&mut tx, workspace.id).await? == 0 {
        report
            .findings
            .push(ReconciliationFinding::WorkspaceWithoutOwner {
                workspace_id: workspace.id,
            });
    }
    tx.commit().await?;
    Ok(report)
}

async fn count_active_owners(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: WorkspaceId,
) -> Result<i64, WorkspaceError> {
    Ok(sqlx::query_scalar(
        "SELECT count(*) FROM workspace_memberships
         WHERE workspace_id = $1 AND role = 'owner' AND status = 'active'",
    )
    .bind(workspace_id.as_uuid())
    .fetch_one(&mut **tx)
    .await?)
}

async fn find_user_id(
    tx: &mut Transaction<'_, Postgres>,
    external_user_id: &ExternalUserId,
) -> Result<Option<UserId>, WorkspaceError> {
    let id: Option<Uuid> = sqlx::query_scalar("SELECT id FROM users WHERE workos_user_id = $1")
        .bind(external_user_id.as_str())
        .fetch_optional(&mut **tx)
        .await?;
    Ok(id.map(UserId::new))
}

async fn find_workspace_by_organization(
    tx: &mut Transaction<'_, Postgres>,
    external_organization_id: &ExternalOrganizationId,
) -> Result<Option<WorkspaceRecord>, WorkspaceError> {
    let row = sqlx::query(
        "SELECT id, kind, name, slug, workos_organization_id
         FROM workspaces WHERE workos_organization_id = $1",
    )
    .bind(external_organization_id.as_str())
    .fetch_optional(&mut **tx)
    .await?;
    row.as_ref().map(workspace_record).transpose()
}

/// Reconciliation (03a): lists every WorkOS Organization and its memberships
/// and repairs local projections that drifted. This is an application
/// function, not an endpoint: the 04a integration inbox calls it, and until a
/// workspace-scoped job scope exists (05a) it runs as an explicit non-queue
/// task. `audit.request_id` is that task's or the inbox receipt's ID.
pub async fn reconcile_organizations(
    pool: &PgPool,
    directory: &dyn OrganizationDirectory,
    audit: AuditRequest<'_>,
) -> Result<Vec<ReconciliationReport>, WorkspaceError> {
    let mut reports = Vec::new();
    for organization in directory.list_organizations().await? {
        let memberships = directory
            .list_memberships(&organization.external_organization_id)
            .await?;
        let snapshot = OrganizationSnapshot {
            organization,
            memberships,
        };
        reports.push(apply_organization_snapshot(pool, &snapshot, audit).await?);
    }
    Ok(reports)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InvitationRecord {
    pub id: Uuid,
    pub workspace_id: WorkspaceId,
    pub email: String,
    pub role: WorkspaceRole,
    pub invited_by: UserId,
    pub expires_at: chrono::DateTime<chrono::Utc>,
}

/// How one acceptance attempt ended. `Expired` and `Unavailable` change
/// nothing for the user; `Expired` also marks the invitation expired and
/// audits it, so that write must be committed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AcceptOutcome {
    Accepted(MembershipRecord),
    Expired,
    Unavailable,
    EmailMismatch,
}

/// The actor's active role in a workspace, if it may invite: only active
/// owners and admins do.
async fn inviting_role(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: WorkspaceId,
    actor: UserId,
) -> Result<WorkspaceRole, WorkspaceError> {
    let role: Option<String> = sqlx::query_scalar(
        "SELECT role FROM workspace_memberships
         WHERE workspace_id = $1 AND user_id = $2 AND status = 'active'",
    )
    .bind(workspace_id.as_uuid())
    .bind(actor.as_uuid())
    .fetch_optional(&mut **tx)
    .await?;
    let role = role
        .as_deref()
        .and_then(WorkspaceRole::parse)
        .ok_or(WorkspaceError::NotPermitted)?;
    if role.rank() < WorkspaceRole::Admin.rank() {
        return Err(WorkspaceError::NotPermitted);
    }
    Ok(role)
}

/// Creates a pending invitation (`workspace:members:invite`). Only an active
/// owner or admin may invite, never to a role above their own. A stale pending
/// invitation to the same address is expired (and audited) first, so a
/// re-invitation after expiry works while a live one is refused.
pub async fn invite_to_workspace(
    pool: &PgPool,
    actor: UserId,
    workspace_id: WorkspaceId,
    email: &str,
    role: WorkspaceRole,
    expires_at: chrono::DateTime<chrono::Utc>,
    audit: AuditRequest<'_>,
) -> Result<InvitationRecord, WorkspaceError> {
    let email = normalize_invitation_email(email).ok_or(WorkspaceError::InvalidInvitation)?;
    if expires_at <= chrono::Utc::now() {
        return Err(WorkspaceError::InvalidInvitation);
    }
    let mut tx = pool.begin().await?;
    lock_workspace(&mut tx, workspace_id).await?;
    let inviter = inviting_role(&mut tx, workspace_id, actor).await?;
    if role.rank() > inviter.rank() {
        return Err(WorkspaceError::RoleAboveInviter);
    }
    let already_member: bool = sqlx::query_scalar(
        "SELECT EXISTS (
           SELECT 1 FROM workspace_memberships m JOIN users u ON u.id = m.user_id
           WHERE m.workspace_id = $1 AND m.status = 'active'
             AND lower(btrim(u.primary_email)) = $2)",
    )
    .bind(workspace_id.as_uuid())
    .bind(&email)
    .fetch_one(&mut *tx)
    .await?;
    if already_member {
        return Err(WorkspaceError::AlreadyMember);
    }
    let stale = sqlx::query(
        "UPDATE workspace_invitations SET status = 'expired', updated_at = now()
         WHERE workspace_id = $1 AND email = $2 AND status = 'pending' AND expires_at <= now()
         RETURNING role",
    )
    .bind(workspace_id.as_uuid())
    .bind(&email)
    .fetch_all(&mut *tx)
    .await?;
    for row in &stale {
        let stale_role: String = row.try_get("role")?;
        let stale_role =
            WorkspaceRole::parse(&stale_role).ok_or(WorkspaceError::Corrupt("role"))?;
        write_tenancy_audit(
            &mut tx,
            TenancyEvent::InvitationExpired(stale_role),
            AuditPrincipal::System,
            None,
            Some(workspace_id),
            None,
            audit,
        )
        .await?;
    }
    let id = Uuid::new_v4();
    let inserted = sqlx::query(
        "INSERT INTO workspace_invitations
           (id, workspace_id, email, role, invited_by, status, expires_at)
         VALUES ($1, $2, $3, $4, $5, 'pending', $6)
         ON CONFLICT (workspace_id, email) WHERE status = 'pending' DO NOTHING",
    )
    .bind(id)
    .bind(workspace_id.as_uuid())
    .bind(&email)
    .bind(role.as_str())
    .bind(actor.as_uuid())
    .bind(expires_at)
    .execute(&mut *tx)
    .await?
    .rows_affected()
        == 1;
    if !inserted {
        // Dropping the transaction rolls back the stale-expiry audit too; the
        // caller retries with nothing half-applied.
        return Err(WorkspaceError::InvitationAlreadyPending);
    }
    write_tenancy_audit(
        &mut tx,
        TenancyEvent::InvitationCreated(role),
        AuditPrincipal::User(actor),
        None,
        Some(workspace_id),
        None,
        audit,
    )
    .await?;
    tx.commit().await?;
    Ok(InvitationRecord {
        id,
        workspace_id,
        email,
        role,
        invited_by: actor,
        expires_at,
    })
}

/// Revokes a pending invitation. Only an active owner or admin of its
/// workspace may, and only while it is still pending.
pub async fn revoke_invitation(
    pool: &PgPool,
    actor: UserId,
    invitation_id: Uuid,
    audit: AuditRequest<'_>,
) -> Result<(), WorkspaceError> {
    let mut tx = pool.begin().await?;
    let workspace: Option<Uuid> =
        sqlx::query_scalar("SELECT workspace_id FROM workspace_invitations WHERE id = $1")
            .bind(invitation_id)
            .fetch_optional(&mut *tx)
            .await?;
    let workspace_id = WorkspaceId::new(workspace.ok_or(WorkspaceError::UnknownInvitation)?);
    inviting_role(&mut tx, workspace_id, actor).await?;
    let revoked = sqlx::query_scalar::<_, String>(
        "UPDATE workspace_invitations
         SET status = 'revoked', revoked_at = now(), updated_at = now()
         WHERE id = $1 AND status = 'pending' RETURNING role",
    )
    .bind(invitation_id)
    .fetch_optional(&mut *tx)
    .await?
    .ok_or(WorkspaceError::InvitationUnavailable)?;
    let role = WorkspaceRole::parse(&revoked).ok_or(WorkspaceError::Corrupt("role"))?;
    write_tenancy_audit(
        &mut tx,
        TenancyEvent::InvitationRevoked(role),
        AuditPrincipal::User(actor),
        None,
        Some(workspace_id),
        None,
        audit,
    )
    .await?;
    tx.commit().await?;
    Ok(())
}

/// One acceptance attempt inside the caller's transaction. Single use is the
/// conditional UPDATE: of any number of concurrent attempts exactly one
/// matches `status = 'pending'`. Expiry is judged by the database clock at
/// this moment. The user's email must equal the invitation's.
pub(crate) async fn accept_in_transaction(
    tx: &mut Transaction<'_, Postgres>,
    user_id: UserId,
    invitation_id: Uuid,
    audit: AuditRequest<'_>,
) -> Result<AcceptOutcome, WorkspaceError> {
    let user = sqlx::query("SELECT primary_email, status FROM users WHERE id = $1")
        .bind(user_id.as_uuid())
        .fetch_optional(&mut **tx)
        .await?;
    let user = user.ok_or(WorkspaceError::NotPermitted)?;
    let status: String = user.try_get("status")?;
    if status != "active" {
        return Err(WorkspaceError::NotPermitted);
    }
    let user_email: String = user.try_get("primary_email")?;
    let invitation =
        sqlx::query("SELECT workspace_id, email FROM workspace_invitations WHERE id = $1")
            .bind(invitation_id)
            .fetch_optional(&mut **tx)
            .await?
            .ok_or(WorkspaceError::UnknownInvitation)?;
    let workspace_id = WorkspaceId::new(invitation.try_get("workspace_id")?);
    let invited_email: String = invitation.try_get("email")?;
    if normalize_invitation_email(&user_email).as_deref() != Some(invited_email.as_str()) {
        return Ok(AcceptOutcome::EmailMismatch);
    }
    let accepted = sqlx::query(
        "UPDATE workspace_invitations
         SET status = 'accepted', accepted_by_user_id = $2, accepted_at = now(), updated_at = now()
         WHERE id = $1 AND status = 'pending' AND expires_at > now()
         RETURNING role",
    )
    .bind(invitation_id)
    .bind(user_id.as_uuid())
    .fetch_optional(&mut **tx)
    .await?;
    if let Some(row) = accepted {
        let role: String = row.try_get("role")?;
        let role = WorkspaceRole::parse(&role).ok_or(WorkspaceError::Corrupt("role"))?;
        lock_workspace(tx, workspace_id).await?;
        let existing: bool = sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM workspace_memberships WHERE workspace_id = $1 AND user_id = $2)",
        )
        .bind(workspace_id.as_uuid())
        .bind(user_id.as_uuid())
        .fetch_one(&mut **tx)
        .await?;
        if existing {
            return Err(WorkspaceError::AlreadyMember);
        }
        let membership = insert_membership_row(
            tx,
            workspace_id,
            user_id,
            role,
            MembershipStatus::Active,
            GrantSource::Local,
            None,
        )
        .await?;
        write_tenancy_audit(
            tx,
            TenancyEvent::InvitationAccepted(role),
            AuditPrincipal::User(user_id),
            Some(user_id),
            Some(workspace_id),
            None,
            audit,
        )
        .await?;
        write_tenancy_audit(
            tx,
            TenancyEvent::MembershipCreated(role, GrantSource::Local),
            AuditPrincipal::User(user_id),
            Some(user_id),
            Some(workspace_id),
            None,
            audit,
        )
        .await?;
        return Ok(AcceptOutcome::Accepted(membership));
    }
    let expired = sqlx::query_scalar::<_, String>(
        "UPDATE workspace_invitations SET status = 'expired', updated_at = now()
         WHERE id = $1 AND status = 'pending' AND expires_at <= now() RETURNING role",
    )
    .bind(invitation_id)
    .fetch_optional(&mut **tx)
    .await?;
    let Some(role) = expired else {
        return Ok(AcceptOutcome::Unavailable);
    };
    let role = WorkspaceRole::parse(&role).ok_or(WorkspaceError::Corrupt("role"))?;
    write_tenancy_audit(
        tx,
        TenancyEvent::InvitationExpired(role),
        AuditPrincipal::System,
        None,
        Some(workspace_id),
        None,
        audit,
    )
    .await?;
    Ok(AcceptOutcome::Expired)
}

/// Accepts an invitation for an authenticated user: binds it to them and
/// creates a `local` membership. An expired invitation is marked expired (and
/// audited) before the refusal is returned.
pub async fn accept_invitation(
    pool: &PgPool,
    user_id: UserId,
    invitation_id: Uuid,
    audit: AuditRequest<'_>,
) -> Result<MembershipRecord, WorkspaceError> {
    let mut tx = pool.begin().await?;
    let outcome = accept_in_transaction(&mut tx, user_id, invitation_id, audit).await?;
    tx.commit().await?;
    match outcome {
        AcceptOutcome::Accepted(membership) => Ok(membership),
        AcceptOutcome::Expired => Err(WorkspaceError::InvitationExpired),
        AcceptOutcome::Unavailable => Err(WorkspaceError::InvitationUnavailable),
        AcceptOutcome::EmailMismatch => Err(WorkspaceError::InvitationEmailMismatch),
    }
}

/// First sign-in (03a): joins the oldest pending, unexpired invitation that
/// matches the user's email. Candidates are tried oldest first (`created_at`,
/// then `id`); an expired one is marked expired and skipped, one that another
/// attempt wins is skipped. Returns the membership joined, or `None` when the
/// user has no usable invitation and should get a personal workspace.
pub(crate) async fn accept_first_pending_invitation(
    tx: &mut Transaction<'_, Postgres>,
    user_id: UserId,
    email: &str,
    audit: AuditRequest<'_>,
) -> Result<Option<MembershipRecord>, WorkspaceError> {
    let Some(email) = normalize_invitation_email(email) else {
        return Ok(None);
    };
    let candidates: Vec<Uuid> = sqlx::query_scalar(
        "SELECT id FROM workspace_invitations
         WHERE email = $1 AND status = 'pending'
         ORDER BY created_at, id",
    )
    .bind(&email)
    .fetch_all(&mut **tx)
    .await?;
    for invitation_id in candidates {
        if let AcceptOutcome::Accepted(membership) =
            accept_in_transaction(tx, user_id, invitation_id, audit).await?
        {
            return Ok(Some(membership));
        }
    }
    Ok(None)
}
