#![allow(
    clippy::expect_used,
    clippy::panic,
    clippy::unwrap_used,
    clippy::string_slice
)]
use std::collections::HashMap;

use axum::{
    body::{Body, to_bytes},
    http::{Request, Response, StatusCode},
};
use deepref_http_api::{config::ApiConfig, routes::router, state::AppState};
use serde_json::Value;
use sqlx::{PgPool, postgres::PgPoolOptions};
use tower::ServiceExt;
use uuid::Uuid;

async fn database() -> Option<PgPool> {
    let url = std::env::var("DATABASE_URL").ok()?;
    let pool = PgPoolOptions::new()
        .max_connections(8)
        .connect(&url)
        .await
        .ok()?;
    deepref_postgres::migrate(&pool).await.ok()?;
    Some(pool)
}

fn api_config() -> ApiConfig {
    let runtime = deepref_config::RuntimeConfig::from_map(
        "deepref-api-study-test",
        &HashMap::from([("APP_ENV".to_owned(), "local".to_owned())]),
    )
    .expect("local test runtime should parse");
    ApiConfig {
        runtime,
        bind_addr: "127.0.0.1:0".parse().expect("test bind address is valid"),
        cors_allow_any: false,
        cors_origins: Vec::new(),
    }
}

async fn request(pool: &PgPool, method: &str, uri: String, body: Option<Value>) -> Response<Body> {
    let mut builder = Request::builder()
        .method(method)
        .uri(uri)
        .header("x-actor-kind", "user")
        .header("x-actor-id", "study-http-test");
    if body.is_some() {
        builder = builder.header("content-type", "application/json");
    }
    router(AppState::core(pool.clone()), &api_config())
        .oneshot(
            builder
                .body(Body::from(
                    body.map_or_else(String::new, |body| body.to_string()),
                ))
                .expect("study request should be valid"),
        )
        .await
        .expect("study request should be handled")
}

async fn json(response: Response<Body>) -> Value {
    let body = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("response body should be readable");
    serde_json::from_slice(&body).expect("response should be JSON")
}

#[tokio::test]
async fn membership_get_move_and_stale_conflict_are_project_scoped() {
    let Some(pool) = database().await else { return };
    let project_id = Uuid::new_v4();
    let other_project_id = Uuid::new_v4();
    let report_id = Uuid::new_v4();
    sqlx::query("INSERT INTO projects(id,name) VALUES($1,'study HTTP test')")
        .bind(project_id)
        .execute(&pool)
        .await
        .expect("project inserts");
    sqlx::query("INSERT INTO projects(id,name) VALUES($1,'other study HTTP test')")
        .bind(other_project_id)
        .execute(&pool)
        .await
        .expect("other project inserts");
    sqlx::query("INSERT INTO reports(id,title) VALUES($1,'study HTTP report')")
        .bind(report_id)
        .execute(&pool)
        .await
        .expect("report inserts");
    sqlx::query("INSERT INTO project_reports(project_id,report_id) VALUES($1,$2)")
        .bind(project_id)
        .bind(report_id)
        .execute(&pool)
        .await
        .expect("report membership inserts");

    let create_a = request(
        &pool,
        "POST",
        format!("/projects/{project_id}/studies"),
        Some(serde_json::json!({"title":"HTTP source study"})),
    )
    .await;
    assert_eq!(create_a.status(), StatusCode::CREATED);
    let study_a_body = json(create_a).await;
    let study_a = study_a_body["id"]
        .as_str()
        .unwrap()
        .parse::<Uuid>()
        .unwrap();

    let create_b = request(
        &pool,
        "POST",
        format!("/projects/{project_id}/studies"),
        Some(serde_json::json!({"title":"HTTP target study"})),
    )
    .await;
    assert_eq!(create_b.status(), StatusCode::CREATED);
    let study_b_body = json(create_b).await;
    let study_b = study_b_body["id"]
        .as_str()
        .unwrap()
        .parse::<Uuid>()
        .unwrap();

    let unassigned = request(
        &pool,
        "GET",
        format!("/projects/{project_id}/reports/{report_id}/study"),
        None,
    )
    .await;
    assert_eq!(unassigned.status(), StatusCode::NO_CONTENT);

    let wrong_project = request(
        &pool,
        "GET",
        format!("/projects/{other_project_id}/reports/{report_id}/study"),
        None,
    )
    .await;
    assert_eq!(wrong_project.status(), StatusCode::NOT_FOUND);

    let assigned = request(
        &pool,
        "PUT",
        format!("/projects/{project_id}/reports/{report_id}/study"),
        Some(serde_json::json!({
            "study_id": study_a,
            "role": "report_of_study",
            "expected_revision": 0
        })),
    )
    .await;
    assert_eq!(assigned.status(), StatusCode::OK);
    let assigned_body = json(assigned).await;
    assert_eq!(assigned_body["revision"], 1);

    let membership = request(
        &pool,
        "GET",
        format!("/projects/{project_id}/reports/{report_id}/study"),
        None,
    )
    .await;
    assert_eq!(membership.status(), StatusCode::OK);
    let membership_body = json(membership).await;
    assert_eq!(membership_body["study_id"], study_a.to_string());
    assert_eq!(membership_body["role"], "report_of_study");
    assert_eq!(membership_body["study_revision"], 1);
    assert_eq!(membership_body["study"]["id"], study_a.to_string());

    let moved = request(
        &pool,
        "PUT",
        format!("/projects/{project_id}/reports/{report_id}/study"),
        Some(serde_json::json!({
            "study_id": study_b,
            "role": "follow_up",
            "expected_revision": 0,
            "expected_previous_study_revision": 1
        })),
    )
    .await;
    assert_eq!(moved.status(), StatusCode::OK);

    let stale = request(
        &pool,
        "PUT",
        format!("/projects/{project_id}/reports/{report_id}/study"),
        Some(serde_json::json!({
            "study_id": study_a,
            "role": "report_of_study",
            "expected_revision": 0,
            "expected_previous_study_revision": 1
        })),
    )
    .await;
    assert_eq!(stale.status(), StatusCode::CONFLICT);
    let stale_body = json(stale).await;
    assert_eq!(stale_body["code"], "STUDY_REVISION_CONFLICT");
    assert_eq!(stale_body["details"]["current"]["id"], study_a.to_string());
    assert_eq!(stale_body["details"]["current"]["revision"], 2);

    sqlx::query("DELETE FROM projects WHERE id IN ($1, $2)")
        .bind(project_id)
        .bind(other_project_id)
        .execute(&pool)
        .await
        .expect("test project cleanup");
}
