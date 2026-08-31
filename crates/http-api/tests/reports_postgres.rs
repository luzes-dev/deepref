use std::collections::HashMap;

use axum::{
    body::{Body, to_bytes},
    http::{Request, Response, StatusCode},
};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
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
        "deepref-api-reports-test",
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

async fn request(pool: &PgPool, uri: String) -> Response<Body> {
    router(AppState::core(pool.clone()), &api_config())
        .oneshot(
            Request::builder()
                .uri(uri)
                .body(Body::empty())
                .expect("report request should be valid"),
        )
        .await
        .expect("report request should be handled")
}

async fn response_json(response: Response<Body>) -> serde_json::Value {
    let body = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("response body should be readable");
    serde_json::from_slice(&body).expect("response should be JSON")
}

#[tokio::test]
async fn postgres_reports_are_listed_and_loaded_by_uuid_without_doi_identity() {
    let Some(pool) = database().await else {
        return;
    };

    let project_id = Uuid::new_v4();
    let doi_report_id = Uuid::new_v4();
    let identifier_free_report_id = Uuid::new_v4();
    let doi_identifier_id = Uuid::new_v4();
    let doi = format!("10.5555/deepref-report-{}", Uuid::new_v4());

    sqlx::query("INSERT INTO projects (id,name) VALUES ($1,'report identity test')")
        .bind(project_id)
        .execute(&pool)
        .await
        .expect("test project should insert");
    sqlx::query(
        "INSERT INTO reports (id,title,publication_year) VALUES ($1,'DOI report',2024),($2,'Identifier-free report',2023)",
    )
    .bind(doi_report_id)
    .bind(identifier_free_report_id)
    .execute(&pool)
    .await
    .expect("test reports should insert");
    sqlx::query(
        "INSERT INTO report_identifiers (id,report_id,scheme,value,normalized_value) VALUES ($1,$2,'doi',$3,$3)",
    )
    .bind(doi_identifier_id)
    .bind(doi_report_id)
    .bind(&doi)
    .execute(&pool)
    .await
    .expect("test DOI identifier should insert");
    sqlx::query("INSERT INTO project_reports (project_id,report_id) VALUES ($1,$2),($1,$3)")
        .bind(project_id)
        .bind(doi_report_id)
        .bind(identifier_free_report_id)
        .execute(&pool)
        .await
        .expect("test memberships should insert");

    let list = request(&pool, format!("/projects/{project_id}/reports?limit=10")).await;
    assert_eq!(list.status(), StatusCode::OK);
    let list = response_json(list).await;
    let items = list["items"]
        .as_array()
        .expect("report list should be an array");
    assert_eq!(items.len(), 2);
    let listed_ids = items
        .iter()
        .filter_map(|item| item["report_id"].as_str())
        .map(str::to_owned)
        .collect::<Vec<_>>();
    assert!(listed_ids.contains(&doi_report_id.to_string()));
    assert!(listed_ids.contains(&identifier_free_report_id.to_string()));
    assert!(
        items
            .iter()
            .all(|item| !item.as_object().unwrap().contains_key("doi_key"))
    );

    for (report_id, expected_doi, expected_title) in [
        (doi_report_id, Some(doi.as_str()), "DOI report"),
        (identifier_free_report_id, None, "Identifier-free report"),
    ] {
        let detail = request(&pool, format!("/projects/{project_id}/reports/{report_id}")).await;
        assert_eq!(detail.status(), StatusCode::OK);
        let detail = response_json(detail).await;
        assert_eq!(detail["report_id"], report_id.to_string());
        assert_eq!(detail["doi"].as_str(), expected_doi);
        assert_eq!(detail["title"], expected_title);
        assert!(!detail.as_object().unwrap().contains_key("doi_key"));
    }

    let doi_key = URL_SAFE_NO_PAD.encode(doi.as_bytes());
    let legacy_identity = request(&pool, format!("/projects/{project_id}/reports/{doi_key}")).await;
    assert_eq!(legacy_identity.status(), StatusCode::BAD_REQUEST);

    sqlx::query("DELETE FROM projects WHERE id=$1")
        .bind(project_id)
        .execute(&pool)
        .await
        .expect("test project should clean up");
    sqlx::query("DELETE FROM reports WHERE id IN ($1,$2)")
        .bind(doi_report_id)
        .bind(identifier_free_report_id)
        .execute(&pool)
        .await
        .expect("test reports should clean up");
}
