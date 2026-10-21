#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]
//! The policy decision point against PostgreSQL, and the same-transaction
//! guarantee of the administrative audit (handoff 03b).

use std::sync::atomic::{AtomicUsize, Ordering};

use deepref_app::authorization::{PostgresAuthorizationStore, viewable_project_ids};
use deepref_app::identity::{AuditRequest, NewBrowserSession, sign_in as create_sign_in};
use deepref_app::workspaces::{
    add_workspace_member, change_membership_role, change_project_access_role, grant_project_access,
    remove_membership, revoke_project_access, set_membership_status,
};
use deepref_application::authorization::{
    AuthorizationStore, Authorizer, AuthzError, PolicyDecisionPoint,
};
use deepref_application::identity::{
    OpaqueSecret, ProviderProfile, ProviderSession, SessionRefKey, SessionTokenKey,
};
use deepref_domain::{
    ExternalUserId, MembershipStatus, Principal, ProjectId, ProjectRole, UserId, WorkspaceId,
    WorkspaceRole,
    authorization::{Permission, Resource, project_role_grants, workspace_role_grants},
};
use sqlx::PgPool;
use uuid::Uuid;

async fn database() -> Option<PgPool> {
    deepref_app::test_isolation::isolated_pool_from_env("DATABASE_URL", 8).await
}

static COUNTER: AtomicUsize = AtomicUsize::new(1);

async fn sign_in(pool: &PgPool, user: &str) -> UserId {
    const PASSWORD: &str = "a-cookie-password-of-at-least-32-characters";
    let token = format!("tok-{}", COUNTER.fetch_add(1, Ordering::SeqCst));
    let session = ProviderSession {
        session_id: format!("prov-{token}"),
        external_user_id: ExternalUserId::parse(user).unwrap(),
    };
    let hash = SessionTokenKey::from_secret(PASSWORD).token_hash(&token);
    let session_ref = SessionRefKey::from_secret(PASSWORD).session_ref(&session.session_id);
    create_sign_in(
        pool,
        &session,
        &ProviderProfile {
            email: format!("{user}@example.org"),
            email_verified: true,
            display_name: None,
        },
        NewBrowserSession {
            token_hash: &hash,
            sealed_provider_session: &OpaqueSecret::new("sealed"),
            session_ref: &session_ref,
            expires_at: chrono::Utc::now() + chrono::Duration::days(1),
        },
        AuditRequest::default(),
    )
    .await
    .unwrap();
    UserId::new(
        sqlx::query_scalar("SELECT id FROM users WHERE workos_user_id = $1")
            .bind(user)
            .fetch_one(pool)
            .await
            .unwrap(),
    )
}

async fn personal_workspace(pool: &PgPool, user: UserId) -> WorkspaceId {
    WorkspaceId::new(
        sqlx::query_scalar(
            "SELECT workspace_id FROM workspace_memberships WHERE user_id = $1 AND role = 'owner'",
        )
        .bind(user.as_uuid())
        .fetch_one(pool)
        .await
        .unwrap(),
    )
}

async fn project_in(pool: &PgPool, workspace: WorkspaceId) -> ProjectId {
    let id = Uuid::new_v4();
    sqlx::query("INSERT INTO projects (id, name, workspace_id) VALUES ($1, 'p', $2)")
        .bind(id)
        .bind(workspace.as_uuid())
        .execute(pool)
        .await
        .unwrap();
    ProjectId::new(id)
}

fn pdp(pool: &PgPool) -> PolicyDecisionPoint<PostgresAuthorizationStore> {
    PolicyDecisionPoint::new(PostgresAuthorizationStore::new(pool.clone()))
}

async fn allowed(pool: &PgPool, user: UserId, permission: Permission, resource: Resource) -> bool {
    match pdp(pool)
        .require(&Principal::user(user), permission, resource)
        .await
    {
        Ok(()) => true,
        Err(AuthzError::Denied(_)) => false,
        Err(AuthzError::Store(error)) => panic!("store failed: {error}"),
    }
}

