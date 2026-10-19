#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]
//! The WorkOS adapter against a local stand-in for the WorkOS API. Real
//! session sealing, JWKS signature checks and expiry run; nothing leaves the
//! machine. `fixtures/workos-test-signing-key.der` (PKCS#1) is a throwaway test key.
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::{SystemTime, UNIX_EPOCH},
};

use deepref_application::identity::{
    IdentityError, IdentityProvider, LoginState, OpaqueSecret, RefreshOutcome, ReturnPath,
    SessionStatus,
};
use deepref_config::RuntimeConfig;
use deepref_domain::ExternalUserId;
use deepref_integrations::WorkOsIdentityProvider;
use jsonwebtoken::{Algorithm, EncodingKey, Header};
use serde_json::json;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
};

const SIGNING_KEY: &[u8] = include_bytes!("fixtures/workos-test-signing-key.der");
const JWK_MODULUS: &str = "8A1CDnFQ7iO-uQJXoghTf88CdGY8YMlL4g24oS4jcdR_0-9z5ZJabqV55Hs8wU2kmjiPpMu-pTYENvKoqmFhj6dfjjapJQnTrk92HtTH2Vn9RtShXR-Z8jtLTwxv8PC8g6_2mPnSfWcj_TTfwZAyPJFqO7qGIUQKfzvZro0Kd9rE1c46xM9xGotUQjq6VgfFVzWPQpzMSk2Z3Km6hFmLqahU0M5n3JMZNk-nUk2ObKg7Zv4sd3uAlURynTgGhKVd6EHZEENvXTsz7l8YM59PyhHXhWyXQ6CdckO_9Ki81MCodR4piP8WHtPKoBXpdICBsvNRqUhRdMFPgep6HFHhsQ";
/// Long enough that a loaded machine still signs in before the token expires.
const LIFETIME_BEFORE_EXPIRY: i64 = 5;
const PASSWORD: &str = "a-cookie-password-of-at-least-32-characters";

/// A well-formed RSA modulus that is not the signing key's.
fn other_modulus() -> String {
    let mut chars: Vec<char> = JWK_MODULUS.chars().collect();
    chars[100] = if chars[100] == 'A' { 'B' } else { 'A' };
    chars.into_iter().collect()
}

fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64
}

fn access_token(sub: &str, sid: &str, expires_in: i64, not_before_in: Option<i64>) -> String {
    let mut header = Header::new(Algorithm::RS256);
    header.kid = Some("key_1".to_owned());
    let mut claims = json!({ "sub": sub, "sid": sid, "iat": now(), "exp": now() + expires_in });
    if let Some(offset) = not_before_in {
        claims["nbf"] = json!(now() + offset);
    }
    jsonwebtoken::encode(&header, &claims, &EncodingKey::from_rsa_der(SIGNING_KEY)).unwrap()
}

#[derive(Default)]
struct MockState {
    exchange_lifetime: i64,
    refresh_status: u16,
    jwks_modulus: String,
    jwks_status: u16,
    /// Replaces the served `keys` array when set.
    jwks_keys: Option<serde_json::Value>,
    /// Stamps `nbf` on exchanged access tokens, relative to now.
    not_before_in: Option<i64>,
    /// Statuses for the next signing-key requests, in order, before
    /// `jwks_status` applies again.
    jwks_script: Vec<u16>,
    requests: Vec<(String, String, String)>,
}

fn user(id: &str) -> serde_json::Value {
    json!({
        "object": "user", "id": id, "email": "ada@example.org", "email_verified": true,
        "name": "Ada Lovelace",
        "created_at": "2026-01-01T00:00:00.000Z", "updated_at": "2026-01-01T00:00:00.000Z"
    })
}

