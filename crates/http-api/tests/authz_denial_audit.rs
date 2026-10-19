#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]
//! Denials are audited, and the audit is bounded (handoff 03b).
use axum::http::{Method, StatusCode};
use deepref_app::authorization::{Denial, DenialAuditor, DeniedFor, DeniedResource};
use deepref_domain::{ProjectId, authorization::Permission};
use sqlx::PgPool;
use uuid::Uuid;

mod support;
use support::authz::Harness;

async fn denial_rows(
    pool: &PgPool,
    user: Uuid,
) -> Vec<(serde_json::Value, Option<Uuid>, Option<Uuid>)> {
    sqlx::query_as(
        "SELECT metadata, project_id, workspace_id FROM admin_audit_events
         WHERE event_type = 'authorization.denied' AND principal_user_id = $1",
    )
    .bind(user)
    .fetch_all(pool)
    .await
    .unwrap()
}

#[tokio::test]
async fn a_flood_of_denials_stores_a_small_bounded_number_of_rows() {
    let Some(h) = Harness::new().await else {
        return;
    };
    let a = h.sign_in("user_A", "a@example.org").await;
    let b = h.sign_in("user_B", "b@example.org").await;
    let a_project = h.create_project(&a, "A").await;

    let mut sent = 0;
    for i in 0..1000 {
        // Three permissions against A's project, and random ids against nothing.
        let (method, path) = match i % 4 {
            0 => (Method::GET, format!("/projects/{a_project}")),
            1 => (Method::PATCH, format!("/projects/{a_project}")),
            2 => (Method::DELETE, format!("/projects/{a_project}")),
            _ => (Method::GET, format!("/projects/{}", Uuid::new_v4())),
        };
        let (status, _) = h.send(&b, method, &path, None).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        sent += 1;
    }
    assert_eq!(sent, 1000);

    let rows = denial_rows(&h.pool, b.user.as_uuid()).await;
    // project:view, project:edit, project:delete, each in at most two minute
    // windows if the run crosses a minute boundary.
    assert!(!rows.is_empty());
    assert!(rows.len() <= 6, "1000 denials stored {} rows", rows.len());
    for (metadata, project, workspace) in &rows {
        let keys: std::collections::BTreeSet<_> =
            metadata.as_object().unwrap().keys().cloned().collect();
        assert_eq!(
            keys,
            ["permission", "resource_type", "window"]
                .iter()
                .map(|k| (*k).to_owned())
                .collect(),
            "fixed-size metadata only: {metadata}"
        );
        assert_eq!(metadata["resource_type"], "project");
        assert!(workspace.is_none());
        if let Some(project) = project {
            assert!(project == &a_project || !project.is_nil());
        }
    }
    // A's own activity wrote no denial.
    assert!(denial_rows(&h.pool, a.user.as_uuid()).await.is_empty());
}

#[tokio::test]
async fn a_second_process_cannot_exceed_the_per_window_bound() {
    let Some(h) = Harness::new().await else {
        return;
    };
    let b = h.sign_in("user_B", "b@example.org").await;
    let project = ProjectId::new(Uuid::new_v4());
    let denial = Denial {
        user: b.user,
        denied_for: DeniedFor::Permission(Permission::ProjectView),
        resource: DeniedResource::Project(Some(project)),
        request_id: None,
    };
    // Two auditors stand in for two API processes: each starts with an empty
    // in-process set, so only the database can hold the bound.
    let first = DenialAuditor::new(h.pool.clone());
    let second = DenialAuditor::new(h.pool.clone());
    let wrote_first = first.record(denial).await.unwrap();
    let wrote_second = second.record(denial).await.unwrap();
    assert!(wrote_first);
    assert!(
        !wrote_second,
        "the unique index refuses a second row for the same key and window"
    );
    assert!(
        !first.record(denial).await.unwrap(),
        "and this process remembers it"
    );
    assert_eq!(denial_rows(&h.pool, b.user.as_uuid()).await.len(), 1);
}

#[tokio::test]
async fn only_a_parsed_identifier_is_ever_stored() {
    let Some(h) = Harness::new().await else {
        return;
    };
    let b = h.sign_in("user_B", "b@example.org").await;
    // A path segment that is not a UUID never reaches the audit.
    let (status, _) = h
        .get(
            &b,
            "/projects/%27%3B%20DROP%20TABLE%20projects%3B--/reports",
        )
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let rows = denial_rows(&h.pool, b.user.as_uuid()).await;
    assert_eq!(rows.len(), 1);
    let (metadata, project, workspace) = &rows[0];
    assert!(
        project.is_none() && workspace.is_none(),
        "no id parsed, so none stored"
    );
    assert_eq!(metadata["permission"], "project:view");
    assert!(!metadata.to_string().to_lowercase().contains("drop"));
}

#[tokio::test]
async fn the_database_refuses_an_unbounded_denial_shape() {
    let Some(h) = Harness::new().await else {
        return;
    };
    let b = h.sign_in("user_B", "b@example.org").await;
    let insert = |metadata: serde_json::Value| {
        let pool = h.pool.clone();
        let user = b.user.as_uuid();
        async move {
            sqlx::query(
                "INSERT INTO admin_audit_events (id, event_type, principal_kind, principal_user_id, metadata)
                 VALUES (gen_random_uuid(), 'authorization.denied', 'user', $1, $2)",
            )
            .bind(user)
            .bind(metadata)
            .execute(&pool)
            .await
        }
    };
    for bad in [
        serde_json::json!({"permission": "project:view", "resource_type": "none", "window": "1", "body": "x"}),
        serde_json::json!({"permission": "free text", "resource_type": "none", "window": "1"}),
        serde_json::json!({"permission": "project:view", "resource_type": "none", "window": "not-a-number"}),
        serde_json::json!({"permission": "project:view", "resource_type": "everything", "window": "1"}),
        serde_json::json!({"permission": "project:view", "resource_type": "none"}),
    ] {
        assert!(insert(bad.clone()).await.is_err(), "{bad}");
    }
    insert(
        serde_json::json!({"permission": "project:view", "resource_type": "none", "window": "7"}),
    )
    .await
    .unwrap();
    assert!(
        insert(serde_json::json!({"permission": "project:view", "resource_type": "none", "window": "7"}))
            .await
            .is_err(),
        "one row per user, permission, resource type and window"
    );
}
