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
use deepref_domain::ProviderKind;
use deepref_http_api::{config::ApiConfig, routes::router, state::AppState};
use serde_json::json;
use sqlx::{PgPool, Row};
use tower::ServiceExt;
use uuid::Uuid;

async fn database() -> Option<PgPool> {
    deepref_app::test_isolation::isolated_pool_from_env_first(
        &["TEST_DATABASE_URL", "DATABASE_URL"],
        5,
    )
    .await
}

/// Every test but the missing-contact one identifies itself with this address.
const TEST_CONTACT_EMAIL: &str = "tests@example.org";

#[tokio::test]
async fn doi_acquisitions_require_a_contact_email_and_expose_project_scoped_items_and_depth() {
    let Some(pool) = database().await else { return };
    let project = Uuid::new_v4();
    sqlx::query("INSERT INTO projects (id,name) VALUES ($1,'canonical DOI')")
        .bind(project)
        .execute(&pool)
        .await
        .unwrap();
    let create = || {
        Request::builder()
            .method("POST")
            .uri(format!("/projects/{project}/acquisitions"))
            .header("content-type", "application/json")
            .body(Body::from(
                json!({"seed_dois":["10.1000/canonical"],"max_depth":2}).to_string(),
            ))
            .unwrap()
    };
    let rejected = call_with_contact_email(&pool, create(), "").await;
    assert_eq!(rejected.status(), StatusCode::SERVICE_UNAVAILABLE);
    let count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM acquisition_runs WHERE project_id=$1")
            .bind(project)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(count, 0);
    let created = call(&pool, create()).await;
    assert_eq!(created.status(), StatusCode::CREATED);
    let body = json_body(created).await;
    assert_eq!(body["max_depth"], 2);
    let id = Uuid::parse_str(body["id"].as_str().unwrap()).unwrap();
    let items = call(
        &pool,
        Request::builder()
            .uri(format!("/projects/{project}/acquisitions/{id}/items"))
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(items.status(), StatusCode::OK);
    let items = json_body(items).await;
    assert_eq!(items["items"][0]["identifier"], "10.1000/canonical");
    assert_eq!(items["items"][0]["kind"], "doi");
    assert_eq!(items["items"][0]["depth"], 0);
    assert!(items["items"][0]["report_id"].is_string());
    let wrong_project = Uuid::new_v4();
    for suffix in ["items", "cancel"] {
        let response = call(
            &pool,
            Request::builder()
                .method(if suffix == "cancel" { "POST" } else { "GET" })
                .uri(format!(
                    "/projects/{wrong_project}/acquisitions/{id}/{suffix}"
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }
}

fn api_config() -> ApiConfig {
    let runtime = deepref_config::RuntimeConfig::from_map(
        "deepref-api-acquisition-test",
        &HashMap::from([("APP_ENV".to_owned(), "local".to_owned())]),
    )
    .expect("local test runtime should parse");
    ApiConfig {
        documents: deepref_config::DocumentConfig::from_source(&|name| {
            (name == "APP_ENV").then(|| "local".into())
        })
        .expect("local test document configuration"),
        runtime,
        bind_addr: "127.0.0.1:0".parse().unwrap(),
        cors_allow_any: false,
        cors_origins: vec!["http://localhost:3000".parse().unwrap()],
    }
}

async fn call(pool: &PgPool, request: Request<Body>) -> Response<Body> {
    call_with_contact_email(pool, request, TEST_CONTACT_EMAIL).await
}

async fn call_with_contact_email(
    pool: &PgPool,
    request: Request<Body>,
    contact_email: &str,
) -> Response<Body> {
    let integrations = deepref_config::IntegrationsConfig {
        contact_email: contact_email.to_owned(),
        ..Default::default()
    };
    call_with_integrations(pool, request, integrations).await
}

async fn call_with_integrations(
    pool: &PgPool,
    request: Request<Body>,
    integrations: deepref_config::IntegrationsConfig,
) -> Response<Body> {
    let config = api_config();
    router(
        AppState::core(pool.clone()).with_integrations(integrations),
        &config,
    )
    .oneshot(request)
    .await
    .expect("request should be handled")
}

async fn call_with_workflow_policy(
    pool: &PgPool,
    request: Request<Body>,
    enabled: bool,
) -> Response<Body> {
    let mut config = api_config();
    config.runtime.workflow.niche_nodes_enabled = enabled;
    let state = AppState::core(pool.clone()).with_workflow_config(config.runtime.workflow.clone());
    router(state, &config)
        .oneshot(request)
        .await
        .expect("workflow request")
}

async fn json_body(response: Response<Body>) -> serde_json::Value {
    let body = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("body should be readable");
    serde_json::from_slice(&body).expect("body should be JSON")
}

fn refresh_request(project_id: Uuid, acquisition_id: Uuid, key: &str) -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri(format!(
            "/projects/{project_id}/acquisitions/{acquisition_id}/refresh"
        ))
        .header("idempotency-key", key)
        .body(Body::empty())
        .expect("refresh request should be valid")
}

fn create_acquisition_request(
    project_id: Uuid,
    key: Option<&str>,
    body: serde_json::Value,
) -> Request<Body> {
    let mut builder = Request::builder()
        .method("POST")
        .uri(format!("/projects/{project_id}/acquisitions"))
        .header("content-type", "application/json");
    if let Some(key) = key {
        builder = builder.header("idempotency-key", key);
    }
    builder
        .body(Body::from(body.to_string()))
        .expect("acquisition request should be valid")
}

/// A DOI run row written directly, as an earlier deployment would have left it.
async fn insert_keyed_doi_run(
    pool: &PgPool,
    project_id: Uuid,
    key: &str,
    config: serde_json::Value,
) -> Uuid {
    let run_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO acquisition_runs
         (id,project_id,source,strategy,idempotency_key,config,metadata,status,max_depth,seed_count,
          queued_count,fetched_count,failed_count,metadata_provider,citation_provider,created_at)
         VALUES ($1,$2,'crossref','citation_traversal',$3,$4,'{}','queued',3,1,1,0,0,'crossref','crossref',now())",
    )
    .bind(run_id)
    .bind(project_id)
    .bind(key)
    .bind(config)
    .execute(pool)
    .await
    .expect("keyed test acquisition should insert");
    run_id
}

async fn create_completed_doi_acquisition(pool: &PgPool, project_id: Uuid, doi: &str) -> Uuid {
    let response = call(
        pool,
        Request::builder()
            .method("POST")
            .uri(format!("/projects/{project_id}/acquisitions"))
            .header("content-type", "application/json")
            .body(Body::from(
                json!({"seed_dois": [doi], "max_depth": 3}).to_string(),
            ))
            .expect("acquisition request should be valid"),
    )
    .await;
    assert_eq!(response.status(), StatusCode::CREATED);
    let body = json_body(response).await;
    let acquisition_id = body["id"]
        .as_str()
        .expect("created acquisition should have an id")
        .parse::<Uuid>()
        .expect("acquisition id should be a UUID");
    sqlx::query(
        "UPDATE acquisition_runs
         SET status='completed',started_at=now(),completed_at=now(),queued_count=0,fetched_count=1
         WHERE id=$1 AND project_id=$2",
    )
    .bind(acquisition_id)
    .bind(project_id)
    .execute(pool)
    .await
    .expect("test acquisition should become completed");
    acquisition_id
}

async fn insert_completed_acquisition(
    pool: &PgPool,
    project_id: Uuid,
    source: &str,
    strategy: &str,
    format: Option<&str>,
    config: serde_json::Value,
) -> Uuid {
    let acquisition_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO acquisition_runs
         (id,project_id,source,strategy,format,config,metadata,status,created_at,started_at,completed_at)
         VALUES ($1,$2,$3,$4,$5,$6,'{}','completed',now(),now(),now())",
    )
    .bind(acquisition_id)
    .bind(project_id)
    .bind(source)
    .bind(strategy)
    .bind(format)
    .bind(config)
    .execute(pool)
    .await
    .expect("test acquisition should insert");
    acquisition_id
}

async fn count_run_items(pool: &PgPool, acquisition_id: Uuid) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM acquisition_items WHERE acquisition_id=$1")
        .bind(acquisition_id)
        .fetch_one(pool)
        .await
        .expect("ingestion item count should load")
}

