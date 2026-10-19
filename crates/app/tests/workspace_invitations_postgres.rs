#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]
//! Workspace invitations (03a): creation, role ceiling, revocation, expiry,
//! single use under concurrency, email binding and sign-in onboarding.

use std::sync::atomic::{AtomicUsize, Ordering};

use chrono::{Duration, Utc};
use deepref_app::identity::{AuditRequest, NewBrowserSession, SignIn, sign_in as create_sign_in};
use deepref_app::workspaces::{
    WorkspaceError, accept_invitation, add_workspace_member, invite_to_workspace,
    revoke_invitation, set_membership_status,
};
use deepref_application::identity::{
    OpaqueSecret, ProviderProfile, ProviderSession, SessionRefKey, SessionTokenKey,
};
use deepref_domain::{ExternalUserId, MembershipStatus, UserId, WorkspaceId, WorkspaceRole};
use sqlx::PgPool;
use uuid::Uuid;

async fn database() -> Option<PgPool> {
    deepref_app::test_isolation::isolated_pool_from_env("DATABASE_URL", 8).await
}

static COOKIE_COUNTER: AtomicUsize = AtomicUsize::new(1);

const AUDIT: AuditRequest<'static> = AuditRequest {
    request_id: Some("test-request"),
    session_ref: None,
};

async fn sign_in(pool: &PgPool, user: &str, email: &str) -> SignIn {
    sign_in_verified(pool, user, email, true).await
}

async fn sign_in_verified(pool: &PgPool, user: &str, email: &str, verified: bool) -> SignIn {
    const PASSWORD: &str = "a-cookie-password-of-at-least-32-characters";
    let cookie_token = format!(
        "inv-session-{}",
        COOKIE_COUNTER.fetch_add(1, Ordering::SeqCst)
    );
    let provider_session = ProviderSession {
        session_id: format!("provider-session-{cookie_token}"),
        external_user_id: ExternalUserId::parse(user).expect("valid external id"),
    };
    let token_hash = SessionTokenKey::from_secret(PASSWORD).token_hash(&cookie_token);
    let session_ref =
        SessionRefKey::from_secret(PASSWORD).session_ref(&provider_session.session_id);
    create_sign_in(
        pool,
        &provider_session,
        &ProviderProfile {
            email: email.to_owned(),
            email_verified: verified,
            display_name: Some("Test Person".to_owned()),
        },
        NewBrowserSession {
            token_hash: &token_hash,
            sealed_provider_session: &OpaqueSecret::new("sealed test provider session"),
            session_ref: &session_ref,
            expires_at: Utc::now() + Duration::days(30),
        },
        AUDIT,
    )
    .await
    .expect("sign-in succeeds")
}

async fn user(pool: &PgPool, user: &str, email: &str) -> UserId {
    sign_in(pool, user, email).await;
    let id: Uuid = sqlx::query_scalar("SELECT id FROM users WHERE workos_user_id = $1")
        .bind(user)
        .fetch_one(pool)
        .await
        .expect("user exists");
    UserId::new(id)
}

async fn personal_workspace(pool: &PgPool, user: UserId) -> WorkspaceId {
    let id: Uuid = sqlx::query_scalar(
        "SELECT w.id FROM workspaces w JOIN workspace_memberships m ON m.workspace_id = w.id
         WHERE m.user_id = $1 AND w.kind = 'personal'",
    )
    .bind(user.as_uuid())
    .fetch_one(pool)
    .await
    .expect("personal workspace");
    WorkspaceId::new(id)
}

async fn scalar(pool: &PgPool, sql: &'static str, bind: Uuid) -> i64 {
    sqlx::query_scalar(sql)
        .bind(bind)
        .fetch_one(pool)
        .await
        .unwrap()
}

async fn events(pool: &PgPool, event: &str) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM admin_audit_events WHERE event_type = $1")
        .bind(event)
        .fetch_one(pool)
        .await
        .unwrap()
}

async fn invitation_status(pool: &PgPool, id: Uuid) -> String {
    sqlx::query_scalar("SELECT status FROM workspace_invitations WHERE id = $1")
        .bind(id)
        .fetch_one(pool)
        .await
        .unwrap()
}

fn in_a_day() -> chrono::DateTime<Utc> {
    Utc::now() + Duration::days(1)
}

