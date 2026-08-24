use std::collections::HashMap;

use axum::{
    body::{Body, to_bytes},
    http::{Request, Response, StatusCode},
};
use deepref_http_api::{config::ApiConfig, routes::router, state::AppState};
use sqlx::{PgPool, postgres::PgPoolOptions};
use tower::ServiceExt;
use uuid::Uuid;

async fn database() -> Option<PgPool> {
    let url = match std::env::var("DATABASE_URL") {
        Ok(url) => url,
        Err(_) => return None,
    };
    let pool = PgPoolOptions::new()
        .max_connections(5)
        .connect(&url)
        .await
        .unwrap_or_else(|error| {
            panic!("DATABASE_URL is set but PostgreSQL is unavailable: {error}")
        });
    deepref_postgres::migrate(&pool)
        .await
        .unwrap_or_else(|error| panic!("failed to apply PostgreSQL migrations: {error}"));
    Some(pool)
}

fn api_config() -> ApiConfig {
    let runtime = deepref_config::RuntimeConfig::from_map(
        "deepref-api-screening-test",
        &HashMap::from([("APP_ENV".to_owned(), "local".to_owned())]),
    )
    .expect("local test runtime should parse");
    ApiConfig {
        runtime,
        bind_addr: "127.0.0.1:0".parse().expect("test bind address is valid"),
        cors_allow_any: false,
        cors_origins: vec![
            "http://localhost:3000"
                .parse()
                .expect("test origin is valid"),
        ],
    }
}

async fn screen(
    pool: &PgPool,
    project_id: Uuid,
    report_id: Uuid,
    body: serde_json::Value,
) -> Response<Body> {
    router(AppState::core(pool.clone()), &api_config())
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!(
                    "/projects/{project_id}/reports/{report_id}/screening"
                ))
                .header("content-type", "application/json")
                .body(Body::from(body.to_string()))
                .expect("screening request should be valid"),
        )
        .await
        .expect("screening request should be handled")
}

async fn undo(
    pool: &PgPool,
    project_id: Uuid,
    report_id: Uuid,
    body: serde_json::Value,
) -> Response<Body> {
    router(AppState::core(pool.clone()), &api_config())
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!(
                    "/projects/{project_id}/reports/{report_id}/screening/undo"
                ))
                .header("content-type", "application/json")
                .body(Body::from(body.to_string()))
                .expect("undo request should be valid"),
        )
        .await
        .expect("undo request should be handled")
}

async fn response_json(response: Response<Body>) -> serde_json::Value {
    let body = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("response body should be readable");
    serde_json::from_slice(&body).expect("response should be JSON")
}

