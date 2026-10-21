#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]
//! Workspaces, onboarding, reconciliation, membership removal and the
//! administrative audit of tenancy changes (03a).

use std::sync::atomic::{AtomicUsize, Ordering};

use deepref_app::identity::{
    AuditRequest, Deactivation, NewBrowserSession, SignIn, deactivate_user,
    sign_in as create_sign_in,
};
use deepref_app::workspaces::{
    MIGRATION_WORKSPACE_ID, OrganizationSnapshot, WorkspaceError, add_workspace_member,
    apply_organization_snapshot, change_membership_role, change_project_access_role,
    delete_workspace, grant_project_access, reconcile_organizations, remove_membership,
    rename_workspace, revoke_project_access, set_membership_status,
};
use deepref_application::identity::{
    OpaqueSecret, ProviderProfile, ProviderSession, SessionRefKey, SessionTokenKey,
};
use deepref_application::workspaces::{
    DirectoryError, DirectoryFuture, DirectoryMembership, DirectoryMembershipStatus,
    DirectoryOrganization, OrganizationDirectory, ReconciliationFinding,
};
use deepref_domain::{
    ExternalOrganizationId, ExternalUserId, GrantSource, MembershipStatus, ProjectId, ProjectRole,
    UserStatus, WorkspaceKind, WorkspaceRole,
};
use sqlx::PgPool;
use uuid::Uuid;

async fn database() -> Option<PgPool> {
    deepref_app::test_isolation::isolated_pool_from_env("DATABASE_URL", 8).await
}

fn external(id: &str) -> ExternalUserId {
    ExternalUserId::parse(id).expect("valid external id")
}

fn organization(id: &str) -> ExternalOrganizationId {
    ExternalOrganizationId::parse(id).expect("valid organization id")
}

static COOKIE_COUNTER: AtomicUsize = AtomicUsize::new(1);

/// Signs a provider identity in exactly as `/auth/callback` does.
async fn sign_in(pool: &PgPool, user: &str, email: &str, display_name: Option<&str>) -> SignIn {
    const PASSWORD: &str = "a-cookie-password-of-at-least-32-characters";
    let cookie_token = format!(
        "test-session-{}",
        COOKIE_COUNTER.fetch_add(1, Ordering::SeqCst)
    );
    let provider_session = ProviderSession {
        session_id: format!("provider-session-{cookie_token}"),
        external_user_id: external(user),
    };
    let token_hash = SessionTokenKey::from_secret(PASSWORD).token_hash(&cookie_token);
    let session_ref =
        SessionRefKey::from_secret(PASSWORD).session_ref(&provider_session.session_id);
    create_sign_in(
        pool,
        &provider_session,
        &ProviderProfile {
            email: email.to_owned(),
            email_verified: true,
            display_name: display_name.map(str::to_owned),
        },
        NewBrowserSession {
            token_hash: &token_hash,
            sealed_provider_session: &OpaqueSecret::new("sealed test provider session"),
            session_ref: &session_ref,
            expires_at: chrono::Utc::now() + chrono::Duration::days(30),
        },
        AuditRequest {
            request_id: Some("test-request"),
            session_ref: Some(&session_ref),
        },
    )
    .await
    .expect("sign-in succeeds")
}

/// The DeepRef user id a provider identity resolved to.
async fn user_id(pool: &PgPool, user: &str) -> uuid::Uuid {
    sqlx::query_scalar("SELECT id FROM users WHERE workos_user_id = $1")
        .bind(user)
        .fetch_one(pool)
        .await
        .expect("user exists")
}

async fn sign_in_user(pool: &PgPool, user: &str) -> uuid::Uuid {
    sign_in(
        pool,
        user,
        &format!("{user}@example.org"),
        Some("Test Person"),
    )
    .await;
    user_id(pool, user).await
}

/// The one workspace `sign_in` created for a first-time user.
async fn personal_workspace(pool: &PgPool, user: &str) -> uuid::Uuid {
    sqlx::query_scalar(
        "SELECT w.id FROM workspaces w
         JOIN workspace_memberships m ON m.workspace_id = w.id
         JOIN users u ON u.id = m.user_id
         WHERE u.workos_user_id = $1 AND w.kind = 'personal'",
    )
    .bind(user)
    .fetch_one(pool)
    .await
    .expect("personal workspace exists")
}

async fn count(pool: &PgPool, sql: &str) -> i64 {
    sqlx::query_scalar(sqlx::AssertSqlSafe(sql))
        .fetch_one(pool)
        .await
        .expect("count")
}

async fn events(pool: &PgPool, event: &str) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM admin_audit_events WHERE event_type = $1")
        .bind(event)
        .fetch_one(pool)
        .await
        .expect("count events")
}

async fn membership(
    pool: &PgPool,
    workspace_id: uuid::Uuid,
    user_id: uuid::Uuid,
) -> Option<(String, String, String)> {
    sqlx::query_as(
        "SELECT role, status, grant_source FROM workspace_memberships
         WHERE workspace_id = $1 AND user_id = $2",
    )
    .bind(workspace_id)
    .bind(user_id)
    .fetch_optional(pool)
    .await
    .expect("membership loads")
}

