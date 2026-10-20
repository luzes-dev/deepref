#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]
//! Browser sign-in, sessions, logout, deactivation and the audit trail,
//! end to end through the router against PostgreSQL. The identity provider is
//! a fake; nothing reaches WorkOS.
use std::sync::{Arc, atomic::Ordering};

use axum::{
    Router,
    body::Body,
    http::{Method, Request, Response, StatusCode, header},
};
use deepref_app::identity::{
    SessionUpdate, claim_refresh_lease, deactivate_user, find_active_session,
    find_user_by_external_id, update_active_session,
};
use deepref_application::identity::{OpaqueSecret, SessionRefKey, SessionTokenKey};
use deepref_domain::ExternalUserId;
use sqlx::PgPool;
use tower::ServiceExt;

mod support;
use support::{
    COOKIE_PASSWORD, LOGIN_COOKIE, ORIGIN, SESSION_COOKIE, auth_app, auth_app_waiting,
    auth_app_with, auth_runtime, fake_identity::FakeIdentity,
};

async fn database() -> Option<PgPool> {
    deepref_app::test_isolation::isolated_pool_from_env("DATABASE_URL", 8).await
}

struct Harness {
    app: Router,
    pool: PgPool,
    fake: Arc<FakeIdentity>,
}

async fn harness() -> Option<Harness> {
    let pool = database().await?;
    let fake = Arc::new(FakeIdentity::default());
    Some(Harness {
        app: auth_app(pool.clone(), Arc::clone(&fake)),
        pool,
        fake,
    })
}

async fn json(response: Response<Body>) -> serde_json::Value {
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null)
}

fn set_cookies(response: &Response<Body>) -> Vec<String> {
    response
        .headers()
        .get_all(header::SET_COOKIE)
        .iter()
        .map(|v| v.to_str().unwrap().to_owned())
        .collect()
}

fn cookie_value(response: &Response<Body>, name: &str) -> Option<String> {
    set_cookies(response).iter().find_map(|cookie| {
        let (pair, _) = cookie.split_once(';')?;
        let (key, value) = pair.split_once('=')?;
        (key == name && !value.is_empty()).then(|| value.to_owned())
    })
}

fn get(path: &str, cookie: Option<(&str, &str)>) -> Request<Body> {
    let mut request = Request::builder().uri(path);
    if let Some((name, value)) = cookie {
        request = request.header(header::COOKIE, format!("{name}={value}"));
    }
    request.body(Body::empty()).unwrap()
}

fn post(path: &str, session: &str, origin: Option<&str>, body: &str) -> Request<Body> {
    let mut request = Request::builder()
        .method(Method::POST)
        .uri(path)
        .header(header::COOKIE, format!("{SESSION_COOKIE}={session}"))
        .header(header::CONTENT_TYPE, "application/json");
    if let Some(origin) = origin {
        request = request.header(header::ORIGIN, origin);
    }
    request.body(Body::from(body.to_owned())).unwrap()
}

/// Runs the whole browser flow and returns the opaque browser session token.
async fn sign_in(h: &Harness, user: &str, email: &str) -> String {
    let login = h
        .app
        .clone()
        .oneshot(get("/auth/login?return_to=/projects", None))
        .await
        .unwrap();
    assert_eq!(login.status(), StatusCode::SEE_OTHER);
    let login_cookie = cookie_value(&login, LOGIN_COOKIE).expect("login cookie");
    let location = login.headers()[header::LOCATION]
        .to_str()
        .unwrap()
        .to_owned();
    let state = location.split("state=").nth(1).unwrap().to_owned();
    let code = h.fake.issue_code(user, email, Some("Test Person"));
    let callback = h
        .app
        .clone()
        .oneshot(get(
            &format!("/auth/callback?code={code}&state={state}"),
            Some((LOGIN_COOKIE, &login_cookie)),
        ))
        .await
        .unwrap();
    assert_eq!(callback.status(), StatusCode::SEE_OTHER);
    assert_eq!(
        callback.headers()[header::LOCATION],
        format!("{ORIGIN}/projects")
    );
    cookie_value(&callback, SESSION_COOKIE).expect("session cookie")
}

async fn count(pool: &PgPool, sql: &'static str) -> i64 {
    sqlx::query_scalar(sql).fetch_one(pool).await.unwrap()
}

async fn user_id(pool: &PgPool, external: &str) -> uuid::Uuid {
    find_user_by_external_id(pool, &ExternalUserId::parse(external).unwrap())
        .await
        .unwrap()
        .unwrap()
        .id
        .as_uuid()
}

async fn replace_provider_session(h: &Harness, cookie_token: &str, sealed: &str) {
    let token_hash = SessionTokenKey::from_secret(COOKIE_PASSWORD).token_hash(cookie_token);
    let session_id = h.fake.provider_session_id(sealed);
    let session_ref = SessionRefKey::from_secret(COOKIE_PASSWORD).session_ref(&session_id);
    sqlx::query(
        "UPDATE user_sessions
         SET sealed_provider_session = $2, session_ref = $3
         WHERE session_token_hash = $1",
    )
    .bind(token_hash)
    .bind(sealed)
    .bind(session_ref)
    .execute(&h.pool)
    .await
    .unwrap();
}