#[tokio::test]
async fn the_database_agrees_with_the_domain_mapping_for_every_role_and_permission() {
    let Some(pool) = database().await else { return };
    let owner = sign_in(&pool, "owner").await;
    let workspace = personal_workspace(&pool, owner).await;
    let project = project_in(&pool, workspace).await;
    let mut index = 0;
    for role in [
        WorkspaceRole::Owner,
        WorkspaceRole::Admin,
        WorkspaceRole::Member,
    ] {
        index += 1;
        let user = sign_in(&pool, &format!("ws_{index}")).await;
        add_workspace_member(&pool, owner, workspace, user, role, AuditRequest::default())
            .await
            .unwrap();
        for permission in Permission::ALL {
            let resource = match permission.scope() {
                deepref_domain::authorization::ResourceKind::Workspace => {
                    Resource::Workspace(workspace)
                }
                deepref_domain::authorization::ResourceKind::Project => Resource::Project(project),
            };
            assert_eq!(
                allowed(&pool, user, permission, resource).await,
                workspace_role_grants(role, permission),
                "workspace {role:?} {permission:?}"
            );
        }
    }
    for role in [
        ProjectRole::ProjectAdmin,
        ProjectRole::Member,
        ProjectRole::Viewer,
    ] {
        index += 1;
        let guest = sign_in(&pool, &format!("guest_{index}")).await;
        grant_project_access(&pool, owner, project, guest, role, AuditRequest::default())
            .await
            .unwrap();
        for permission in Permission::ALL {
            let resource = match permission.scope() {
                deepref_domain::authorization::ResourceKind::Workspace => {
                    Resource::Workspace(workspace)
                }
                deepref_domain::authorization::ResourceKind::Project => Resource::Project(project),
            };
            assert_eq!(
                allowed(&pool, guest, permission, resource).await,
                project_role_grants(role, permission),
                "project {role:?} {permission:?}"
            );
        }
    }
}

#[tokio::test]
async fn missing_foreign_and_deactivated_all_deny() {
    let Some(pool) = database().await else { return };
    let a = sign_in(&pool, "a").await;
    let b = sign_in(&pool, "b").await;
    let a_workspace = personal_workspace(&pool, a).await;
    let project = project_in(&pool, a_workspace).await;
    assert!(
        allowed(
            &pool,
            a,
            Permission::ProjectView,
            Resource::Project(project)
        )
        .await
    );
    assert!(
        !allowed(
            &pool,
            b,
            Permission::ProjectView,
            Resource::Project(project)
        )
        .await
    );
    assert!(
        !allowed(
            &pool,
            a,
            Permission::ProjectView,
            Resource::Project(ProjectId::new(Uuid::new_v4()))
        )
        .await,
        "a nonexistent project denies even its would-be owner"
    );
    assert!(
        !allowed(
            &pool,
            b,
            Permission::WorkspaceView,
            Resource::Workspace(a_workspace)
        )
        .await
    );
    assert!(
        !allowed(
            &pool,
            a,
            Permission::WorkspaceView,
            Resource::Workspace(WorkspaceId::new(Uuid::new_v4()))
        )
        .await
    );
    // A user row that does not exist holds nothing.
    assert!(
        !allowed(
            &pool,
            UserId::new(Uuid::new_v4()),
            Permission::ProjectView,
            Resource::Project(project)
        )
        .await
    );
    // Global deactivation denies everywhere on the next call.
    sqlx::query("UPDATE users SET status = 'deactivated' WHERE id = $1")
        .bind(a.as_uuid())
        .execute(&pool)
        .await
        .unwrap();
    assert!(
        !allowed(
            &pool,
            a,
            Permission::ProjectView,
            Resource::Project(project)
        )
        .await
    );
}

#[tokio::test]
async fn a_project_without_a_workspace_cannot_be_resolved_and_denies() {
    let Some(pool) = database().await else { return };
    let a = sign_in(&pool, "a").await;
    let workspace = personal_workspace(&pool, a).await;
    let project = project_in(&pool, workspace).await;
    // Simulate the expand window where a project could lack a workspace.
    sqlx::query("ALTER TABLE projects DISABLE TRIGGER projects_workspace_default_trigger")
        .execute(&pool)
        .await
        .unwrap();
    let orphan = Uuid::new_v4();
    // The foreign key allows NULL until the contract migration.
    sqlx::query("INSERT INTO projects (id, name, workspace_id) VALUES ($1, 'orphan', NULL)")
        .bind(orphan)
        .execute(&pool)
        .await
        .unwrap();
    let _ = project;
    assert!(
        !allowed(
            &pool,
            a,
            Permission::ProjectView,
            Resource::Project(ProjectId::new(orphan))
        )
        .await
    );
}