async fn count_run_jobs(pool: &PgPool, acquisition_id: Uuid) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM jobs WHERE payload->'payload'->>'acquisition_id'=$1")
        .bind(acquisition_id.to_string())
        .fetch_one(pool)
        .await
        .expect("job count should load")
}

async fn clean_acquisition_projects(pool: &PgPool, project_ids: &[Uuid], run_ids: &[Uuid]) {
    for run_id in run_ids {
        sqlx::query("DELETE FROM jobs WHERE payload->'payload'->>'acquisition_id'=$1")
            .bind(run_id.to_string())
            .execute(pool)
            .await
            .expect("test jobs should clean up");
    }
    for project_id in project_ids {
        sqlx::query("DELETE FROM projects WHERE id=$1")
            .bind(project_id)
            .execute(pool)
            .await
            .expect("test project should clean up");
    }
}

#[tokio::test]
async fn imports_are_idempotent_and_preserve_raw_records_without_reports() {
    let Some(pool) = database().await else {
        return;
    };
    let project_id = Uuid::new_v4();
    sqlx::query("INSERT INTO projects (id,name) VALUES ($1,'acquisition import test')")
        .bind(project_id)
        .execute(&pool)
        .await
        .unwrap();

    let payload = json!({
        "format": "ris",
        "content": "TY  - JOUR\nTI  - Über die Wirkung von Kaffee\nAU  - Müller, Ana\nPY  - 2024/\nJO  - Journal of Café Studies\nDO  - 10.5555/example-unicode\nER  -\n"
    });
    let request = Request::builder()
        .method("POST")
        .uri(format!("/projects/{project_id}/imports"))
        .header("content-type", "application/json")
        .header("idempotency-key", "import-test-1")
        .body(Body::from(payload.to_string()))
        .unwrap();
    let first = call(&pool, request).await;
    assert_eq!(first.status(), StatusCode::CREATED);
    let first = json_body(first).await;
    let run_id = first["id"].as_str().unwrap().parse::<Uuid>().unwrap();

    let record_count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM records WHERE project_id=$1 AND acquisition_run_id=$2",
    )
    .bind(project_id)
    .bind(run_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(record_count, 1);
    let record = sqlx::query(
        "SELECT report_id,source,source_key,journal,authors,source_identifiers
         FROM records WHERE project_id=$1 AND acquisition_run_id=$2",
    )
    .bind(project_id)
    .bind(run_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(record.get::<Option<Uuid>, _>("report_id").is_none());
    assert_eq!(record.get::<String, _>("source"), "import:ris");
    assert!(
        record
            .get::<String, _>("source_key")
            .starts_with(&run_id.to_string())
    );
    assert_eq!(
        record.get::<String, _>("journal"),
        "Journal of Café Studies"
    );
    assert_eq!(
        record.get::<serde_json::Value, _>("source_identifiers")[0]["normalized_value"],
        "10.5555/example-unicode"
    );
    assert_eq!(
        record.get::<serde_json::Value, _>("authors")[0]["family"],
        "Müller"
    );
    let identifier_count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM record_identifiers WHERE record_id IN (SELECT id FROM records WHERE acquisition_run_id=$1)",
    )
    .bind(run_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(identifier_count, 1);

    let repeat = Request::builder()
        .method("POST")
        .uri(format!("/projects/{project_id}/imports"))
        .header("content-type", "application/json")
        .header("idempotency-key", "import-test-1")
        .body(Body::from(payload.to_string()))
        .unwrap();
    let repeat = call(&pool, repeat).await;
    assert_eq!(repeat.status(), StatusCode::OK);
    assert_eq!(json_body(repeat).await["id"], run_id.to_string());
    let repeated_count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM records WHERE acquisition_run_id=$1")
            .bind(run_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(repeated_count, 1);

    let conflict_payload = json!({
        "format": "doi",
        "content": "10.5555/different"
    });
    let conflict = Request::builder()
        .method("POST")
        .uri(format!("/projects/{project_id}/imports"))
        .header("content-type", "application/json")
        .header("idempotency-key", "import-test-1")
        .body(Body::from(conflict_payload.to_string()))
        .unwrap();
    assert_eq!(call(&pool, conflict).await.status(), StatusCode::CONFLICT);

    let csv_without_mapping = Request::builder()
        .method("POST")
        .uri(format!("/projects/{project_id}/imports"))
        .header("content-type", "application/json")
        .body(Body::from(
            json!({"format": "csv", "content": "title\nA paper\n"}).to_string(),
        ))
        .unwrap();
    assert_eq!(
        call(&pool, csv_without_mapping).await.status(),
        StatusCode::BAD_REQUEST
    );

    let oversized_import = Request::builder()
        .method("POST")
        .uri(format!("/projects/{project_id}/imports"))
        .header("content-type", "application/json")
        .body(Body::from(
            json!({
                "format": "doi",
                "content": "x".repeat(2 * 1024 * 1024 + 1)
            })
            .to_string(),
        ))
        .unwrap();
    assert_eq!(
        call(&pool, oversized_import).await.status(),
        StatusCode::PAYLOAD_TOO_LARGE
    );

    let list = Request::builder()
        .uri(format!("/projects/{project_id}/acquisitions?limit=1"))
        .body(Body::empty())
        .unwrap();
    let list = call(&pool, list).await;
    assert_eq!(list.status(), StatusCode::OK);
    let list = json_body(list).await;
    assert_eq!(list["items"].as_array().unwrap().len(), 1);
    assert!(list["next_cursor"].is_null());

    sqlx::query("DELETE FROM projects WHERE id=$1")
        .bind(project_id)
        .execute(&pool)
        .await
        .unwrap();
}

#[tokio::test]
async fn doi_acquisition_persists_provenance_before_durable_jobs_are_claimable() {
    let Some(pool) = database().await else {
        return;
    };
    let project_id = Uuid::new_v4();
    sqlx::query("INSERT INTO projects (id,name) VALUES ($1,'acquisition traversal test')")
        .bind(project_id)
        .execute(&pool)
        .await
        .unwrap();
    let request = Request::builder()
        .method("POST")
        .uri(format!("/projects/{project_id}/acquisitions"))
        .header("content-type", "application/json")
        .body(Body::from(
            json!({"seed_dois": [
                "https://doi.org/10.5555/traversal",
                "DOI:10.5555/TRAVERSAL.",
                "10.5555/traversal"
            ]})
            .to_string(),
        ))
        .unwrap();
    let response = call(&pool, request).await;
    assert_eq!(response.status(), StatusCode::CREATED);
    let body = json_body(response).await;
    let run_id = body["id"].as_str().unwrap().parse::<Uuid>().unwrap();
    assert_eq!(body["seed_count"], 1);
    assert_eq!(body["queued_count"], 1);
    let linked: Option<Uuid> =
        sqlx::query_scalar("SELECT acquisition_run_id FROM records WHERE acquisition_run_id=$1")
            .bind(run_id)
            .fetch_optional(&pool)
            .await
            .unwrap();
    assert!(linked.is_none());
    let run_exists: bool =
        sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM acquisition_runs WHERE id=$1)")
            .bind(run_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    let jobs: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM jobs WHERE kind='work_fetch_requested' AND payload->'payload'->>'acquisition_id'=$1",
    )
    .bind(run_id.to_string())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(run_exists);
    assert_eq!(jobs, 1);
    let items: i64 =
        sqlx::query_scalar("SELECT count(*) FROM acquisition_items WHERE acquisition_id=$1")
            .bind(run_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(items, 1);
    sqlx::query("DELETE FROM jobs WHERE payload->'payload'->>'acquisition_id'=$1")
        .bind(run_id.to_string())
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("DELETE FROM projects WHERE id=$1")
        .bind(project_id)
        .execute(&pool)
        .await
        .unwrap();
}

#[tokio::test]
async fn create_acquisition_idempotency_is_replay_safe_under_concurrency() {
    let Some(pool) = database().await else {
        return;
    };
    let project_id = Uuid::new_v4();
    sqlx::query("INSERT INTO projects (id,name) VALUES ($1,'concurrent acquisition test')")
        .bind(project_id)
        .execute(&pool)
        .await
        .unwrap();

    let request = || {
        Request::builder()
            .method("POST")
            .uri(format!("/projects/{project_id}/acquisitions"))
            .header("content-type", "application/json")
            .header("idempotency-key", "concurrent-acquisition")
            .body(Body::from(
                json!({"seed_dois": ["10.5555/concurrent"]}).to_string(),
            ))
            .unwrap()
    };
    let (first, second) = tokio::join!(call(&pool, request()), call(&pool, request()));
    let statuses = [first.status(), second.status()];
    assert!(statuses.contains(&StatusCode::CREATED));
    assert!(statuses.contains(&StatusCode::OK));

    let acquisition_count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM acquisition_runs WHERE project_id=$1 AND idempotency_key=$2",
    )
    .bind(project_id)
    .bind("concurrent-acquisition")
    .fetch_one(&pool)
    .await
    .unwrap();
    let ingestion_count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM acquisition_runs WHERE project_id=$1")
            .bind(project_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    let job_count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM jobs WHERE kind='work_fetch_requested' AND payload->'payload'->>'project_id'=$1",
    )
    .bind(project_id.to_string())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(acquisition_count, 1);
    assert_eq!(ingestion_count, 1);
    assert_eq!(job_count, 1);

    let conflict_request = |doi: &str| {
        Request::builder()
            .method("POST")
            .uri(format!("/projects/{project_id}/acquisitions"))
            .header("content-type", "application/json")
            .header("idempotency-key", "concurrent-acquisition-conflict")
            .body(Body::from(json!({"seed_dois": [doi]}).to_string()))
            .unwrap()
    };
    let (conflict_first, conflict_second) = tokio::join!(
        call(&pool, conflict_request("10.5555/first")),
        call(&pool, conflict_request("10.5555/second")),
    );
    let conflict_statuses = [conflict_first.status(), conflict_second.status()];
    assert!(conflict_statuses.contains(&StatusCode::CREATED));
    assert!(conflict_statuses.contains(&StatusCode::CONFLICT));

    sqlx::query("DELETE FROM jobs WHERE payload->'payload'->>'project_id'=$1")
        .bind(project_id.to_string())
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("DELETE FROM projects WHERE id=$1")
        .bind(project_id)
        .execute(&pool)
        .await
        .unwrap();
}

#[tokio::test]
async fn importing_into_a_missing_project_returns_not_found() {
    let Some(pool) = database().await else {
        return;
    };
    let project_id = Uuid::new_v4();
    let request = Request::builder()
        .method("POST")
        .uri(format!("/projects/{project_id}/imports"))
        .header("content-type", "application/json")
        .body(Body::from(
            json!({"format": "doi", "content": "10.5555/missing-project"}).to_string(),
        ))
        .unwrap();
    assert_eq!(call(&pool, request).await.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn completed_provider_refresh_is_lineaged_idempotent_and_preserves_source_data() {
    let Some(pool) = database().await else {
        return;
    };
    let project_id = Uuid::new_v4();
    sqlx::query("INSERT INTO projects (id,name) VALUES ($1,'provider refresh test')")
        .bind(project_id)
        .execute(&pool)
        .await
        .expect("test project should insert");

    let source_id =
        create_completed_doi_acquisition(&pool, project_id, "10.5555/refresh-source").await;
    let source_record_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO records (id,project_id,acquisition_run_id,report_id,source,source_key,title,raw)
         SELECT $1,$2,$3,report_id,'crossref',$4,'Source record','{}'
         FROM acquisition_items WHERE acquisition_id=$3 AND canonical_doi='10.5555/refresh-source'",
    )
    .bind(source_record_id)
    .bind(project_id)
    .bind(source_id)
    .bind(format!("{source_id}:source"))
    .execute(&pool)
    .await
    .expect("source record should insert");
    sqlx::query(
        "UPDATE acquisition_items
         SET status='fetched',attempts=1,fetched_at=now()
         WHERE acquisition_id=$2 AND report_id=(SELECT report_id FROM records WHERE id=$1)",
    )
    .bind(source_record_id)
    .bind(source_id)
    .execute(&pool)
    .await
    .expect("source provenance should become fetched");

    let source_before = sqlx::query(
        "SELECT source,strategy,status,config,metadata FROM acquisition_runs WHERE id=$1",
    )
    .bind(source_id)
    .fetch_one(&pool)
    .await
    .expect("source should load");
    let source_config_before: serde_json::Value = source_before.get("config");
    let source_metadata_before: serde_json::Value = source_before.get("metadata");
    let source_jobs_before = count_run_jobs(&pool, source_id).await;
    let source_items_before = count_run_items(&pool, source_id).await;

    let first = call(
        &pool,
        refresh_request(project_id, source_id, "refresh-key-1"),
    )
    .await;
    assert_eq!(first.status(), StatusCode::CREATED);
    let first_body = json_body(first).await;
    let refresh_id = first_body["id"]
        .as_str()
        .expect("refresh should have an id")
        .parse::<Uuid>()
        .expect("refresh id should be a UUID");
    assert_ne!(refresh_id, source_id);
    assert_eq!(first_body["strategy"], "provider_refresh");
    assert_eq!(first_body["status"], "queued");
    assert_eq!(first_body["refresh_of"], source_id.to_string());
    assert_eq!(first_body["seed_count"], 1);
    assert_eq!(first_body["queued_count"], 1);
    assert_eq!(count_run_items(&pool, refresh_id).await, 1);
    assert_eq!(count_run_jobs(&pool, refresh_id).await, 1);

    let refresh_row = sqlx::query(
        "SELECT source,strategy,status,config,metadata FROM acquisition_runs WHERE id=$1",
    )
    .bind(refresh_id)
    .fetch_one(&pool)
    .await
    .expect("refresh should persist");
    assert_eq!(refresh_row.get::<String, _>("source"), "crossref");
    assert_eq!(refresh_row.get::<String, _>("strategy"), "provider_refresh");
    assert_eq!(refresh_row.get::<String, _>("status"), "queued");
    let refresh_config: serde_json::Value = refresh_row.get("config");
    assert_eq!(
        refresh_config["seed_dois"],
        source_config_before["seed_dois"]
    );
    assert_eq!(
        refresh_config["max_depth"],
        source_config_before["max_depth"]
    );
    assert_eq!(
        refresh_config["metadata_providers"],
        source_config_before["metadata_providers"]
    );
    assert_eq!(refresh_config["refresh_of"], source_id.to_string());
    let refresh_metadata: serde_json::Value = refresh_row.get("metadata");
    assert_eq!(refresh_metadata["refresh_of"], source_id.to_string());
    assert_eq!(refresh_metadata["refresh_kind"], "provider_refresh");

    let replay = call(
        &pool,
        refresh_request(project_id, source_id, "refresh-key-1"),
    )
    .await;
    assert_eq!(replay.status(), StatusCode::OK);
    assert_eq!(json_body(replay).await["id"], refresh_id.to_string());
    assert_eq!(count_run_items(&pool, refresh_id).await, 1);
    assert_eq!(count_run_jobs(&pool, refresh_id).await, 1);

    let second_source_id =
        create_completed_doi_acquisition(&pool, project_id, "10.5555/refresh-second-source").await;
    let conflicting_replay = call(
        &pool,
        refresh_request(project_id, second_source_id, "refresh-key-1"),
    )
    .await;
    assert_eq!(conflicting_replay.status(), StatusCode::CONFLICT);
    assert_eq!(
        json_body(conflicting_replay).await["code"],
        "IDEMPOTENCY_KEY_REUSED"
    );
    assert_eq!(count_run_items(&pool, second_source_id).await, 1);

    sqlx::query(
        "UPDATE acquisition_runs SET status='completed',started_at=now(),completed_at=now()
         WHERE id=$1",
    )
    .bind(refresh_id)
    .execute(&pool)
    .await
    .expect("refresh should become completed for the lineage test");
    let chained = call(
        &pool,
        refresh_request(project_id, refresh_id, "refresh-key-2"),
    )
    .await;
    assert_eq!(chained.status(), StatusCode::CREATED);
    let chained_body = json_body(chained).await;
    let chained_id = chained_body["id"]
        .as_str()
        .expect("chained refresh should have an id")
        .parse::<Uuid>()
        .expect("chained refresh id should be a UUID");
    assert_eq!(chained_body["refresh_of"], refresh_id.to_string());
    assert_eq!(chained_body["strategy"], "provider_refresh");
    assert_eq!(count_run_items(&pool, chained_id).await, 1);
    assert_eq!(count_run_jobs(&pool, chained_id).await, 1);

    let list = call(
        &pool,
        Request::builder()
            .uri(format!("/projects/{project_id}/acquisitions?limit=10"))
            .body(Body::empty())
            .expect("acquisition list request should be valid"),
    )
    .await;
    assert_eq!(list.status(), StatusCode::OK);
    let list_body = json_body(list).await;
    let listed_refresh = list_body["items"]
        .as_array()
        .and_then(|items| {
            items
                .iter()
                .find(|item| item["id"] == refresh_id.to_string())
        })
        .expect("refresh should be present in the acquisition list");
    assert_eq!(listed_refresh["refresh_of"], source_id.to_string());

    let source_after = sqlx::query(
        "SELECT source,strategy,status,config,metadata FROM acquisition_runs WHERE id=$1",
    )
    .bind(source_id)
    .fetch_one(&pool)
    .await
    .expect("source should remain available");
    assert_eq!(
        source_after.get::<String, _>("source"),
        source_before.get::<String, _>("source")
    );
    assert_eq!(
        source_after.get::<String, _>("strategy"),
        source_before.get::<String, _>("strategy")
    );
    assert_eq!(source_after.get::<String, _>("status"), "completed");
    assert_eq!(
        source_after.get::<serde_json::Value, _>("config"),
        source_config_before
    );
    assert_eq!(
        source_after.get::<serde_json::Value, _>("metadata"),
        source_metadata_before
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM records WHERE acquisition_run_id=$1")
            .bind(source_id)
            .fetch_one(&pool)
            .await
            .expect("source records should remain"),
        1
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM acquisition_items WHERE acquisition_id=$1 AND status='fetched'",
        )
        .bind(source_id)
        .fetch_one(&pool)
        .await
        .expect("source provenance should remain"),
        1
    );
    assert_eq!(count_run_items(&pool, source_id).await, source_items_before);
    assert_eq!(count_run_jobs(&pool, source_id).await, source_jobs_before);

    clean_acquisition_projects(
        &pool,
        &[project_id],
        &[source_id, refresh_id, second_source_id, chained_id],
    )
    .await;
}

#[tokio::test]
async fn new_doi_runs_store_their_chain_in_config() {
    let Some(pool) = database().await else {
        return;
    };
    let project_id = Uuid::new_v4();
    sqlx::query("INSERT INTO projects (id,name) VALUES ($1,'chain storage test')")
        .bind(project_id)
        .execute(&pool)
        .await
        .expect("test project should insert");

    let created = call(
        &pool,
        create_acquisition_request(
            project_id,
            None,
            json!({"seed_dois": ["10.5555/stored-chain"]}),
        ),
    )
    .await;
    assert_eq!(created.status(), StatusCode::CREATED);
    let body = json_body(created).await;
    let run_id = body["id"]
        .as_str()
        .expect("created acquisition should have an id")
        .parse::<Uuid>()
        .expect("acquisition id should be a UUID");
    assert_eq!(body["source"], "crossref");

    let row = sqlx::query(
        "SELECT source,config,metadata_provider,citation_provider FROM acquisition_runs WHERE id=$1",
    )
    .bind(run_id)
    .fetch_one(&pool)
    .await
    .expect("created run should persist");
    assert_eq!(row.get::<String, _>("source"), "crossref");
    assert_eq!(row.get::<String, _>("metadata_provider"), "crossref");
    assert_eq!(row.get::<String, _>("citation_provider"), "crossref");
    assert_eq!(
        row.get::<serde_json::Value, _>("config"),
        json!({
            "seed_dois": ["10.5555/stored-chain"],
            "max_depth": body["max_depth"],
            "metadata_providers": ["crossref"],
        })
    );

    clean_acquisition_projects(&pool, &[project_id], &[run_id]).await;
}

#[tokio::test]
async fn legacy_doi_rows_refresh_into_the_current_config_shape() {
    let Some(pool) = database().await else {
        return;
    };
    let project_id = Uuid::new_v4();
    sqlx::query("INSERT INTO projects (id,name) VALUES ($1,'legacy refresh test')")
        .bind(project_id)
        .execute(&pool)
        .await
        .expect("test project should insert");
    let legacy_id = insert_completed_acquisition(
        &pool,
        project_id,
        "crossref",
        "citation_traversal",
        None,
        json!({
            "seed_dois": ["10.5555/legacy"],
            "max_depth": 2,
            "metadata_provider": "crossref",
            "citation_provider": "crossref",
        }),
    )
    .await;

    let refresh = call(
        &pool,
        refresh_request(project_id, legacy_id, "legacy-refresh-key"),
    )
    .await;
    assert_eq!(refresh.status(), StatusCode::CREATED);
    let body = json_body(refresh).await;
    let refresh_id = body["id"]
        .as_str()
        .expect("refresh should have an id")
        .parse::<Uuid>()
        .expect("refresh id should be a UUID");
    assert_eq!(body["refresh_of"], legacy_id.to_string());

    let config: serde_json::Value =
        sqlx::query_scalar("SELECT config FROM acquisition_runs WHERE id=$1")
            .bind(refresh_id)
            .fetch_one(&pool)
            .await
            .expect("refresh config should load");
    assert_eq!(
        config,
        json!({
            "seed_dois": ["10.5555/legacy"],
            "max_depth": 2,
            "metadata_providers": ["crossref"],
            "refresh_of": legacy_id,
        })
    );

    let items = call(
        &pool,
        Request::builder()
            .uri(format!(
                "/projects/{project_id}/acquisitions/{refresh_id}/items"
            ))
            .body(Body::empty())
            .expect("items request should be valid"),
    )
    .await;
    assert_eq!(items.status(), StatusCode::OK);
    assert_eq!(json_body(items).await["items"][0]["kind"], "doi");

    clean_acquisition_projects(&pool, &[project_id], &[legacy_id, refresh_id]).await;
}

#[tokio::test]
async fn a_refresh_replays_the_chain_its_source_stored_not_the_current_default() {
    let Some(pool) = database().await else {
        return;
    };
    let project_id = Uuid::new_v4();
    sqlx::query("INSERT INTO projects (id,name) VALUES ($1,'chain replay test')")
        .bind(project_id)
        .execute(&pool)
        .await
        .expect("test project should insert");
    let source_id = insert_completed_acquisition(
        &pool,
        project_id,
        "crossref",
        "citation_traversal",
        None,
        json!({
            "seed_dois": ["10.5555/chained"],
            "max_depth": 1,
            "metadata_providers": ["openalex", "crossref"],
        }),
    )
    .await;

    let refresh = call(
        &pool,
        refresh_request(project_id, source_id, "chain-replay-key"),
    )
    .await;
    assert_eq!(refresh.status(), StatusCode::CREATED);
    let refresh_id = json_body(refresh).await["id"]
        .as_str()
        .expect("refresh should have an id")
        .parse::<Uuid>()
        .expect("refresh id should be a UUID");

    let config: serde_json::Value =
        sqlx::query_scalar("SELECT config FROM acquisition_runs WHERE id=$1")
            .bind(refresh_id)
            .fetch_one(&pool)
            .await
            .expect("refresh config should load");
    assert_eq!(
        config["metadata_providers"],
        json!(["openalex", "crossref"])
    );
    assert_eq!(config["refresh_of"], source_id.to_string());

    clean_acquisition_projects(&pool, &[project_id], &[source_id, refresh_id]).await;
}

#[tokio::test]
async fn a_retry_matches_a_legacy_row_and_a_changed_default_chain_conflicts() {
    let Some(pool) = database().await else {
        return;
    };
    let project_id = Uuid::new_v4();
    sqlx::query("INSERT INTO projects (id,name) VALUES ($1,'retry comparison test')")
        .bind(project_id)
        .execute(&pool)
        .await
        .expect("test project should insert");

    let legacy_run = insert_keyed_doi_run(
        &pool,
        project_id,
        "legacy-retry",
        json!({
            "seed_dois": ["10.5555/retry"],
            "max_depth": 3,
            "metadata_provider": "crossref",
            "citation_provider": "crossref",
        }),
    )
    .await;
    let retry = call(
        &pool,
        create_acquisition_request(
            project_id,
            Some("legacy-retry"),
            json!({"seed_dois": ["10.5555/retry"], "max_depth": 3}),
        ),
    )
    .await;
    assert_eq!(retry.status(), StatusCode::OK);
    assert_eq!(json_body(retry).await["id"], legacy_run.to_string());

    // The same key and seeds, but the row chose a longer chain than the deployment default.
    insert_keyed_doi_run(
        &pool,
        project_id,
        "changed-default",
        json!({
            "seed_dois": ["10.5555/changed"],
            "max_depth": 3,
            "metadata_providers": ["openalex", "crossref"],
        }),
    )
    .await;
    let conflict = call(
        &pool,
        create_acquisition_request(
            project_id,
            Some("changed-default"),
            json!({"seed_dois": ["10.5555/changed"], "max_depth": 3}),
        ),
    )
    .await;
    assert_eq!(conflict.status(), StatusCode::CONFLICT);
    assert_eq!(json_body(conflict).await["code"], "IDEMPOTENCY_KEY_REUSED");

    clean_acquisition_projects(&pool, &[project_id], &[legacy_run]).await;
}

#[tokio::test]
async fn doi_acquisitions_accept_only_enabled_metadata_providers() {
    let Some(pool) = database().await else {
        return;
    };
    let project_id = Uuid::new_v4();
    sqlx::query("INSERT INTO projects (id,name) VALUES ($1,'provider validation test')")
        .bind(project_id)
        .execute(&pool)
        .await
        .expect("test project should insert");

    for (providers, message) in [
        (
            json!(["openalex"]),
            "openalex is not enabled in this deployment (enabled: crossref)",
        ),
        (
            json!(["crossref", "datacite"]),
            "datacite is not enabled in this deployment (enabled: crossref)",
        ),
        (
            json!(["pubmed"]),
            "pubmed is not enabled in this deployment (enabled: crossref)",
        ),
    ] {
        let response = call(
            &pool,
            create_acquisition_request(
                project_id,
                None,
                json!({"seed_dois": ["10.5555/provider-validation"], "metadata_providers": providers}),
            ),
        )
        .await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST, "{providers}");
        assert_eq!(json_body(response).await["message"], message);
    }
    let empty = call(
        &pool,
        create_acquisition_request(
            project_id,
            None,
            json!({"seed_dois": ["10.5555/provider-validation"], "metadata_providers": []}),
        ),
    )
    .await;
    assert_eq!(empty.status(), StatusCode::BAD_REQUEST);
    let runs: i64 = sqlx::query_scalar("SELECT count(*) FROM acquisition_runs WHERE project_id=$1")
        .bind(project_id)
        .fetch_one(&pool)
        .await
        .expect("run count should load");
    assert_eq!(runs, 0);

    let created = call(
        &pool,
        create_acquisition_request(
            project_id,
            Some("provider-retry"),
            json!({"seed_dois": ["10.5555/provider-validation"], "metadata_providers": ["crossref", "crossref"]}),
        ),
    )
    .await;
    assert_eq!(created.status(), StatusCode::CREATED);
    let run_id = json_body(created).await["id"]
        .as_str()
        .expect("created acquisition should have an id")
        .parse::<Uuid>()
        .expect("acquisition id should be a UUID");
    let config: serde_json::Value =
        sqlx::query_scalar("SELECT config FROM acquisition_runs WHERE id=$1")
            .bind(run_id)
            .fetch_one(&pool)
            .await
            .expect("created config should load");
    assert_eq!(config["metadata_providers"], json!(["crossref"]));
    assert!(config.get("metadata_provider").is_none());

    // A retry that omits the chain resolves to the same enabled chain, so it replays the run.
    let replay = call(
        &pool,
        create_acquisition_request(
            project_id,
            Some("provider-retry"),
            json!({"seed_dois": ["10.5555/provider-validation"]}),
        ),
    )
    .await;
    assert_eq!(replay.status(), StatusCode::OK);
    assert_eq!(json_body(replay).await["id"], run_id.to_string());

    clean_acquisition_projects(&pool, &[project_id], &[run_id]).await;
}

