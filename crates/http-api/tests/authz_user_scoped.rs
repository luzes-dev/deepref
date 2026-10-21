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
async fn scientific_assignment_management_requires_workspace_manage() {
    let Some(h) = Harness::new().await else {
        return;
    };
    let owner = h
        .sign_in("assignment_owner", "assignment-owner@example.org")
        .await;
    let member = h
        .sign_in("assignment_member", "assignment-member@example.org")
        .await;
    let project = h.create_project(&owner, "Assignment authorization").await;
    h.add_member(&owner, &member, WorkspaceRole::Member).await;
    let base = format!("/projects/{project}/scientific-assignments");
    let body = format!(
        r#"{{"user_id":"{}","role":"fresh_ai_auditor"}}"#,
        member.user.as_uuid()
    );

    let (status, _) = h.json(&owner, Method::POST, &base, &body).await;
    assert_eq!(status, StatusCode::CREATED);
    let (status, list) = h.get(&owner, &base).await;
    assert_eq!(status, StatusCode::OK);
    let assignments: Vec<serde_json::Value> = serde_json::from_slice(&list).unwrap();
    assert_eq!(assignments.len(), 1);

    let (status, _) = h.json(&member, Method::POST, &base, &body).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let revoke = format!("{base}/{}/fresh_ai_auditor", member.user.as_uuid());
    let (status, _) = h.send(&member, Method::DELETE, &revoke, None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let active: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM scientific_assignments WHERE project_id=$1 AND revoked_at IS NULL",
    )
    .bind(project)
    .fetch_one(&h.pool)
    .await
    .unwrap();
    assert_eq!(active, 1, "denied requests do not mutate assignments");
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

async fn make_workflow(h: &Harness, owner: &support::authz::Person, project: Uuid) -> String {
    let (status, body) = h
        .json(
            owner,
            Method::POST,
            &format!("/projects/{project}/automations/workflows"),
            r#"{"name":"Flow"}"#,
        )
        .await;
    assert!(status.is_success(), "{status} {body}");
    body["id"].as_str().unwrap().to_owned()
}

#[tokio::test]
async fn workflow_credentials_need_automation_manage() {
    let Some(h) = Harness::new().await else {
        return;
    };
    let owner = h.sign_in("user_A", "a@example.org").await;
    let member = h.sign_in("user_B", "b@example.org").await;
    let guest = h.sign_in("user_C", "c@example.org").await;
    h.add_member(&owner, &member, WorkspaceRole::Member).await;
    let project = h.create_project(&owner, "P").await;
    deepref_app::workspaces::grant_project_access(
        &h.pool,
        owner.user,
        deepref_domain::ProjectId::new(project),
        guest.user,
        deepref_domain::ProjectRole::Viewer,
        deepref_app::identity::AuditRequest::default(),
    )
    .await
    .unwrap();
    let workflow = make_workflow(&h, &owner, project).await;
    let path = format!("/projects/{project}/automations/workflows/{workflow}/endpoints");

    let (status, full) = h.json(&owner, Method::GET, &path, "").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(full["credentials_visible"], true);
    let token_path = full["webhook_path"].as_str().unwrap().to_owned();
    let secret = full["webhook_secret"].as_str().unwrap().to_owned();
    let email_path = full["email_path"].as_str().unwrap().to_owned();

    for person in [&member, &guest] {
        let (status, redacted) = h.json(person, Method::GET, &path, "").await;
        assert_eq!(status, StatusCode::OK, "viewing the workflow is allowed");
        assert_eq!(redacted["credentials_visible"], false);
        assert!(redacted["webhook_path"].is_null());
        assert!(redacted["webhook_secret"].is_null());
        assert!(redacted["email_path"].is_null());
        assert!(redacted["signature_required"].is_boolean());
        let text = redacted.to_string();
        for leaked in [&token_path, &secret, &email_path] {
            assert!(!text.contains(leaked.as_str()), "{text}");
        }
        // And the listing and detail never carry them either.
        for sibling in [
            format!("/projects/{project}/automations/workflows"),
            format!("/projects/{project}/automations/workflows/{workflow}"),
        ] {
            let (_, bytes) = h.get(person, &sibling).await;
            let text = String::from_utf8_lossy(&bytes);
            assert!(
                !text.contains(&secret) && !text.contains(token_path.rsplit('/').next().unwrap()),
                "{sibling}"
            );
        }
    }
    // Rotation stays a manage action.
    let (status, _) = h
        .json(
            &member,
            Method::POST,
            &format!("{path}/webhook/rotate"),
            "{}",
        )
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn the_assistant_cannot_start_an_automation_its_user_may_not() {
    let Some(h) = Harness::new().await else {
        return;
    };
    let owner = h.sign_in("user_A", "a@example.org").await;
    let member = h.sign_in("user_B", "b@example.org").await;
    h.add_member(&owner, &member, WorkspaceRole::Member).await;
    let project = h.create_project(&owner, "P").await;
    let (status, conversation) = h
        .json(
            &member,
            Method::POST,
            &format!("/projects/{project}/assistant/conversations"),
            r#"{"title":"c"}"#,
        )
        .await;
    assert!(status.is_success(), "{conversation}");
    let conversation = conversation["id"].as_str().unwrap().to_owned();
    let command = serde_json::json!({
        "tool": "trigger_workflow",
        "args": { "project_id": project, "definition_id": Uuid::new_v4() },
    })
    .to_string();
    let body =
        serde_json::json!({ "conversation_id": conversation, "message": command }).to_string();
    let path = format!("/projects/{project}/assistant/chat");

    let (status, bytes) = h.send(&member, Method::POST, &path, Some(&body)).await;
    let stream = String::from_utf8_lossy(&bytes).into_owned();
    assert!(
        status == StatusCode::FORBIDDEN || stream.contains("assistant_forbidden"),
        "{status} {stream}"
    );
    let denied: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM admin_audit_events
         WHERE event_type = 'authorization.denied' AND principal_user_id = $1
           AND metadata->>'permission' = 'automation:manage'",
    )
    .bind(member.user.as_uuid())
    .fetch_one(&h.pool)
    .await
    .unwrap();
    assert_eq!(denied, 1, "the dispatch-level denial is audited");
    let runs: i64 = sqlx::query_scalar("SELECT count(*) FROM durable_runs")
        .fetch_one(&h.pool)
        .await
        .unwrap();
    assert_eq!(runs, 0);

    // The owner holds automation:manage, so the permission check passes (the
    // random definition then simply does not exist).
    let (status, bytes) = h.send(&owner, Method::POST, &path, Some(&body)).await;
    assert!(
        !String::from_utf8_lossy(&bytes).contains("assistant_forbidden")
            && status != StatusCode::FORBIDDEN,
        "{status} {}",
        String::from_utf8_lossy(&bytes)
    );
}

#[tokio::test]
async fn a_plan_stops_when_access_is_lost_during_confirmation() {
    let Some(h) = Harness::new().await else {
        return;
    };
    let owner = h.sign_in("user_A", "a@example.org").await;
    let member = h.sign_in("user_B", "b@example.org").await;
    h.add_member(&owner, &member, WorkspaceRole::Member).await;
    let project = h.create_project(&owner, "P").await;
    let conversation: Uuid = sqlx::query_scalar(
        "INSERT INTO assistant_conversations (project_id, title) VALUES ($1, 'c') RETURNING id",
    )
    .bind(project)
    .fetch_one(&h.pool)
    .await
    .unwrap();
    let actions: Vec<_> = (1..=2)
        .map(|n| {
            serde_json::json!({
                "id": format!("a{n}"), "tool": "screen_reports", "summary": "Exclude one",
                "rationale": "off topic", "affected_count": 1,
                "args": { "report_ids": [Uuid::new_v4()], "decision": "exclude" },
                "executable": true
            })
        })
        .collect();
    let plan = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO assistant_plans
           (id, project_id, conversation_id, status, summary, actions, created_by_kind,
            created_by_id, model, prompt_version, evidence)
         VALUES ($1, $2, $3, 'pending', 's', $4, 'user', 'x', 'm', 'v', '[]')",
    )
    .bind(plan)
    .bind(project)
    .bind(conversation)
    .bind(serde_json::Value::Array(actions))
    .execute(&h.pool)
    .await
    .unwrap();
    // The member passes the route check, then loses access while the plan is
    // being confirmed: the status flip to confirmed removes the membership.
    sqlx::query(sqlx::AssertSqlSafe(format!(
        "CREATE FUNCTION revoke_on_confirm() RETURNS trigger LANGUAGE plpgsql AS $$
         BEGIN
           DELETE FROM workspace_memberships WHERE user_id = '{}'::uuid AND role = 'member';
           RETURN NEW;
         END $$",
        member.user.as_uuid()
    )))
    .execute(&h.pool)
    .await
    .unwrap();
    sqlx::query(
        "CREATE TRIGGER revoke_on_confirm AFTER UPDATE ON assistant_plans
         FOR EACH ROW WHEN (NEW.status = 'confirmed') EXECUTE FUNCTION revoke_on_confirm()",
    )
    .execute(&h.pool)
    .await
    .unwrap();
    let (status, body) = h
        .json(
            &member,
            Method::POST,
            &format!("/projects/{project}/assistant/plans/{plan}/confirm"),
            "{}",
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["status"], "failed", "{body}");
    let results = body["results"].as_array().unwrap();
    assert_eq!(results.len(), 2);
    for result in results {
        assert_eq!(result["status"], "skipped", "{result}");
        assert!(
            result["message"].as_str().unwrap().contains("access"),
            "{result}"
        );
    }
    let denied: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM admin_audit_events
         WHERE event_type = 'authorization.denied' AND principal_user_id = $1
           AND metadata->>'permission' = 'screening:perform'",
    )
    .bind(member.user.as_uuid())
    .fetch_one(&h.pool)
    .await
    .unwrap();
    assert_eq!(denied, 1);
}

