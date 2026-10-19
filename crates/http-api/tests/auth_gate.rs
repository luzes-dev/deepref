#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]
//! Default-deny gating and the origin rule. No database is needed: every
//! request here is decided before a query runs.
use std::sync::Arc;

use axum::{
    Router,
    body::Body,
    http::{Method, Request, StatusCode, header},
};
use tower::ServiceExt;

mod support;
use support::{ORIGIN, SESSION_COOKIE, auth_app, fake_identity::FakeIdentity, unused_pool};

fn app() -> Router {
    auth_app(unused_pool(), Arc::new(FakeIdentity::default()))
}

async fn send(app: &Router, request: Request<Body>) -> (StatusCode, serde_json::Value) {
    let response = app.clone().oneshot(request).await.unwrap();
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null),
    )
}

fn path_with_values(path: &str) -> String {
    let mut out = String::new();
    let mut in_param = false;
    for c in path.chars() {
        match c {
            '{' => {
                in_param = true;
                out.push_str("00000000-0000-0000-0000-000000000001");
            }
            '}' => in_param = false,
            c if !in_param => out.push(c),
            _ => {}
        }
    }
    out
}

const OPEN: &[&str] = &[
    "/health",
    "/health/live",
    "/health/ready",
    "/health/dependencies",
    "/openapi.json",
    "/auth/login",
    "/auth/callback",
];

#[tokio::test]
async fn every_route_requires_a_session_unless_it_is_listed_open() {
    let app = app();
    let document = deepref_http_api::routes::openapi_document_json().unwrap();
    let mut checked = 0;
    for (path, item) in document["paths"].as_object().unwrap() {
        if OPEN.contains(&path.as_str()) || path == "/auth/logout" {
            continue;
        }
        for method in item.as_object().unwrap().keys() {
            let Ok(method) = method.to_uppercase().parse::<Method>() else {
                continue;
            };
            let request = Request::builder()
                .method(method.clone())
                .uri(path_with_values(path))
                .header("x-actor-kind", "user")
                .header("x-actor-id", "someone-elses-id")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from("{}"))
                .unwrap();
            let (status, body) = send(&app, request).await;
            assert_eq!(
                status,
                StatusCode::UNAUTHORIZED,
                "{method} {path} must reject an unauthenticated request carrying actor headers"
            );
            assert_eq!(body["code"], "UNAUTHENTICATED", "{method} {path}");
            checked += 1;
        }
    }
    assert!(checked > 100, "the sweep covered {checked} operations");
}

#[tokio::test]
async fn listed_open_routes_need_no_session() {
    let app = app();
    let request = Request::builder()
        .uri("/health/live")
        .body(Body::empty())
        .unwrap();
    assert_eq!(send(&app, request).await.0, StatusCode::OK);
    let request = Request::builder()
        .uri("/openapi.json")
        .body(Body::empty())
        .unwrap();
    assert_eq!(send(&app, request).await.0, StatusCode::OK);
}

#[tokio::test]
async fn a_tampered_cookie_is_rejected_as_an_expired_session_and_cleared() {
    let app = app();
    let request = Request::builder()
        .uri("/me")
        .header(
            header::COOKIE,
            format!("{SESSION_COOKIE}=not-issued-by-the-provider"),
        )
        .body(Body::empty())
        .unwrap();
    let response = app.clone().oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    let cleared = response
        .headers()
        .get_all(header::SET_COOKIE)
        .iter()
        .any(|v| {
            v.to_str()
                .unwrap()
                .starts_with(&format!("{SESSION_COOKIE}=;"))
        });
    assert!(cleared, "the bad cookie is cleared");
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(body["code"], "SESSION_EXPIRED");
}

#[tokio::test]
async fn logout_is_post_only() {
    let app = app();
    for method in [Method::GET, Method::PUT, Method::DELETE] {
        let request = Request::builder()
            .method(method.clone())
            .uri("/auth/logout")
            .body(Body::empty())
            .unwrap();
        assert_eq!(
            send(&app, request).await.0,
            StatusCode::METHOD_NOT_ALLOWED,
            "{method}"
        );
    }
}

fn post(origin: Option<&str>, fetch_site: Option<&str>, cookie: bool) -> Request<Body> {
    let mut request = Request::builder().method(Method::POST).uri("/projects");
    if let Some(origin) = origin {
        request = request.header(header::ORIGIN, origin);
    }
    if let Some(site) = fetch_site {
        request = request.header("sec-fetch-site", site);
    }
    if cookie {
        request = request.header(header::COOKIE, format!("{SESSION_COOKIE}=whatever"));
    }
    request
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(r#"{"name":"x"}"#))
        .unwrap()
}

#[tokio::test]
async fn cookie_authenticated_unsafe_requests_from_other_origins_get_403() {
    let app = app();
    for origin in [
        "https://evil.example",
        "null",
        "http://app.example.com",
        "https://app.example.com:8443",
        "https://sibling.example.com",
        "https://deeply.nested.app.example.com",
    ] {
        let (status, body) = send(&app, post(Some(origin), None, true)).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{origin}");
        assert_eq!(body["code"], "ORIGIN_REJECTED", "{origin}");
    }
    for site in ["same-site", "cross-site", "none"] {
        let (status, _) = send(&app, post(None, Some(site), true)).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "sec-fetch-site: {site}");
    }
    let (status, _) = send(&app, post(None, None, true)).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "neither header");
}

#[tokio::test]
async fn same_origin_requests_pass_the_origin_rule() {
    // The cookie is not a real session, so the request then fails
    // authentication (401). Reaching 401 shows the origin rule let it through.
    let app = app();
    assert_eq!(
        send(&app, post(Some(ORIGIN), None, true)).await.0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        send(&app, post(None, Some("same-origin"), true)).await.0,
        StatusCode::UNAUTHORIZED
    );
}

#[tokio::test]
async fn the_origin_rule_does_not_apply_without_the_session_cookie() {
    let app = app();
    let (status, body) = send(&app, post(Some("https://evil.example"), None, false)).await;
    assert_eq!(
        status,
        StatusCode::UNAUTHORIZED,
        "authentication decides, not the origin rule"
    );
    assert_eq!(body["code"], "UNAUTHENTICATED");
}

#[tokio::test]
async fn login_rejects_unsafe_return_targets_without_setting_a_cookie() {
    let app = app();
    for target in [
        "https://evil.example/",
        "//evil.example",
        "/\\evil.example",
        "javascript:alert(1)",
        "/api/auth/login",
    ] {
        let request = Request::builder()
            .uri(format!("/auth/login?return_to={}", urlencoding(target)))
            .body(Body::empty())
            .unwrap();
        let response = app.clone().oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST, "{target}");
        assert!(
            response.headers().get(header::SET_COOKIE).is_none(),
            "{target}"
        );
        assert!(
            response.headers().get(header::LOCATION).is_none(),
            "{target}"
        );
    }
}

fn urlencoding(value: &str) -> String {
    value
        .bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                (b as char).to_string()
            }
            _ => format!("%{b:02X}"),
        })
        .collect()
}