async fn expire_now(pool: &PgPool, id: Uuid) {
    sqlx::query(
        "UPDATE workspace_invitations SET expires_at = now() - interval '1 minute' WHERE id = $1",
    )
    .bind(id)
    .execute(pool)
    .await
    .unwrap();
}

#[tokio::test]
async fn first_sign_in_with_a_pending_invitation_joins_and_creates_no_personal_workspace() {
    let Some(pool) = database().await else { return };
    let owner = user(&pool, "owner", "owner@example.org").await;
    let workspace = personal_workspace(&pool, owner).await;
    let invitation = invite_to_workspace(
        &pool,
        owner,
        workspace,
        "  New.Person@Example.ORG ",
        WorkspaceRole::Admin,
        in_a_day(),
        AUDIT,
    )
    .await
    .unwrap();
    assert_eq!(invitation.email, "new.person@example.org");

    sign_in(&pool, "invitee", "NEW.person@example.org").await;
    let invitee = user(&pool, "invitee", "NEW.person@example.org").await;
    let row: (String, String, String) = sqlx::query_as(
        "SELECT role, status, grant_source FROM workspace_memberships
         WHERE workspace_id = $1 AND user_id = $2",
    )
    .bind(workspace.as_uuid())
    .bind(invitee.as_uuid())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(row, ("admin".into(), "active".into(), "local".into()));
    assert_eq!(
        scalar(
            &pool,
            "SELECT count(*) FROM workspace_memberships WHERE user_id = $1",
            invitee.as_uuid()
        )
        .await,
        1,
        "only the inviting workspace"
    );
    assert_eq!(
        scalar(
            &pool,
            "SELECT count(*) FROM workspaces w JOIN workspace_memberships m ON m.workspace_id = w.id
             WHERE m.user_id = $1 AND w.kind = 'personal' AND m.role = 'owner'",
            invitee.as_uuid()
        )
        .await,
        0,
        "no personal workspace"
    );
    assert_eq!(invitation_status(&pool, invitation.id).await, "accepted");
    assert_eq!(events(&pool, "invitation.created").await, 1);
    assert_eq!(events(&pool, "invitation.accepted").await, 1);

    // The audit holds no email.
    let leaks: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM admin_audit_events WHERE row_to_json(admin_audit_events)::text ILIKE '%example.org%'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(leaks, 0);

    // A retried callback onboards nothing twice.
    sign_in(&pool, "invitee", "new.person@example.org").await;
    assert_eq!(
        scalar(
            &pool,
            "SELECT count(*) FROM workspace_memberships WHERE user_id = $1",
            invitee.as_uuid()
        )
        .await,
        1
    );
}

#[tokio::test]
async fn an_expired_invitation_is_refused_marked_expired_and_audited() {
    let Some(pool) = database().await else { return };
    let owner = user(&pool, "owner", "owner@example.org").await;
    let workspace = personal_workspace(&pool, owner).await;

    // Expired at sign-in: the invitee gets a personal workspace instead.
    let stale = invite_to_workspace(
        &pool,
        owner,
        workspace,
        "late@example.org",
        WorkspaceRole::Member,
        in_a_day(),
        AUDIT,
    )
    .await
    .unwrap();
    expire_now(&pool, stale.id).await;
    let late = user(&pool, "late", "late@example.org").await;
    assert_eq!(invitation_status(&pool, stale.id).await, "expired");
    assert_eq!(events(&pool, "invitation.expired").await, 1);
    personal_workspace(&pool, late).await;

    // Expired at explicit acceptance.
    let other = user(&pool, "other", "other@example.org").await;
    let second = invite_to_workspace(
        &pool,
        owner,
        workspace,
        "other@example.org",
        WorkspaceRole::Member,
        in_a_day(),
        AUDIT,
    )
    .await
    .unwrap();
    expire_now(&pool, second.id).await;
    let error = accept_invitation(&pool, other, second.id, AUDIT)
        .await
        .unwrap_err();
    assert!(
        matches!(error, WorkspaceError::InvitationExpired),
        "{error:?}"
    );
    assert_eq!(invitation_status(&pool, second.id).await, "expired");
    assert_eq!(events(&pool, "invitation.expired").await, 2);
    assert_eq!(
        scalar(
            &pool,
            "SELECT count(*) FROM workspace_memberships WHERE user_id = $1",
            other.as_uuid()
        )
        .await,
        1,
        "only the personal workspace"
    );

    // A stale pending invitation does not block inviting the address again.
    let third = invite_to_workspace(
        &pool,
        owner,
        workspace,
        "again@example.org",
        WorkspaceRole::Member,
        in_a_day(),
        AUDIT,
    )
    .await
    .unwrap();
    expire_now(&pool, third.id).await;
    invite_to_workspace(
        &pool,
        owner,
        workspace,
        "again@example.org",
        WorkspaceRole::Member,
        in_a_day(),
        AUDIT,
    )
    .await
    .unwrap();
    assert_eq!(invitation_status(&pool, third.id).await, "expired");
    assert_eq!(events(&pool, "invitation.expired").await, 3);
}