#[tokio::test]
async fn sign_in_provisions_a_user_and_sets_a_hardened_session_cookie() {
    let Some(h) = harness().await else { return };
    let login = h
        .app
        .clone()
        .oneshot(get("/auth/login", None))
        .await
        .unwrap();
    assert_eq!(login.status(), StatusCode::SEE_OTHER);
    assert!(
        login.headers()[header::LOCATION]
            .to_str()
            .unwrap()
            .starts_with("https://auth.fake.example/authorize"),
        "redirects to the provider"
    );
    let login_cookie = cookie_value(&login, LOGIN_COOKIE).unwrap();
    let state = login.headers()[header::LOCATION]
        .to_str()
        .unwrap()
        .split("state=")
        .nth(1)
        .unwrap()
        .to_owned();
    let code = h.fake.issue_code("user_A", "a@example.org", None);
    let callback = h
        .app
        .clone()
        .oneshot(get(
            &format!("/auth/callback?code={code}&state={state}"),
            Some((LOGIN_COOKIE, &login_cookie)),
        ))
        .await
        .unwrap();

    let cookies = set_cookies(&callback);
    let session = cookies
        .iter()
        .find(|c| c.starts_with(SESSION_COOKIE))
        .unwrap();
    for attribute in ["HttpOnly", "Secure", "SameSite=Lax", "Path=/"] {
        assert!(session.contains(attribute), "{attribute}: {session}");
    }
    assert!(!session.contains("Domain"), "{session}");
    let raw_cookie = cookie_value(&callback, SESSION_COOKIE).unwrap();
    let token_hash = SessionTokenKey::from_secret(COOKIE_PASSWORD).token_hash(&raw_cookie);
    let stored: (Vec<u8>, String) = sqlx::query_as(
        "SELECT session_token_hash, sealed_provider_session
         FROM user_sessions WHERE session_token_hash = $1",
    )
    .bind(&token_hash)
    .fetch_one(&h.pool)
    .await
    .unwrap();
    assert_eq!(stored.0, token_hash);
    assert_ne!(
        stored.1, raw_cookie,
        "provider credentials stay server-side"
    );
    assert!(!stored.1.contains("sess-") && !stored.1.contains("eyJ"));
    assert!(
        cookies
            .iter()
            .any(|c| c.starts_with(&format!("{LOGIN_COOKIE}=;"))),
        "the one-time login cookie is cleared"
    );
    assert_eq!(callback.headers()[header::CACHE_CONTROL], "no-store");
    assert_eq!(count(&h.pool, "SELECT count(*) FROM users").await, 1);
}

#[tokio::test]
async fn a_replayed_callback_is_idempotent_and_writes_one_provisioned_row() {
    let Some(h) = harness().await else { return };
    sign_in(&h, "user_A", "a@example.org").await;
    sign_in(&h, "user_A", "a@example.org").await;
    assert_eq!(count(&h.pool, "SELECT count(*) FROM users").await, 1);
    assert_eq!(
        count(
            &h.pool,
            "SELECT count(*) FROM admin_audit_events WHERE event_type = 'user.provisioned'"
        )
        .await,
        1
    );
    assert_eq!(
        count(
            &h.pool,
            "SELECT count(*) FROM admin_audit_events WHERE event_type = 'auth.login'"
        )
        .await,
        2
    );
}

#[tokio::test]
async fn a_callback_with_a_missing_or_wrong_state_is_rejected() {
    let Some(h) = harness().await else { return };
    let login = h
        .app
        .clone()
        .oneshot(get("/auth/login", None))
        .await
        .unwrap();
    let login_cookie = cookie_value(&login, LOGIN_COOKIE).unwrap();
    let code = h.fake.issue_code("user_A", "a@example.org", None);

    let attempts = [
        // wrong state, valid login cookie
        get(
            &format!("/auth/callback?code={code}&state=forged"),
            Some((LOGIN_COOKIE, &login_cookie)),
        ),
        // no state at all
        get(
            &format!("/auth/callback?code={code}"),
            Some((LOGIN_COOKIE, &login_cookie)),
        ),
        // a callback started by another site: no login cookie in this browser
        get(&format!("/auth/callback?code={code}&state=whatever"), None),
        // a tampered login cookie
        get(
            &format!("/auth/callback?code={code}&state=whatever"),
            Some((LOGIN_COOKIE, "deadbeef")),
        ),
    ];
    for request in attempts {
        let response = h.app.clone().oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::SEE_OTHER);
        assert_eq!(
            response.headers()[header::LOCATION],
            format!("{ORIGIN}/signin?error=invalid_state")
        );
        assert!(
            cookie_value(&response, SESSION_COOKIE).is_none(),
            "no session is issued"
        );
    }
    assert_eq!(count(&h.pool, "SELECT count(*) FROM users").await, 0);
    assert_eq!(
        count(&h.pool, "SELECT count(*) FROM admin_audit_events").await,
        0
    );
}

#[tokio::test]
async fn provider_failures_during_callback_redirect_with_a_code_and_no_session() {
    let Some(h) = harness().await else { return };
    for (failure, code) in [
        (
            deepref_application::identity::IdentityError::Unavailable,
            "provider_unavailable",
        ),
        (
            deepref_application::identity::IdentityError::Rejected,
            "invalid_code",
        ),
    ] {
        let login = h
            .app
            .clone()
            .oneshot(get("/auth/login", None))
            .await
            .unwrap();
        let login_cookie = cookie_value(&login, LOGIN_COOKIE).unwrap();
        let state = login.headers()[header::LOCATION]
            .to_str()
            .unwrap()
            .split("state=")
            .nth(1)
            .unwrap()
            .to_owned();
        *h.fake.fail_exchange.lock().unwrap() = Some(failure);
        let response = h
            .app
            .clone()
            .oneshot(get(
                &format!("/auth/callback?code=c&state={state}"),
                Some((LOGIN_COOKIE, &login_cookie)),
            ))
            .await
            .unwrap();
        assert_eq!(
            response.headers()[header::LOCATION],
            format!("{ORIGIN}/signin?error={code}")
        );
        assert!(cookie_value(&response, SESSION_COOKIE).is_none());
    }
}

#[tokio::test]
async fn a_valid_session_resolves_one_stable_user_id() {
    let Some(h) = harness().await else { return };
    let session = sign_in(&h, "user_A", "a@example.org").await;
    let me = h
        .app
        .clone()
        .oneshot(get("/me", Some((SESSION_COOKIE, &session))))
        .await
        .unwrap();
    assert_eq!(me.status(), StatusCode::OK);
    let body = json(me).await;
    assert_eq!(body["auth_enabled"], true);
    assert_eq!(
        body["user"]["id"],
        user_id(&h.pool, "user_A").await.to_string()
    );
    assert_eq!(body["user"]["email"], "a@example.org");
}