/// A provider snapshot for one organization.
fn snapshot(
    organization_id: &str,
    name: &str,
    members: &[(&str, &str, DirectoryMembershipStatus)],
) -> OrganizationSnapshot {
    OrganizationSnapshot {
        organization: DirectoryOrganization {
            external_organization_id: organization(organization_id),
            name: name.to_owned(),
        },
        memberships: members
            .iter()
            .enumerate()
            .map(|(index, (member, role, status))| DirectoryMembership {
                external_membership_id: format!("omem_{organization_id}_{index}"),
                external_user_id: external(member),
                role_slug: Some((*role).to_owned()),
                status: *status,
            })
            .collect(),
        fetch_started_at: chrono::Utc::now(),
    }
}

#[derive(Default)]
struct FakeDirectory {
    organizations: Vec<DirectoryOrganization>,
    memberships: Vec<DirectoryMembership>,
    unavailable: bool,
}

impl OrganizationDirectory for FakeDirectory {
    fn list_organizations<'a>(&'a self) -> DirectoryFuture<'a, Vec<DirectoryOrganization>> {
        if self.unavailable {
            return Box::pin(async { Err(DirectoryError::Unavailable) });
        }
        let organizations = self.organizations.clone();
        Box::pin(async move { Ok(organizations) })
    }
    fn list_memberships<'a>(
        &'a self,
        _: &'a ExternalOrganizationId,
    ) -> DirectoryFuture<'a, Vec<DirectoryMembership>> {
        if self.unavailable {
            return Box::pin(async { Err(DirectoryError::Unavailable) });
        }
        let memberships = self.memberships.clone();
        Box::pin(async move { Ok(memberships) })
    }
}

#[tokio::test]
async fn first_sign_in_creates_one_personal_workspace_exactly_once() {
    let Some(pool) = database().await else { return };
    let first = sign_in(&pool, "user_A", "ada@example.org", Some("Ada Lovelace")).await;
    assert!(matches!(
        first,
        SignIn::Active {
            provisioned: true,
            ..
        }
    ));

    let workspace = personal_workspace(&pool, "user_A").await;
    let rows: Vec<(String, String, bool)> = sqlx::query_as(
        "SELECT name, slug, workos_organization_id IS NULL FROM workspaces WHERE id = $1",
    )
    .bind(workspace)
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].0, "Ada Lovelace's workspace");
    assert!(rows[0].2, "no organization is needed");
    assert!(!rows[0].1.is_empty());

    let user = user_id(&pool, "user_A").await;
    assert_eq!(
        membership(&pool, workspace, user).await,
        Some(("owner".to_owned(), "active".to_owned(), "local".to_owned())),
        "the user owns their personal workspace"
    );
    assert_eq!(events(&pool, "workspace.created").await, 1);
    assert_eq!(events(&pool, "membership.created").await, 1);

    // A retried callback provisions no second user and no second workspace.
    let retry = sign_in(&pool, "user_A", "ada@example.org", Some("Ada Lovelace")).await;
    assert!(matches!(
        retry,
        SignIn::Active {
            provisioned: false,
            ..
        }
    ));
    assert_eq!(
        count(
            &pool,
            "SELECT count(*) FROM workspaces WHERE kind = 'personal'"
        )
        .await,
        1,
        "one personal workspace exactly once"
    );
    assert_eq!(
        count(
            &pool,
            "SELECT count(*) FROM workspaces WHERE kind = 'migration'"
        )
        .await,
        1,
        "the migration workspace is untouched"
    );
    assert_eq!(
        count(&pool, "SELECT count(*) FROM workspace_memberships").await,
        1
    );
    assert_eq!(events(&pool, "workspace.created").await, 1);
    assert_eq!(events(&pool, "membership.created").await, 1);

    // The audit row names the workspace it created, with its closed metadata.
    let row: (Option<Uuid>, Option<Uuid>, String) = sqlx::query_as(
        "SELECT workspace_id, project_id, metadata::text FROM admin_audit_events
         WHERE event_type = 'workspace.created'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(row.0, Some(workspace));
    assert_eq!(row.1, None);
    assert_eq!(row.2, "{\"workspace_kind\": \"personal\"}");
}

