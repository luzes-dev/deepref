#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]
//! Users, sign-in provisioning, deactivation and the append-only audit trail.
use std::sync::{
    Mutex,
    atomic::{AtomicUsize, Ordering},
};

use deepref_app::identity::{
    AuditRequest, Deactivation, NewBrowserSession, RefreshLease, SessionUpdate, SignIn,
    apply_provider_user_deleted, claim_refresh_lease, deactivate_user, find_active_session,
    find_user_by_external_id, record_logout, release_refresh_lease, sign_in as create_sign_in,
    update_active_session,
};
use deepref_application::identity::{
    IdentityError, IdentityFuture, IdentityProvider, LoginStart, LoginState, OpaqueSecret,
    ProviderLogin, ProviderProfile, ProviderSession, RefreshOutcome, SessionRefKey, SessionStatus,
    SessionTokenKey,
};
use deepref_domain::{ExternalUserId, UserStatus};
use sqlx::{PgPool, Row};

async fn database() -> Option<PgPool> {
    deepref_app::test_isolation::isolated_pool_from_env("DATABASE_URL", 8).await
}

fn external(id: &str) -> ExternalUserId {
    ExternalUserId::parse(id).expect("valid external id")
}

fn session(user: &str, session_id: &str) -> ProviderSession {
    ProviderSession {
        session_id: session_id.to_owned(),
        external_user_id: external(user),
    }
}

fn profile(email: &str, name: Option<&str>) -> ProviderProfile {
    ProviderProfile {
        email: email.to_owned(),
        email_verified: true,
        display_name: name.map(str::to_owned),
    }
}

async fn count(pool: &PgPool, sql: &'static str) -> i64 {
    sqlx::query_scalar(sql)
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

static COOKIE_COUNTER: AtomicUsize = AtomicUsize::new(1);

async fn sign_in(
    pool: &PgPool,
    provider_session: &ProviderSession,
    profile: &ProviderProfile,
    audit: AuditRequest<'_>,
) -> Result<SignIn, deepref_app::identity::IdentityStoreError> {
    const PASSWORD: &str = "a-cookie-password-of-at-least-32-characters";
    let cookie_token = format!(
        "test-session-{}",
        COOKIE_COUNTER.fetch_add(1, Ordering::SeqCst)
    );
    let token_hash = SessionTokenKey::from_secret(PASSWORD).token_hash(&cookie_token);
    let session_ref =
        SessionRefKey::from_secret(PASSWORD).session_ref(&provider_session.session_id);
    let sealed = OpaqueSecret::new("sealed test provider session");
    let audit = AuditRequest {
        request_id: audit.request_id,
        session_ref: Some(&session_ref),
    };
    create_sign_in(
        pool,
        provider_session,
        profile,
        NewBrowserSession {
            token_hash: &token_hash,
            sealed_provider_session: &sealed,
            session_ref: &session_ref,
            expires_at: chrono::Utc::now() + chrono::Duration::days(30),
        },
        audit,
    )
    .await
}

#[derive(Default)]
struct FakeProvider {
    sessions: Vec<String>,
    revoked: Mutex<Vec<String>>,
}

impl IdentityProvider for FakeProvider {
    fn begin_login(&self, _: &str) -> Result<LoginStart, IdentityError> {
        Err(IdentityError::Unavailable)
    }
    fn complete_login<'a>(
        &'a self,
        _: &'a str,
        _: &'a OpaqueSecret,
    ) -> IdentityFuture<'a, ProviderLogin> {
        Box::pin(async { Err(IdentityError::Unavailable) })
    }
    fn inspect_session<'a>(&'a self, _: &'a OpaqueSecret) -> IdentityFuture<'a, SessionStatus> {
        Box::pin(async { Ok(SessionStatus::Invalid) })
    }
    fn refresh_session<'a>(&'a self, _: &'a OpaqueSecret) -> IdentityFuture<'a, RefreshOutcome> {
        Box::pin(async { Ok(RefreshOutcome::Rejected) })
    }
    fn revoke_session<'a>(&'a self, session_id: &'a str) -> IdentityFuture<'a, ()> {
        Box::pin(async move {
            self.revoked.lock().unwrap().push(session_id.to_owned());
            Ok(())
        })
    }
    fn list_session_ids<'a>(&'a self, _: &'a ExternalUserId) -> IdentityFuture<'a, Vec<String>> {
        let sessions = self.sessions.clone();
        Box::pin(async move { Ok(sessions) })
    }
    fn seal_login_state(&self, _: &LoginState) -> Result<OpaqueSecret, IdentityError> {
        Err(IdentityError::Unavailable)
    }
    fn open_login_state(&self, _: &str) -> Option<LoginState> {
        None
    }
}