#[tokio::test]
async fn the_deployment_allowlist_filters_the_chain_and_an_empty_one_is_unavailable() {
    let Some(pool) = database().await else {
        return;
    };
    let project_id = Uuid::new_v4();
    sqlx::query("INSERT INTO projects (id,name) VALUES ($1,'allowlist test')")
        .bind(project_id)
        .execute(&pool)
        .await
        .expect("test project should insert");

    // OpenAlex is configured but has no adapter in this build, so only Crossref is enabled.
    let configured_chain = deepref_config::IntegrationsConfig {
        contact_email: TEST_CONTACT_EMAIL.to_owned(),
        metadata_providers: vec![ProviderKind::OpenAlex, ProviderKind::Crossref],
        ..Default::default()
    };
    let omitted = call_with_integrations(
        &pool,
        create_acquisition_request(
            project_id,
            None,
            json!({"seed_dois": ["10.5555/allowlist"]}),
        ),
        configured_chain.clone(),
    )
    .await;
    assert_eq!(omitted.status(), StatusCode::CREATED);
    let omitted_id = json_body(omitted).await["id"]
        .as_str()
        .expect("created acquisition should have an id")
        .parse::<Uuid>()
        .expect("acquisition id should be a UUID");
    let config: serde_json::Value =
        sqlx::query_scalar("SELECT config FROM acquisition_runs WHERE id=$1")
            .bind(omitted_id)
            .fetch_one(&pool)
            .await
            .expect("created config should load");
    assert_eq!(config["metadata_providers"], json!(["crossref"]));

    let named = call_with_integrations(
        &pool,
        create_acquisition_request(
            project_id,
            None,
            json!({"seed_dois": ["10.5555/allowlist-named"], "metadata_providers": ["openalex"]}),
        ),
        configured_chain,
    )
    .await;
    assert_eq!(named.status(), StatusCode::BAD_REQUEST);
    assert_eq!(
        json_body(named).await["message"],
        "openalex is not enabled in this deployment (enabled: crossref)"
    );

    let no_adapter = deepref_config::IntegrationsConfig {
        contact_email: TEST_CONTACT_EMAIL.to_owned(),
        metadata_providers: vec![ProviderKind::DataCite],
        ..Default::default()
    };
    let unavailable = call_with_integrations(
        &pool,
        create_acquisition_request(
            project_id,
            None,
            json!({"seed_dois": ["10.5555/unavailable"]}),
        ),
        no_adapter,
    )
    .await;
    assert_eq!(unavailable.status(), StatusCode::SERVICE_UNAVAILABLE);

    clean_acquisition_projects(&pool, &[project_id], &[omitted_id]).await;
}