#[tokio::test]
async fn a_revoked_invitation_cannot_be_used_and_only_admins_revoke() {
    let Some(pool) = database().await else { return };
    let owner = user(&pool, "owner", "owner@example.org").await;
    let workspace = personal_workspace(&pool, owner).await;
    let member = user(&pool, "member", "member@example.org").await;
    add_workspace_member(
        &pool,
        owner,
        workspace,
        member,
        WorkspaceRole::Member,
        AUDIT,
    )
    .await
    .unwrap();
    let invitation = invite_to_workspace(
        &pool,
        owner,
        workspace,
        "gone@example.org",
        WorkspaceRole::Member,
        in_a_day(),
        AUDIT,
    )
    .await
    .unwrap();

    let error = revoke_invitation(&pool, member, invitation.id, AUDIT)
        .await
        .unwrap_err();
    assert!(matches!(error, WorkspaceError::NotPermitted), "{error:?}");
    revoke_invitation(&pool, owner, invitation.id, AUDIT)
        .await
        .unwrap();
    assert_eq!(invitation_status(&pool, invitation.id).await, "revoked");
    assert_eq!(events(&pool, "invitation.revoked").await, 1);
    let error = revoke_invitation(&pool, owner, invitation.id, AUDIT)
        .await
        .unwrap_err();
    assert!(
        matches!(error, WorkspaceError::InvitationUnavailable),
        "{error:?}"
    );

    let gone = user(&pool, "gone", "gone@example.org").await;
    personal_workspace(&pool, gone).await;
    let error = accept_invitation(&pool, gone, invitation.id, AUDIT)
        .await
        .unwrap_err();
    assert!(
        matches!(error, WorkspaceError::InvitationUnavailable),
        "{error:?}"
    );
    assert_eq!(events(&pool, "invitation.accepted").await, 0);
}

#[tokio::test]
async fn an_invitation_is_single_use_under_concurrent_acceptance() {
    let Some(pool) = database().await else { return };
    let owner = user(&pool, "owner", "owner@example.org").await;
    let workspace = personal_workspace(&pool, owner).await;
    // Distinct provider identities may share an address; only one may use the invitation.
    let mut users = Vec::new();
    for index in 0..6 {
        users.push(user(&pool, &format!("twin_{index}"), "twin@example.org").await);
    }
    let invitation = invite_to_workspace(
        &pool,
        owner,
        workspace,
        "twin@example.org",
        WorkspaceRole::Member,
        in_a_day(),
        AUDIT,
    )
    .await
    .unwrap();
    let attempts = users.iter().map(|user| {
        let pool = pool.clone();
        let user = *user;
        let id = invitation.id;
        tokio::spawn(async move { accept_invitation(&pool, user, id, AUDIT).await })
    });
    let handles: Vec<_> = attempts.collect();
    let mut results = Vec::new();
    for handle in handles {
        results.push(handle.await);
    }
    let wins = results
        .iter()
        .filter(|r| r.as_ref().unwrap().is_ok())
        .count();
    assert_eq!(wins, 1, "exactly one acceptance succeeds");
    for result in results {
        if let Err(error) = result.unwrap() {
            assert!(
                matches!(error, WorkspaceError::InvitationUnavailable),
                "{error:?}"
            );
        }
    }
    assert_eq!(
        scalar(&pool, "SELECT count(*) FROM workspace_memberships WHERE workspace_id = $1 AND role = 'member'", workspace.as_uuid()).await,
        1
    );
    assert_eq!(events(&pool, "invitation.accepted").await, 1);
}