async fn serve(state: Arc<Mutex<MockState>>) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move {
        loop {
            let Ok((mut stream, _)) = listener.accept().await else {
                return;
            };
            let state = Arc::clone(&state);
            tokio::spawn(async move {
                let mut buffer = Vec::new();
                let mut chunk = [0u8; 4096];
                let (head, mut body) = loop {
                    let read = stream.read(&mut chunk).await.unwrap_or(0);
                    if read == 0 {
                        return;
                    }
                    buffer.extend_from_slice(&chunk[..read]);
                    if let Some(end) = buffer.windows(4).position(|w| w == b"\r\n\r\n") {
                        break (
                            String::from_utf8_lossy(&buffer[..end]).into_owned(),
                            buffer[end + 4..].to_vec(),
                        );
                    }
                };
                let length = head
                    .lines()
                    .find_map(|l| {
                        l.to_ascii_lowercase()
                            .strip_prefix("content-length:")
                            .map(|v| v.trim().parse::<usize>().unwrap())
                    })
                    .unwrap_or(0);
                while body.len() < length {
                    let read = stream.read(&mut chunk).await.unwrap_or(0);
                    if read == 0 {
                        break;
                    }
                    body.extend_from_slice(&chunk[..read]);
                }
                let body = String::from_utf8_lossy(&body).into_owned();
                let mut first = head.lines().next().unwrap().split(' ');
                let (method, path) = (
                    first.next().unwrap().to_owned(),
                    first.next().unwrap().to_owned(),
                );
                let (status, payload) = {
                    let mut state = state.lock().unwrap();
                    state
                        .requests
                        .push((method.clone(), path.clone(), body.clone()));
                    route(&mut state, &method, &path, &body)
                };
                let response = format!(
                    "HTTP/1.1 {status} X\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{payload}",
                    payload.len()
                );
                let _ = stream.write_all(response.as_bytes()).await;
            });
        }
    });
    format!("http://{address}")
}

fn route(state: &mut MockState, method: &str, path: &str, body: &str) -> (u16, String) {
    if path.starts_with("/sso/jwks/") {
        let status = if state.jwks_script.is_empty() {
            state.jwks_status
        } else {
            state.jwks_script.remove(0)
        };
        if status != 200 {
            return (status, json!({ "message": "down" }).to_string());
        }
        if let Some(keys) = &state.jwks_keys {
            return (200, json!({ "keys": keys }).to_string());
        }
        return (
            200,
            json!({ "keys": [{ "kty": "RSA", "kid": "key_1", "alg": "RS256", "use": "sig",
                "n": state.jwks_modulus, "e": "AQAB" }] })
            .to_string(),
        );
    }
    match (method, path.split('?').next().unwrap()) {
        ("POST", "/user_management/authenticate") => {
            let request: serde_json::Value = serde_json::from_str(body).unwrap();
            match request["grant_type"].as_str() {
                Some("authorization_code") => (
                    200,
                    json!({ "user": user("user_01"), "access_token": access_token("user_01", "session_01", state.exchange_lifetime, state.not_before_in),
                        "refresh_token": "refresh_1" }).to_string(),
                ),
                Some("refresh_token") if state.refresh_status == 200 => (
                    200,
                    json!({ "user": user("user_01"), "access_token": access_token("user_01", "session_01", 300, None),
                        "refresh_token": "refresh_2" }).to_string(),
                ),
                _ => (
                    state.refresh_status,
                    json!({ "code": "invalid_grant", "message": "refresh token already used" }).to_string(),
                ),
            }
        }
        ("POST", "/user_management/sessions/revoke") => (200, "{}".to_owned()),
        ("GET", p) if p.ends_with("/sessions") => (
            200,
            json!({ "data": [{ "object": "session", "id": "session_01", "user_id": "user_01",
                "status": "active", "auth_method": "password",
                "expires_at": "2027-01-01T00:00:00.000Z", "ended_at": null,
                "created_at": "2026-01-01T00:00:00.000Z", "updated_at": "2026-01-01T00:00:00.000Z" }],
                "list_metadata": { "before": null, "after": null } })
            .to_string(),
        ),
        _ => (404, json!({ "message": "not found" }).to_string()),
    }
}

fn config(password: &str) -> deepref_config::WorkOsConfig {
    let values = HashMap::from([
        ("APP_ENV", "development"),
        ("WORKOS_CLIENT_ID", "client_TEST"),
        ("WORKOS_API_KEY", "sk_test_not_real"),
        ("WORKOS_COOKIE_PASSWORD", password),
        ("WORKOS_WEBHOOK_SECRET", "whsec_not_real"),
    ])
    .into_iter()
    .map(|(k, v)| (k.to_owned(), v.to_owned()))
    .collect();
    RuntimeConfig::from_map("test", &values)
        .unwrap()
        .workos
        .unwrap()
}

struct Harness {
    provider: WorkOsIdentityProvider,
    state: Arc<Mutex<MockState>>,
    base_url: String,
}

async fn harness() -> Harness {
    let state = Arc::new(Mutex::new(MockState {
        exchange_lifetime: 300,
        refresh_status: 200,
        jwks_modulus: JWK_MODULUS.to_owned(),
        jwks_status: 200,
        jwks_keys: None,
        not_before_in: None,
        jwks_script: Vec::new(),
        requests: Vec::new(),
    }));
    let base_url = serve(Arc::clone(&state)).await;
    Harness {
        provider: WorkOsIdentityProvider::with_base_url(&config(PASSWORD), &base_url),
        state,
        base_url,
    }
}