#[tokio::test]
async fn reconciliation_repairs_a_missed_addition_and_a_missed_removal() {
    let Some(pool) = database().await else { return };
    let user_a = sign_in_user(&pool, "user_A").await;
    let user_b = sign_in_user(&pool, "user_B").await;
    let active = DirectoryMembershipStatus::Active;

    // A missed addition: the provider lists A and B, DeepRef knows neither.
    let both = snapshot(
        "org_1",
        "Example Org",
        &[("user_A", "admin", active), ("user_B", "member", active)],
    );
    let report = apply_organization_snapshot(&pool, &both, AuditRequest::default())
        .await
        .expect("reconciliation applies");
    assert_eq!(report.created, 2, "both memberships are created");
    assert!(report.findings.is_empty(), "{:?}", report.findings);
    let workspace = report.workspace_id.as_uuid();
    let kind: String = sqlx::query_scalar("SELECT kind FROM workspaces WHERE id = $1")
        .bind(workspace)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(kind, WorkspaceKind::Organization.as_str());
    assert_eq!(
        membership(&pool, workspace, user_a).await,
        Some((
            WorkspaceRole::Admin.as_str().to_owned(),
            "active".to_owned(),
            GrantSource::WorkosMirrored.as_str().to_owned()
        ))
    );
    assert_eq!(
        events(&pool, "workspace.created").await,
        3,
        "two personal workspaces and the organization's"
    );
    assert_eq!(events(&pool, "membership.created").await, 4);

    // A missed removal: B is gone at the provider. The row is deactivated,
    // never deleted, and nothing else changes.
    let without_b = snapshot("org_1", "Example Org", &[("user_A", "admin", active)]);
    let report = apply_organization_snapshot(&pool, &without_b, AuditRequest::default())
        .await
        .expect("reconciliation applies");
    assert_eq!(report.status_changes, 1, "{:?}", report);
    assert_eq!(
        membership(&pool, workspace, user_b).await,
        Some((
            WorkspaceRole::Member.as_str().to_owned(),
            MembershipStatus::Deactivated.as_str().to_owned(),
            GrantSource::WorkosMirrored.as_str().to_owned()
        )),
        "a membership missing from WorkOS is deactivated, not deleted"
    );

    // The projection repairs itself when a newer fetch lists B again.
    let readded = snapshot(
        "org_1",
        "Example Org",
        &[("user_A", "admin", active), ("user_B", "member", active)],
    );
    let report = apply_organization_snapshot(&pool, &readded, AuditRequest::default())
        .await
        .expect("reconciliation applies");
    assert_eq!(report.status_changes, 1);
    assert_eq!(
        membership(&pool, workspace, user_b).await,
        Some((
            WorkspaceRole::Member.as_str().to_owned(),
            "active".to_owned(),
            GrantSource::WorkosMirrored.as_str().to_owned()
        ))
    );

    // Idempotent: the same snapshot again changes nothing.
    let report = apply_organization_snapshot(&pool, &readded, AuditRequest::default())
        .await
        .expect("reconciliation applies");
    assert_eq!(
        (report.created, report.role_changes, report.status_changes),
        (0, 0, 0),
        "{:?}",
        report
    );
    assert_eq!(events(&pool, "membership.created").await, 4);
}

#[tokio::test]
async fn reconciliation_preserves_local_grants_and_surfaces_conflicts() {
    let Some(pool) = database().await else { return };
    let user_a = sign_in_user(&pool, "user_A").await;
    let user_b = sign_in_user(&pool, "user_B").await;
    let active = DirectoryMembershipStatus::Active;

    let with_a = snapshot("org_1", "Example Org", &[("user_A", "member", active)]);
    let report = apply_organization_snapshot(&pool, &with_a, AuditRequest::default())
        .await
        .expect("reconciliation applies");
    let workspace = report.workspace_id.as_uuid();

    // A DeepRef-only grant beside a mirrored workspace.
    add_workspace_member(
        &pool,
        deepref_domain::UserId::new(user_a),
        report.workspace_id,
        deepref_domain::UserId::new(user_b),
        WorkspaceRole::Admin,
        AuditRequest::default(),
    )
    .await
    .expect("local grant is added");

    // The provider does not list B: the local grant is preserved and the
    // disagreement is surfaced.
    let report = apply_organization_snapshot(&pool, &with_a, AuditRequest::default())
        .await
        .expect("reconciliation applies");
    assert!(
        report.findings.contains(
            &ReconciliationFinding::LocalGrantWithoutProviderMembership {
                user_id: deepref_domain::UserId::new(user_b)
            }
        ),
        "{:?}",
        report.findings
    );
    assert_eq!(
        membership(&pool, workspace, user_b).await,
        Some((
            WorkspaceRole::Admin.as_str().to_owned(),
            "active".to_owned(),
            GrantSource::Local.as_str().to_owned()
        )),
        "a local grant is never revoked by reconciliation"
    );

    // The provider lists B with another role: the local role is preserved and
    // the conflict is surfaced, not resolved.
    let with_both = snapshot(
        "org_1",
        "Example Org",
        &[("user_A", "member", active), ("user_B", "member", active)],
    );
    let report = apply_organization_snapshot(&pool, &with_both, AuditRequest::default())
        .await
        .expect("reconciliation applies");
    assert!(
        report
            .findings
            .contains(&ReconciliationFinding::LocalGrantRoleConflict {
                user_id: deepref_domain::UserId::new(user_b),
                local_role: WorkspaceRole::Admin,
                provider_role: WorkspaceRole::Member,
            }),
        "{:?}",
        report.findings
    );
    assert_eq!(
        membership(&pool, workspace, user_b).await,
        Some((
            WorkspaceRole::Admin.as_str().to_owned(),
            "active".to_owned(),
            GrantSource::Local.as_str().to_owned()
        )),
        "the local role survives the provider's opinion"
    );
    // A is mirrored and does follow the provider.
    assert_eq!(
        membership(&pool, workspace, user_a).await,
        Some((
            WorkspaceRole::Member.as_str().to_owned(),
            "active".to_owned(),
            GrantSource::WorkosMirrored.as_str().to_owned()
        ))
    );
}