#[tokio::test]
async fn provider_refresh_rejects_invalid_sources_and_enforces_project_scope() {
    let Some(pool) = database().await else {
        return;
    };
    let project_id = Uuid::new_v4();
    let other_project_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO projects (id,name) VALUES ($1,'refresh validation test'),($2,'other project')",
    )
    .bind(project_id)
    .bind(other_project_id)
    .execute(&pool)
    .await
    .expect("test projects should insert");

    let valid_config = json!({
        "seed_dois": ["10.5555/valid-refresh"],
        "max_depth": 2,
        "metadata_provider": "crossref",
        "citation_provider": "crossref",
    });
    let queued_id = insert_completed_acquisition(
        &pool,
        project_id,
        "crossref",
        "citation_traversal",
        None,
        valid_config.clone(),
    )
    .await;
    sqlx::query(
        "UPDATE acquisition_runs SET status='queued',started_at=NULL,completed_at=NULL WHERE id=$1",
    )
    .bind(queued_id)
    .execute(&pool)
    .await
    .expect("queued source should update");
    let queued = call(&pool, refresh_request(project_id, queued_id, "queued-key")).await;
    assert_eq!(queued.status(), StatusCode::CONFLICT);
    assert_eq!(json_body(queued).await["code"], "ACQUISITION_NOT_COMPLETED");

    let import_id = insert_completed_acquisition(
        &pool,
        project_id,
        "import:ris",
        "file_import",
        Some("ris"),
        json!({"format": "ris"}),
    )
    .await;
    let import = call(&pool, refresh_request(project_id, import_id, "import-key")).await;
    assert_eq!(import.status(), StatusCode::CONFLICT);
    assert_eq!(
        json_body(import).await["code"],
        "ACQUISITION_REFRESH_UNSUPPORTED"
    );

    let malformed_id = insert_completed_acquisition(
        &pool,
        project_id,
        "crossref",
        "citation_traversal",
        None,
        json!({"seed_dois": ["10.5555/malformed"]}),
    )
    .await;
    let malformed = call(
        &pool,
        refresh_request(project_id, malformed_id, "malformed-key"),
    )
    .await;
    assert_eq!(malformed.status(), StatusCode::CONFLICT);
    assert_eq!(
        json_body(malformed).await["code"],
        "ACQUISITION_REFRESH_CONFIG_INVALID"
    );

    let unsupported_id = insert_completed_acquisition(
        &pool,
        project_id,
        "crossref",
        "manual_import",
        None,
        valid_config.clone(),
    )
    .await;
    let unsupported = call(
        &pool,
        refresh_request(project_id, unsupported_id, "unsupported-key"),
    )
    .await;
    assert_eq!(unsupported.status(), StatusCode::CONFLICT);
    assert_eq!(
        json_body(unsupported).await["code"],
        "ACQUISITION_REFRESH_UNSUPPORTED"
    );

    let missing_key = call(
        &pool,
        Request::builder()
            .method("POST")
            .uri(format!(
                "/projects/{project_id}/acquisitions/{queued_id}/refresh"
            ))
            .body(Body::empty())
            .expect("missing-key request should be valid"),
    )
    .await;
    assert_eq!(missing_key.status(), StatusCode::BAD_REQUEST);

    let cross_project_source = insert_completed_acquisition(
        &pool,
        other_project_id,
        "crossref",
        "citation_traversal",
        None,
        valid_config,
    )
    .await;
    let cross_project = call(
        &pool,
        refresh_request(project_id, cross_project_source, "cross-project-key"),
    )
    .await;
    assert_eq!(cross_project.status(), StatusCode::NOT_FOUND);

    let missing_source = call(
        &pool,
        refresh_request(project_id, Uuid::new_v4(), "missing-source-key"),
    )
    .await;
    assert_eq!(missing_source.status(), StatusCode::NOT_FOUND);
    let missing_project = call(
        &pool,
        refresh_request(Uuid::new_v4(), queued_id, "missing-project-key"),
    )
    .await;
    assert_eq!(missing_project.status(), StatusCode::NOT_FOUND);

    clean_acquisition_projects(&pool, &[project_id, other_project_id], &[]).await;
}

