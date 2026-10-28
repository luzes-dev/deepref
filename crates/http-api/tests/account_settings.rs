#![allow(clippy::unwrap_used)]
use axum::http::{Method, StatusCode};
mod support;
use support::authz::Harness;

#[tokio::test]
async fn only_current_memberships_and_pdp_capabilities_are_listed() {
    let Some(h) = Harness::new().await else {
        return;
    };
    let a = h.sign_in("settings-a", "a@example.org").await;
    let b = h.sign_in("settings-b", "b@example.org").await;
    let (status, body) = h.json(&a, Method::GET, "/me/workspaces", "").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["items"].as_array().unwrap().len(), 1);
    assert_eq!(body["items"][0]["id"], a.workspace.as_uuid().to_string());
    assert_eq!(body["items"][0]["can_manage_keys"], true);
    assert_eq!(body["items"][0]["can_manage_billing"], true);
    assert!(
        !body
            .to_string()
            .contains(&b.workspace.as_uuid().to_string())
    );
    sqlx::query("UPDATE workspace_memberships SET role='member' WHERE user_id=$1")
        .bind(a.user.as_uuid())
        .execute(&h.pool)
        .await
        .unwrap();
    let (_, body) = h.json(&a, Method::GET, "/me/workspaces", "").await;
    assert_eq!(body["items"][0]["can_manage_keys"], false);
    assert_eq!(body["items"][0]["can_manage_billing"], false);
    sqlx::query("UPDATE workspace_memberships SET status='deactivated' WHERE user_id=$1")
        .bind(a.user.as_uuid())
        .execute(&h.pool)
        .await
        .unwrap();
    let (_, body) = h.json(&a, Method::GET, "/me/workspaces", "").await;
    assert_eq!(body["items"], serde_json::json!([]));
}

#[tokio::test]
async fn stable_pages_work_on_one_connection_and_reject_machine_or_revoked_session() {
    use axum::{
        body::Body,
        http::{Request, header},
    };
    use tower::ServiceExt;
    use uuid::Uuid;
    let Some(pool) = deepref_app::test_isolation::isolated_pool_from_env("DATABASE_URL", 1).await
    else {
        return;
    };
    let h = Harness::on(pool);
    let a = h.sign_in("settings-pages", "pages@example.org").await;
    for _ in 0..31 {
        let workspace = Uuid::new_v4();
        sqlx::query("INSERT INTO workspaces(id,kind,name,slug) VALUES($1,'organization','Repeated name',$2)").bind(workspace).bind(workspace.to_string()).execute(&h.pool).await.unwrap();
        sqlx::query("INSERT INTO workspace_memberships(id,workspace_id,user_id,role,status,grant_source) VALUES($1,$2,$3,'owner','active','local')").bind(Uuid::new_v4()).bind(workspace).bind(a.user.as_uuid()).execute(&h.pool).await.unwrap();
    }
    let (_, first) = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        h.json(&a, Method::GET, "/me/workspaces", ""),
    )
    .await
    .unwrap();
    assert_eq!(first["items"].as_array().unwrap().len(), 25);
    let cursor = first["next_cursor"].as_str().unwrap();
    let (_, second) = h
        .json(
            &a,
            Method::GET,
            &format!("/me/workspaces?after={cursor}"),
            "",
        )
        .await;
    assert_eq!(second["items"].as_array().unwrap().len(), 7);
    assert!(second["next_cursor"].is_null());
    let ids: Vec<&str> = first["items"]
        .as_array()
        .unwrap()
        .iter()
        .chain(second["items"].as_array().unwrap())
        .map(|item| item["id"].as_str().unwrap())
        .collect();
    assert!(ids.windows(2).all(|pair| pair[0] < pair[1]));
    let (_, issued) = h
        .json(
            &a,
            Method::POST,
            &format!("/workspaces/{}/api-keys", a.workspace.as_uuid()),
            r#"{"name":"Read-only","scopes":["workspace:view"]}"#,
        )
        .await;
    let secret = issued["secret"].as_str().unwrap();
    let response = h
        .app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/me/workspaces")
                .header(header::AUTHORIZATION, format!("Bearer {secret}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert!(
        !response.status().is_success(),
        "a key never discovers human settings"
    );
    sqlx::query("UPDATE user_sessions SET revoked_at=now() WHERE user_id=$1")
        .bind(a.user.as_uuid())
        .execute(&h.pool)
        .await
        .unwrap();
    assert_eq!(
        h.send(&a, Method::GET, "/me/workspaces", None).await.0,
        StatusCode::UNAUTHORIZED
    );
}