/// Waits until the clock has passed the session's access token expiry, however
/// slowly the machine runs, instead of sleeping a fixed time.
async fn wait_until_expired(provider: &WorkOsIdentityProvider, sealed: &OpaqueSecret) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
    loop {
        if matches!(
            provider.inspect_session(sealed).await,
            Ok(SessionStatus::Expired(_))
        ) {
            return;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "the token never expired"
        );
        tokio::time::sleep(std::time::Duration::from_millis(250)).await;
    }
}

async fn login(h: &Harness) -> (OpaqueSecret, deepref_application::identity::ProviderLogin) {
    let started = h
        .provider
        .begin_login("https://app.example.com/api/auth/callback")
        .unwrap();
    let login = h
        .provider
        .complete_login("code_1", &started.code_verifier)
        .await
        .unwrap();
    (login.sealed_session.clone(), login)
}

#[tokio::test]
async fn sign_in_url_carries_state_and_a_pkce_challenge() {
    let h = harness().await;
    let started = h
        .provider
        .begin_login("https://app.example.com/api/auth/callback")
        .unwrap();
    let url = url_params(&started.authorization_url);
    assert_eq!(url["client_id"], "client_TEST");
    assert_eq!(url["response_type"], "code");
    assert_eq!(url["provider"], "authkit");
    assert_eq!(
        url["redirect_uri"],
        "https://app.example.com/api/auth/callback"
    );
    assert_eq!(url["code_challenge_method"], "S256");
    assert_eq!(url["state"], started.state);
    assert!(url["code_challenge"].len() >= 43);
    assert!(started.code_verifier.expose().len() >= 43);
    assert!(
        !started
            .authorization_url
            .contains(started.code_verifier.expose())
    );
}

fn url_params(url: &str) -> HashMap<String, String> {
    let query = url.split_once('?').unwrap().1;
    query
        .split('&')
        .filter_map(|pair| pair.split_once('='))
        .map(|(k, v)| (k.to_owned(), urldecode(v)))
        .collect()
}

fn urldecode(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            out.push(
                u8::from_str_radix(std::str::from_utf8(&bytes[i + 1..i + 3]).unwrap(), 16).unwrap(),
            );
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).unwrap()
}

#[tokio::test]
async fn the_code_exchange_yields_a_sealed_session_with_the_verified_identity() {
    let h = harness().await;
    let (sealed, login) = login(&h).await;
    assert_eq!(login.session.session_id, "session_01");
    assert_eq!(login.session.external_user_id.as_str(), "user_01");
    assert_eq!(login.profile.email, "ada@example.org");
    assert_eq!(login.profile.display_name.as_deref(), Some("Ada Lovelace"));

    let exchange = h
        .state
        .lock()
        .unwrap()
        .requests
        .iter()
        .find(|(_, path, _)| path == "/user_management/authenticate")
        .cloned()
        .unwrap();
    let body: serde_json::Value = serde_json::from_str(&exchange.2).unwrap();
    assert_eq!(body["code"], "code_1");
    assert!(body["code_verifier"].as_str().unwrap().len() >= 43);

    assert!(
        !sealed.expose().contains("eyJ"),
        "no plaintext JWT in the cookie"
    );
    assert!(!sealed.expose().contains("refresh_1"));
    assert_eq!(
        h.provider.inspect_session(&sealed).await.unwrap(),
        SessionStatus::Active(login.session)
    );
}

#[tokio::test]
async fn tampered_foreign_and_empty_cookies_are_invalid() {
    let h = harness().await;
    let (sealed, _) = login(&h).await;

    let mut flipped = sealed.expose().to_owned();
    let middle = flipped.len() / 2;
    let replacement = if flipped.as_bytes()[middle] == b'A' {
        "B"
    } else {
        "A"
    };
    flipped.replace_range(middle..=middle, replacement);
    let other = WorkOsIdentityProvider::with_base_url(
        &config("another-cookie-password-of-32-characters!"),
        &h.base_url,
    );
    for (name, provider, cookie) in [
        ("bit flip", &h.provider, flipped.as_str()),
        ("empty", &h.provider, ""),
        ("garbage", &h.provider, "not-a-sealed-session"),
        ("sealed under another password", &other, sealed.expose()),
    ] {
        assert_eq!(
            provider
                .inspect_session(&OpaqueSecret::new(cookie))
                .await
                .unwrap(),
            SessionStatus::Invalid,
            "{name}"
        );
    }
}

