#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]
//! Routes that name no project in the path (handoff 03b): the project list,
//! project creation, the notification inbox and the settings write.
use axum::http::{Method, StatusCode};
use deepref_app::{NotificationDraft, record_notification};
use deepref_domain::WorkspaceRole;
use uuid::Uuid;

mod support;
use support::authz::Harness;

fn ids(page: &serde_json::Value) -> Vec<String> {
    page["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| item["id"].as_str().unwrap().to_owned())
        .collect()
}

#[tokio::test]
async fn project_list_includes_only_viewable_projects() {
    let Some(h) = Harness::new().await else {
        return;
    };
    let a = h.sign_in("user_A", "a@example.org").await;
    let b = h.sign_in("user_B", "b@example.org").await;
    let a1 = h.create_project(&a, "A1").await;
    let b1 = h.create_project(&b, "B1").await;

    let (_, listed) = h.json(&a, Method::GET, "/projects", "").await;
    assert_eq!(ids(&listed), vec![a1.to_string()]);
    let (_, listed) = h.json(&b, Method::GET, "/projects", "").await;
    assert_eq!(ids(&listed), vec![b1.to_string()]);

    h.add_member(&a, &b, WorkspaceRole::Member).await;
    let (_, listed) = h.json(&b, Method::GET, "/projects", "").await;
    let mut seen = ids(&listed);
    seen.sort();
    let mut expected = vec![a1.to_string(), b1.to_string()];
    expected.sort();
    assert_eq!(seen, expected, "membership adds the workspace's projects");

    deepref_app::workspaces::remove_membership(
        &h.pool,
        a.user,
        a.workspace,
        b.user,
        deepref_app::identity::AuditRequest::default(),
    )
    .await
    .unwrap();
    let (_, listed) = h.json(&b, Method::GET, "/projects", "").await;
    assert_eq!(
        ids(&listed),
        vec![b1.to_string()],
        "removal takes effect on the next list"
    );
}

#[tokio::test]
async fn create_project_in_a_foreign_workspace_is_indistinguishable_from_a_missing_one() {
    let Some(h) = Harness::new().await else {
        return;
    };
    let a = h.sign_in("user_A", "a@example.org").await;
    let b = h.sign_in("user_B", "b@example.org").await;

    let foreign = format!(
        r#"{{"name":"Intruder","workspace_id":"{}"}}"#,
        a.workspace.as_uuid()
    );
    let missing = format!(
        r#"{{"name":"Intruder","workspace_id":"{}"}}"#,
        Uuid::new_v4()
    );
    let (foreign_status, foreign_body) =
        h.send(&b, Method::POST, "/projects", Some(&foreign)).await;
    let (missing_status, missing_body) =
        h.send(&b, Method::POST, "/projects", Some(&missing)).await;
    assert_eq!(foreign_status, StatusCode::NOT_FOUND);
    assert_eq!(foreign_status, missing_status);
    assert_eq!(foreign_body, missing_body);
    let created: i64 = sqlx::query_scalar("SELECT count(*) FROM projects WHERE name = 'Intruder'")
        .fetch_one(&h.pool)
        .await
        .unwrap();
    assert_eq!(created, 0, "nothing was created anywhere");

    // Without workspace_id, a person with exactly one workspace gets it.
    let own = h.create_project(&b, "Mine").await;
    let workspace: Uuid = sqlx::query_scalar("SELECT workspace_id FROM projects WHERE id = $1")
        .bind(own)
        .fetch_one(&h.pool)
        .await
        .unwrap();
    assert_eq!(workspace, b.workspace.as_uuid());

    // Naming the own workspace explicitly works too.
    let explicit = format!(
        r#"{{"name":"Explicit","workspace_id":"{}"}}"#,
        b.workspace.as_uuid()
    );
    let (status, _) = h.send(&b, Method::POST, "/projects", Some(&explicit)).await;
    assert_eq!(status, StatusCode::CREATED);
}

#[tokio::test]
async fn create_project_needs_a_workspace_when_there_are_several_and_a_role_that_can_create() {
    let Some(h) = Harness::new().await else {
        return;
    };
    let a = h.sign_in("user_A", "a@example.org").await;
    let b = h.sign_in("user_B", "b@example.org").await;
    h.add_member(&a, &b, WorkspaceRole::Member).await;

    let (status, body) = h
        .json(&b, Method::POST, "/projects", r#"{"name":"Ambiguous"}"#)
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body["code"], "INVALID_REQUEST");

    let into_a = format!(
        r#"{{"name":"By member","workspace_id":"{}"}}"#,
        a.workspace.as_uuid()
    );
    let (status, _) = h.send(&b, Method::POST, "/projects", Some(&into_a)).await;
    assert_eq!(status, StatusCode::CREATED, "a member holds project:create");
}

#[tokio::test]
async fn notifications_include_only_viewable_projects() {
    let Some(h) = Harness::new().await else {
        return;
    };
    let a = h.sign_in("user_A", "a@example.org").await;
    let b = h.sign_in("user_B", "b@example.org").await;
    let a1 = h.create_project(&a, "A1").await;
    let b1 = h.create_project(&b, "B1").await;
    for (project, title) in [(a1, "A's news"), (b1, "B's news")] {
        record_notification(
            &h.pool,
            &NotificationDraft::info("test", Some(project), title, None, serde_json::json!({})),
        )
        .await
        .unwrap();
    }

    let (_, listed) = h.json(&a, Method::GET, "/notifications", "").await;
    let titles: Vec<_> = listed["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|n| n["title"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(titles, vec!["A's news".to_owned()]);

    let (_, count) = h
        .json(&b, Method::GET, "/notifications/unread-count", "")
        .await;
    assert_eq!(count["count"], 1);
    let b_revision = count["latest_revision"].as_i64().unwrap();
    let (_, a_count) = h
        .json(&a, Method::GET, "/notifications/unread-count", "")
        .await;
    assert_ne!(
        a_count["latest_revision"].as_i64().unwrap(),
        b_revision,
        "another tenant's arrivals are not observable"
    );

    // A cannot mark B's notification read, by id or by project.
    let (_, b_list) = h.json(&b, Method::GET, "/notifications", "").await;
    let b_notification = b_list["items"][0]["id"].as_str().unwrap().to_owned();
    let (_, marked) = h
        .json(
            &a,
            Method::POST,
            "/notifications/mark-read",
            &format!(r#"{{"ids":["{b_notification}"]}}"#),
        )
        .await;
    assert_eq!(marked["updated"], 0);
    let (_, marked) = h
        .json(
            &a,
            Method::POST,
            "/notifications/mark-read",
            &format!(r#"{{"all":true,"project_id":"{b1}"}}"#),
        )
        .await;
    assert_eq!(marked["updated"], 0);
    let (_, count) = h
        .json(&b, Method::GET, "/notifications/unread-count", "")
        .await;
    assert_eq!(count["count"], 1, "B's notification is still unread");

    // Asking for a foreign project's notifications returns nothing, like an unknown one.
    let (status, foreign) = h
        .json(
            &a,
            Method::GET,
            &format!("/notifications?project_id={b1}"),
            "",
        )
        .await;
    let (_, unknown) = h
        .json(
            &a,
            Method::GET,
            &format!("/notifications?project_id={}", Uuid::new_v4()),
            "",
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(foreign["items"], unknown["items"]);

    // Losing access removes the notifications on the next request.
    h.add_member(&a, &b, WorkspaceRole::Member).await;
    let (_, with_access) = h.json(&b, Method::GET, "/notifications", "").await;
    assert_eq!(with_access["items"].as_array().unwrap().len(), 2);
    deepref_app::workspaces::remove_membership(
        &h.pool,
        a.user,
        a.workspace,
        b.user,
        deepref_app::identity::AuditRequest::default(),
    )
    .await
    .unwrap();
    let (_, without) = h.json(&b, Method::GET, "/notifications", "").await;
    assert_eq!(without["items"].as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn settings_update_is_reserved_for_operators() {
    let Some(h) = Harness::new().await else {
        return;
    };
    let a = h.sign_in("user_A", "a@example.org").await;
    let (status, _) = h.get(&a, "/settings").await;
    assert_eq!(
        status,
        StatusCode::OK,
        "defaults are readable by any signed-in user"
    );
    let (status, _) = h
        .json(&a, Method::PATCH, "/settings", r#"{"max_concurrency":50}"#)
        .await;
    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "even a workspace owner cannot change deployment settings"
    );
    let max: i32 = sqlx::query_scalar("SELECT max_concurrency FROM settings WHERE id = 1")
        .fetch_optional(&h.pool)
        .await
        .unwrap()
        .unwrap_or(0);
    assert_ne!(max, 50);
}

#[tokio::test]
async fn user_scoped_routes_answer_the_caller_only() {
    let Some(h) = Harness::new().await else {
        return;
    };
    let a = h.sign_in("user_A", "a@example.org").await;
    let (status, me) = h.json(&a, Method::GET, "/me", "").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(me["user"]["id"], a.user.as_uuid().to_string());
    for path in [
        "/ai/status",
        "/automations/catalog",
        "/automations/templates",
    ] {
        let (status, _) = h.get(&a, path).await;
        assert_eq!(status, StatusCode::OK, "{path}");
    }
}
