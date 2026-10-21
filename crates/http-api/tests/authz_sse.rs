#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]
//! A revoked member's open stream closes; a resume is refused (handoff 03b).
use std::{sync::Arc, time::Duration};

use axum::{
    body::Body,
    http::{Method, Request, StatusCode, header},
};
use deepref_domain::WorkspaceRole;
use futures::StreamExt;
use tower::ServiceExt;
use uuid::Uuid;

mod support;
use support::{
    ORIGIN, SESSION_COOKIE, auth_runtime,
    authz::{CORRELATION, Harness},
    fake_identity::FakeIdentity,
};

/// A harness whose streams re-check every 100 ms.
async fn harness_with_fast_recheck() -> Option<Harness> {
    let pool = support::authz::database().await?;
    let fake = Arc::new(FakeIdentity::default());
    let runtime = support::runtime_with(&[
        ("DEEPREF_AUTH_ENABLED", "true"),
        ("DEEPREF_PUBLIC_ORIGIN", ORIGIN),
    ]);
    let config = deepref_http_api::config::ApiConfig {
        documents: deepref_config::DocumentConfig::from_source(&|name| {
            (name == "APP_ENV").then(|| "development".into())
        })
        .unwrap(),
        runtime,
        bind_addr: "127.0.0.1:8080".parse().unwrap(),
        cors_allow_any: false,
        cors_origins: vec![ORIGIN.parse().unwrap()],
    };
    let state = deepref_http_api::state::AppState::core(pool.clone())
        .with_auth(auth_runtime(Arc::clone(&fake), Duration::from_secs(5)))
        .with_stream_recheck(Duration::from_millis(100));
    let app = deepref_http_api::routes::router(state, &config);
    Some(Harness::with_app(pool, fake, app))
}

/// Creates a run in `project` that never finishes, so its stream stays open.
async fn open_run(pool: &sqlx::PgPool, project: Uuid) -> Uuid {
    let conversation: Uuid = sqlx::query_scalar(
        "INSERT INTO assistant_conversations (id, project_id, title) VALUES (gen_random_uuid(), $1, 'c') RETURNING id",
    )
    .bind(project)
    .fetch_one(pool)
    .await
    .unwrap();
    let message: Uuid = sqlx::query_scalar(
        "INSERT INTO assistant_messages (conversation_id, role, content) VALUES ($1, 'user', 'hi') RETURNING id",
    )
    .bind(conversation)
    .fetch_one(pool)
    .await
    .unwrap();
    let run = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO assistant_agent_runs
           (id, project_id, conversation_id, trigger_message_id, actor_kind, actor_id, status,
            model_route, runtime_version)
         VALUES ($1, $2, $3, $4, 'user', 'test', 'running', '{}', 'test')",
    )
    .bind(run)
    .bind(project)
    .bind(conversation)
    .bind(message)
    .execute(pool)
    .await
    .unwrap();
    run
}

fn stream_request(person_cookie: &str, project: Uuid, run: Uuid) -> Request<Body> {
    Request::builder()
        .method(Method::GET)
        .uri(format!("/projects/{project}/assistant/runs/{run}/events"))
        .header(header::COOKIE, format!("{SESSION_COOKIE}={person_cookie}"))
        .header(header::ORIGIN, ORIGIN)
        .header("x-correlation-id", CORRELATION)
        .body(Body::empty())
        .unwrap()
}

#[tokio::test]
async fn revoked_member_stream_closes_and_resume_is_refused() {
    let Some(h) = harness_with_fast_recheck().await else {
        return;
    };
    let owner = h.sign_in("user_A", "a@example.org").await;
    let member = h.sign_in("user_B", "b@example.org").await;
    h.add_member(&owner, &member, WorkspaceRole::Member).await;
    let project = h.create_project(&owner, "Streamed").await;
    let run = open_run(&h.pool, project).await;

    let response = h
        .app
        .clone()
        .oneshot(stream_request(&member.cookie, project, run))
        .await
        .unwrap();
    assert_eq!(
        response.status(),
        StatusCode::OK,
        "a member may observe the run"
    );
    let mut stream = response.into_body().into_data_stream();

    // The stream is open and idle: nothing arrives, and it does not end.
    let idle = tokio::time::timeout(Duration::from_millis(400), stream.next()).await;
    assert!(
        idle.is_err() || matches!(idle, Ok(Some(Ok(_)))),
        "stream stays open while access holds"
    );

    deepref_app::workspaces::remove_membership(
        &h.pool,
        owner.user,
        owner.workspace,
        member.user,
        deepref_app::identity::AuditRequest::default(),
    )
    .await
    .unwrap();

    // Within a few re-check intervals the stream ends.
    let ended = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            match stream.next().await {
                None => return,
                Some(Ok(_)) => {}
                Some(Err(_)) => return,
            }
        }
    })
    .await;
    assert!(ended.is_ok(), "the revoked member's stream must close");

    // A resume is a new request and is refused like a missing run.
    let resumed = h
        .app
        .clone()
        .oneshot(stream_request(&member.cookie, project, run))
        .await
        .unwrap();
    assert_eq!(resumed.status(), StatusCode::NOT_FOUND);

    // The owner is unaffected.
    let still = h
        .app
        .clone()
        .oneshot(stream_request(&owner.cookie, project, run))
        .await
        .unwrap();
    assert_eq!(still.status(), StatusCode::OK);
}

#[tokio::test]
async fn a_stream_of_another_workspaces_run_is_not_served() {
    let Some(h) = harness_with_fast_recheck().await else {
        return;
    };
    let a = h.sign_in("user_A", "a@example.org").await;
    let b = h.sign_in("user_B", "b@example.org").await;
    let a_project = h.create_project(&a, "A").await;
    let b_project = h.create_project(&b, "B").await;
    let a_run = open_run(&h.pool, a_project).await;

    // Foreign project id: the PEP denies.
    let denied = h
        .app
        .clone()
        .oneshot(stream_request(&b.cookie, a_project, a_run))
        .await
        .unwrap();
    assert_eq!(denied.status(), StatusCode::NOT_FOUND);
    // Own project id with a foreign run id: the run belongs to another project.
    let crossed = h
        .app
        .clone()
        .oneshot(stream_request(&b.cookie, b_project, a_run))
        .await
        .unwrap();
    assert_eq!(crossed.status(), StatusCode::NOT_FOUND);
}