#[tokio::test]
async fn the_inviter_cannot_grant_a_role_above_their_own() {
    let Some(pool) = database().await else { return };
    let owner = user(&pool, "owner", "owner@example.org").await;
    let workspace = personal_workspace(&pool, owner).await;
    let admin = user(&pool, "admin", "admin@example.org").await;
    let member = user(&pool, "member", "member@example.org").await;
    add_workspace_member(&pool, owner, workspace, admin, WorkspaceRole::Admin, AUDIT)
        .await
        .unwrap();
    add_workspace_member(
        &pool,
        owner,
        workspace,
        member,
        WorkspaceRole::Member,
        AUDIT,
    )
    .await
    .unwrap();

    let error = invite_to_workspace(
        &pool,
        admin,
        workspace,
        "x@example.org",
        WorkspaceRole::Owner,
        in_a_day(),
        AUDIT,
    )
    .await
    .unwrap_err();
    assert!(
        matches!(error, WorkspaceError::RoleAboveInviter),
        "{error:?}"
    );
    invite_to_workspace(
        &pool,
        admin,
        workspace,
        "x@example.org",
        WorkspaceRole::Admin,
        in_a_day(),
        AUDIT,
    )
    .await
    .unwrap();
    invite_to_workspace(
        &pool,
        owner,
        workspace,
        "y@example.org",
        WorkspaceRole::Owner,
        in_a_day(),
        AUDIT,
    )
    .await
    .unwrap();

    let error = invite_to_workspace(
        &pool,
        member,
        workspace,
        "z@example.org",
        WorkspaceRole::Member,
        in_a_day(),
        AUDIT,
    )
    .await
    .unwrap_err();
    assert!(matches!(error, WorkspaceError::NotPermitted), "{error:?}");
    let outsider = user(&pool, "outsider", "outsider@example.org").await;
    let error = invite_to_workspace(
        &pool,
        outsider,
        workspace,
        "z@example.org",
        WorkspaceRole::Member,
        in_a_day(),
        AUDIT,
    )
    .await
    .unwrap_err();
    assert!(matches!(error, WorkspaceError::NotPermitted), "{error:?}");
    set_membership_status(
        &pool,
        owner,
        workspace,
        admin,
        MembershipStatus::Deactivated,
        AUDIT,
    )
    .await
    .unwrap();
    let error = invite_to_workspace(
        &pool,
        admin,
        workspace,
        "z@example.org",
        WorkspaceRole::Member,
        in_a_day(),
        AUDIT,
    )
    .await
    .unwrap_err();
    assert!(
        matches!(error, WorkspaceError::NotPermitted),
        "deactivated admin: {error:?}"
    );

    let error = invite_to_workspace(
        &pool,
        owner,
        workspace,
        "x@example.org",
        WorkspaceRole::Member,
        in_a_day(),
        AUDIT,
    )
    .await
    .unwrap_err();
    assert!(
        matches!(error, WorkspaceError::InvitationAlreadyPending),
        "{error:?}"
    );
    let error = invite_to_workspace(
        &pool,
        owner,
        workspace,
        "member@example.org",
        WorkspaceRole::Member,
        in_a_day(),
        AUDIT,
    )
    .await
    .unwrap_err();
    assert!(matches!(error, WorkspaceError::AlreadyMember), "{error:?}");
    let error = invite_to_workspace(
        &pool,
        owner,
        workspace,
        "w@example.org",
        WorkspaceRole::Member,
        Utc::now() - Duration::hours(1),
        AUDIT,
    )
    .await
    .unwrap_err();
    assert!(
        matches!(error, WorkspaceError::InvalidInvitation),
        "{error:?}"
    );
    assert_eq!(
        events(&pool, "invitation.created").await,
        2,
        "refused invitations leave no audit row"
    );
}

#[tokio::test]
async fn an_invitation_binds_only_to_its_own_address() {
    let Some(pool) = database().await else { return };
    let owner = user(&pool, "owner", "owner@example.org").await;
    let workspace = personal_workspace(&pool, owner).await;
    let invitation = invite_to_workspace(
        &pool,
        owner,
        workspace,
        "right@example.org",
        WorkspaceRole::Member,
        in_a_day(),
        AUDIT,
    )
    .await
    .unwrap();

    let wrong = user(&pool, "wrong", "wrong@example.org").await;
    let error = accept_invitation(&pool, wrong, invitation.id, AUDIT)
        .await
        .unwrap_err();
    assert!(
        matches!(error, WorkspaceError::InvitationEmailMismatch),
        "{error:?}"
    );
    personal_workspace(&pool, wrong).await;
    assert_eq!(
        invitation_status(&pool, invitation.id).await,
        "pending",
        "a mismatch consumes nothing"
    );

    let right = user(&pool, "right", "right@example.org").await;
    personal_workspace(&pool, right).await;
    assert_eq!(invitation_status(&pool, invitation.id).await, "accepted");
}