#[tokio::test]
async fn membership_changes_take_effect_on_the_next_decision() {
    let Some(pool) = database().await else { return };
    let owner = sign_in(&pool, "owner").await;
    let user = sign_in(&pool, "user").await;
    let workspace = personal_workspace(&pool, owner).await;
    let project = project_in(&pool, workspace).await;
    let resource = Resource::Project(project);
    add_workspace_member(
        &pool,
        owner,
        workspace,
        user,
        WorkspaceRole::Admin,
        AuditRequest::default(),
    )
    .await
    .unwrap();
    assert!(allowed(&pool, user, Permission::ProjectDelete, resource).await);
    change_membership_role(
        &pool,
        owner,
        workspace,
        user,
        WorkspaceRole::Member,
        AuditRequest::default(),
    )
    .await
    .unwrap();
    assert!(!allowed(&pool, user, Permission::ProjectDelete, resource).await);
    assert!(allowed(&pool, user, Permission::ProjectEdit, resource).await);
    set_membership_status(
        &pool,
        owner,
        workspace,
        user,
        MembershipStatus::Deactivated,
        AuditRequest::default(),
    )
    .await
    .unwrap();
    assert!(!allowed(&pool, user, Permission::ProjectView, resource).await);
    set_membership_status(
        &pool,
        owner,
        workspace,
        user,
        MembershipStatus::Active,
        AuditRequest::default(),
    )
    .await
    .unwrap();
    assert!(allowed(&pool, user, Permission::ProjectView, resource).await);
    remove_membership(&pool, owner, workspace, user, AuditRequest::default())
        .await
        .unwrap();
    assert!(!allowed(&pool, user, Permission::ProjectView, resource).await);
}

#[tokio::test]
async fn one_query_answers_a_decision() {
    // The store trait returns the workspace and every fact together; this
    // pins the contract that a decision needs exactly one round trip.
    let Some(pool) = database().await else { return };
    let owner = sign_in(&pool, "owner").await;
    let workspace = personal_workspace(&pool, owner).await;
    let project = project_in(&pool, workspace).await;
    let access = PostgresAuthorizationStore::new(pool.clone())
        .access(owner, Resource::Project(project))
        .await
        .unwrap();
    assert_eq!(access.workspace_id, Some(workspace));
    assert!(access.grants.can_view_project());
}

#[tokio::test]
async fn the_list_filter_agrees_with_the_decision_point() {
    let Some(pool) = database().await else { return };
    let a = sign_in(&pool, "a").await;
    let b = sign_in(&pool, "b").await;
    let c = sign_in(&pool, "c").await;
    let a_ws = personal_workspace(&pool, a).await;
    let b_ws = personal_workspace(&pool, b).await;
    let a1 = project_in(&pool, a_ws).await;
    let a2 = project_in(&pool, a_ws).await;
    let b1 = project_in(&pool, b_ws).await;
    add_workspace_member(
        &pool,
        a,
        a_ws,
        c,
        WorkspaceRole::Member,
        AuditRequest::default(),
    )
    .await
    .unwrap();
    grant_project_access(
        &pool,
        b,
        b1,
        c,
        ProjectRole::Viewer,
        AuditRequest::default(),
    )
    .await
    .unwrap();
    for user in [a, b, c] {
        let mut listed = viewable_project_ids(&pool, user).await.unwrap();
        listed.sort_by_key(|p| p.as_uuid());
        let mut decided = Vec::new();
        for project in [a1, a2, b1] {
            if allowed(
                &pool,
                user,
                Permission::ProjectView,
                Resource::Project(project),
            )
            .await
            {
                decided.push(project);
            }
        }
        decided.sort_by_key(|p| p.as_uuid());
        assert_eq!(listed, decided);
    }
}

async fn audit_count(pool: &PgPool, event: &str) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM admin_audit_events WHERE event_type = $1")
        .bind(event)
        .fetch_one(pool)
        .await
        .unwrap()
}