#[tokio::test]
async fn reconciliation_fails_closed_on_roles_and_users_it_cannot_resolve() {
    let Some(pool) = database().await else { return };
    sign_in_user(&pool, "user_A").await;

    let mixed = OrganizationSnapshot {
        fetch_started_at: chrono::Utc::now(),
        organization: DirectoryOrganization {
            external_organization_id: organization("org_1"),
            name: "Example Org".to_owned(),
        },
        memberships: vec![
            DirectoryMembership {
                external_membership_id: "omem_unknown_user".to_owned(),
                external_user_id: external("user_never_signed_in"),
                role_slug: Some("member".to_owned()),
                status: DirectoryMembershipStatus::Active,
            },
            DirectoryMembership {
                external_membership_id: "omem_unknown_role".to_owned(),
                external_user_id: external("user_A"),
                role_slug: Some("janitor".to_owned()),
                status: DirectoryMembershipStatus::Active,
            },
            DirectoryMembership {
                external_membership_id: "omem_pending".to_owned(),
                external_user_id: external("user_A"),
                role_slug: Some("member".to_owned()),
                status: DirectoryMembershipStatus::Pending,
            },
        ],
    };
    let report = apply_organization_snapshot(&pool, &mixed, AuditRequest::default())
        .await
        .expect("reconciliation applies");
    assert_eq!(report.created, 0);
    assert!(
        report
            .findings
            .contains(&ReconciliationFinding::ProviderMemberWithoutLocalUser {
                external_user_id: external("user_never_signed_in")
            })
    );
    assert!(
        report
            .findings
            .contains(&ReconciliationFinding::ProviderRoleUnknown {
                external_user_id: external("user_A"),
                role_slug: Some("janitor".to_owned())
            })
    );
    assert!(
        report
            .findings
            .contains(&ReconciliationFinding::ProviderMembershipPending {
                external_user_id: external("user_A")
            })
    );
    assert_eq!(
        count(&pool, "SELECT count(*) FROM workspace_memberships").await,
        1,
        "only the personal workspace membership exists"
    );
    let mirrored: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM workspace_memberships
         WHERE workspace_id = $1 AND grant_source = 'workos_mirrored'",
    )
    .bind(report.workspace_id.as_uuid())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(mirrored, 0, "an unknown role grants nothing");
}

#[tokio::test]
async fn provider_state_that_removes_the_last_owner_is_surfaced() {
    let Some(pool) = database().await else { return };
    let user_a = sign_in_user(&pool, "user_A").await;
    let active = DirectoryMembershipStatus::Active;

    let with_owner = snapshot("org_1", "Example Org", &[("user_A", "owner", active)]);
    let report = apply_organization_snapshot(&pool, &with_owner, AuditRequest::default())
        .await
        .expect("reconciliation applies");
    let workspace = report.workspace_id.as_uuid();
    assert!(report.findings.is_empty(), "{:?}", report.findings);

    // The provider removes the last owner. Reconciliation mirrors the state
    // (this is not a voluntary DeepRef change, so the last-owner guard does
    // not refuse it) but surfaces the loss.
    let report = apply_organization_snapshot(
        &pool,
        &snapshot("org_1", "Example Org", &[]),
        AuditRequest::default(),
    )
    .await
    .expect("reconciliation applies");
    assert!(
        report
            .findings
            .contains(&ReconciliationFinding::WorkspaceWithoutOwner {
                workspace_id: report.workspace_id
            }),
        "{:?}",
        report.findings
    );
    assert_eq!(
        membership(&pool, workspace, user_a).await.map(|m| m.1),
        Some(MembershipStatus::Deactivated.as_str().to_owned())
    );
}

#[tokio::test]
async fn last_owner_changes_are_refused_until_ownership_is_transferred() {
    let Some(pool) = database().await else { return };
    let user_a = sign_in_user(&pool, "user_A").await;
    let user_b = sign_in_user(&pool, "user_B").await;
    let workspace = deepref_domain::WorkspaceId::new(personal_workspace(&pool, "user_A").await);
    let owner = deepref_domain::UserId::new(user_a);

    let removal = remove_membership(&pool, owner, workspace, owner, AuditRequest::default()).await;
    assert!(matches!(removal, Err(WorkspaceError::LastOwner)));
    let deactivation = set_membership_status(
        &pool,
        owner,
        workspace,
        owner,
        MembershipStatus::Deactivated,
        AuditRequest::default(),
    )
    .await;
    assert!(matches!(deactivation, Err(WorkspaceError::LastOwner)));
    let demotion = change_membership_role(
        &pool,
        owner,
        workspace,
        owner,
        WorkspaceRole::Member,
        AuditRequest::default(),
    )
    .await;
    assert!(matches!(demotion, Err(WorkspaceError::LastOwner)));
    assert_eq!(
        events(&pool, "membership.removed").await,
        0,
        "a refused change records nothing"
    );

    // Ownership is transferred: now the change may happen.
    add_workspace_member(
        &pool,
        owner,
        workspace,
        deepref_domain::UserId::new(user_b),
        WorkspaceRole::Owner,
        AuditRequest::default(),
    )
    .await
    .expect("a second owner is added");
    let removed = remove_membership(&pool, owner, workspace, owner, AuditRequest::default())
        .await
        .expect("the former owner is removed");
    assert_eq!(removed.role, WorkspaceRole::Owner);
    assert_eq!(events(&pool, "membership.removed").await, 1);
    assert_eq!(
        membership(&pool, workspace.as_uuid(), user_a).await,
        None,
        "the membership is gone"
    );
}