/// Lets the first `real` decisions through to the real decision point, then
/// answers every later one with `then`.
struct Scripted {
    real: deepref_application::authorization::PolicyDecisionPoint<
        deepref_app::authorization::PostgresAuthorizationStore,
    >,
    calls: std::sync::atomic::AtomicUsize,
    real_calls: usize,
    then: Result<(), ()>,
}

impl deepref_application::authorization::Authorizer for Scripted {
    fn require<'a>(
        &'a self,
        principal: &'a deepref_domain::Principal,
        permission: deepref_domain::authorization::Permission,
        resource: deepref_domain::authorization::Resource,
    ) -> deepref_application::authorization::AuthorizationFuture<
        'a,
        Result<(), deepref_application::authorization::AuthzError>,
    > {
        use deepref_application::authorization::AuthzError;
        let call = self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        if call < self.real_calls {
            return self.real.require(principal, permission, resource);
        }
        let answer = match self.then {
            Ok(()) => Ok(()),
            Err(()) => Err(AuthzError::Store("injected".to_owned())),
        };
        Box::pin(async move { answer })
    }
}

struct Deny {
    real: deepref_application::authorization::PolicyDecisionPoint<
        deepref_app::authorization::PostgresAuthorizationStore,
    >,
    calls: std::sync::atomic::AtomicUsize,
    real_calls: usize,
}

