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
use serde_json::json;
use sqlx::{PgPool, Row, postgres::PgPoolOptions};
use tower::ServiceExt;
use uuid::Uuid;

async fn database() -> Option<PgPool> {
    let url = std::env::var("TEST_DATABASE_URL")
        .or_else(|_| std::env::var("DATABASE_URL"))
        .ok()?;
    let pool = PgPoolOptions::new()
        .max_connections(5)
        .connect(&url)
        .await
        .expect("DATABASE_URL is set but PostgreSQL is unavailable");
    deepref_postgres::migrate(&pool)
        .await
        .expect("all migrations should apply");
    Some(pool)
}

fn api_config() -> ApiConfig {
    let runtime = deepref_config::RuntimeConfig::from_map(
        "deepref-api-acquisition-test",
        &HashMap::from([("APP_ENV".to_owned(), "local".to_owned())]),
    )
    .expect("local test runtime should parse");
    ApiConfig {
        runtime,
        bind_addr: "127.0.0.1:0".parse().unwrap(),
        cors_allow_any: false,
        cors_origins: vec!["http://localhost:3000".parse().unwrap()],
    }
}

async fn call(pool: &PgPool, request: Request<Body>) -> Response<Body> {
    router(AppState::core(pool.clone()), &api_config())
        .oneshot(request)
        .await
        .expect("request should be handled")
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
    sqlx::query_scalar("SELECT count(*) FROM ingestion_items WHERE ingestion_id=$1")
        .bind(acquisition_id)
        .fetch_one(pool)
        .await
        .expect("ingestion item count should load")
}

async fn count_run_jobs(pool: &PgPool, acquisition_id: Uuid) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM jobs WHERE payload->'payload'->>'ingestion_id'=$1")
        .bind(acquisition_id.to_string())
        .fetch_one(pool)
        .await
        .expect("job count should load")
}

async fn clean_acquisition_projects(pool: &PgPool, project_ids: &[Uuid], run_ids: &[Uuid]) {
    for run_id in run_ids {
        sqlx::query("DELETE FROM jobs WHERE payload->'payload'->>'ingestion_id'=$1")
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
        "SELECT count(*) FROM jobs WHERE kind='work_fetch_requested' AND payload->'payload'->>'ingestion_id'=$1",
    )
    .bind(run_id.to_string())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(run_exists);
    assert_eq!(jobs, 1);
    let items: i64 =
        sqlx::query_scalar("SELECT count(*) FROM ingestion_items WHERE ingestion_id=$1")
            .bind(run_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(items, 1);
    let legacy_link: Uuid =
        sqlx::query_scalar("SELECT legacy_ingestion_id FROM acquisition_runs WHERE id=$1")
            .bind(run_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(legacy_link, run_id);
    sqlx::query("DELETE FROM jobs WHERE payload->'payload'->>'ingestion_id'=$1")
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
        sqlx::query_scalar("SELECT count(*) FROM ingestions WHERE project_id=$1")
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
    let source_event_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO records (id,project_id,acquisition_run_id,source,source_key,title,raw)
         VALUES ($1,$2,$3,'crossref',$4,'Source record','{}')",
    )
    .bind(source_record_id)
    .bind(project_id)
    .bind(source_id)
    .bind(format!("{source_id}:source"))
    .execute(&pool)
    .await
    .expect("source record should insert");
    sqlx::query(
        "INSERT INTO record_provenance
         (record_id,acquisition_run_id,canonical_doi,depth,parent_doi,status,attempts,queued_at,fetched_at,work_event_id)
         VALUES ($1,$2,'10.5555/refresh-source',0,NULL,'fetched',1,now(),now(),$3)",
    )
    .bind(source_record_id)
    .bind(source_id)
    .bind(source_event_id)
    .execute(&pool)
    .await
    .expect("source provenance should insert");

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
        refresh_config["metadata_provider"],
        source_config_before["metadata_provider"]
    );
    assert_eq!(
        refresh_config["citation_provider"],
        source_config_before["citation_provider"]
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
            "SELECT count(*) FROM record_provenance WHERE acquisition_run_id=$1",
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
