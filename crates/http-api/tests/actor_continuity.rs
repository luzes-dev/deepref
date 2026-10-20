#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]
//! HTTP actor attribution and fresh-auditor continuity through authenticated sessions.

use axum::{
    body::Body,
    http::{Method, Request, StatusCode, header},
};
use deepref_domain::WorkspaceRole;
use sqlx::PgPool;
use tower::ServiceExt;
use uuid::Uuid;

mod support;
use support::authz::{CORRELATION, Harness, Person};

async fn post_with_actor_headers(
    harness: &Harness,
    person: &Person,
    path: &str,
    body: serde_json::Value,
    actor_id: &str,
    actor_kind: &str,
) -> (StatusCode, Vec<u8>) {
    let response = harness
        .app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri(path)
                .header(
                    header::COOKIE,
                    format!("{}={}", support::SESSION_COOKIE, person.cookie),
                )
                .header(header::ORIGIN, support::ORIGIN)
                .header(header::CONTENT_TYPE, "application/json")
                .header("x-correlation-id", CORRELATION)
                .header("x-actor-id", actor_id)
                .header("x-actor-kind", actor_kind)
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap()
        .to_vec();
    (status, bytes)
}

async fn add_screening_fixture(
    pool: &PgPool,
    project_id: Uuid,
    protocol_id: Option<Uuid>,
) -> (Uuid, Uuid) {
    let protocol_id = protocol_id.unwrap_or_else(Uuid::new_v4);
    if sqlx::query_scalar::<_, bool>("SELECT EXISTS(SELECT 1 FROM protocol_versions WHERE id=$1)")
        .bind(protocol_id)
        .fetch_one(pool)
        .await
        .unwrap()
    {
        // Reuse the already-published project protocol for another report.
    } else {
        sqlx::query(
            "INSERT INTO protocol_versions(id,project_id,version,name,status,criteria) \
             VALUES($1,$2,1,'Actor continuity protocol','published','[]')",
        )
        .bind(protocol_id)
        .bind(project_id)
        .execute(pool)
        .await
        .unwrap();
    }
    let report_id = Uuid::new_v4();
    sqlx::query("INSERT INTO reports(id,title) VALUES($1,'Actor continuity report')")
        .bind(report_id)
        .execute(pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO project_reports(project_id,report_id) VALUES($1,$2)")
        .bind(project_id)
        .bind(report_id)
        .execute(pool)
        .await
        .unwrap();
    (protocol_id, report_id)
}

async fn submit_screening(
    harness: &Harness,
    person: &Person,
    project_id: Uuid,
    report_id: Uuid,
    protocol_id: Uuid,
    forged_id: &str,
    forged_kind: &str,
) -> StatusCode {
    let body = serde_json::json!({
        "stage": "title_abstract",
        "decision": "include",
        "protocol_version_id": protocol_id,
        "expected_revision": 0,
        "actor_id": forged_id,
        "actor_kind": forged_kind,
    });
    let (status, body) = post_with_actor_headers(
        harness,
        person,
        &format!("/projects/{project_id}/reports/{report_id}/screening"),
        body,
        forged_id,
        forged_kind,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{}", String::from_utf8_lossy(&body));
    status
}

#[tokio::test]
async fn actor_headers_and_body_fields_cannot_forge_a_human_or_machine_actor() {
    let Some(harness) = Harness::new().await else {
        return;
    };
    let alice = harness
        .sign_in("actor-continuity-alice", "actor-alice@example.org")
        .await;
    let bob = harness
        .sign_in("actor-continuity-bob", "actor-bob@example.org")
        .await;
    let project_id = harness.create_project(&alice, "Actor attribution").await;
    harness
        .add_member(&alice, &bob, WorkspaceRole::Member)
        .await;
    let (protocol_id, report_a) = add_screening_fixture(&harness.pool, project_id, None).await;
    let (_, report_b) = add_screening_fixture(&harness.pool, project_id, Some(protocol_id)).await;

    submit_screening(
        &harness,
        &alice,
        project_id,
        report_a,
        protocol_id,
        "forged-other-human",
        "system",
    )
    .await;
    submit_screening(
        &harness,
        &bob,
        project_id,
        report_b,
        protocol_id,
        "ai:forged-automation",
        "automation",
    )
    .await;

    let stored = sqlx::query(
        "SELECT report_id,actor_kind,actor_id FROM screening_events \
         WHERE project_id=$1 AND report_id=ANY($2) ORDER BY report_id",
    )
    .bind(project_id)
    .bind(vec![report_a, report_b])
    .fetch_all(&harness.pool)
    .await
    .unwrap();
    assert_eq!(stored.len(), 2);
    for row in stored {
        let report_id: Uuid = sqlx::Row::get(&row, "report_id");
        let (expected, kind): (String, String) = sqlx::query_as(
            "SELECT actor_id,actor_kind FROM screening_events \
             WHERE project_id=$1 AND report_id=$2",
        )
        .bind(project_id)
        .bind(report_id)
        .fetch_one(&harness.pool)
        .await
        .unwrap();
        let expected_user = if report_id == report_a {
            alice.user
        } else {
            bob.user
        };
        assert_eq!(kind, "user");
        assert_eq!(expected, expected_user.as_uuid().to_string());
    }

    let (status, _) = harness
        .send(
            &alice,
            Method::POST,
            "/legacy-actor/map",
            Some(r#"{"actor_kind":"user","actor_id":"self","user_id":"00000000-0000-0000-0000-000000000001"}"#),
        )
        .await;
    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "no mapping HTTP route exists"
    );
    let mapping_count: i64 = sqlx::query_scalar("SELECT count(*) FROM legacy_actor_mappings")
        .fetch_one(&harness.pool)
        .await
        .unwrap();
    assert_eq!(mapping_count, 0);
}

#[tokio::test]
async fn authenticated_users_have_distinct_fresh_auditor_blinding() {
    let Some(harness) = Harness::new().await else {
        return;
    };
    let alice = harness
        .sign_in("actor-blinding-alice", "blinding-alice@example.org")
        .await;
    let bob = harness
        .sign_in("actor-blinding-bob", "blinding-bob@example.org")
        .await;
    let project_id = harness.create_project(&alice, "Two person blinding").await;
    harness
        .add_member(&alice, &bob, WorkspaceRole::Member)
        .await;
    let (protocol_id, cohort_report) = add_screening_fixture(&harness.pool, project_id, None).await;
    let (_, other_report) =
        add_screening_fixture(&harness.pool, project_id, Some(protocol_id)).await;

    submit_screening(
        &harness,
        &alice,
        project_id,
        cohort_report,
        protocol_id,
        "old-client-label",
        "user",
    )
    .await;
    submit_screening(
        &harness,
        &bob,
        project_id,
        other_report,
        protocol_id,
        "bob-tab",
        "user",
    )
    .await;

    for user in [alice.user, bob.user] {
        sqlx::query(
            "INSERT INTO scientific_assignments(project_id,user_id,role,assigned_by) \
             VALUES($1,$2,'fresh_ai_auditor',$3)",
        )
        .bind(project_id)
        .bind(user.as_uuid())
        .bind(alice.user.as_uuid())
        .execute(&harness.pool)
        .await
        .unwrap();
    }
    let cohort_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO ai_screening_cohorts \
         (id,project_id,protocol_version_id,semantic_bundle_hash,semantic_identity,policy_version,status,target_percent,approved_by,approved_by_actor_user_id) \
         VALUES($1,$2,$3,$4,'{}',1,'auditing',95,$5,$6)",
    )
    .bind(cohort_id)
    .bind(project_id)
    .bind(protocol_id)
    .bind("a".repeat(64))
    .bind(alice.user.as_uuid().to_string())
    .bind(alice.user.as_uuid())
    .execute(&harness.pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO ai_screening_cohort_members(project_id,cohort_id,report_id,evaluated_revision,sampled) \
         VALUES($1,$2,$3,0,true)",
    )
    .bind(project_id)
    .bind(cohort_id)
    .bind(cohort_report)
    .execute(&harness.pool)
    .await
    .unwrap();

    let alice_is_fresh: bool = sqlx::query_scalar("SELECT ai_first_fresh_auditor_v3($1,$2,$3)")
        .bind(cohort_id)
        .bind(cohort_report)
        .bind(alice.user.as_uuid())
        .fetch_one(&harness.pool)
        .await
        .unwrap();
    let bob_is_fresh: bool = sqlx::query_scalar("SELECT ai_first_fresh_auditor_v3($1,$2,$3)")
        .bind(cohort_id)
        .bind(cohort_report)
        .bind(bob.user.as_uuid())
        .fetch_one(&harness.pool)
        .await
        .unwrap();
    assert!(
        !alice_is_fresh,
        "Alice's authenticated decision is in scope"
    );
    assert!(
        bob_is_fresh,
        "Bob's separate identity has no exposure in scope"
    );

    let bob_sees_queue_item: bool =
        sqlx::query_scalar("SELECT ai_first_queue_visible_v3($1,$2,$3)")
            .bind(project_id)
            .bind(cohort_report)
            .bind(bob.user.as_uuid())
            .fetch_one(&harness.pool)
            .await
            .unwrap();
    assert!(bob_sees_queue_item);
}