#[tokio::test]
async fn acquisition_cancel_requires_an_existing_active_run() {
    let Some(pool) = database().await else { return };
    let project = Uuid::new_v4();
    sqlx::query("INSERT INTO projects (id,name) VALUES ($1,'cancel test')")
        .bind(project)
        .execute(&pool)
        .await
        .unwrap();
    let cancel = |id| {
        Request::builder()
            .method("POST")
            .uri(format!("/projects/{project}/acquisitions/{id}/cancel"))
            .body(Body::empty())
            .unwrap()
    };
    assert_eq!(
        call(&pool, cancel(Uuid::new_v4())).await.status(),
        StatusCode::NOT_FOUND
    );
    for status in ["queued", "running", "completed", "failed", "cancelled"] {
        let id = Uuid::new_v4();
        sqlx::query("INSERT INTO acquisition_runs (id,project_id,source,strategy,status,max_depth,seed_count,created_at) VALUES ($1,$2,'crossref','citation_traversal',$3,1,1,now())")
            .bind(id).bind(project).bind(status).execute(&pool).await.unwrap();
        let response = call(&pool, cancel(id)).await;
        let active = status == "queued" || status == "running";
        assert_eq!(
            response.status(),
            if active {
                StatusCode::ACCEPTED
            } else {
                StatusCode::CONFLICT
            }
        );
        let stored: String = sqlx::query_scalar("SELECT status FROM acquisition_runs WHERE id=$1")
            .bind(id)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(stored, if active { "cancelled" } else { status });
        assert_eq!(call(&pool, cancel(id)).await.status(), StatusCode::CONFLICT);
    }
}

