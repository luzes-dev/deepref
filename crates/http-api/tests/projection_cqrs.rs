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

    for suffix in ["projection", "graph?strict=true"] {
        let (foreign_status, foreign_body) = harness
            .get(&bob, &format!("/projects/{alice_project_id}/{suffix}"))
            .await;
        let (missing_status, missing_body) = harness
            .get(&bob, &format!("/projects/{nonexistent_id}/{suffix}"))
            .await;
        assert_eq!(foreign_status, StatusCode::NOT_FOUND);
        assert_eq!(foreign_status, missing_status);
        let mut foreign: Value = serde_json::from_slice(&foreign_body).unwrap();
        let mut missing: Value = serde_json::from_slice(&missing_body).unwrap();
        // Per-request telemetry identity is not part of resource visibility.
        foreign.as_object_mut().unwrap().remove("correlation_id");
        missing.as_object_mut().unwrap().remove("correlation_id");
        assert_eq!(foreign, missing);
    }

    // 3. Alice (authorized owner) accesses her project projection -> 200 OK with truthful DTO
    let (status, bytes) = harness
        .get(&alice, &format!("/projects/{alice_project_id}/projection"))
        .await;
    assert_eq!(status, StatusCode::OK);

    let dto: ProjectionStatusDto = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(dto.project_id, alice_project_id);
    // Project was just created without a recompute: must be pending, requested_revision 0, applied_revision 0
    assert_eq!(dto.state.as_str(), "pending");
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

    assert_eq!(json["projection"]["state"], "pending");
    assert_eq!(json["projection"]["requested_revision"], 0);
    assert_eq!(json["projection"]["applied_revision"], 0);
    assert_eq!(json["projection"]["watermark"], 0);
    assert_eq!(json["projection"]["last_error"], Value::Null);
    assert_eq!(json["projection"]["rebuild_state"], Value::Null);
    for expected in ["ready", "lagging", "rebuilding", "failed", "ready"] {
        match expected {
            "ready" => deepref_app::recompute_project_metrics(&harness.pool, alice_project_id)
                .await
                .unwrap(),
            "lagging" => {
                let (status, _) = harness
                    .json(
                        &alice,
                        axum::http::Method::POST,
                        &format!("/projects/{alice_project_id}/metrics/recompute"),
                        "{}",
                    )
                    .await;
                assert_eq!(status, StatusCode::ACCEPTED);
            }
            "rebuilding" => {
                deepref_app::rebuild_project_projection(&harness.pool, alice_project_id)
                    .await
                    .unwrap();
            }
            "failed" => {
                sqlx::query("UPDATE projection_state SET state='failed',last_error='HTTP failed repair fixture' WHERE project_id=$1").bind(alice_project_id).execute(&harness.pool).await.unwrap();
            }
            _ => unreachable!(),
        }
        let (status, bytes) = harness
            .get(&alice, &format!("/projects/{alice_project_id}/projection"))
            .await;
        assert_eq!(status, StatusCode::OK);
        let expected_metadata: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(expected_metadata["state"], expected);
        let (status,bytes)=harness.get(&alice,&format!("/projects/{alice_project_id}/graph?strict=true&fields=screening,metrics,study,appraisal,provenance")).await;
        assert_eq!(status, StatusCode::OK);
        let graph: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(
            graph["projection"], expected_metadata,
            "strict metadata must preserve the entire freshness contract for {expected}"
        );
        for suffix in ["graph", "graph?strict=false", "recommendations"] {
            let (status, bytes) = harness
                .get(&alice, &format!("/projects/{alice_project_id}/{suffix}"))
                .await;
            assert_eq!(status, StatusCode::OK);
            let body: Value = serde_json::from_slice(&bytes).unwrap();
            assert_eq!(
                body["projection"], expected_metadata,
                "{suffix} must preserve truthful complete metadata for {expected}"
            );
        }
    }

    // Clean up
    let _ = sqlx::query("DELETE FROM projects WHERE id = $1")
        .bind(alice_project_id)
        .execute(&harness.pool)
        .await;
}
