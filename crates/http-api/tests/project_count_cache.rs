#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode, header},
};
use deepref_integrations::valkey_project_counts::ValkeyProjectCountCache;
use serde_json::Value;
use std::{sync::Arc, time::Duration};
use tower::ServiceExt;
use uuid::Uuid;
mod support;
use support::authz::{Harness, Person};

async fn request(h: &Harness, person: &Person, path: &str, strict: bool) -> (StatusCode, Value) {
    let mut builder = Request::builder().uri(path).header(
        header::COOKIE,
        format!("{}={}", support::SESSION_COOKIE, person.cookie),
    );
    if strict {
        builder = builder.header(header::CACHE_CONTROL, "no-cache");
    }
    let response = h
        .app
        .clone()
        .oneshot(builder.body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let bytes = to_bytes(response.into_body(), 128 * 1024).await.unwrap();
    (status, serde_json::from_slice(&bytes).unwrap())
}

#[tokio::test]
async fn warm_counts_never_authorize_foreign_users_revoked_members_or_revoked_keys() {
    let Ok(address) = std::env::var("TEST_VALKEY_URL") else {
        assert_ne!(std::env::var("DEEPREF_REQUIRE_VALKEY").as_deref(), Ok("1"));
        return;
    };
    let Some(pool) = support::authz::database().await else {
        return;
    };
    let cache = Arc::new(
        ValkeyProjectCountCache::from_config(&deepref_config::ValkeyConfig {
            url: address.into(),
            namespace: format!("contract-{}", Uuid::new_v4()),
            timeout: Duration::from_millis(100),
        })
        .unwrap(),
    );
    let h = Harness::with_state(pool, |mut state| {
        state.project_count_cache = Some(cache);
        state
    });
    let owner = h.sign_in("count-owner", "count-owner@example.org").await;
    let foreign = h
        .sign_in("count-foreign", "count-foreign@example.org")
        .await;
    let project = h.create_project(&owner, "private counts").await;
    let route = format!("/projects/{project}");
    let (status, first) = request(&h, &owner, &route, false).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(first["article_count"], 0);
    assert!(first["article_count_as_of"].is_string());
    let report = Uuid::new_v4();
    sqlx::query("INSERT INTO reports(id,title,raw) VALUES($1,'new count report','{}')")
        .bind(report)
        .execute(&h.pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO project_reports(project_id,report_id) VALUES($1,$2)")
        .bind(project)
        .bind(report)
        .execute(&h.pool)
        .await
        .unwrap();
    assert_eq!(
        request(&h, &owner, &route, false).await.1["article_count"],
        0
    );
    assert_eq!(
        request(&h, &owner, &route, true).await.1["article_count"],
        1
    );
    assert_eq!(
        request(&h, &foreign, &route, false).await.0,
        StatusCode::NOT_FOUND
    );
    let (status, key) = h
        .json(
            &owner,
            axum::http::Method::POST,
            &format!("/workspaces/{}/api-keys", owner.workspace.as_uuid()),
            r#"{"name":"count test","scopes":["project:view"]}"#,
        )
        .await;
    assert_eq!(status, StatusCode::CREATED);
    let bearer = |token: &str| {
        Request::builder()
            .uri(&route)
            .header(header::AUTHORIZATION, format!("Bearer {token}"))
            .body(Body::empty())
            .unwrap()
    };
    let token = key["secret"].as_str().unwrap();
    assert_eq!(
        h.app.clone().oneshot(bearer(token)).await.unwrap().status(),
        StatusCode::OK
    );
    sqlx::query("UPDATE api_keys SET revoked_at=now() WHERE id=$1")
        .bind(Uuid::parse_str(key["key"]["id"].as_str().unwrap()).unwrap())
        .execute(&h.pool)
        .await
        .unwrap();
    assert_eq!(
        h.app.clone().oneshot(bearer(token)).await.unwrap().status(),
        StatusCode::UNAUTHORIZED
    );
    sqlx::query("DELETE FROM workspace_memberships WHERE user_id=$1 AND workspace_id=$2")
        .bind(owner.user.as_uuid())
        .bind(owner.workspace.as_uuid())
        .execute(&h.pool)
        .await
        .unwrap();
    assert_eq!(
        request(&h, &owner, &route, false).await.0,
        StatusCode::NOT_FOUND
    );
    let (status, list) = request(&h, &owner, "/projects", false).await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        !list.to_string().contains(&project.to_string()),
        "live list filtering runs before cache access"
    );
}