#[tokio::test]
async fn an_access_token_not_signed_by_the_provider_key_is_rejected() {
    let h = harness().await;
    h.state.lock().unwrap().jwks_modulus = other_modulus();
    let started = h
        .provider
        .begin_login("https://app.example.com/cb")
        .unwrap();
    let result = h
        .provider
        .complete_login("code_1", &started.code_verifier)
        .await;
    assert!(matches!(result, Err(IdentityError::Rejected)), "{result:?}");
}

#[tokio::test]
async fn an_expired_access_token_reads_as_expired_and_refreshes() {
    let h = harness().await;
    h.state.lock().unwrap().exchange_lifetime = LIFETIME_BEFORE_EXPIRY;
    let (sealed, login) = login(&h).await;
    wait_until_expired(&h.provider, &sealed).await;
    assert_eq!(
        h.provider.inspect_session(&sealed).await.unwrap(),
        SessionStatus::Expired(login.session.clone())
    );

    let RefreshOutcome::Refreshed {
        sealed_session,
        session,
    } = h.provider.refresh_session(&sealed).await.unwrap()
    else {
        panic!("refresh should succeed");
    };
    assert_eq!(session, login.session);
    assert_ne!(sealed_session, sealed);
    assert_eq!(
        h.provider.inspect_session(&sealed_session).await.unwrap(),
        SessionStatus::Active(session)
    );
}

#[tokio::test]
async fn a_revoked_refresh_token_is_rejected_and_an_outage_is_unavailable() {
    let h = harness().await;
    h.state.lock().unwrap().exchange_lifetime = LIFETIME_BEFORE_EXPIRY;
    let (sealed, _) = login(&h).await;
    wait_until_expired(&h.provider, &sealed).await;

    h.state.lock().unwrap().refresh_status = 401;
    assert!(matches!(
        h.provider.refresh_session(&sealed).await,
        Ok(RefreshOutcome::Rejected)
    ));
    h.state.lock().unwrap().refresh_status = 500;
    assert!(matches!(
        h.provider.refresh_session(&sealed).await,
        Err(IdentityError::Unavailable)
    ));
}

#[tokio::test]
async fn login_state_round_trips_and_is_not_interchangeable_with_a_session() {
    let h = harness().await;
    let state = LoginState {
        state: "s".to_owned(),
        code_verifier: OpaqueSecret::new("v".repeat(43)),
        return_to: ReturnPath::parse("/projects").unwrap(),
        expires_at_unix: now() + 600,
    };
    let sealed = h.provider.seal_login_state(&state).unwrap();
    assert_eq!(h.provider.open_login_state(sealed.expose()), Some(state));
    assert_eq!(h.provider.open_login_state("garbage"), None);
    assert_eq!(
        h.provider.inspect_session(&sealed).await.unwrap(),
        SessionStatus::Invalid,
        "a login-state cookie is not a session"
    );
    let (session, _) = login(&h).await;
    assert_eq!(h.provider.open_login_state(session.expose()), None);
}

#[tokio::test]
async fn sessions_can_be_listed_and_revoked() {
    let h = harness().await;
    let ids = h
        .provider
        .list_session_ids(&ExternalUserId::parse("user_01").unwrap())
        .await
        .unwrap();
    assert_eq!(ids, ["session_01"]);
    h.provider.revoke_session("session_01").await.unwrap();
    let revoke = h
        .state
        .lock()
        .unwrap()
        .requests
        .iter()
        .find(|(_, path, _)| path == "/user_management/sessions/revoke")
        .cloned()
        .unwrap();
    assert!(revoke.2.contains("session_01"));
}

/// A provider with no cached signing keys, as after a restart.
fn cold_provider(h: &Harness) -> WorkOsIdentityProvider {
    WorkOsIdentityProvider::with_base_url(&config(PASSWORD), &h.base_url)
}

#[tokio::test]
async fn a_signing_key_outage_is_unavailable_not_an_invalid_session() {
    let h = harness().await;
    let (sealed, login) = login(&h).await;

    h.state.lock().unwrap().jwks_status = 503;
    let restarted = cold_provider(&h);
    assert!(
        matches!(
            restarted.inspect_session(&sealed).await,
            Err(IdentityError::Unavailable)
        ),
        "a JWKS outage must not read as an invalid session"
    );

    // Once the keys are back the same cookie is valid again: nothing was lost.
    h.state.lock().unwrap().jwks_status = 200;
    assert_eq!(
        restarted.inspect_session(&sealed).await.unwrap(),
        SessionStatus::Active(login.session)
    );
}

#[tokio::test]
async fn a_bad_signature_is_invalid_even_when_the_keys_are_reachable() {
    let h = harness().await;
    let (sealed, _) = login(&h).await;
    h.state.lock().unwrap().jwks_modulus = other_modulus();
    assert_eq!(
        cold_provider(&h).inspect_session(&sealed).await.unwrap(),
        SessionStatus::Invalid
    );
}

