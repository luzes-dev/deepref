#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]
use axum::{
    body::Body,
    http::{Method, Request, StatusCode, header},
};
use deepref_app::account_export::{EXPORT_ATTESTATION_TOKEN, activate_export_retention};
use serde_json::json;
use tower::ServiceExt;
use uuid::Uuid;
mod support;
use support::authz::Harness;

#[tokio::test]
async fn test_export_request() {
    let Some(h) = Harness::new().await else {
        return;
    };
    let a = h.sign_in("export_owner", "a@example.org").await;
    assert_eq!(
        h.send(&a, Method::POST, "/me/export", None).await.0,
        StatusCode::CONFLICT
    );
    activate_export_retention(&h.pool, EXPORT_ATTESTATION_TOKEN)
        .await
        .unwrap();
    let (status, body) = h.json(&a, Method::POST, "/me/export", "").await;
    assert_eq!(status, StatusCode::ACCEPTED, "{body}");
    assert_eq!(body["status"], "requested");
    assert_eq!(
        h.send(&a, Method::POST, "/me/export", None).await.0,
        StatusCode::CONFLICT
    );
}
#[tokio::test]
async fn test_export_status() {
    let Some(h) = Harness::new().await else {
        return;
    };
    let a = h.sign_in("export_a", "a@example.org").await;
    let b = h.sign_in("export_b", "b@example.org").await;
    activate_export_retention(&h.pool, EXPORT_ATTESTATION_TOKEN)
        .await
        .unwrap();
    let (_, requested) = h.json(&a, Method::POST, "/me/export", "").await;
    let (status, body) = h.json(&a, Method::GET, "/me/export", "").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["id"], requested["id"]);
    let (_, other) = h.json(&b, Method::GET, "/me/export", "").await;
    assert!(other.is_null());
}
#[tokio::test]
async fn test_export_download() {
    let Some(h) = Harness::new().await else {
        return;
    };
    let a = h.sign_in("export_a", "a@example.org").await;
    let b = h.sign_in("export_b", "b@example.org").await;
    activate_export_retention(&h.pool, EXPORT_ATTESTATION_TOKEN)
        .await
        .unwrap();
    let (_, requested) = h.json(&a, Method::POST, "/me/export", "").await;
    let path = format!("/me/export/{}/download", requested["id"].as_str().unwrap());
    assert_eq!(
        h.send(&a, Method::GET, &path, None).await.0,
        StatusCode::CONFLICT
    );
    let foreign = h.send(&b, Method::GET, &path, None).await;
    let absent = h
        .send(
            &b,
            Method::GET,
            &format!("/me/export/{}/download", Uuid::new_v4()),
            None,
        )
        .await;
    assert_eq!(foreign, absent);
    assert_eq!(foreign.0, StatusCode::NOT_FOUND);
}
#[tokio::test]
async fn test_export_cancel() {
    let Some(h) = Harness::new().await else {
        return;
    };
    let a = h.sign_in("export_a", "a@example.org").await;
    let b = h.sign_in("export_b", "b@example.org").await;
    activate_export_retention(&h.pool, EXPORT_ATTESTATION_TOKEN)
        .await
        .unwrap();
    let (_, requested) = h.json(&a, Method::POST, "/me/export", "").await;
    let path = format!("/me/export/{}", requested["id"].as_str().unwrap());
    assert_eq!(
        h.send(&b, Method::DELETE, &path, None).await.0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        h.send(&a, Method::DELETE, &path, None).await.0,
        StatusCode::NO_CONTENT
    );
    let (_, body) = h.json(&a, Method::GET, "/me/export", "").await;
    assert_eq!(body["status"], "cancelled");
}

#[tokio::test]
async fn cancelled_export_cannot_continue_an_already_prepared_download_with_one_connection() {
    let Some(pool) = deepref_app::test_isolation::isolated_pool_from_env("DATABASE_URL", 1).await
    else {
        return;
    };
    let store = deepref_documents::DocumentStore::memory();
    let h = Harness::with_state(pool, |state| state.with_document_store(store.clone()));
    let user = h.sign_in("prepared-download", "download@example.org").await;
    activate_export_retention(&h.pool, EXPORT_ATTESTATION_TOKEN)
        .await
        .unwrap();
    let (_, requested) = h.json(&user, Method::POST, "/me/export", "").await;
    let id = requested["id"].as_str().unwrap();
    let jobs = deepref_app::claim_jobs(
        &h.pool,
        "http-download",
        std::time::Duration::from_secs(3600),
        &[],
        1,
    )
    .await
    .unwrap();
    assert_eq!(jobs.len(), 1);
    deepref_app::account_export::process_export_job(
        &h.pool,
        &store,
        &json!({"export_id":id}),
        &jobs[0].lease("http-download"),
    )
    .await
    .unwrap();
    let response = h
        .app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("/me/export/{id}/download"))
                .header(
                    header::COOKIE,
                    format!("{}={}", support::SESSION_COOKIE, user.cookie),
                )
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    // The response has acquired the real object and written its access audit,
    // but no body has been polled. Cancellation must fence that next read.
    assert_eq!(
        h.send(&user, Method::DELETE, &format!("/me/export/{id}"), None)
            .await
            .0,
        StatusCode::NO_CONTENT
    );
    let bytes = axum::body::to_bytes(response.into_body(), 1024 * 1024).await;
    assert!(
        bytes.is_err(),
        "a prepared response cannot stream a cancelled archive"
    );
}
#[tokio::test]
async fn machine_credentials_cannot_reach_any_export_route() {
    let Some(h) = Harness::new().await else {
        return;
    };
    let a = h.sign_in("export_machine", "a@example.org").await;
    let (status, key) = h
        .json(
            &a,
            Method::POST,
            &format!("/workspaces/{}/api-keys", a.workspace.as_uuid()),
            &json!({"name":"machine","scopes":["project:view"]}).to_string(),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{key}");
    for (method, path) in [
        (Method::POST, "/me/export".to_owned()),
        (Method::GET, "/me/export".to_owned()),
        (
            Method::GET,
            format!("/me/export/{}/download", Uuid::new_v4()),
        ),
        (Method::DELETE, format!("/me/export/{}", Uuid::new_v4())),
    ] {
        let response = h
            .app
            .clone()
            .oneshot(
                Request::builder()
                    .method(method)
                    .uri(path)
                    .header(
                        header::AUTHORIZATION,
                        format!("Bearer {}", key["secret"].as_str().unwrap()),
                    )
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }
}