#[tokio::test]
async fn postgres_screening_enforces_transition_and_project_boundaries() {
    let Some(pool) = database().await else {
        return;
    };

    let project_id = Uuid::new_v4();
    let other_project_id = Uuid::new_v4();
    let report_id = Uuid::new_v4();
    let protocol_id = Uuid::new_v4();
    let other_protocol_id = Uuid::new_v4();
    let exclusion_reason_id = Uuid::new_v4();
    let other_exclusion_reason_id = Uuid::new_v4();
    let wrong_stage_exclusion_reason_id = Uuid::new_v4();

    sqlx::query("INSERT INTO projects (id,name) VALUES ($1,'screening test'),($2,'other project')")
        .bind(project_id)
        .bind(other_project_id)
        .execute(&pool)
        .await
        .expect("test projects should insert");
    sqlx::query("INSERT INTO reports (id,title) VALUES ($1,'screening report')")
        .bind(report_id)
        .execute(&pool)
        .await
        .expect("test report should insert");
    sqlx::query("INSERT INTO project_reports (project_id,report_id) VALUES ($1,$2)")
        .bind(project_id)
        .bind(report_id)
        .execute(&pool)
        .await
        .expect("test report link should insert");
    sqlx::query(
        "INSERT INTO protocol_versions (id,project_id,version,name,status,criteria) VALUES ($1,$2,1,'test','published','[]'),($3,$4,1,'other','published','[]')",
    )
    .bind(protocol_id)
    .bind(project_id)
    .bind(other_protocol_id)
    .bind(other_project_id)
    .execute(&pool)
    .await
    .expect("test protocols should insert");
    sqlx::query(
        "INSERT INTO exclusion_reasons (id,project_id,code,label,stage) VALUES ($1,$2,'test','Test reason','full_text'),($3,$4,'other_test','Other reason','full_text'),($5,$2,'wrong_stage','Wrong stage reason','title_abstract')",
    )
    .bind(exclusion_reason_id)
    .bind(project_id)
    .bind(other_exclusion_reason_id)
    .bind(other_project_id)
    .bind(wrong_stage_exclusion_reason_id)
    .execute(&pool)
    .await
    .expect("test exclusion reasons should insert");

    let invalid_full_text = screen(
        &pool,
        project_id,
        report_id,
        serde_json::json!({
            "stage": "full_text",
            "decision": "include",
            "protocol_version_id": protocol_id,
            "expected_revision": 0
        }),
    )
    .await;
    assert_eq!(invalid_full_text.status(), StatusCode::BAD_REQUEST);

    let cross_project_protocol = screen(
        &pool,
        project_id,
        report_id,
        serde_json::json!({
            "stage": "title_abstract",
            "decision": "include",
            "protocol_version_id": other_protocol_id,
            "expected_revision": 0
        }),
    )
    .await;
    assert_eq!(cross_project_protocol.status(), StatusCode::NOT_FOUND);

    let cross_project_reason = screen(
        &pool,
        project_id,
        report_id,
        serde_json::json!({
            "stage": "title_abstract",
            "decision": "exclude",
            "protocol_version_id": protocol_id,
            "exclusion_reason_id": other_exclusion_reason_id,
            "expected_revision": 0
        }),
    )
    .await;
    assert_eq!(cross_project_reason.status(), StatusCode::BAD_REQUEST);

    let title_include = screen(
        &pool,
        project_id,
        report_id,
        serde_json::json!({
            "stage": "title_abstract",
            "decision": "include",
            "protocol_version_id": protocol_id,
            "expected_revision": 0
        }),
    )
    .await;
    assert_eq!(title_include.status(), StatusCode::OK);

    for decision in ["include", "maybe"] {
        let response = screen(
            &pool,
            project_id,
            report_id,
            serde_json::json!({
                "stage": "full_text",
                "decision": decision,
                "protocol_version_id": protocol_id,
                "exclusion_reason_id": exclusion_reason_id,
                "expected_revision": 1
            }),
        )
        .await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }
    let exclude_without_reason = screen(
        &pool,
        project_id,
        report_id,
        serde_json::json!({
            "stage": "full_text",
            "decision": "exclude",
            "protocol_version_id": protocol_id,
            "expected_revision": 1
        }),
    )
    .await;
    assert_eq!(exclude_without_reason.status(), StatusCode::BAD_REQUEST);

    let wrong_stage_full_text_exclude = screen(
        &pool,
        project_id,
        report_id,
        serde_json::json!({
            "stage": "full_text",
            "decision": "exclude",
            "protocol_version_id": protocol_id,
            "exclusion_reason_id": wrong_stage_exclusion_reason_id,
            "expected_revision": 1
        }),
    )
    .await;
    assert_eq!(
        wrong_stage_full_text_exclude.status(),
        StatusCode::BAD_REQUEST
    );
    let wrong_stage_error = response_json(wrong_stage_full_text_exclude).await;
    assert!(
        wrong_stage_error["message"]
            .as_str()
            .is_some_and(|message| message.contains("screening"))
    );
    let revision_after_rejected_request: i64 = sqlx::query_scalar(
        "SELECT revision FROM screening_state WHERE project_id=$1 AND report_id=$2",
    )
    .bind(project_id)
    .bind(report_id)
    .fetch_one(&pool)
    .await
    .expect("rejected screening should not change revision");
    assert_eq!(revision_after_rejected_request, 1);

    let full_text_exclude = screen(
        &pool,
        project_id,
        report_id,
        serde_json::json!({
            "stage": "full_text",
            "decision": "exclude",
            "protocol_version_id": protocol_id,
            "exclusion_reason_id": exclusion_reason_id,
            "expected_revision": 1
        }),
    )
    .await;
    assert_eq!(full_text_exclude.status(), StatusCode::OK);

    let title_maybe = screen(
        &pool,
        project_id,
        report_id,
        serde_json::json!({
            "stage": "title_abstract",
            "decision": "maybe",
            "protocol_version_id": protocol_id,
            "expected_revision": 2
        }),
    )
    .await;
    assert_eq!(title_maybe.status(), StatusCode::OK);
    let state = response_json(title_maybe).await;
    assert_eq!(state["title_abstract_status"], "maybe");
    assert_eq!(state["full_text_status"], "not_required");
    assert_eq!(state["final_status"], "maybe");

    let persisted: (String, String, Option<Uuid>) = sqlx::query_as(
        "SELECT full_text_status,final_status,full_text_exclusion_reason_id FROM screening_state WHERE project_id=$1 AND report_id=$2",
    )
    .bind(project_id)
    .bind(report_id)
    .fetch_one(&pool)
    .await
    .expect("screening state should persist");
    assert_eq!(
        persisted,
        ("not_required".to_owned(), "maybe".to_owned(), None)
    );

    sqlx::query("DELETE FROM jobs WHERE payload->>'project_id' = $1")
        .bind(project_id.to_string())
        .execute(&pool)
        .await
        .expect("test jobs should clean up");
    sqlx::query("DELETE FROM projects WHERE id IN ($1,$2)")
        .bind(project_id)
        .bind(other_project_id)
        .execute(&pool)
        .await
        .expect("test projects should clean up");
}