#[tokio::test]
async fn a_key_fetch_that_fails_twice_around_a_good_probe_keeps_the_session() {
    let h = harness().await;
    let (sealed, _) = login(&h).await;
    // The SDK's fetch fails, the adapter's own probe succeeds, and the SDK's
    // second fetch fails again. The token is genuinely signed, so the session
    // must stay recoverable rather than read as invalid.
    h.state.lock().unwrap().jwks_script = vec![503, 200, 503];
    let restarted = cold_provider(&h);
    assert!(
        matches!(
            restarted.inspect_session(&sealed).await,
            Err(IdentityError::Unavailable)
        ),
        "a flapping key endpoint must not read as an invalid session"
    );
}

#[tokio::test]
async fn invalid_grant_ends_the_session_at_http_400_as_well_as_401() {
    let h = harness().await;
    h.state.lock().unwrap().exchange_lifetime = LIFETIME_BEFORE_EXPIRY;
    let (sealed, _) = login(&h).await;
    wait_until_expired(&h.provider, &sealed).await;

    h.state.lock().unwrap().refresh_status = 400;
    assert!(matches!(
        h.provider.refresh_session(&sealed).await,
        Ok(RefreshOutcome::Rejected)
    ));
    for transient in [429, 500, 502] {
        h.state.lock().unwrap().refresh_status = transient;
        assert!(
            matches!(
                h.provider.refresh_session(&sealed).await,
                Err(IdentityError::Unavailable)
            ),
            "{transient}"
        );
    }
}

fn jwks_key(extra: serde_json::Value) -> serde_json::Value {
    let mut key = json!({ "kty": "RSA", "kid": "key_1", "alg": "RS256", "use": "sig",
        "n": JWK_MODULUS, "e": "AQAB" });
    for (name, value) in extra.as_object().unwrap() {
        key[name] = value.clone();
    }
    key
}

async fn exchange_with_keys(keys: serde_json::Value) -> Result<(), IdentityError> {
    let h = harness().await;
    h.state.lock().unwrap().jwks_keys = Some(keys);
    let started = h
        .provider
        .begin_login("https://app.example.com/cb")
        .unwrap();
    h.provider
        .complete_login("code_1", &started.code_verifier)
        .await
        .map(|_| ())
}

#[tokio::test]
async fn unusable_signing_keys_are_unavailable_not_an_invalid_session() {
    let other_kid = jwks_key(json!({ "kid": "key_2" }));
    let cases = [
        ("no key for the kid", json!([other_kid])),
        (
            "duplicate keys for the kid",
            json!([jwks_key(json!({})), jwks_key(json!({}))]),
        ),
        ("not an RSA key", json!([jwks_key(json!({ "kty": "EC" }))])),
        ("wrong use", json!([jwks_key(json!({ "use": "enc" }))])),
        (
            "bad key material",
            json!([jwks_key(json!({ "n": "!!not-base64!!" }))]),
        ),
        (
            "key_ops without verify",
            json!([jwks_key(json!({ "key_ops": ["sign"] }))]),
        ),
    ];
    for (name, keys) in cases {
        assert!(
            matches!(
                exchange_with_keys(keys).await,
                Err(IdentityError::Unavailable)
            ),
            "{name}"
        );
    }
}

#[tokio::test]
async fn a_token_that_is_not_yet_valid_is_retryable_not_an_invalid_session() {
    let h = harness().await;
    h.state.lock().unwrap().not_before_in = Some(3600);
    let started = h
        .provider
        .begin_login("https://app.example.com/cb")
        .unwrap();
    let result = h
        .provider
        .complete_login("code_1", &started.code_verifier)
        .await;
    assert!(
        matches!(result, Err(IdentityError::Unavailable)),
        "{result:?}"
    );
}

#[tokio::test]
async fn an_expired_token_read_through_a_key_blip_still_reads_as_expired() {
    let h = harness().await;
    h.state.lock().unwrap().exchange_lifetime = LIFETIME_BEFORE_EXPIRY;
    let (sealed, login) = login(&h).await;
    wait_until_expired(&h.provider, &sealed).await;
    // The SDK's key fetch fails, the adapter's probe succeeds and shows a
    // genuinely signed but expired token, then the SDK's retry succeeds.
    h.state.lock().unwrap().jwks_script = vec![503, 200, 200];
    assert_eq!(
        cold_provider(&h).inspect_session(&sealed).await.unwrap(),
        SessionStatus::Expired(login.session)
    );
}
