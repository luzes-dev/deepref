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
use deepref_app::identity::{deactivate_user, find_user_by_external_id};
use deepref_application::identity::{SessionRefKey, SessionTokenKey};
use deepref_domain::ExternalUserId;
use sqlx::PgPool;
use tower::ServiceExt;

mod support;
use support::{
    COOKIE_PASSWORD, LOGIN_COOKIE, ORIGIN, SESSION_COOKIE, auth_app, fake_identity::FakeIdentity,
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
