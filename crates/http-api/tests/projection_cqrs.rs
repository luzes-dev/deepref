#![allow(
    clippy::expect_used,
    clippy::panic,
    clippy::unwrap_used,
    clippy::string_slice
)]

use axum::http::StatusCode;
use deepref_http_api::routes::ProjectionStatusDto;
use serde_json::Value;
use uuid::Uuid;

mod support;
use support::authz::Harness;

#[tokio::test]
async fn projection_tenant_isolation_and_foreign_workspace_blocking() {
    let Some(harness) = Harness::new().await else {
        return;
    };

    let alice = harness
        .sign_in("alice-proj", "alice-proj@example.com")
        .await;
    let bob = harness.sign_in("bob-proj", "bob-proj@example.com").await;

    // Alice creates a project in her workspace
    let alice_project_id = Uuid::new_v4();
    sqlx::query("INSERT INTO projects (id, workspace_id, name) VALUES ($1, $2, 'Alice Project')")
        .bind(alice_project_id)
        .bind(alice.workspace.as_uuid())
        .execute(&harness.pool)
        .await
        .expect("alice project created");

    // 1. Nonexistent project ID returns 404
    let nonexistent_id = Uuid::new_v4();
    let (status, _) = harness
        .get(&alice, &format!("/projects/{nonexistent_id}/projection"))
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // 2. Bob (foreign workspace principal) tries to access Alice's project projection -> blocked (404/403)
    let (status, _) = harness
        .get(&bob, &format!("/projects/{alice_project_id}/projection"))
        .await;
    assert!(
        status == StatusCode::NOT_FOUND || status == StatusCode::FORBIDDEN,
        "foreign principal must be blocked by PDP middleware, got {status}"
    );

    // 3. Alice (authorized owner) accesses her project projection -> 200 OK with truthful DTO
    let (status, bytes) = harness
        .get(&alice, &format!("/projects/{alice_project_id}/projection"))
        .await;
    assert_eq!(status, StatusCode::OK);

    let dto: ProjectionStatusDto = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(dto.project_id, alice_project_id);
    // Project was just created without a recompute: must be pending, requested_revision 0, applied_revision 0
    assert_eq!(dto.state, "pending");
    assert_eq!(dto.requested_revision, 0);
    assert_eq!(dto.applied_revision, 0);
    assert_eq!(dto.watermark, 0);
    assert_eq!(dto.lag, 0);
    assert!(dto.last_success_at.is_none());
    assert!(dto.last_error.is_none());
    assert!(dto.rebuild_state.is_none());

    // 4. Graph endpoint with strict=true returns bounded graph and explicit projection metadata
    let (status, bytes) = harness
        .get(
            &alice,
            &format!("/projects/{alice_project_id}/graph?strict=true"),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    let json: Value = serde_json::from_slice(&bytes).unwrap();
    assert!(json.get("nodes").is_some());
    assert!(json.get("projection").is_some());
    assert_eq!(json["projection"]["revision"], 0);
    assert_eq!(json["projection"]["lag"], 0);

    // Clean up
    let _ = sqlx::query("DELETE FROM projects WHERE id = $1")
        .bind(alice_project_id)
        .execute(&harness.pool)
        .await;
}