#[tokio::test]
async fn one_provider_identity_maps_to_one_user_id() {
    let Some(pool) = database().await else { return };
    let first = sign_in(
        &pool,
        &session("user_A", "sess_1"),
        &profile("a@example.org", Some("Ada")),
        AuditRequest::default(),
    )
    .await
    .unwrap();
    let second = sign_in(
        &pool,
        &session("user_A", "sess_2"),
        &profile("a@example.org", Some("Ada")),
        AuditRequest::default(),
    )
    .await
    .unwrap();
    let (
        SignIn::Active {
            user: a,
            provisioned: true,
        },
        SignIn::Active {
            user: b,
            provisioned: false,
        },
    ) = (first, second)
    else {
        panic!("first sign-in provisions, second does not");
    };
    assert_eq!(a.id, b.id);
    assert_eq!(count(&pool, "SELECT count(*) FROM users").await, 1);
}

#[tokio::test]
async fn the_same_email_never_links_two_provider_identities() {
    let Some(pool) = database().await else { return };
    for id in ["user_A", "user_B"] {
        sign_in(
            &pool,
            &session(id, "sess"),
            &profile("shared@example.org", None),
            AuditRequest::default(),
        )
        .await
        .unwrap();
    }
    assert_eq!(count(&pool, "SELECT count(*) FROM users").await, 2);
}

#[tokio::test]
async fn concurrent_duplicate_callbacks_provision_once() {
    let Some(pool) = database().await else { return };
    let results = futures_join(&pool).await;
    assert_eq!(
        results.iter().filter(|provisioned| **provisioned).count(),
        1,
        "exactly one caller provisions"
    );
    assert_eq!(count(&pool, "SELECT count(*) FROM users").await, 1);
    assert_eq!(events(&pool, "user.provisioned").await, 1);
    assert_eq!(events(&pool, "auth.login").await, 4);
}

async fn futures_join(pool: &PgPool) -> Vec<bool> {
    let mut tasks = Vec::new();
    for n in 0..4 {
        let pool = pool.clone();
        tasks.push(tokio::spawn(async move {
            sign_in(
                &pool,
                &session("user_A", &format!("sess_{n}")),
                &profile("a@example.org", None),
                AuditRequest::default(),
            )
            .await
            .unwrap()
        }));
    }
    let mut out = Vec::new();
    for task in tasks {
        match task.await.unwrap() {
            SignIn::Active { provisioned, .. } => out.push(provisioned),
            SignIn::Deactivated => panic!("unexpected deactivation"),
        }
    }
    out
}

#[tokio::test]
async fn profile_changes_follow_the_provider_without_touching_status() {
    let Some(pool) = database().await else { return };
    sign_in(
        &pool,
        &session("user_A", "s"),
        &profile("old@example.org", Some("Old")),
        AuditRequest::default(),
    )
    .await
    .unwrap();
    sign_in(
        &pool,
        &session("user_A", "s"),
        &profile("new@example.org", Some("New")),
        AuditRequest::default(),
    )
    .await
    .unwrap();
    let user = find_user_by_external_id(&pool, &external("user_A"))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(user.email, "new@example.org");
    assert_eq!(user.display_name.as_deref(), Some("New"));
    assert_eq!(user.status, UserStatus::Active);
}