#[tokio::test]
async fn an_expired_session_refreshes_and_sets_the_renewed_cookie() {
    let Some(h) = harness().await else { return };
    let cookie_token = sign_in(&h, "user_A", "a@example.org").await;
    let expired = h.fake.mint_expired_session("user_A");
    replace_provider_session(&h, &cookie_token, &expired).await;
    let response = h
        .app
        .clone()
        .oneshot(get("/me", Some((SESSION_COOKIE, &cookie_token))))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert!(cookie_value(&response, SESSION_COOKIE).is_none());
    let again = h
        .app
        .clone()
        .oneshot(get("/me", Some((SESSION_COOKIE, &cookie_token))))
        .await
        .unwrap();
    assert_eq!(again.status(), StatusCode::OK);
}

#[tokio::test]
async fn concurrent_requests_on_one_expired_cookie_refresh_once() {
    let Some(h) = harness().await else { return };
    let cookie_token = sign_in(&h, "user_A", "a@example.org").await;
    let expired = h.fake.mint_expired_session("user_A");
    replace_provider_session(&h, &cookie_token, &expired).await;
    let mut tasks = Vec::new();
    for _ in 0..6 {
        let app = h.app.clone();
        let cookie_token = cookie_token.clone();
        tasks.push(tokio::spawn(async move {
            app.oneshot(get("/me", Some((SESSION_COOKIE, &cookie_token))))
                .await
                .unwrap()
                .status()
        }));
    }
    for task in tasks {
        assert_eq!(task.await.unwrap(), StatusCode::OK);
    }
    assert_eq!(h.fake.refresh_calls.load(Ordering::SeqCst), 1);
}

/// A session whose access token has expired, with another replica already
/// holding the refresh lease: it has spent the refresh token at the provider
/// and not yet committed. Returns the row and the lease holder's id.
async fn session_under_a_competing_refresh(
    h: &Harness,
    lease: std::time::Duration,
) -> (
    String,
    deepref_app::identity::StoredBrowserSession,
    uuid::Uuid,
) {
    let cookie_token = sign_in(h, "user_A", "a@example.org").await;
    let expired = h.fake.mint_expired_session("user_A");
    replace_provider_session(h, &cookie_token, &expired).await;
    let token_hash = SessionTokenKey::from_secret(COOKIE_PASSWORD).token_hash(&cookie_token);
    let stored = find_active_session(&h.pool, &token_hash)
        .await
        .unwrap()
        .unwrap();
    let rival = uuid::Uuid::new_v4();
    let claim = claim_refresh_lease(&h.pool, stored.id, stored.version, rival, lease)
        .await
        .unwrap();
    assert_eq!(claim, deepref_app::identity::RefreshLease::Claimed);
    (cookie_token, stored, rival)
}

/// The rival's commit: a freshly rotated sealed session.
async fn commit_rival_refresh(
    h: &Harness,
    stored: &deepref_app::identity::StoredBrowserSession,
    rival: uuid::Uuid,
) {
    let renewed = h.fake.mint_session("user_A");
    let session_ref = SessionRefKey::from_secret(COOKIE_PASSWORD)
        .session_ref(&h.fake.provider_session_id(&renewed));
    let committed = update_active_session(
        &h.pool,
        stored.id,
        stored.version,
        rival,
        &OpaqueSecret::new(renewed),
        &session_ref,
    )
    .await
    .unwrap();
    assert_eq!(committed, SessionUpdate::Updated);
}