#[tokio::test]
async fn membership_removal_touches_only_one_workspace() {
    let Some(pool) = database().await else { return };
    let user_a = sign_in_user(&pool, "user_A").await;
    let personal = deepref_domain::WorkspaceId::new(personal_workspace(&pool, "user_A").await);
    let active = DirectoryMembershipStatus::Active;
    let report = apply_organization_snapshot(
        &pool,
        &snapshot("org_1", "Example Org", &[("user_A", "member", active)]),
        AuditRequest::default(),
    )
    .await
    .expect("reconciliation applies");
    let organization_workspace = report.workspace_id;

    let actor = deepref_domain::UserId::new(user_a);
    remove_membership(
        &pool,
        actor,
        organization_workspace,
        actor,
        AuditRequest::default(),
    )
    .await
    .expect("the organization membership is removed");

    assert_eq!(
        membership(&pool, organization_workspace.as_uuid(), user_a).await,
        None
    );
    assert_eq!(
        membership(&pool, personal.as_uuid(), user_a).await,
        Some((
            WorkspaceRole::Owner.as_str().to_owned(),
            "active".to_owned(),
            GrantSource::Local.as_str().to_owned()
        )),
        "the user keeps their other workspace and their account"
    );
    let removed: (Option<Uuid>, Option<Uuid>) = sqlx::query_as(
        "SELECT workspace_id, project_id FROM admin_audit_events
         WHERE event_type = 'membership.removed'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(removed.0, Some(organization_workspace.as_uuid()));
    assert_eq!(removed.1, None);
}

#[tokio::test]
async fn global_deactivation_is_not_blocked_by_the_last_owner_guard() {
    let Some(pool) = database().await else { return };
    let user_a = sign_in_user(&pool, "user_A").await;
    let workspace = personal_workspace(&pool, "user_A").await;

    // The user is the only owner of their workspace; account-level
    // deactivation (02) proceeds regardless.
    let outcome = deactivate_user(&pool, &external("user_A"), "receipt-1")
        .await
        .expect("deactivation succeeds");
    assert!(matches!(outcome, Deactivation::Deactivated(_)));
    assert_eq!(
        events(&pool, "user.deactivated").await,
        1,
        "the account change is audited"
    );

    let status: String = sqlx::query_scalar("SELECT status FROM users WHERE id = $1")
        .bind(user_a)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(status, UserStatus::Deactivated.as_str());
    assert_eq!(
        membership(&pool, workspace, user_a).await,
        Some((
            WorkspaceRole::Owner.as_str().to_owned(),
            "active".to_owned(),
            GrantSource::Local.as_str().to_owned()
        )),
        "historical membership provenance is preserved"
    );
    assert_eq!(
        events(&pool, "membership.status_changed").await,
        0,
        "no membership change is forced by account deactivation"
    );

    // A replay changes nothing further.
    let replay = deactivate_user(&pool, &external("user_A"), "receipt-2")
        .await
        .expect("replay is safe");
    assert!(matches!(replay, Deactivation::AlreadyDeactivated(_)));
    assert_eq!(events(&pool, "user.deactivated").await, 1);
}

#[tokio::test]
async fn project_access_changes_are_audited_with_their_workspace_and_project() {
    let Some(pool) = database().await else { return };
    let user_a = sign_in_user(&pool, "user_A").await;
    let user_b = sign_in_user(&pool, "user_B").await;
    let workspace = personal_workspace(&pool, "user_A").await;
    let project = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO projects (id, name, workspace_id) VALUES ($1, 'Tenanted project', $2)",
    )
    .bind(project)
    .bind(workspace)
    .execute(&pool)
    .await
    .unwrap();
    let actor = deepref_domain::UserId::new(user_a);
    let subject = deepref_domain::UserId::new(user_b);
    let project_id = ProjectId::new(project);

    let access = grant_project_access(
        &pool,
        actor,
        project_id,
        subject,
        ProjectRole::Viewer,
        AuditRequest::default(),
    )
    .await
    .expect("access is granted");
    assert_eq!(access.role, ProjectRole::Viewer);
    assert_eq!(access.grant_source, GrantSource::Local);

    let changed = change_project_access_role(
        &pool,
        actor,
        project_id,
        subject,
        ProjectRole::Member,
        AuditRequest::default(),
    )
    .await
    .expect("the role changes");
    assert_eq!(changed.role, ProjectRole::Member);
    // A no-op change records nothing.
    change_project_access_role(
        &pool,
        actor,
        project_id,
        subject,
        ProjectRole::Member,
        AuditRequest::default(),
    )
    .await
    .expect("the no-op change succeeds");
    assert_eq!(events(&pool, "project_access.role_changed").await, 1);

    revoke_project_access(&pool, actor, project_id, subject, AuditRequest::default())
        .await
        .expect("access is revoked");
    assert_eq!(events(&pool, "project_access.granted").await, 1);
    assert_eq!(events(&pool, "project_access.removed").await, 1);
    assert_eq!(count(&pool, "SELECT count(*) FROM project_access").await, 0);

    let row: (Option<Uuid>, Option<Uuid>, String) = sqlx::query_as(
        "SELECT workspace_id, project_id, metadata::text FROM admin_audit_events
         WHERE event_type = 'project_access.granted'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(row.0, Some(workspace));
    assert_eq!(row.1, Some(project));
    assert_eq!(row.2, "{\"role\": \"viewer\", \"grant_source\": \"local\"}");

    // Failures: unknown project, duplicate grant.
    let unknown = grant_project_access(
        &pool,
        actor,
        ProjectId::new(Uuid::new_v4()),
        subject,
        ProjectRole::Viewer,
        AuditRequest::default(),
    )
    .await;
    assert!(matches!(unknown, Err(WorkspaceError::UnknownProject)));
    grant_project_access(
        &pool,
        actor,
        project_id,
        subject,
        ProjectRole::Viewer,
        AuditRequest::default(),
    )
    .await
    .expect("regranting after revocation works");
    let duplicate = grant_project_access(
        &pool,
        actor,
        project_id,
        subject,
        ProjectRole::Viewer,
        AuditRequest::default(),
    )
    .await;
    assert!(matches!(
        duplicate,
        Err(WorkspaceError::AccessAlreadyGranted)
    ));
}

#[tokio::test]
async fn workspace_lifecycle_is_audited_and_guards_the_migration_workspace() {
    let Some(pool) = database().await else { return };
    let user_a = sign_in_user(&pool, "user_A").await;
    let workspace = deepref_domain::WorkspaceId::new(personal_workspace(&pool, "user_A").await);
    let actor = deepref_domain::UserId::new(user_a);

    let renamed = rename_workspace(
        &pool,
        actor,
        workspace,
        "  Ada's research  ",
        AuditRequest::default(),
    )
    .await
    .expect("the workspace renames");
    assert_eq!(renamed.name, "Ada's research");
    assert_eq!(
        events(&pool, "workspace.renamed").await,
        1,
        "renames are audited"
    );
    let invalid = rename_workspace(&pool, actor, workspace, "   ", AuditRequest::default()).await;
    assert!(matches!(invalid, Err(WorkspaceError::InvalidName)));

    // A workspace that still owns projects survives.
    let project = Uuid::new_v4();
    sqlx::query("INSERT INTO projects (id, name, workspace_id) VALUES ($1, 'Kept', $2)")
        .bind(project)
        .bind(workspace.as_uuid())
        .execute(&pool)
        .await
        .unwrap();
    let with_projects = delete_workspace(&pool, actor, workspace, AuditRequest::default()).await;
    assert!(matches!(
        with_projects,
        Err(WorkspaceError::WorkspaceHasProjects)
    ));
    sqlx::query("DELETE FROM projects WHERE id = $1")
        .bind(project)
        .execute(&pool)
        .await
        .unwrap();

    let migration = delete_workspace(
        &pool,
        actor,
        MIGRATION_WORKSPACE_ID,
        AuditRequest::default(),
    )
    .await;
    assert!(matches!(
        migration,
        Err(WorkspaceError::MigrationWorkspaceProtected)
    ));

    delete_workspace(&pool, actor, workspace, AuditRequest::default())
        .await
        .expect("an empty workspace deletes");
    assert_eq!(events(&pool, "workspace.deleted").await, 1);
    let remaining: i64 = sqlx::query_scalar("SELECT count(*) FROM workspaces WHERE id = $1")
        .bind(workspace.as_uuid())
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(remaining, 0);
}

#[tokio::test]
async fn the_reconciliation_entry_point_walks_every_organization() {
    let Some(pool) = database().await else { return };
    let user_a = sign_in_user(&pool, "user_A").await;
    let active = DirectoryMembershipStatus::Active;
    let directory = FakeDirectory {
        organizations: vec![DirectoryOrganization {
            external_organization_id: organization("org_1"),
            name: "Example Org".to_owned(),
        }],
        memberships: vec![DirectoryMembership {
            external_membership_id: "omem_1".to_owned(),
            external_user_id: external("user_A"),
            role_slug: Some("member".to_owned()),
            status: active,
        }],
        unavailable: false,
    };
    let reports = reconcile_organizations(
        &pool,
        &directory,
        AuditRequest {
            request_id: Some("reconcile-run-1"),
            session_ref: None,
        },
    )
    .await
    .expect("reconciliation runs");
    assert_eq!(reports.len(), 1);
    assert_eq!(reports[0].created, 1);
    assert_eq!(
        membership(&pool, reports[0].workspace_id.as_uuid(), user_a)
            .await
            .map(|m| m.0),
        Some(WorkspaceRole::Member.as_str().to_owned())
    );
    // The run's receipt ID is the audit correlation ID of its changes.
    let request_id: String = sqlx::query_scalar(
        "SELECT request_id FROM admin_audit_events
         WHERE event_type = 'membership.created' AND workspace_id = $1",
    )
    .bind(reports[0].workspace_id.as_uuid())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(request_id, "reconcile-run-1");

    let unavailable = FakeDirectory {
        unavailable: true,
        ..FakeDirectory::default()
    };
    let failed = reconcile_organizations(&pool, &unavailable, AuditRequest::default()).await;
    assert!(matches!(failed, Err(WorkspaceError::Directory(_))));
}

fn at(snapshot: OrganizationSnapshot, minutes_ago: i64) -> OrganizationSnapshot {
    OrganizationSnapshot {
        fetch_started_at: chrono::Utc::now() - chrono::Duration::minutes(minutes_ago),
        ..snapshot
    }
}

#[tokio::test]
async fn a_stale_snapshot_cannot_restore_access_a_newer_one_removed() {
    let Some(pool) = database().await else { return };
    sign_in_user(&pool, "user_A").await;
    let user_b = sign_in_user(&pool, "user_B").await;
    let both = [
        ("user_A", "owner", DirectoryMembershipStatus::Active),
        ("user_B", "member", DirectoryMembershipStatus::Active),
    ];
    let only_a = [("user_A", "owner", DirectoryMembershipStatus::Active)];

    let first = apply_organization_snapshot(
        &pool,
        &at(snapshot("org_1", "Org", &both), 30),
        AuditRequest::default(),
    )
    .await
    .unwrap();
    let workspace = first.workspace_id.as_uuid();
    // A newer fetch saw B removed.
    apply_organization_snapshot(
        &pool,
        &at(snapshot("org_1", "Org", &only_a), 10),
        AuditRequest::default(),
    )
    .await
    .unwrap();
    assert_eq!(
        membership(&pool, workspace, user_b).await.map(|m| m.1),
        Some("deactivated".to_owned())
    );
    let audited = events(&pool, "membership.status_changed").await;

    // A delayed fetch that began before the removal is applied afterwards.
    let stale = apply_organization_snapshot(
        &pool,
        &at(snapshot("org_1", "Org", &both), 20),
        AuditRequest::default(),
    )
    .await
    .unwrap();
    assert_eq!(
        stale.findings,
        vec![ReconciliationFinding::StaleSnapshot {
            workspace_id: first.workspace_id
        }]
    );
    assert_eq!(
        membership(&pool, workspace, user_b).await.map(|m| m.1),
        Some("deactivated".to_owned()),
        "the stale snapshot restored nothing"
    );
    assert_eq!(events(&pool, "membership.status_changed").await, audited);
}

#[tokio::test]
async fn concurrent_reconciliations_of_one_organization_serialize() {
    let Some(pool) = database().await else { return };
    sign_in_user(&pool, "user_A").await;
    let members = [("user_A", "owner", DirectoryMembershipStatus::Active)];
    let runs = (0..6).map(|_| {
        let pool = pool.clone();
        let snapshot = snapshot("org_1", "Org", &members);
        tokio::spawn(async move {
            apply_organization_snapshot(&pool, &snapshot, AuditRequest::default()).await
        })
    });
    let handles: Vec<_> = runs.collect();
    for handle in handles {
        handle.await.unwrap().expect("no run fails");
    }
    assert_eq!(
        count(
            &pool,
            "SELECT count(*) FROM workspaces WHERE workos_organization_id = 'org_1'"
        )
        .await,
        1,
        "one workspace per organization"
    );
    assert_eq!(
        count(
            &pool,
            "SELECT count(*) FROM workspace_memberships WHERE grant_source = 'workos_mirrored'"
        )
        .await,
        1
    );
}

#[tokio::test]
async fn a_globally_deactivated_owner_does_not_count_as_a_replacement_owner() {
    let Some(pool) = database().await else { return };
    let user_a = sign_in_user(&pool, "user_A").await;
    let user_b = sign_in_user(&pool, "user_B").await;
    let workspace = deepref_domain::WorkspaceId::new(personal_workspace(&pool, "user_A").await);
    let a = deepref_domain::UserId::new(user_a);
    let b = deepref_domain::UserId::new(user_b);
    add_workspace_member(
        &pool,
        a,
        workspace,
        b,
        WorkspaceRole::Owner,
        AuditRequest::default(),
    )
    .await
    .unwrap();
    deactivate_user(&pool, &external("user_B"), "receipt-1")
        .await
        .unwrap();

    let removal = remove_membership(&pool, a, workspace, a, AuditRequest::default()).await;
    assert!(
        matches!(removal, Err(WorkspaceError::LastOwner)),
        "{removal:?}"
    );
    let demotion = change_membership_role(
        &pool,
        a,
        workspace,
        a,
        WorkspaceRole::Member,
        AuditRequest::default(),
    )
    .await;
    assert!(
        matches!(demotion, Err(WorkspaceError::LastOwner)),
        "{demotion:?}"
    );
}

#[tokio::test]
async fn an_unknown_provider_role_deactivates_a_mirrored_grant_and_keeps_local_ones() {
    let Some(pool) = database().await else { return };
    let user_a = sign_in_user(&pool, "user_A").await;
    let user_b = sign_in_user(&pool, "user_B").await;
    sign_in_user(&pool, "user_C").await;
    let known = [
        ("user_A", "owner", DirectoryMembershipStatus::Active),
        ("user_C", "owner", DirectoryMembershipStatus::Active),
    ];
    let report = apply_organization_snapshot(
        &pool,
        &at(snapshot("org_1", "Org", &known), 10),
        AuditRequest::default(),
    )
    .await
    .unwrap();
    let workspace = report.workspace_id;
    add_workspace_member(
        &pool,
        deepref_domain::UserId::new(user_a),
        workspace,
        deepref_domain::UserId::new(user_b),
        WorkspaceRole::Member,
        AuditRequest::default(),
    )
    .await
    .unwrap();

    let unknown = [
        ("user_A", "janitor", DirectoryMembershipStatus::Active),
        ("user_B", "janitor", DirectoryMembershipStatus::Active),
        ("user_C", "owner", DirectoryMembershipStatus::Active),
    ];
    let report = apply_organization_snapshot(
        &pool,
        &at(snapshot("org_1", "Org", &unknown), 1),
        AuditRequest::default(),
    )
    .await
    .unwrap();
    assert!(
        report
            .findings
            .contains(&ReconciliationFinding::ProviderRoleUnknown {
                external_user_id: external("user_A"),
                role_slug: Some("janitor".to_owned())
            })
    );
    assert_eq!(
        membership(&pool, workspace.as_uuid(), user_a)
            .await
            .map(|m| m.1),
        Some("deactivated".to_owned()),
        "the mirrored grant with an unresolved role loses access"
    );
    assert_eq!(
        membership(&pool, workspace.as_uuid(), user_b)
            .await
            .map(|m| m.1),
        Some("active".to_owned()),
        "a local grant is untouched"
    );
}

#[tokio::test]
async fn a_user_provisioned_the_n_minus_one_way_is_onboarded_by_the_next_sign_in() {
    let Some(pool) = database().await else { return };
    // The previous release inserted the user row alone: no membership, no
    // onboarding mark.
    let id = uuid::Uuid::new_v4();
    sqlx::query(
        "INSERT INTO users (id, workos_user_id, primary_email, display_name, status)
         VALUES ($1, 'user_old', 'old@example.org', 'Old Binary', 'active')",
    )
    .bind(id)
    .execute(&pool)
    .await
    .unwrap();
    assert_eq!(
        count(&pool, "SELECT count(*) FROM workspace_memberships").await,
        0
    );

    // Not the inserting call, yet onboarded; a retry and a concurrent pair add nothing.
    let first = sign_in(&pool, "user_old", "old@example.org", Some("Old Binary")).await;
    assert!(matches!(
        first,
        SignIn::Active {
            provisioned: false,
            ..
        }
    ));
    personal_workspace(&pool, "user_old").await;
    let attempts = (0..4).map(|_| {
        let pool = pool.clone();
        tokio::spawn(async move { sign_in(&pool, "user_old", "old@example.org", None).await })
    });
    let handles: Vec<_> = attempts.collect();
    for handle in handles {
        handle.await.unwrap();
    }
    assert_eq!(
        count(
            &pool,
            "SELECT count(*) FROM workspaces WHERE kind = 'personal'"
        )
        .await,
        1
    );
    assert_eq!(events(&pool, "workspace.created").await, 1);
    let onboarded: Option<chrono::DateTime<chrono::Utc>> =
        sqlx::query_scalar("SELECT onboarded_at FROM users WHERE id = $1")
            .bind(id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(onboarded.is_some());
}

#[tokio::test]
async fn deleting_a_workspace_audits_each_removed_membership_and_ends_its_invitations() {
    let Some(pool) = database().await else { return };
    let user_a = sign_in_user(&pool, "user_A").await;
    let user_b = sign_in_user(&pool, "user_B").await;
    let a = deepref_domain::UserId::new(user_a);
    let workspace = deepref_domain::WorkspaceId::new(personal_workspace(&pool, "user_A").await);
    add_workspace_member(
        &pool,
        a,
        workspace,
        deepref_domain::UserId::new(user_b),
        WorkspaceRole::Member,
        AuditRequest::default(),
    )
    .await
    .unwrap();
    let pending = deepref_app::workspaces::invite_to_workspace(
        &pool,
        a,
        workspace,
        "p@example.org",
        WorkspaceRole::Member,
        chrono::Utc::now() + chrono::Duration::days(1),
        AuditRequest::default(),
    )
    .await
    .unwrap();
    let revoked = deepref_app::workspaces::invite_to_workspace(
        &pool,
        a,
        workspace,
        "r@example.org",
        WorkspaceRole::Member,
        chrono::Utc::now() + chrono::Duration::days(1),
        AuditRequest::default(),
    )
    .await
    .unwrap();
    deepref_app::workspaces::revoke_invitation(&pool, a, revoked.id, AuditRequest::default())
        .await
        .unwrap();

    delete_workspace(&pool, a, workspace, AuditRequest::default())
        .await
        .expect("a workspace with invitations and members is deletable");
    assert_eq!(
        count(&pool, "SELECT count(*) FROM workspace_invitations").await,
        0
    );
    assert_eq!(
        events(&pool, "membership.removed").await,
        2,
        "one per member"
    );
    assert_eq!(events(&pool, "workspace.deleted").await, 1);
    let ended: String = sqlx::query_scalar(
        "SELECT metadata->>'invitation_id' FROM admin_audit_events
         WHERE event_type = 'invitation.revoked' AND metadata->>'invitation_id' = $1",
    )
    .bind(pending.id.to_string())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        ended,
        pending.id.to_string(),
        "the pending invitation is revoked by name"
    );
}

#[tokio::test]
async fn an_existing_member_without_the_onboarding_mark_gets_no_personal_workspace() {
    let Some(pool) = database().await else { return };
    let owner = sign_in_user(&pool, "owner").await;
    let workspace = deepref_domain::WorkspaceId::new(personal_workspace(&pool, "owner").await);
    // An existing user the onboarding backfill has not reached: an active
    // membership, no `onboarded_at`.
    let id = uuid::Uuid::new_v4();
    sqlx::query(
        "INSERT INTO users (id, workos_user_id, primary_email, status)
         VALUES ($1, 'user_member', 'member@example.org', 'active')",
    )
    .bind(id)
    .execute(&pool)
    .await
    .unwrap();
    add_workspace_member(
        &pool,
        deepref_domain::UserId::new(owner),
        workspace,
        deepref_domain::UserId::new(id),
        WorkspaceRole::Member,
        AuditRequest::default(),
    )
    .await
    .unwrap();

    sign_in(&pool, "user_member", "member@example.org", None).await;
    assert_eq!(
        count(
            &pool,
            "SELECT count(*) FROM workspaces WHERE kind = 'personal'"
        )
        .await,
        1,
        "only the owner's personal workspace exists"
    );
    let marked: Option<chrono::DateTime<chrono::Utc>> =
        sqlx::query_scalar("SELECT onboarded_at FROM users WHERE id = $1")
            .bind(id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(marked.is_some(), "sign-in marks the user onboarded");
}