#[tokio::test]
async fn provisioning_and_its_audit_row_commit_or_roll_back_together() {
    let Some(pool) = database().await else { return };
    let too_long = "x".repeat(201);
    let failed = sign_in(
        &pool,
        &session("user_A", "s"),
        &profile("a@example.org", None),
        AuditRequest {
            request_id: Some(&too_long),
            session_ref: None,
        },
    )
    .await;
    assert!(failed.is_err(), "the audit constraint rejects the row");
    assert_eq!(count(&pool, "SELECT count(*) FROM users").await, 0);
}

#[tokio::test]
async fn audit_rows_hold_a_keyed_hash_and_no_secrets() {
    let Some(pool) = database().await else { return };
    let key = SessionRefKey::from_secret("a-cookie-password-of-at-least-32-characters");
    let raw_session_id = "sess_RAW_IDENTIFIER_01";
    let session_ref = key.session_ref(raw_session_id);
    sign_in(
        &pool,
        &session("user_A", raw_session_id),
        &profile("secret.person@example.org", Some("Secret Person")),
        AuditRequest {
            request_id: Some("0b0e0d0c-0000-4000-8000-000000000001"),
            session_ref: Some(&session_ref),
        },
    )
    .await
    .unwrap();
    let user = find_user_by_external_id(&pool, &external("user_A"))
        .await
        .unwrap()
        .unwrap();
    let token_hash: Vec<u8> = sqlx::query_scalar(
        "SELECT session_token_hash FROM user_sessions WHERE user_id = $1 ORDER BY created_at DESC LIMIT 1",
    )
    .bind(user.id.as_uuid())
    .fetch_one(&pool)
    .await
    .unwrap();
    record_logout(
        &pool,
        &token_hash,
        AuditRequest {
            request_id: Some("0b0e0d0c-0000-4000-8000-000000000002"),
            session_ref: Some(&session_ref),
        },
    )
    .await
    .unwrap();

    let rows: Vec<String> =
        sqlx::query_scalar("SELECT row_to_json(a)::text FROM admin_audit_events a")
            .fetch_all(&pool)
            .await
            .unwrap();
    assert_eq!(
        rows.len(),
        6,
        "provisioned, personal workspace and owner membership, login, logout, session revoked"
    );
    for row in &rows {
        for secret in [raw_session_id, "secret.person", "Secret Person", "user_A"] {
            assert!(!row.contains(secret), "audit row leaks {secret}: {row}");
        }
    }
    let stored: Vec<u8> = sqlx::query_scalar(
        "SELECT session_ref FROM admin_audit_events WHERE event_type = 'auth.logout'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(stored, session_ref);
    assert_ne!(stored, raw_session_id.as_bytes());
    let principal: (String, Option<uuid::Uuid>) = sqlx::query_as(
        "SELECT principal_kind, principal_user_id FROM admin_audit_events WHERE event_type = 'auth.logout'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(principal, ("user".to_owned(), Some(user.id.as_uuid())));
}

#[tokio::test]
async fn audit_events_cannot_be_updated_or_deleted() {
    let Some(pool) = database().await else { return };
    sign_in(
        &pool,
        &session("user_A", "s"),
        &profile("a@example.org", None),
        AuditRequest::default(),
    )
    .await
    .unwrap();
    let update = sqlx::query("UPDATE admin_audit_events SET request_id = 'tampered'")
        .execute(&pool)
        .await;
    let delete = sqlx::query("DELETE FROM admin_audit_events")
        .execute(&pool)
        .await;
    let truncate = sqlx::query("TRUNCATE admin_audit_events")
        .execute(&pool)
        .await;
    for (name, result) in [
        ("update", update),
        ("delete", delete),
        ("truncate", truncate),
    ] {
        let error = result.expect_err(name);
        assert!(error.to_string().contains("append-only"), "{name}: {error}");
    }
    assert_eq!(events(&pool, "auth.login").await, 1);
}

#[tokio::test]
async fn deactivation_is_compare_and_set_replay_safe_and_audited_once() {
    let Some(pool) = database().await else { return };
    sign_in(
        &pool,
        &session("user_A", "s"),
        &profile("a@example.org", Some("Ada")),
        AuditRequest::default(),
    )
    .await
    .unwrap();
    let user = find_user_by_external_id(&pool, &external("user_A"))
        .await
        .unwrap()
        .unwrap();

    let first = deactivate_user(&pool, &external("user_A"), "receipt-1")
        .await
        .unwrap();
    let replay = deactivate_user(&pool, &external("user_A"), "receipt-2")
        .await
        .unwrap();
    assert_eq!(first, Deactivation::Deactivated(user.id));
    assert_eq!(replay, Deactivation::AlreadyDeactivated(user.id));
    assert_eq!(events(&pool, "user.deactivated").await, 1);
    let row = sqlx::query(
        "SELECT principal_kind, principal_user_id, target_user_id, request_id
         FROM admin_audit_events WHERE event_type = 'user.deactivated'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(row.get::<String, _>("principal_kind"), "system");
    assert_eq!(row.get::<Option<uuid::Uuid>, _>("principal_user_id"), None);
    assert_eq!(
        row.get::<uuid::Uuid, _>("target_user_id"),
        user.id.as_uuid()
    );
    assert_eq!(row.get::<String, _>("request_id"), "receipt-1");

    let kept = find_user_by_external_id(&pool, &external("user_A"))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(kept.id, user.id, "the UserId survives deactivation");
    assert_eq!(kept.status, UserStatus::Deactivated);
    assert_eq!(kept.email, "a@example.org", "PII is not cleared");
    assert_eq!(
        count(
            &pool,
            "SELECT count(*) FROM users WHERE deleted_at IS NOT NULL"
        )
        .await,
        0
    );
    let earlier_rows: i64 =
        sqlx::query_scalar("SELECT count(*) FROM admin_audit_events WHERE target_user_id = $1")
            .bind(user.id.as_uuid())
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        earlier_rows, 5,
        "provisioned, personal membership, login, session revoke and deactivation keep the UserId"
    );
}

#[tokio::test]
async fn a_later_profile_update_does_not_reactivate_a_deactivated_user() {
    let Some(pool) = database().await else { return };
    sign_in(
        &pool,
        &session("user_A", "s"),
        &profile("a@example.org", None),
        AuditRequest::default(),
    )
    .await
    .unwrap();
    deactivate_user(&pool, &external("user_A"), "receipt")
        .await
        .unwrap();

    let again = sign_in(
        &pool,
        &session("user_A", "s2"),
        &profile("changed@example.org", Some("Changed")),
        AuditRequest::default(),
    )
    .await
    .unwrap();
    assert_eq!(again, SignIn::Deactivated);
    let user = find_user_by_external_id(&pool, &external("user_A"))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(user.status, UserStatus::Deactivated);
    assert_eq!(
        events(&pool, "auth.login").await,
        1,
        "no login for a deactivated user"
    );
}

#[tokio::test]
async fn deactivating_an_unknown_provider_user_changes_nothing() {
    let Some(pool) = database().await else { return };
    let outcome = deactivate_user(&pool, &external("user_nobody"), "receipt")
        .await
        .unwrap();
    assert_eq!(outcome, Deactivation::UnknownUser);
    assert_eq!(
        count(&pool, "SELECT count(*) FROM admin_audit_events").await,
        0
    );
}

#[tokio::test]
async fn user_deleted_delivery_revokes_provider_sessions_once() {
    let Some(pool) = database().await else { return };
    sign_in(
        &pool,
        &session("user_A", "s"),
        &profile("a@example.org", None),
        AuditRequest::default(),
    )
    .await
    .unwrap();
    let provider = FakeProvider {
        sessions: vec!["sess_a".to_owned(), "sess_b".to_owned()],
        ..Default::default()
    };
    let first = apply_provider_user_deleted(&pool, &provider, &external("user_A"), "receipt-1")
        .await
        .unwrap();
    assert!(matches!(first, Deactivation::Deactivated(_)));
    apply_provider_user_deleted(&pool, &provider, &external("user_A"), "receipt-1")
        .await
        .unwrap();

    assert_eq!(*provider.revoked.lock().unwrap(), ["sess_a", "sess_b"]);
    assert_eq!(events(&pool, "auth.session_revoked").await, 1);
    assert_eq!(events(&pool, "user.deactivated").await, 1);
    let refs: Vec<Vec<u8>> = sqlx::query_scalar(
        "SELECT session_ref FROM admin_audit_events WHERE event_type = 'auth.session_revoked' ORDER BY session_ref",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    assert!(
        refs.iter()
            .all(|r| r.len() == 32 && r != b"sess_a" && r != b"sess_b")
    );
}

async fn only_session(pool: &PgPool) -> (uuid::Uuid, Vec<u8>) {
    sqlx::query_as("SELECT id, session_token_hash FROM user_sessions")
        .fetch_one(pool)
        .await
        .unwrap()
}

#[tokio::test]
async fn a_stale_refresh_cannot_overwrite_a_newer_sealed_session() {
    let Some(pool) = database().await else { return };
    sign_in(
        &pool,
        &session("user_A", "sess_1"),
        &profile("a@example.org", None),
        AuditRequest::default(),
    )
    .await
    .unwrap();
    let (id, token_hash) = only_session(&pool).await;
    let base = find_active_session(&pool, &token_hash)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(base.version, 0);
    let (winner, stale) = (
        OpaqueSecret::new("sealed rotated by the winner"),
        OpaqueSecret::new("sealed rotated by a stalled replica"),
    );

    // Two replicas race for the refresh lease from the same base version.
    let (first, second) = (uuid::Uuid::new_v4(), uuid::Uuid::new_v4());
    let ttl = std::time::Duration::from_secs(30);
    let (a, b) = tokio::join!(
        claim_refresh_lease(&pool, id, base.version, first, ttl),
        claim_refresh_lease(&pool, id, base.version, second, ttl),
    );
    let claims = [a.unwrap(), b.unwrap()];
    assert_eq!(
        claims
            .iter()
            .filter(|c| **c == RefreshLease::Claimed)
            .count(),
        1,
        "{claims:?}"
    );
    assert_eq!(
        claims.iter().filter(|c| **c == RefreshLease::Held).count(),
        1,
        "{claims:?}"
    );
    let (holder, loser) = if claims[0] == RefreshLease::Claimed {
        (first, second)
    } else {
        (second, first)
    };

    // Without the lease a persist changes nothing.
    assert_eq!(
        update_active_session(&pool, id, base.version, loser, &stale, &base.session_ref)
            .await
            .unwrap(),
        SessionUpdate::Conflict
    );
    assert_eq!(
        update_active_session(&pool, id, base.version, holder, &winner, &base.session_ref)
            .await
            .unwrap(),
        SessionUpdate::Updated
    );
    let stored = find_active_session(&pool, &token_hash)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(stored.version, 1);
    let kept = stored.sealed_provider_session.expose().to_owned();
    assert_eq!(kept, winner.expose());

    // A later stall from the same old base is superseded, and loses on persist.
    assert_eq!(
        claim_refresh_lease(&pool, id, base.version, loser, ttl)
            .await
            .unwrap(),
        RefreshLease::Superseded
    );
    let late = update_active_session(&pool, id, base.version, holder, &stale, &base.session_ref)
        .await
        .unwrap();
    assert_eq!(late, SessionUpdate::Conflict);
    let after = find_active_session(&pool, &token_hash)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(after.sealed_provider_session.expose(), kept);

    // Persisting from the current version under a fresh lease advances it.
    let next = OpaqueSecret::new("sealed rotated again");
    assert_eq!(
        claim_refresh_lease(&pool, id, after.version, loser, ttl)
            .await
            .unwrap(),
        RefreshLease::Claimed
    );
    assert_eq!(
        update_active_session(&pool, id, after.version, loser, &next, &after.session_ref)
            .await
            .unwrap(),
        SessionUpdate::Updated
    );

    // A revoked row is Gone, not a conflict to retry.
    record_logout(&pool, &token_hash, AuditRequest::default())
        .await
        .unwrap();
    assert_eq!(
        update_active_session(
            &pool,
            id,
            after.version + 1,
            loser,
            &next,
            &after.session_ref
        )
        .await
        .unwrap(),
        SessionUpdate::Gone
    );
}

#[tokio::test]
async fn a_refresh_lease_is_released_or_expires_and_never_outlives_a_revocation() {
    let Some(pool) = database().await else { return };
    sign_in(
        &pool,
        &session("user_A", "sess_1"),
        &profile("a@example.org", None),
        AuditRequest::default(),
    )
    .await
    .unwrap();
    let (id, token_hash) = only_session(&pool).await;
    let (a, b) = (uuid::Uuid::new_v4(), uuid::Uuid::new_v4());
    let long = std::time::Duration::from_secs(60);
    assert_eq!(
        claim_refresh_lease(&pool, id, 0, a, long).await.unwrap(),
        RefreshLease::Claimed
    );
    assert_eq!(
        claim_refresh_lease(&pool, id, 0, b, long).await.unwrap(),
        RefreshLease::Held
    );
    // Only the holder can release; a stranger's release changes nothing.
    release_refresh_lease(&pool, id, b).await.unwrap();
    assert_eq!(
        claim_refresh_lease(&pool, id, 0, b, long).await.unwrap(),
        RefreshLease::Held
    );
    release_refresh_lease(&pool, id, a).await.unwrap();
    assert_eq!(
        claim_refresh_lease(&pool, id, 0, b, std::time::Duration::from_millis(200))
            .await
            .unwrap(),
        RefreshLease::Claimed
    );
    // An expired lease can be taken over, and the stalled holder then loses.
    tokio::time::sleep(std::time::Duration::from_millis(400)).await;
    assert_eq!(
        claim_refresh_lease(&pool, id, 0, a, long).await.unwrap(),
        RefreshLease::Claimed
    );
    let sealed = OpaqueSecret::new("sealed by the stalled holder");
    assert_eq!(
        update_active_session(&pool, id, 0, b, &sealed, &[0; 32])
            .await
            .unwrap(),
        SessionUpdate::Conflict
    );
    record_logout(&pool, &token_hash, AuditRequest::default())
        .await
        .unwrap();
    assert_eq!(
        claim_refresh_lease(&pool, id, 0, b, long).await.unwrap(),
        RefreshLease::Gone
    );
}

#[tokio::test]
async fn logout_revokes_by_cookie_digest_without_the_provider_and_audits_once() {
    let Some(pool) = database().await else { return };
    sign_in(
        &pool,
        &session("user_A", "sess_1"),
        &profile("a@example.org", None),
        AuditRequest::default(),
    )
    .await
    .unwrap();
    let (_, token_hash) = only_session(&pool).await;
    let revoked = record_logout(&pool, &token_hash, AuditRequest::default())
        .await
        .unwrap()
        .expect("the live session is revoked");
    assert_eq!(
        revoked.sealed_provider_session.expose(),
        "sealed test provider session"
    );
    assert_eq!(events(&pool, "auth.logout").await, 1);
    assert_eq!(events(&pool, "auth.session_revoked").await, 1);
    assert!(
        record_logout(&pool, &token_hash, AuditRequest::default())
            .await
            .unwrap()
            .is_none(),
        "a replay changes and records nothing"
    );
    assert_eq!(events(&pool, "auth.logout").await, 1);
}