#[tokio::test]
async fn a_competing_refresh_in_flight_is_retryable_and_keeps_the_cookie() {
    let Some(h) = harness().await else { return };
    let (cookie_token, stored, rival) =
        session_under_a_competing_refresh(&h, std::time::Duration::from_secs(60)).await;
    let impatient = auth_app_waiting(
        h.pool.clone(),
        Arc::clone(&h.fake),
        std::time::Duration::from_millis(300),
    );

    let contended = impatient
        .clone()
        .oneshot(get("/me", Some((SESSION_COOKIE, &cookie_token))))
        .await
        .unwrap();
    assert_eq!(contended.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert!(
        set_cookies(&contended).is_empty(),
        "a valid session is not cleared while another request refreshes it: {:?}",
        set_cookies(&contended)
    );
    assert_eq!(json(contended).await["code"], "AUTH_PROVIDER_UNAVAILABLE");
    assert_eq!(
        h.fake.refresh_calls.load(Ordering::SeqCst),
        0,
        "the token is spent once, by the lease holder only"
    );

    commit_rival_refresh(&h, &stored, rival).await;
    let after = impatient
        .oneshot(get("/me", Some((SESSION_COOKIE, &cookie_token))))
        .await
        .unwrap();
    assert_eq!(after.status(), StatusCode::OK);
    assert_eq!(h.fake.refresh_calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn a_request_waiting_on_a_competing_refresh_gets_the_winners_session() {
    let Some(h) = harness().await else { return };
    let (cookie_token, stored, rival) =
        session_under_a_competing_refresh(&h, std::time::Duration::from_secs(60)).await;
    let app = h.app.clone();
    let waiting = tokio::spawn(async move {
        app.oneshot(get("/me", Some((SESSION_COOKIE, &cookie_token))))
            .await
            .unwrap()
    });
    tokio::time::sleep(std::time::Duration::from_millis(400)).await;
    assert!(!waiting.is_finished(), "the request waits for the holder");
    commit_rival_refresh(&h, &stored, rival).await;
    let response = waiting.await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert!(set_cookies(&response).is_empty());
    assert_eq!(h.fake.refresh_calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn an_expired_refresh_lease_lets_a_later_request_refresh() {
    let Some(h) = harness().await else { return };
    // The rival died holding a one-second lease and never committed.
    let (cookie_token, _, _) =
        session_under_a_competing_refresh(&h, std::time::Duration::from_secs(1)).await;
    let response = h
        .app
        .clone()
        .oneshot(get("/me", Some((SESSION_COOKIE, &cookie_token))))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(h.fake.refresh_calls.load(Ordering::SeqCst), 1);
}

async fn wait_for_refresh_call(h: &Harness) {
    for _ in 0..200 {
        if h.fake.refresh_calls.load(Ordering::SeqCst) >= 1 {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    panic!("the refresh never reached the provider");
}

/// Hands the session's refresh lease to `rival`, as a takeover after expiry.
async fn take_over_lease(h: &Harness, session_id: uuid::Uuid, rival: uuid::Uuid) {
    sqlx::query(
        "UPDATE user_sessions
         SET refresh_lease_owner = $2, refresh_lease_until = now() + interval '60 seconds'
         WHERE id = $1",
    )
    .bind(session_id)
    .bind(rival)
    .execute(&h.pool)
    .await
    .unwrap();
}

async fn revoked_at_is_null(h: &Harness, session_id: uuid::Uuid) -> bool {
    sqlx::query_scalar("SELECT revoked_at IS NULL FROM user_sessions WHERE id = $1")
        .bind(session_id)
        .fetch_one(&h.pool)
        .await
        .unwrap()
}

#[tokio::test]
async fn a_refresh_that_outlives_its_budget_is_abandoned_keeping_cookie_and_lease_free() {
    let Some(h) = harness().await else { return };
    let cookie_token = sign_in(&h, "user_A", "a@example.org").await;
    let expired = h.fake.mint_expired_session("user_A");
    replace_provider_session(&h, &cookie_token, &expired).await;
    // A two-second lease gives the refresh a one-second budget; the provider
    // takes three.
    *h.fake.refresh_delay.lock().unwrap() = std::time::Duration::from_secs(3);
    let app = auth_app_with(
        h.pool.clone(),
        auth_runtime(Arc::clone(&h.fake), std::time::Duration::from_secs(5))
            .with_refresh_lease(std::time::Duration::from_secs(2)),
    );
    let started = std::time::Instant::now();
    let slow = app
        .clone()
        .oneshot(get("/me", Some((SESSION_COOKIE, &cookie_token))))
        .await
        .unwrap();
    assert!(
        started.elapsed() < std::time::Duration::from_millis(2500),
        "abandoned inside the lease, not after the provider answered"
    );
    assert_eq!(slow.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert!(set_cookies(&slow).is_empty(), "{:?}", set_cookies(&slow));
    let owner: Option<uuid::Uuid> =
        sqlx::query_scalar("SELECT refresh_lease_owner FROM user_sessions")
            .fetch_one(&h.pool)
            .await
            .unwrap();
    assert!(owner.is_none(), "the abandoned refresh released its lease");

    *h.fake.refresh_delay.lock().unwrap() = std::time::Duration::ZERO;
    let retry = app
        .oneshot(get("/me", Some((SESSION_COOKIE, &cookie_token))))
        .await
        .unwrap();
    assert_eq!(retry.status(), StatusCode::OK);
}

#[tokio::test]
async fn a_rejection_after_the_lease_was_taken_over_does_not_clear_the_cookie() {
    let Some(h) = harness().await else { return };
    let cookie_token = sign_in(&h, "user_A", "a@example.org").await;
    let revoked = h.fake.mint_revoked_session("user_A");
    replace_provider_session(&h, &cookie_token, &revoked).await;
    let token_hash = SessionTokenKey::from_secret(COOKIE_PASSWORD).token_hash(&cookie_token);
    let stored = find_active_session(&h.pool, &token_hash)
        .await
        .unwrap()
        .unwrap();
    *h.fake.refresh_delay.lock().unwrap() = std::time::Duration::from_millis(600);
    let app = auth_app_waiting(
        h.pool.clone(),
        Arc::clone(&h.fake),
        std::time::Duration::from_secs(1),
    );
    let competitor = tokio::spawn({
        let cookie_token = cookie_token.clone();
        async move {
            app.oneshot(get("/me", Some((SESSION_COOKIE, &cookie_token))))
                .await
                .unwrap()
        }
    });
    wait_for_refresh_call(&h).await;
    // The original owner is slow and its lease lapsed: another replica owns it
    // now, and this competitor's invalid_grant is a replay, not a verdict.
    take_over_lease(&h, stored.id, uuid::Uuid::new_v4()).await;
    let response = competitor.await.unwrap();
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert!(
        set_cookies(&response).is_empty(),
        "{:?}",
        set_cookies(&response)
    );
    assert!(revoked_at_is_null(&h, stored.id).await);
}

#[tokio::test]
async fn a_rejection_after_the_owner_committed_uses_the_owners_session() {
    let Some(h) = harness().await else { return };
    let cookie_token = sign_in(&h, "user_A", "a@example.org").await;
    let revoked = h.fake.mint_revoked_session("user_A");
    replace_provider_session(&h, &cookie_token, &revoked).await;
    let token_hash = SessionTokenKey::from_secret(COOKIE_PASSWORD).token_hash(&cookie_token);
    let stored = find_active_session(&h.pool, &token_hash)
        .await
        .unwrap()
        .unwrap();
    *h.fake.refresh_delay.lock().unwrap() = std::time::Duration::from_millis(600);
    let app = h.app.clone();
    let competitor = tokio::spawn({
        let cookie_token = cookie_token.clone();
        async move {
            app.oneshot(get("/me", Some((SESSION_COOKIE, &cookie_token))))
                .await
                .unwrap()
        }
    });
    wait_for_refresh_call(&h).await;
    let rival = uuid::Uuid::new_v4();
    take_over_lease(&h, stored.id, rival).await;
    commit_rival_refresh(&h, &stored, rival).await;
    let response = competitor.await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert!(set_cookies(&response).is_empty());
    assert!(revoked_at_is_null(&h, stored.id).await);
}

#[tokio::test]
async fn a_rejection_from_the_current_lease_owner_ends_the_session() {
    let Some(h) = harness().await else { return };
    let cookie_token = sign_in(&h, "user_A", "a@example.org").await;
    let revoked = h.fake.mint_revoked_session("user_A");
    replace_provider_session(&h, &cookie_token, &revoked).await;
    let response = h
        .app
        .clone()
        .oneshot(get("/me", Some((SESSION_COOKIE, &cookie_token))))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    assert!(
        set_cookies(&response)
            .iter()
            .any(|c| c.starts_with(&format!("{SESSION_COOKIE}=;")))
    );
    let live: i64 = count(
        &h.pool,
        "SELECT count(*) FROM user_sessions WHERE revoked_at IS NULL",
    )
    .await;
    assert_eq!(live, 0, "the fenced decision revoked the local session");
}

#[tokio::test]
async fn waiting_requests_in_one_process_share_one_lease_poller() {
    let Some(h) = harness().await else { return };
    let (cookie_token, _, _) =
        session_under_a_competing_refresh(&h, std::time::Duration::from_secs(60)).await;
    let runtime = auth_runtime(Arc::clone(&h.fake), std::time::Duration::from_millis(1500));
    let app = auth_app_with(h.pool.clone(), runtime.clone());
    let mut tasks = Vec::new();
    for _ in 0..20 {
        let app = app.clone();
        let cookie_token = cookie_token.clone();
        tasks.push(tokio::spawn(async move {
            app.oneshot(get("/me", Some((SESSION_COOKIE, &cookie_token))))
                .await
                .unwrap()
                .status()
        }));
    }
    for task in tasks {
        assert_eq!(task.await.unwrap(), StatusCode::SERVICE_UNAVAILABLE);
    }
    let queries = runtime.refresh_lease_queries();
    assert!(
        (1..=8).contains(&queries),
        "20 waiters made {queries} lease queries; backoff over 1.5s allows about 5"
    );
}

#[tokio::test]
async fn the_refresh_wait_holds_even_when_the_lease_query_stalls() {
    let Some(h) = harness().await else { return };
    let cookie_token = sign_in(&h, "user_A", "a@example.org").await;
    let expired = h.fake.mint_expired_session("user_A");
    replace_provider_session(&h, &cookie_token, &expired).await;
    let token_hash = SessionTokenKey::from_secret(COOKIE_PASSWORD).token_hash(&cookie_token);
    let stored = find_active_session(&h.pool, &token_hash)
        .await
        .unwrap()
        .unwrap();
    // Another transaction holds the row, so the lease UPDATE blocks.
    let mut blocker = h.pool.begin().await.unwrap();
    sqlx::query("SELECT 1 FROM user_sessions WHERE id = $1 FOR UPDATE")
        .bind(stored.id)
        .execute(&mut *blocker)
        .await
        .unwrap();
    let impatient = auth_app_waiting(
        h.pool.clone(),
        Arc::clone(&h.fake),
        std::time::Duration::from_millis(500),
    );
    let started = std::time::Instant::now();
    let response = impatient
        .oneshot(get("/me", Some((SESSION_COOKIE, &cookie_token))))
        .await
        .unwrap();
    assert!(
        started.elapsed() < std::time::Duration::from_secs(3),
        "took {:?}",
        started.elapsed()
    );
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert!(set_cookies(&response).is_empty());
    blocker.rollback().await.unwrap();
}

#[tokio::test]
async fn a_session_the_provider_will_not_refresh_is_rejected_and_cleared() {
    let Some(h) = harness().await else { return };
    let cookie_token = sign_in(&h, "user_A", "a@example.org").await;
    let revoked = h.fake.mint_revoked_session("user_A");
    replace_provider_session(&h, &cookie_token, &revoked).await;
    let response = h
        .app
        .clone()
        .oneshot(get("/me", Some((SESSION_COOKIE, &cookie_token))))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    assert!(
        set_cookies(&response)
            .iter()
            .any(|c| c.starts_with(&format!("{SESSION_COOKIE}=;")))
    );
    assert_eq!(json(response).await["code"], "SESSION_EXPIRED");
}

#[tokio::test]
async fn actor_headers_do_not_change_who_a_session_is() {
    let Some(h) = harness().await else { return };
    let session = sign_in(&h, "user_A", "a@example.org").await;
    let real = user_id(&h.pool, "user_A").await;

    let me = h
        .app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/me")
                .header(header::COOKIE, format!("{SESSION_COOKIE}={session}"))
                .header("x-actor-kind", "user")
                .header("x-actor-id", "mallory")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(json(me).await["user"]["id"], real.to_string());

    let mut logout = post("/auth/logout", &session, Some(ORIGIN), "");
    logout
        .headers_mut()
        .insert("x-actor-id", "mallory".parse().unwrap());
    logout
        .headers_mut()
        .insert("x-actor-kind", "user".parse().unwrap());
    assert_eq!(
        h.app.clone().oneshot(logout).await.unwrap().status(),
        StatusCode::NO_CONTENT
    );
    let (principal, target): (Option<uuid::Uuid>, Option<uuid::Uuid>) = sqlx::query_as(
        "SELECT principal_user_id, target_user_id FROM admin_audit_events WHERE event_type = 'auth.logout'",
    )
    .fetch_one(&h.pool)
    .await
    .unwrap();
    assert_eq!((principal, target), (Some(real), Some(real)));
}

#[tokio::test]
async fn logout_requires_post_and_a_get_does_not_sign_out() {
    let Some(h) = harness().await else { return };
    let session = sign_in(&h, "user_A", "a@example.org").await;

    let get_logout = h
        .app
        .clone()
        .oneshot(get("/auth/logout", Some((SESSION_COOKIE, &session))))
        .await
        .unwrap();
    assert_eq!(get_logout.status(), StatusCode::METHOD_NOT_ALLOWED);
    assert_eq!(
        count(
            &h.pool,
            "SELECT count(*) FROM admin_audit_events WHERE event_type = 'auth.logout'"
        )
        .await,
        0
    );
    let still = h
        .app
        .clone()
        .oneshot(get("/me", Some((SESSION_COOKIE, &session))))
        .await
        .unwrap();
    assert_eq!(still.status(), StatusCode::OK);

    let sealed: String = sqlx::query_scalar(
        "SELECT sealed_provider_session FROM user_sessions WHERE session_token_hash = $1",
    )
    .bind(SessionTokenKey::from_secret(COOKIE_PASSWORD).token_hash(&session))
    .fetch_one(&h.pool)
    .await
    .unwrap();
    let provider_session = h.fake.provider_session_id(&sealed);
    let out = h
        .app
        .clone()
        .oneshot(post("/auth/logout", &session, Some(ORIGIN), ""))
        .await
        .unwrap();
    assert_eq!(out.status(), StatusCode::NO_CONTENT);
    assert!(
        set_cookies(&out)
            .iter()
            .any(|c| c.starts_with(&format!("{SESSION_COOKIE}=;")) && c.contains("Max-Age=0")),
        "the cookie is cleared"
    );
    assert_eq!(*h.fake.revoked.lock().unwrap(), [provider_session]);
    assert_eq!(
        count(
            &h.pool,
            "SELECT count(*) FROM admin_audit_events WHERE event_type = 'auth.logout'"
        )
        .await,
        1
    );
}

#[tokio::test]
async fn logout_without_a_session_clears_the_cookie_and_records_nothing() {
    let Some(h) = harness().await else { return };
    let response = h
        .app
        .clone()
        .oneshot(post("/auth/logout", "never-issued", Some(ORIGIN), ""))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NO_CONTENT);
    assert_eq!(
        count(&h.pool, "SELECT count(*) FROM admin_audit_events").await,
        0
    );
}

#[tokio::test]
async fn cross_origin_posts_with_a_valid_session_are_rejected_and_change_nothing() {
    let Some(h) = harness().await else { return };
    let session = sign_in(&h, "user_A", "a@example.org").await;
    let body = r#"{"name":"Injected"}"#;

    for origin in [
        "https://evil.example",
        "null",
        "http://app.example.com",
        "https://app.example.com:8443",
        "https://sibling.example.com",
    ] {
        let response = h
            .app
            .clone()
            .oneshot(post("/projects", &session, Some(origin), body))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN, "{origin}");
        assert_eq!(json(response).await["code"], "ORIGIN_REJECTED");
    }
    for site in [Some("same-site"), Some("cross-site"), Some("none"), None] {
        let mut request = post("/projects", &session, None, body);
        if let Some(site) = site {
            request
                .headers_mut()
                .insert("sec-fetch-site", site.parse().unwrap());
        }
        let response = h.app.clone().oneshot(request).await.unwrap();
        assert_eq!(
            response.status(),
            StatusCode::FORBIDDEN,
            "sec-fetch-site {site:?}"
        );
    }
    let logout = h
        .app
        .clone()
        .oneshot(post(
            "/auth/logout",
            &session,
            Some("https://evil.example"),
            "",
        ))
        .await
        .unwrap();
    assert_eq!(
        logout.status(),
        StatusCode::FORBIDDEN,
        "cross-site logout is refused"
    );
    assert_eq!(count(&h.pool, "SELECT count(*) FROM projects").await, 0);
    assert_eq!(
        count(
            &h.pool,
            "SELECT count(*) FROM admin_audit_events WHERE event_type = 'auth.logout'"
        )
        .await,
        0
    );
    assert!(h.fake.revoked.lock().unwrap().is_empty());
}

#[tokio::test]
async fn same_origin_posts_with_a_valid_session_are_allowed() {
    let Some(h) = harness().await else { return };
    let session = sign_in(&h, "user_A", "a@example.org").await;
    let body = r#"{"name":"Mine"}"#;

    let by_origin = h
        .app
        .clone()
        .oneshot(post("/projects", &session, Some(ORIGIN), body))
        .await
        .unwrap();
    assert_eq!(by_origin.status(), StatusCode::CREATED);

    let mut by_fetch_site = post("/projects", &session, None, body);
    by_fetch_site
        .headers_mut()
        .insert("sec-fetch-site", "same-origin".parse().unwrap());
    assert_eq!(
        h.app.clone().oneshot(by_fetch_site).await.unwrap().status(),
        StatusCode::CREATED
    );
    assert_eq!(count(&h.pool, "SELECT count(*) FROM projects").await, 2);
}

#[tokio::test]
async fn a_deactivated_user_is_rejected_on_the_next_request() {
    let Some(h) = harness().await else { return };
    let session = sign_in(&h, "user_A", "a@example.org").await;
    let before = h
        .app
        .clone()
        .oneshot(get("/projects", Some((SESSION_COOKIE, &session))))
        .await
        .unwrap();
    assert_eq!(before.status(), StatusCode::OK);

    deactivate_user(
        &h.pool,
        &ExternalUserId::parse("user_A").unwrap(),
        "receipt-1",
    )
    .await
    .unwrap();

    let after = h
        .app
        .clone()
        .oneshot(get("/projects", Some((SESSION_COOKIE, &session))))
        .await
        .unwrap();
    assert_eq!(after.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(json(after).await["code"], "SESSION_EXPIRED");
    let mutating = h
        .app
        .clone()
        .oneshot(post("/projects", &session, Some(ORIGIN), r#"{"name":"x"}"#))
        .await
        .unwrap();
    assert_eq!(mutating.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(count(&h.pool, "SELECT count(*) FROM projects").await, 0);

    // A deactivated person who signs in again at the provider gets no session.
    let login = h
        .app
        .clone()
        .oneshot(get("/auth/login", None))
        .await
        .unwrap();
    let login_cookie = cookie_value(&login, LOGIN_COOKIE).unwrap();
    let state = login.headers()[header::LOCATION]
        .to_str()
        .unwrap()
        .split("state=")
        .nth(1)
        .unwrap()
        .to_owned();
    let code = h.fake.issue_code("user_A", "a@example.org", None);
    let callback = h
        .app
        .clone()
        .oneshot(get(
            &format!("/auth/callback?code={code}&state={state}"),
            Some((LOGIN_COOKIE, &login_cookie)),
        ))
        .await
        .unwrap();
    assert_eq!(
        callback.headers()[header::LOCATION],
        format!("{ORIGIN}/signin?error=account_deactivated")
    );
    assert!(cookie_value(&callback, SESSION_COOKIE).is_none());
}

#[tokio::test]
async fn health_probes_stay_public_when_authentication_is_on() {
    let Some(h) = harness().await else { return };
    for path in ["/health/live", "/health/ready", "/health/dependencies"] {
        let response = h.app.clone().oneshot(get(path, None)).await.unwrap();
        assert_ne!(response.status(), StatusCode::UNAUTHORIZED, "{path}");
    }
}

#[tokio::test]
async fn with_authentication_off_existing_behaviour_is_unchanged() {
    let Some(pool) = database().await else { return };
    let config = deepref_http_api::config::ApiConfig {
        documents: deepref_config::DocumentConfig::from_source(&|name| {
            (name == "APP_ENV").then(|| "development".into())
        })
        .unwrap(),
        runtime: support::local_runtime(),
        bind_addr: "127.0.0.1:8080".parse().unwrap(),
        cors_allow_any: false,
        cors_origins: vec![],
    };
    let app =
        deepref_http_api::routes::router(deepref_http_api::state::AppState::core(pool), &config);
    let projects = app.clone().oneshot(get("/projects", None)).await.unwrap();
    assert_eq!(projects.status(), StatusCode::OK);
    let me = app.clone().oneshot(get("/me", None)).await.unwrap();
    assert_eq!(me.status(), StatusCode::OK);
    let body = json(me).await;
    assert_eq!(body["auth_enabled"], false);
    assert!(body["user"].is_null());
    let login = app.oneshot(get("/auth/login", None)).await.unwrap();
    assert_eq!(login.status(), StatusCode::NOT_FOUND);
}

async fn seed_study(pool: &PgPool) -> (uuid::Uuid, uuid::Uuid) {
    let project = uuid::Uuid::new_v4();
    let study = uuid::Uuid::new_v4();
    sqlx::query("INSERT INTO projects (id,name) VALUES ($1,'legacy attribution')")
        .bind(project)
        .execute(pool)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO studies
         (id,project_id,title,design_context,study_revision,updated_by_actor_kind,updated_by_actor_id)
         VALUES ($1,$2,'Study','{}'::jsonb,0,'system','test')",
    )
    .bind(study)
    .bind(project)
    .execute(pool)
    .await
    .unwrap();
    (project, study)
}

fn scientific_write(
    method: Method,
    uri: &str,
    session: Option<&str>,
    actor: &[(&str, &str)],
    body: serde_json::Value,
) -> Request<Body> {
    let mut request = Request::builder()
        .method(method)
        .uri(uri)
        .header(header::CONTENT_TYPE, "application/json");
    if let Some(session) = session {
        request = request
            .header(header::COOKIE, format!("{SESSION_COOKIE}={session}"))
            .header(header::ORIGIN, ORIGIN);
    }
    for (name, value) in actor {
        request = request.header(*name, *value);
    }
    request.body(Body::from(body.to_string())).unwrap()
}

#[tokio::test]
async fn scientific_writes_keep_the_header_label_as_legacy_attribution() {
    let Some(h) = harness().await else { return };
    let session = sign_in(&h, "user_A", "a@example.org").await;
    let real = user_id(&h.pool, "user_A").await;
    let (project, study) = seed_study(&h.pool).await;
    let label = [("x-actor-kind", "user"), ("x-actor-id", "alice")];

    // No session: 401 before actor extraction, whatever label is sent.
    let denied = h
        .app
        .clone()
        .oneshot(scientific_write(
            Method::POST,
            &format!("/projects/{project}/extraction/fields"),
            None,
            &label,
            serde_json::json!({"version": 1, "field_key": "n", "label": "N",
                "value_type": "number", "required": false}),
        ))
        .await
        .unwrap();
    assert_eq!(denied.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(json(denied).await["code"], "UNAUTHENTICATED");
    assert_eq!(
        count(&h.pool, "SELECT count(*) FROM extraction_field_definitions").await,
        0
    );

    // With a session the write records the header label, exactly as before 02.
    let created = h
        .app
        .clone()
        .oneshot(scientific_write(
            Method::POST,
            &format!("/projects/{project}/extraction/fields"),
            Some(&session),
            &label,
            serde_json::json!({"version": 1, "field_key": "n", "label": "N",
                "value_type": "number", "required": false}),
        ))
        .await
        .unwrap();
    assert_eq!(created.status(), StatusCode::CREATED);
    let field = json(created).await["id"].as_str().unwrap().to_owned();
    let saved = h
        .app
        .clone()
        .oneshot(scientific_write(
            Method::PUT,
            &format!("/projects/{project}/studies/{study}/extraction/values/{field}"),
            Some(&session),
            &label,
            serde_json::json!({"value": {"kind": "number", "value": 12.0}}),
        ))
        .await
        .unwrap();
    assert_eq!(saved.status(), StatusCode::OK);
    let saved = json(saved).await;
    assert_eq!(saved["approved_by_actor_id"], "alice");
    assert_ne!(saved["approved_by_actor_id"], real.to_string());

    // No label: the legacy default, not the session UserId.
    let unlabeled = h
        .app
        .clone()
        .oneshot(scientific_write(
            Method::PUT,
            &format!("/projects/{project}/studies/{study}/extraction/values/{field}"),
            Some(&session),
            &[],
            serde_json::json!({"value": {"kind": "number", "value": 13.0}}),
        ))
        .await
        .unwrap();
    assert_eq!(json(unlabeled).await["approved_by_actor_id"], "local-user");

    // Client-supplied system and automation kinds stay rejected.
    for kind in ["system", "automation"] {
        let rejected = h
            .app
            .clone()
            .oneshot(scientific_write(
                Method::PUT,
                &format!("/projects/{project}/studies/{study}/extraction/values/{field}"),
                Some(&session),
                &[("x-actor-kind", kind), ("x-actor-id", "alice")],
                serde_json::json!({"value": {"kind": "number", "value": 14.0}}),
            ))
            .await
            .unwrap();
        assert_eq!(rejected.status(), StatusCode::BAD_REQUEST, "{kind}");
    }

    // Identity is the session's UserId no matter what the label says, and the
    // audit trail never sees the label.
    let me = h
        .app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/me")
                .header(header::COOKIE, format!("{SESSION_COOKIE}={session}"))
                .header("x-actor-id", "alice")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(json(me).await["user"]["id"], real.to_string());
    let out = h
        .app
        .clone()
        .oneshot(post("/auth/logout", &session, Some(ORIGIN), ""))
        .await
        .unwrap();
    assert_eq!(out.status(), StatusCode::NO_CONTENT);
    let principal: uuid::Uuid = sqlx::query_scalar(
        "SELECT principal_user_id FROM admin_audit_events WHERE event_type = 'auth.logout'",
    )
    .fetch_one(&h.pool)
    .await
    .unwrap();
    assert_eq!(principal, real);
}

#[tokio::test]
async fn logout_revokes_and_audits_even_when_the_provider_cannot_inspect_the_session() {
    let Some(h) = harness().await else { return };
    let session = sign_in(&h, "user_A", "a@example.org").await;
    h.fake.inspect_unavailable.store(true, Ordering::SeqCst);

    let out = h
        .app
        .clone()
        .oneshot(post("/auth/logout", &session, Some(ORIGIN), ""))
        .await
        .unwrap();
    assert_eq!(out.status(), StatusCode::NO_CONTENT);
    assert!(
        set_cookies(&out)
            .iter()
            .any(|c| c.starts_with(&format!("{SESSION_COOKIE}=;")))
    );
    let revoked: Option<chrono::DateTime<chrono::Utc>> =
        sqlx::query_scalar("SELECT revoked_at FROM user_sessions")
            .fetch_one(&h.pool)
            .await
            .unwrap();
    assert!(revoked.is_some(), "the local row is revoked");
    for event in ["auth.logout", "auth.session_revoked"] {
        assert_eq!(
            count(
                &h.pool,
                match event {
                    "auth.logout" => "SELECT count(*) FROM admin_audit_events WHERE event_type = 'auth.logout'",
                    _ => "SELECT count(*) FROM admin_audit_events WHERE event_type = 'auth.session_revoked'",
                }
            )
            .await,
            1,
            "{event}"
        );
    }

    // The cookie is dead once the provider is back, and a replay audits nothing.
    h.fake.inspect_unavailable.store(false, Ordering::SeqCst);
    let me = h
        .app
        .clone()
        .oneshot(get("/me", Some((SESSION_COOKIE, &session))))
        .await
        .unwrap();
    assert_eq!(me.status(), StatusCode::UNAUTHORIZED);
    let replay = h
        .app
        .clone()
        .oneshot(post("/auth/logout", &session, Some(ORIGIN), ""))
        .await
        .unwrap();
    assert_eq!(replay.status(), StatusCode::NO_CONTENT);
    assert_eq!(
        count(
            &h.pool,
            "SELECT count(*) FROM admin_audit_events WHERE event_type = 'auth.logout'"
        )
        .await,
        1
    );
}

#[tokio::test]
async fn a_provider_outage_is_503_and_keeps_the_session_cookie() {
    let Some(h) = harness().await else { return };
    let session = sign_in(&h, "user_A", "a@example.org").await;
    h.fake.inspect_unavailable.store(true, Ordering::SeqCst);

    let down = h
        .app
        .clone()
        .oneshot(get("/me", Some((SESSION_COOKIE, &session))))
        .await
        .unwrap();
    assert_eq!(down.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert!(
        set_cookies(&down).is_empty(),
        "a valid cookie is not cleared during an outage: {:?}",
        set_cookies(&down)
    );
    assert_eq!(json(down).await["code"], "AUTH_PROVIDER_UNAVAILABLE");

    h.fake.inspect_unavailable.store(false, Ordering::SeqCst);
    let back = h
        .app
        .clone()
        .oneshot(get("/me", Some((SESSION_COOKIE, &session))))
        .await
        .unwrap();
    assert_eq!(back.status(), StatusCode::OK);
}

#[tokio::test]
async fn machine_hooks_run_their_own_checks_without_a_browser_session() {
    use deepref_domain::{Actor, ActorKind, ProjectId};
    let Some(h) = harness().await else { return };
    let project = uuid::Uuid::new_v4();
    sqlx::query("INSERT INTO projects (id,name) VALUES ($1,'hooks')")
        .bind(project)
        .execute(&h.pool)
        .await
        .unwrap();
    let project = ProjectId::new(project);
    let actor = Actor::new(ActorKind::User, "hook-test").unwrap();
    let graph = serde_json::from_value(serde_json::json!({
        "nodes": [{"id": "trigger", "type": "trigger.webhook", "config": {}}], "edges": []
    }))
    .unwrap();
    let workflow =
        deepref_app::create_workflow(&h.pool, project, "Hooked", "", graph, &actor, true)
            .await
            .unwrap();
    let endpoint = deepref_app::get_webhook_config(&h.pool, project, workflow.id)
        .await
        .unwrap();
    deepref_app::publish_workflow(&h.pool, project, workflow.id, None, &actor, false, true)
        .await
        .unwrap();
    deepref_app::set_workflow_enabled(&h.pool, project, workflow.id, true, true)
        .await
        .unwrap();
    let hook = |path: String, signature: Option<String>, body: &'static str| {
        let mut request = Request::builder().method(Method::POST).uri(path);
        if let Some(signature) = signature {
            request = request.header("X-DeepRef-Signature", signature);
        }
        request.body(Body::from(body)).unwrap()
    };
    let send = |request: Request<Body>| {
        let app = h.app.clone();
        async move { app.oneshot(request).await.unwrap().status() }
    };
    let good = deepref_application::workflows::sign_payload(&endpoint.secret, b"{}");
    let wrong = deepref_application::workflows::sign_payload("not-the-secret", b"{}");
    let unknown = uuid::Uuid::new_v4();

    // No cookie anywhere: the token and signature decide, never the session.
    let path = format!("/hooks/workflows/{}", endpoint.token);
    assert_eq!(
        send(hook(path.clone(), Some(good), "{}")).await,
        StatusCode::ACCEPTED
    );
    assert_eq!(
        send(hook(path.clone(), Some(wrong), "{}")).await,
        StatusCode::FORBIDDEN
    );
    assert_eq!(send(hook(path, None, "{}")).await, StatusCode::FORBIDDEN);
    assert_eq!(
        send(hook(format!("/hooks/workflows/{unknown}"), None, "{}")).await,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        send(hook(format!("/hooks/email/{unknown}"), None, "{}")).await,
        StatusCode::NOT_FOUND
    );
    // A known e-mail address reaches the handler, which rejects the unreadable body.
    assert_eq!(
        send(hook(
            format!("/hooks/email/{}", endpoint.email_token),
            None,
            "not a message"
        ))
        .await,
        StatusCode::BAD_REQUEST
    );

    // The exemption is exactly these two single-segment routes.
    for path in [
        "/hooks/workflows/".to_owned(),
        format!("/hooks/workflows/{}/extra", endpoint.token),
        "/hooks/other/token".to_owned(),
        "/hooks".to_owned(),
    ] {
        assert_eq!(
            send(hook(path.clone(), None, "{}")).await,
            StatusCode::UNAUTHORIZED,
            "{path}"
        );
    }
}