#[tokio::test]
async fn citation_depth_ten_is_rejected_at_every_api_boundary_and_in_database() {
    let Some(pool) = database().await else { return };
    let project = Uuid::new_v4();
    sqlx::query("INSERT INTO projects (id,name) VALUES ($1,'depth test')")
        .bind(project)
        .execute(&pool)
        .await
        .unwrap();
    for (method, uri, body) in [
        (
            "POST",
            "/projects".to_owned(),
            json!({"name":"too deep","default_max_depth":10}),
        ),
        (
            "PATCH",
            format!("/projects/{project}"),
            json!({"name":"too deep","default_max_depth":10}),
        ),
        (
            "PATCH",
            "/settings".to_owned(),
            json!({"default_max_depth":10}),
        ),
        (
            "POST",
            format!("/projects/{project}/acquisitions"),
            json!({"seed_dois":["10.1000/depth"],"max_depth":10}),
        ),
    ] {
        let response = call(
            &pool,
            Request::builder()
                .method(method)
                .uri(uri)
                .header("content-type", "application/json")
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }
    let error = sqlx::query("UPDATE projects SET default_max_depth=10 WHERE id=$1")
        .bind(project)
        .execute(&pool)
        .await
        .unwrap_err();
    assert_eq!(
        error
            .as_database_error()
            .and_then(|error| error.code())
            .as_deref(),
        Some("23514")
    );
}

#[tokio::test]
async fn hooks_resolve_tokens_before_limiting_and_reject_unsigned_new_webhooks() {
    use deepref_domain::{Actor, ActorKind, ProjectId};
    let Some(pool) = database().await else { return };
    let unknown = Uuid::new_v4().to_string();
    for _ in 0..65 {
        let response = call_with_workflow_policy(
            &pool,
            Request::builder()
                .method("POST")
                .uri(format!("/hooks/workflows/{unknown}"))
                .body(Body::from("{}"))
                .unwrap(),
            true,
        )
        .await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }
    let project = Uuid::new_v4();
    sqlx::query("INSERT INTO projects (id,name) VALUES ($1,'signed hook')")
        .bind(project)
        .execute(&pool)
        .await
        .unwrap();
    let project = ProjectId::new(project);
    let actor = Actor::new(ActorKind::User, "hook-test").unwrap();
    let graph = serde_json::from_value(
        json!({"nodes":[{"id":"trigger","type":"trigger.webhook","config":{}}],"edges":[]}),
    )
    .unwrap();
    let workflow = deepref_app::create_workflow(&pool, project, "Signed", "", graph, &actor, true)
        .await
        .unwrap();
    let endpoint = deepref_app::get_webhook_config(&pool, project, workflow.id)
        .await
        .unwrap();
    let response = call_with_workflow_policy(
        &pool,
        Request::builder()
            .method("POST")
            .uri(format!("/hooks/workflows/{}", endpoint.token))
            .body(Body::from("{}"))
            .unwrap(),
        true,
    )
    .await;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    deepref_app::publish_workflow(&pool, project, workflow.id, None, &actor, false, true)
        .await
        .expect("publishes");
    deepref_app::set_workflow_enabled(&pool, project, workflow.id, true, true)
        .await
        .expect("enabled");
    let signature = deepref_application::workflows::sign_payload(&endpoint.secret, b"{}");
    let signed = || {
        Request::builder()
            .method("POST")
            .uri(format!("/hooks/workflows/{}", endpoint.token))
            .header("X-DeepRef-Signature", &signature)
            .header("Idempotency-Key", "signed-positive")
            .body(Body::from("{}"))
            .unwrap()
    };
    let accepted = call_with_workflow_policy(&pool, signed(), true).await;
    assert_eq!(accepted.status(), StatusCode::ACCEPTED);
    let first = json_body(accepted).await;
    let repeated = call_with_workflow_policy(&pool, signed(), true).await;
    assert_eq!(repeated.status(), StatusCode::ACCEPTED);
    assert_eq!(json_body(repeated).await["run_id"], first["run_id"]);
    let denied = call_with_workflow_policy(&pool, signed(), false).await;
    assert_eq!(denied.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn niche_flag_preserves_catalog_and_saved_graphs_but_denies_authoring_activation_and_ingress()
{
    use deepref_domain::{Actor, ActorKind, ProjectId};
    let Some(pool) = database().await else { return };
    let project = Uuid::new_v4();
    sqlx::query("INSERT INTO projects(id,name) VALUES($1,'parked graph')")
        .bind(project)
        .execute(&pool)
        .await
        .unwrap();
    let graph = json!({"nodes":[{"id":"trigger","type":"trigger.webhook","position":{"x":0,"y":0},"config":{}}],"edges":[]});
    let create = || {
        Request::builder()
            .method("POST")
            .uri(format!("/projects/{project}/automations/workflows"))
            .header("Content-Type", "application/json")
            .body(Body::from(
                json!({"name":"Parked","graph":graph}).to_string(),
            ))
            .unwrap()
    };
    let denied = call_with_workflow_policy(&pool, create(), false).await;
    assert_eq!(denied.status(), StatusCode::FORBIDDEN);
    let enabled = call_with_workflow_policy(&pool, create(), true).await;
    assert_eq!(enabled.status(), StatusCode::CREATED);
    let created = json_body(enabled).await;
    let workflow = Uuid::parse_str(created["id"].as_str().unwrap()).unwrap();
    let catalog = call_with_workflow_policy(
        &pool,
        Request::builder()
            .uri("/automations/catalog")
            .body(Body::empty())
            .unwrap(),
        false,
    )
    .await;
    let catalog = json_body(catalog).await;
    let definitions = catalog.as_array().unwrap();
    for id in [
        "trigger.webhook",
        "trigger.email",
        "integration.slack",
        "integration.http_request",
    ] {
        let definition = definitions
            .iter()
            .find(|node| node["id"] == id)
            .expect("historical definition");
        assert_eq!(definition["available"], false);
    }
    assert_eq!(
        definitions
            .iter()
            .find(|node| node["id"] == "integration.email")
            .unwrap()["available"],
        true
    );
    let saved = call_with_workflow_policy(
        &pool,
        Request::builder()
            .uri(format!(
                "/projects/{project}/automations/workflows/{workflow}"
            ))
            .body(Body::empty())
            .unwrap(),
        false,
    )
    .await;
    assert_eq!(saved.status(), StatusCode::OK);
    assert_eq!(
        json_body(saved).await["graph"]["nodes"][0]["type"],
        "trigger.webhook"
    );
    for (method, suffix, body) in [
        ("PUT", "", "{}"),
        ("POST", "/publish", "{}"),
        ("POST", "/test-runs", "{}"),
    ] {
        let response = call_with_workflow_policy(
            &pool,
            Request::builder()
                .method(method)
                .uri(format!(
                    "/projects/{project}/automations/workflows/{workflow}{suffix}"
                ))
                .header("Content-Type", "application/json")
                .body(Body::from(body))
                .unwrap(),
            false,
        )
        .await;
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }
    let actor = Actor::new(ActorKind::User, "historical").unwrap();
    deepref_app::publish_workflow(
        &pool,
        ProjectId::new(project),
        workflow,
        None,
        &actor,
        false,
        true,
    )
    .await
    .unwrap();
    for suffix in ["/enable", "/runs"] {
        let denied = call_with_workflow_policy(
            &pool,
            Request::builder()
                .method("POST")
                .uri(format!(
                    "/projects/{project}/automations/workflows/{workflow}{suffix}"
                ))
                .header("Content-Type", "application/json")
                .body(Body::from("{}"))
                .unwrap(),
            false,
        )
        .await;
        assert_eq!(denied.status(), StatusCode::FORBIDDEN);
    }
    let endpoint = deepref_app::get_webhook_config(&pool, ProjectId::new(project), workflow)
        .await
        .unwrap();
    for path in [
        format!("/hooks/workflows/{}", endpoint.token),
        format!("/hooks/email/{}", endpoint.email_token),
    ] {
        let denied = call_with_workflow_policy(
            &pool,
            Request::builder()
                .method("POST")
                .uri(path)
                .header("Content-Type", "application/json")
                .body(Body::from("{}"))
                .unwrap(),
            false,
        )
        .await;
        assert_eq!(denied.status(), StatusCode::FORBIDDEN);
    }
    let count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM workflow_run_details WHERE workflow_id=$1")
            .bind(workflow)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(count, 0, "denial creates no durable run");
}