impl deepref_application::authorization::Authorizer for Deny {
    fn require<'a>(
        &'a self,
        principal: &'a deepref_domain::Principal,
        permission: deepref_domain::authorization::Permission,
        resource: deepref_domain::authorization::Resource,
    ) -> deepref_application::authorization::AuthorizationFuture<
        'a,
        Result<(), deepref_application::authorization::AuthzError>,
    > {
        let call = self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        // Only decision number `real_calls` is refused; every other is real.
        if call != self.real_calls {
            return self.real.require(principal, permission, resource);
        }
        Box::pin(async {
            Err(deepref_application::authorization::AuthzError::Denied(
                deepref_application::authorization::AuthzDenied,
            ))
        })
    }
}

async fn plan_with_manual_then_screening(h: &Harness, project: Uuid) -> Uuid {
    let conversation: Uuid = sqlx::query_scalar(
        "INSERT INTO assistant_conversations (project_id, title) VALUES ($1, 'c') RETURNING id",
    )
    .bind(project)
    .fetch_one(&h.pool)
    .await
    .unwrap();
    let actions = serde_json::json!([
        { "id": "a1", "tool": "request_protocol_publish", "summary": "Publish", "rationale": "r",
          "affected_count": 0, "args": {}, "executable": false,
          "manual": { "reason": "do it yourself", "link_target": "protocol" } },
        { "id": "a2", "tool": "screen_reports", "summary": "Exclude one", "rationale": "r",
          "affected_count": 1, "executable": true,
          "args": { "report_ids": [Uuid::new_v4()], "decision": "exclude" } }
    ]);
    let plan = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO assistant_plans
           (id, project_id, conversation_id, status, summary, actions, created_by_kind,
            created_by_id, model, prompt_version, evidence)
         VALUES ($1, $2, $3, 'pending', 's', $4, 'user', 'x', 'm', 'v', '[]')",
    )
    .bind(plan)
    .bind(project)
    .bind(conversation)
    .bind(actions)
    .execute(&h.pool)
    .await
    .unwrap();
    plan
}