#[tokio::test]
async fn screening_conflicts_and_history_contracts_include_reconciliation_state() {
    let Some(pool) = database().await else {
        return;
    };
    let project_id = Uuid::new_v4();
    let report_id = Uuid::new_v4();
    let protocol_id = Uuid::new_v4();
    sqlx::query("INSERT INTO projects (id,name) VALUES ($1,'screening contract test')")
        .bind(project_id)
        .execute(&pool)
        .await
        .expect("project should insert");
    sqlx::query(
        "INSERT INTO reports (id,title,abstract_text) VALUES ($1,'Contract report','Abstract')",
    )
    .bind(report_id)
    .execute(&pool)
    .await
    .expect("report should insert");
    sqlx::query("INSERT INTO project_reports (project_id,report_id) VALUES ($1,$2)")
        .bind(project_id)
        .bind(report_id)
        .execute(&pool)
        .await
        .expect("project report should insert");
    sqlx::query(
        "INSERT INTO protocol_versions (id,project_id,version,name,status,criteria) VALUES ($1,$2,1,'contract','published','[]')",
    )
    .bind(protocol_id)
    .bind(project_id)
    .execute(&pool)
    .await
    .expect("protocol should insert");

    let first = screen(
        &pool,
        project_id,
        report_id,
        serde_json::json!({
            "stage": "title_abstract",
            "decision": "include",
            "protocol_version_id": protocol_id,
            "expected_revision": 0
        }),
    )
    .await;
    assert_eq!(first.status(), StatusCode::OK);

    let stale = screen(
        &pool,
        project_id,
        report_id,
        serde_json::json!({
            "stage": "title_abstract",
            "decision": "maybe",
            "protocol_version_id": protocol_id,
            "expected_revision": 0
        }),
    )
    .await;
    assert_eq!(stale.status(), StatusCode::CONFLICT);
    let stale_body = response_json(stale).await;
    assert_eq!(stale_body["code"], "screening_revision_conflict");
    assert_eq!(stale_body["details"]["currentRevision"], 1);
    assert_eq!(
        stale_body["details"]["currentState"]["title_abstract_status"],
        "include"
    );

    let full_text = screen(
        &pool,
        project_id,
        report_id,
        serde_json::json!({
            "stage": "full_text",
            "decision": "include",
            "protocol_version_id": protocol_id,
            "expected_revision": 1
        }),
    )
    .await;
    assert_eq!(full_text.status(), StatusCode::OK);

    let wrong_stage_undo = undo(
        &pool,
        project_id,
        report_id,
        serde_json::json!({
            "stage": "title_abstract",
            "protocol_version_id": protocol_id,
            "expected_revision": 2
        }),
    )
    .await;
    assert_eq!(wrong_stage_undo.status(), StatusCode::CONFLICT);
    let wrong_stage_body = response_json(wrong_stage_undo).await;
    assert_eq!(wrong_stage_body["code"], "screening_undo_not_latest");
    assert_eq!(wrong_stage_body["details"]["currentRevision"], 2);
    assert_eq!(
        wrong_stage_body["details"]["currentState"]["full_text_status"],
        "include"
    );

    let history = router(AppState::core(pool.clone()), &api_config())
        .oneshot(
            Request::builder()
                .uri(format!(
                    "/projects/{project_id}/reports/{report_id}/screening/history"
                ))
                .body(Body::empty())
                .expect("history request should be valid"),
        )
        .await
        .expect("history request should be handled");
    assert_eq!(history.status(), StatusCode::OK);
    let history_body = response_json(history).await;
    assert_eq!(history_body["items"].as_array().map(Vec::len), Some(2));
    assert!(history_body["items"][0]["protocol_version_id"].is_string());
    assert_eq!(
        history_body["items"][1]["supersedes_event_id"],
        serde_json::Value::Null
    );

    sqlx::query("DELETE FROM jobs WHERE payload->>'project_id'=$1")
        .bind(project_id.to_string())
        .execute(&pool)
        .await
        .expect("jobs should clean up");
    sqlx::query("DELETE FROM projects WHERE id=$1")
        .bind(project_id)
        .execute(&pool)
        .await
        .expect("project should clean up");
}
