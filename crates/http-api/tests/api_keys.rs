#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]
//! End-to-end key authentication, live central policy and browser-only lifecycle.
use axum::{
    body::{Body, to_bytes},
    http::{Method, Request, StatusCode, header},
    response::Response,
};
use deepref_application::authorization::Authorizer;
use deepref_domain::{
    ApiKeyId, Principal,
    authorization::{Permission, Resource},
};
use serde_json::{Value, json};
use sqlx::Row;
use tower::ServiceExt;
use uuid::Uuid;
mod support;
use support::authz::{Harness, Person};

async fn bearer(h: &Harness, token: &str, method: Method, path: &str) -> Response {
    h.app
        .clone()
        .oneshot(
            Request::builder()
                .method(method)
                .uri(path)
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .header("x-actor-kind", "user")
                .header("x-actor-id", Uuid::new_v4().to_string())
                .header("x-correlation-id", support::authz::CORRELATION)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap()
}
async fn create(h: &Harness, person: &Person, scopes: &[&str]) -> Value {
    let (status, body) = h
        .json(
            person,
            Method::POST,
            &format!("/workspaces/{}/api-keys", person.workspace.as_uuid()),
            &json!({"name":"automation", "scopes":scopes}).to_string(),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    body
}
async fn body(response: Response) -> Value {
    serde_json::from_slice(&to_bytes(response.into_body(), 128 * 1024).await.unwrap()).unwrap()
}

#[tokio::test]
async fn lifecycle_is_human_only_and_tenant_scoped() {
    let Some(h) = Harness::new().await else {
        return;
    };
    let owner = h.sign_in("keys_owner", "keys@example.org").await;
    let foreign = h.sign_in("keys_foreign", "foreign@example.org").await;
    let project = h.create_project(&owner, "metadata").await;
    let key = create(&h, &owner, &["project:view"]).await;
    let secret = key["secret"].as_str().unwrap();
    let id = key["key"]["id"].as_str().unwrap();
    let path = format!("/workspaces/{}/api-keys", owner.workspace.as_uuid());
    for method in [Method::GET, Method::POST] {
        assert_eq!(
            h.send(
                &foreign,
                method.clone(),
                &path,
                Some(r#"{"name":"foreign","scopes":["project:view"]}"#)
            )
            .await
            .0,
            StatusCode::NOT_FOUND
        );
        assert_eq!(
            bearer(&h, secret, method, &path).await.status(),
            StatusCode::NOT_FOUND
        );
    }
    for suffix in [format!("/{id}"), format!("/{id}/rotate")] {
        let method = if suffix.ends_with("rotate") {
            Method::POST
        } else {
            Method::DELETE
        };
        assert_eq!(
            h.send(&foreign, method, &format!("{path}{suffix}"), None)
                .await
                .0,
            StatusCode::NOT_FOUND
        );
    }
    assert_eq!(
        bearer(&h, secret, Method::GET, &format!("/projects/{project}"))
            .await
            .status(),
        StatusCode::OK
    );
    assert_eq!(
        bearer(
            &h,
            secret,
            Method::GET,
            &format!("/projects/{project}/reports")
        )
        .await
        .status(),
        StatusCode::OK
    );
    for (method, path) in [
        (Method::GET, "/me".into()),
        (Method::GET, "/projects".into()),
        (
            Method::POST,
            format!("/projects/{project}/reports/{}/screening", Uuid::new_v4()),
        ),
        (Method::PATCH, format!("/projects/{project}")),
    ] {
        assert_eq!(
            bearer(&h, secret, method, &path).await.status(),
            StatusCode::NOT_FOUND
        );
    }
    let foreign_project = h.create_project(&foreign, "foreign").await;
    for project in [foreign_project, Uuid::new_v4()] {
        assert_eq!(
            bearer(&h, secret, Method::GET, &format!("/projects/{project}"))
                .await
                .status(),
            StatusCode::NOT_FOUND
        );
    }
    let (status, listed) = h.json(&owner, Method::GET, &path, "{}").await;
    assert_eq!(status, StatusCode::OK);
    assert!(!listed.to_string().contains(secret));
    assert!(listed[0].get("secret").is_none());
    assert!(listed[0].get("secret_hash").is_none());
    assert!(listed[0]["last_used_at"].is_string());
    let stored = sqlx::query("SELECT secret_hash FROM api_keys WHERE id = $1")
        .bind(Uuid::parse_str(id).unwrap())
        .fetch_one(&h.pool)
        .await
        .unwrap();
    assert_eq!(stored.get::<Vec<u8>, _>("secret_hash").len(), 32);
    let audit: Vec<Value> = sqlx::query_scalar(
        "SELECT metadata FROM admin_audit_events WHERE event_type LIKE 'api_key.%'",
    )
    .fetch_all(&h.pool)
    .await
    .unwrap();
    assert_eq!(audit, vec![json!({"key_id": id})]);
    assert!(!format!("{audit:?}").contains(secret));
}

#[tokio::test]
async fn scope_expiry_revocation_and_live_membership_fail_closed() {
    let Some(h) = Harness::new().await else {
        return;
    };
    let owner = h.sign_in("keys_live", "live@example.org").await;
    let project = h.create_project(&owner, "live").await;
    let route = format!("/projects/{project}");
    let limited = create(&h, &owner, &["workspace:view"]).await;
    assert_eq!(
        bearer(&h, limited["secret"].as_str().unwrap(), Method::GET, &route)
            .await
            .status(),
        StatusCode::NOT_FOUND
    );
    let key = create(&h, &owner, &["project:view"]).await;
    let token = key["secret"].as_str().unwrap();
    let id = Uuid::parse_str(key["key"]["id"].as_str().unwrap()).unwrap();
    let principal = deepref_app::api_keys::authenticate(&h.pool, token)
        .await
        .unwrap()
        .unwrap();
    assert!(principal.user_id().is_none());
    let pdp = deepref_application::authorization::PolicyDecisionPoint::new(
        deepref_app::authorization::PostgresAuthorizationStore::new(h.pool.clone()),
    );
    assert!(
        pdp.require(
            &principal,
            Permission::ProjectView,
            Resource::Project(project.into())
        )
        .await
        .is_ok()
    );
    // A stale, already authenticated principal does not bypass revocation.
    let path = format!("/workspaces/{}/api-keys/{id}", owner.workspace.as_uuid());
    assert_eq!(
        h.send(&owner, Method::DELETE, &path, None).await.0,
        StatusCode::NO_CONTENT
    );
    assert!(
        pdp.require(
            &principal,
            Permission::ProjectView,
            Resource::Project(project.into())
        )
        .await
        .is_err()
    );
    assert_eq!(
        bearer(&h, token, Method::GET, &route).await.status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        h.send(&owner, Method::DELETE, &path, None).await.0,
        StatusCode::NO_CONTENT
    );
    let key = create(&h, &owner, &["project:view"]).await;
    let id = Uuid::parse_str(key["key"]["id"].as_str().unwrap()).unwrap();
    sqlx::query("UPDATE api_keys SET created_at = now() - interval '2 days', expires_at = now() - interval '1 day' WHERE id=$1")
        .bind(id).execute(&h.pool).await.unwrap();
    assert_eq!(
        bearer(&h, key["secret"].as_str().unwrap(), Method::GET, &route)
            .await
            .status(),
        StatusCode::UNAUTHORIZED
    );
    let key = create(&h, &owner, &["project:view"]).await;
    let token = key["secret"].as_str().unwrap();
    let principal = deepref_app::api_keys::authenticate(&h.pool, token)
        .await
        .unwrap()
        .unwrap();
    sqlx::query("UPDATE workspace_memberships SET status='deactivated' WHERE user_id=$1")
        .bind(owner.user.as_uuid())
        .execute(&h.pool)
        .await
        .unwrap();
    assert!(
        pdp.require(
            &principal,
            Permission::ProjectView,
            Resource::Project(project.into())
        )
        .await
        .is_err()
    );
    assert_eq!(
        bearer(&h, token, Method::GET, &route).await.status(),
        StatusCode::UNAUTHORIZED
    );
}

#[tokio::test]
async fn rotation_is_atomic_and_secret_response_is_not_cacheable() {
    let Some(h) = Harness::new().await else {
        return;
    };
    let owner = h.sign_in("keys_rotate", "rotate@example.org").await;
    let project = h.create_project(&owner, "rotate").await;
    let key = create(&h, &owner, &["project:view"]).await;
    let id = key["key"]["id"].as_str().unwrap();
    let request = Request::builder()
        .method(Method::POST)
        .uri(format!(
            "/workspaces/{}/api-keys/{id}/rotate",
            owner.workspace.as_uuid()
        ))
        .header(
            header::COOKIE,
            format!("{}={}", support::SESSION_COOKIE, owner.cookie),
        )
        .header(header::ORIGIN, support::ORIGIN)
        .body(Body::empty())
        .unwrap();
    let response = h.app.clone().oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::CREATED);
    assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
    let rotated = body(response).await;
    assert_ne!(rotated["secret"], key["secret"]);
    assert_ne!(rotated["key"]["id"], key["key"]["id"]);
    for (token, status) in [
        (key["secret"].as_str().unwrap(), StatusCode::UNAUTHORIZED),
        (rotated["secret"].as_str().unwrap(), StatusCode::OK),
    ] {
        assert_eq!(
            bearer(&h, token, Method::GET, &format!("/projects/{project}"))
                .await
                .status(),
            status
        );
    }
    let retry = h
        .send(
            &owner,
            Method::POST,
            &format!(
                "/workspaces/{}/api-keys/{id}/rotate",
                owner.workspace.as_uuid()
            ),
            None,
        )
        .await;
    assert_eq!(retry.0, StatusCode::NOT_FOUND);
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM api_keys WHERE revoked_at IS NULL")
        .fetch_one(&h.pool)
        .await
        .unwrap();
    assert_eq!(count, 1);
    let events: Vec<String> = sqlx::query_scalar("SELECT event_type FROM admin_audit_events WHERE event_type LIKE 'api_key.%' ORDER BY event_type").fetch_all(&h.pool).await.unwrap();
    assert_eq!(
        events,
        [
            "api_key.created",
            "api_key.created",
            "api_key.revoked",
            "api_key.rotated"
        ]
    );
}

#[tokio::test]
async fn invalid_keys_are_uniform_and_audit_flood_is_bounded() {
    let Some(h) = Harness::new().await else {
        return;
    };
    let owner = h.sign_in("keys_invalid", "invalid@example.org").await;
    let project = h.create_project(&owner, "invalid").await;
    let key = create(&h, &owner, &["project:view"]).await;
    let secret = key["secret"].as_str().unwrap();
    let route = format!("/projects/{project}");
    let mut changed = secret.as_bytes().to_vec();
    let pos = changed.len() - 3;
    changed[pos] = if changed[pos] == b'A' { b'B' } else { b'A' };
    let wrong = String::from_utf8(changed).unwrap();
    for invalid in [
        "malformed",
        "",
        &wrong,
        &format!("dr_key_v1_{}.{}", Uuid::new_v4(), "A".repeat(43)),
    ] {
        assert_eq!(
            bearer(&h, invalid, Method::GET, &route).await.status(),
            StatusCode::UNAUTHORIZED
        );
    }
    for _ in 0..30 {
        assert_eq!(
            bearer(&h, &Uuid::new_v4().to_string(), Method::GET, &route)
                .await
                .status(),
            StatusCode::UNAUTHORIZED
        );
    }
    let rows: Vec<Value> = sqlx::query_scalar("SELECT metadata FROM admin_audit_events WHERE event_type='auth.rejected' AND metadata->>'stage'='api_key'").fetch_all(&h.pool).await.unwrap();
    assert!(rows.len() <= 2, "minute boundary may split the batch");
    assert!(!serde_json::to_string(&rows).unwrap().contains(secret));
    let response = h
        .app
        .clone()
        .oneshot(
            Request::builder()
                .uri(&route)
                .header(header::AUTHORIZATION, format!("Bearer {secret}"))
                .header(header::AUTHORIZATION, format!("Bearer {secret}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    let response = h
        .app
        .clone()
        .oneshot(
            Request::builder()
                .uri(&route)
                .header(header::AUTHORIZATION, format!("Bearer {secret}"))
                .header(
                    header::COOKIE,
                    format!("{}={}", support::SESSION_COOKIE, owner.cookie),
                )
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn manager_role_and_key_quota_are_enforced_by_live_facts() {
    let Some(h) = Harness::new().await else {
        return;
    };
    let owner = h.sign_in("keys_manager", "manager@example.org").await;
    let member = h.sign_in("keys_member", "member@example.org").await;
    h.add_member(&owner, &member, deepref_domain::WorkspaceRole::Member)
        .await;
    let path = format!("/workspaces/{}/api-keys", owner.workspace.as_uuid());
    assert_eq!(
        h.send(&member, Method::GET, &path, None).await.0,
        StatusCode::NOT_FOUND
    );
    let request = json!({"name":"privileged", "scopes":["screening:perform"]});
    assert_eq!(
        h.json(&owner, Method::POST, &path, &request.to_string())
            .await
            .0,
        StatusCode::BAD_REQUEST
    );
    sqlx::query("INSERT INTO api_keys(id,workspace_id,created_by_user_id,name,prefix,secret_hash,scopes) SELECT gen_random_uuid(),$1,$2,'quota','dr_key_v1_',decode(repeat('00',32),'hex'),ARRAY['project:view'] FROM generate_series(1,100)")
        .bind(owner.workspace.as_uuid()).bind(owner.user.as_uuid()).execute(&h.pool).await.unwrap();
    assert_eq!(
        h.json(
            &owner,
            Method::POST,
            &path,
            &json!({"name":"overflow", "scopes":["project:view"]}).to_string()
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
    // Rotation still works at the limit because the old key is revoked first.
    let id: Uuid = sqlx::query_scalar("SELECT id FROM api_keys LIMIT 1")
        .fetch_one(&h.pool)
        .await
        .unwrap();
    assert_eq!(
        h.send(&owner, Method::POST, &format!("{path}/{id}/rotate"), None)
            .await
            .0,
        StatusCode::CREATED
    );
    let principal = Principal::service(ApiKeyId::new(id), owner.workspace);
    let pdp = deepref_application::authorization::PolicyDecisionPoint::new(
        deepref_app::authorization::PostgresAuthorizationStore::new(h.pool.clone()),
    );
    for permission in Permission::ALL
        .into_iter()
        .filter(|permission| !permission.machine_allowed())
    {
        assert!(
            pdp.require(&principal, permission, Resource::Workspace(owner.workspace))
                .await
                .is_err()
        );
    }
}

#[tokio::test]
async fn workspace_deletion_erases_credentials_and_keeps_revocation_audit() {
    let Some(h) = Harness::new().await else {
        return;
    };
    let owner = h
        .sign_in("keys_delete_workspace", "delete-workspace@example.org")
        .await;
    let key = create(&h, &owner, &["project:view"]).await;
    deepref_app::workspaces::delete_workspace(
        &h.pool,
        owner.user,
        owner.workspace,
        deepref_app::identity::AuditRequest {
            request_id: None,
            session_ref: None,
        },
    )
    .await
    .unwrap();
    assert!(
        deepref_app::api_keys::authenticate(&h.pool, key["secret"].as_str().unwrap())
            .await
            .unwrap()
            .is_none()
    );
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM api_keys")
        .fetch_one(&h.pool)
        .await
        .unwrap();
    assert_eq!(count, 0);
    let count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM admin_audit_events WHERE event_type='api_key.revoked'",
    )
    .fetch_one(&h.pool)
    .await
    .unwrap();
    assert_eq!(count, 1);
    // The audit metadata allowlist also protects against accidental leakage.
    let result = sqlx::query("INSERT INTO admin_audit_events (id,event_type,principal_kind,principal_user_id,workspace_id,metadata) VALUES ($1,'api_key.created','user',$2,$3,$4)")
        .bind(Uuid::new_v4()).bind(owner.user.as_uuid()).bind(owner.workspace.as_uuid())
        .bind(json!({"key_id": key["key"]["id"], "secret": key["secret"]})).execute(&h.pool).await;
    assert!(result.is_err());
}