#[tokio::test]
async fn a_store_error_during_confirmation_settles_the_plan_as_failed() {
    let Some(pool) = support::authz::database().await else {
        return;
    };
    let pool_for_state = pool.clone();
    let h = Harness::with_state(pool, move |state| {
        let real = deepref_application::authorization::PolicyDecisionPoint::new(
            deepref_app::authorization::PostgresAuthorizationStore::new(pool_for_state),
        );
        state.with_authorizer(std::sync::Arc::new(Scripted {
            real,
            calls: 0.into(),
            // Call 1 is the route check. Project creation and sign-in make no
            // decisions through the state authorizer.
            real_calls: 1,
            then: Err(()),
        }))
    });
    let owner = h.sign_in("user_A", "a@example.org").await;
    let project = sqlx::query_scalar::<_, Uuid>(
        "INSERT INTO projects (id, name, workspace_id) VALUES (gen_random_uuid(), 'P', $1) RETURNING id",
    )
    .bind(owner.workspace.as_uuid())
    .fetch_one(&h.pool)
    .await
    .unwrap();
    let plan = plan_with_manual_then_screening(&h, project).await;
    let path = format!("/projects/{project}/assistant/plans/{plan}/confirm");
    let (status, body) = h.json(&owner, Method::POST, &path, "{}").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["status"], "failed", "{body}");
    let results = body["results"].as_array().unwrap();
    assert_eq!(results[0]["status"], "manual", "earlier outcomes are kept");
    assert_eq!(results[1]["status"], "failed");
    // Not stranded: the plan is terminal, not left `confirmed`.
    let stored: String = sqlx::query_scalar("SELECT status FROM assistant_plans WHERE id = $1")
        .bind(plan)
        .fetch_one(&h.pool)
        .await
        .unwrap();
    assert_eq!(stored, "failed");
}

#[tokio::test]
async fn losing_access_after_one_outcome_keeps_it_and_stays_failed() {
    let Some(pool) = support::authz::database().await else {
        return;
    };
    let pool_for_state = pool.clone();
    let h = Harness::with_state(pool, move |state| {
        let real = deepref_application::authorization::PolicyDecisionPoint::new(
            deepref_app::authorization::PostgresAuthorizationStore::new(pool_for_state),
        );
        state.with_authorizer(std::sync::Arc::new(Deny {
            real,
            calls: 0.into(),
            real_calls: 1,
        }))
    });
    let owner = h.sign_in("user_A", "a@example.org").await;
    let project = sqlx::query_scalar::<_, Uuid>(
        "INSERT INTO projects (id, name, workspace_id) VALUES (gen_random_uuid(), 'P', $1) RETURNING id",
    )
    .bind(owner.workspace.as_uuid())
    .fetch_one(&h.pool)
    .await
    .unwrap();
    let plan = plan_with_manual_then_screening(&h, project).await;
    let path = format!("/projects/{project}/assistant/plans/{plan}/confirm");
    let (status, body) = h.json(&owner, Method::POST, &path, "{}").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["status"], "failed", "{body}");
    let results = body["results"].as_array().unwrap();
    assert_eq!(results[0]["status"], "manual", "the first outcome is kept");
    assert_eq!(results[1]["status"], "skipped");
    // Reading the plan refreshes review results; that must not rewrite the
    // access-loss failure as success.
    let (_, again) = h
        .json(
            &owner,
            Method::GET,
            &format!("/projects/{project}/assistant/plans/{plan}"),
            "",
        )
        .await;
    assert_eq!(again["status"], "failed", "{again}");
    let message: String = sqlx::query_scalar(
        "SELECT content FROM assistant_messages m JOIN assistant_plans p ON p.conversation_id = m.conversation_id
         WHERE p.id = $1 AND m.role = 'assistant'",
    )
    .bind(plan)
    .fetch_one(&h.pool)
    .await
    .unwrap();
    assert!(!message.starts_with("Done"), "{message}");
    assert!(message.contains("access"), "{message}");
}