/// Makes the audit insert of the given event fail, so the surrounding change
/// must roll back with it.
async fn refuse_audit_of(pool: &PgPool, event: &str) {
    sqlx::query(
        "CREATE OR REPLACE FUNCTION refuse_test_audit() RETURNS trigger LANGUAGE plpgsql AS $$
         BEGIN
           IF NEW.event_type = TG_ARGV[0] THEN RAISE EXCEPTION 'audit refused for test'; END IF;
           RETURN NEW;
         END $$",
    )
    .execute(pool)
    .await
    .unwrap();
    sqlx::query(sqlx::AssertSqlSafe(format!(
        "CREATE TRIGGER refuse_test_audit_{} BEFORE INSERT ON admin_audit_events
         FOR EACH ROW EXECUTE FUNCTION refuse_test_audit('{event}')",
        event.replace('.', "_")
    )))
    .execute(pool)
    .await
    .unwrap();
}

#[tokio::test]
async fn membership_role_and_access_changes_audit_in_the_same_transaction() {
    let Some(pool) = database().await else { return };
    let owner = sign_in(&pool, "owner").await;
    let user = sign_in(&pool, "user").await;
    let workspace = personal_workspace(&pool, owner).await;
    let project = project_in(&pool, workspace).await;
    let audit = AuditRequest::default();

    // Each change writes exactly one row when it commits (sign-in already wrote
    // the owners' own `membership.created` rows).
    let created_before = audit_count(&pool, "membership.created").await;
    add_workspace_member(&pool, owner, workspace, user, WorkspaceRole::Member, audit)
        .await
        .unwrap();
    assert_eq!(
        audit_count(&pool, "membership.created").await,
        created_before + 1
    );
    change_membership_role(&pool, owner, workspace, user, WorkspaceRole::Admin, audit)
        .await
        .unwrap();
    assert_eq!(audit_count(&pool, "membership.role_changed").await, 1);
    grant_project_access(&pool, owner, project, user, ProjectRole::Viewer, audit)
        .await
        .unwrap();
    assert_eq!(audit_count(&pool, "project_access.granted").await, 1);
    change_project_access_role(&pool, owner, project, user, ProjectRole::Member, audit)
        .await
        .unwrap();
    assert_eq!(audit_count(&pool, "project_access.role_changed").await, 1);

    // When the audit write fails, the change rolls back and no row exists.
    for event in [
        "membership.role_changed",
        "membership.removed",
        "project_access.role_changed",
        "project_access.removed",
        "membership.status_changed",
    ] {
        refuse_audit_of(&pool, event).await;
    }
    let before = audit_count(&pool, "membership.role_changed").await;
    assert!(
        change_membership_role(&pool, owner, workspace, user, WorkspaceRole::Member, audit)
            .await
            .is_err()
    );
    assert_eq!(audit_count(&pool, "membership.role_changed").await, before);
    let role: String = sqlx::query_scalar(
        "SELECT role FROM workspace_memberships WHERE user_id = $1 AND workspace_id = $2",
    )
    .bind(user.as_uuid())
    .bind(workspace.as_uuid())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        role, "admin",
        "the role change rolled back with its audit row"
    );

    assert!(
        set_membership_status(
            &pool,
            owner,
            workspace,
            user,
            MembershipStatus::Deactivated,
            audit
        )
        .await
        .is_err()
    );
    let status: String = sqlx::query_scalar(
        "SELECT status FROM workspace_memberships WHERE user_id = $1 AND workspace_id = $2",
    )
    .bind(user.as_uuid())
    .bind(workspace.as_uuid())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(status, "active");

    assert!(
        remove_membership(&pool, owner, workspace, user, audit)
            .await
            .is_err()
    );
    assert_eq!(audit_count(&pool, "membership.removed").await, 0);
    let still: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM workspace_memberships WHERE user_id = $1 AND workspace_id = $2",
    )
    .bind(user.as_uuid())
    .bind(workspace.as_uuid())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(still, 1, "the removal rolled back");

    assert!(
        change_project_access_role(&pool, owner, project, user, ProjectRole::Viewer, audit)
            .await
            .is_err()
    );
    let grant: String = sqlx::query_scalar(
        "SELECT role FROM project_access WHERE user_id = $1 AND project_id = $2",
    )
    .bind(user.as_uuid())
    .bind(project.as_uuid())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(grant, "member");
    assert!(
        revoke_project_access(&pool, owner, project, user, audit)
            .await
            .is_err()
    );
    assert_eq!(audit_count(&pool, "project_access.removed").await, 0);
    let grants: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM project_access WHERE user_id = $1 AND project_id = $2",
    )
    .bind(user.as_uuid())
    .bind(project.as_uuid())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(grants, 1);
}