#[tokio::test]
async fn the_oldest_matching_invitation_wins_at_first_sign_in() {
    let Some(pool) = database().await else { return };
    let first_owner = user(&pool, "owner_1", "owner1@example.org").await;
    let second_owner = user(&pool, "owner_2", "owner2@example.org").await;
    let first = personal_workspace(&pool, first_owner).await;
    let second = personal_workspace(&pool, second_owner).await;
    let older = invite_to_workspace(
        &pool,
        first_owner,
        first,
        "both@example.org",
        WorkspaceRole::Member,
        in_a_day(),
        AUDIT,
    )
    .await
    .unwrap();
    let newer = invite_to_workspace(
        &pool,
        second_owner,
        second,
        "both@example.org",
        WorkspaceRole::Member,
        in_a_day(),
        AUDIT,
    )
    .await
    .unwrap();

    let both = user(&pool, "both", "both@example.org").await;
    assert_eq!(invitation_status(&pool, older.id).await, "accepted");
    assert_eq!(invitation_status(&pool, newer.id).await, "pending");
    assert_eq!(
        scalar(
            &pool,
            "SELECT count(*) FROM workspace_memberships WHERE user_id = $1",
            both.as_uuid()
        )
        .await,
        1
    );
}

#[tokio::test]
async fn an_unverified_email_joins_no_workspace_until_the_provider_verifies_it() {
    let Some(pool) = database().await else { return };
    let owner = user(&pool, "owner", "owner@example.org").await;
    let workspace = personal_workspace(&pool, owner).await;
    let invitation = invite_to_workspace(
        &pool,
        owner,
        workspace,
        "claim@example.org",
        WorkspaceRole::Admin,
        in_a_day(),
        AUDIT,
    )
    .await
    .unwrap();

    // First sign-in claims the invited address without verification: no join.
    sign_in_verified(&pool, "claimant", "claim@example.org", false).await;
    let claimant: Uuid =
        sqlx::query_scalar("SELECT id FROM users WHERE workos_user_id = 'claimant'")
            .fetch_one(&pool)
            .await
            .unwrap();
    let claimant = UserId::new(claimant);
    personal_workspace(&pool, claimant).await;
    assert_eq!(invitation_status(&pool, invitation.id).await, "pending");
    let error = accept_invitation(&pool, claimant, invitation.id, AUDIT)
        .await
        .unwrap_err();
    assert!(
        matches!(error, WorkspaceError::InvitationEmailUnverified),
        "{error:?}"
    );
    assert_eq!(
        invitation_status(&pool, invitation.id).await,
        "pending",
        "nothing is consumed"
    );

    // Once the provider verifies the address, the same invitation can be accepted.
    sign_in_verified(&pool, "claimant", "claim@example.org", true).await;
    accept_invitation(&pool, claimant, invitation.id, AUDIT)
        .await
        .unwrap();
    assert_eq!(invitation_status(&pool, invitation.id).await, "accepted");
}

#[tokio::test]
async fn invitation_events_name_their_invitation() {
    let Some(pool) = database().await else { return };
    let owner = user(&pool, "owner", "owner@example.org").await;
    let workspace = personal_workspace(&pool, owner).await;
    let one = invite_to_workspace(
        &pool,
        owner,
        workspace,
        "a@example.org",
        WorkspaceRole::Member,
        in_a_day(),
        AUDIT,
    )
    .await
    .unwrap();
    let two = invite_to_workspace(
        &pool,
        owner,
        workspace,
        "b@example.org",
        WorkspaceRole::Member,
        in_a_day(),
        AUDIT,
    )
    .await
    .unwrap();
    revoke_invitation(&pool, owner, two.id, AUDIT)
        .await
        .unwrap();
    let ids: Vec<String> = sqlx::query_scalar(
        "SELECT metadata->>'invitation_id' FROM admin_audit_events
         WHERE event_type LIKE 'invitation.%' ORDER BY event_type, metadata->>'invitation_id'",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    let mut expected = vec![one.id.to_string(), two.id.to_string(), two.id.to_string()];
    expected.sort();
    let mut created_revoked = ids.clone();
    created_revoked.sort();
    assert_eq!(created_revoked, expected);
}
