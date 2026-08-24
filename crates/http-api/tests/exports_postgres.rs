use std::collections::HashMap;

use axum::{
    body::{Body, to_bytes},
    http::{Request, Response, StatusCode, header},
};
use deepref_http_api::{config::ApiConfig, routes::router, state::AppState};
use serde_json::Value;
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
        "deepref-api-exports-test",
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

async fn request(pool: &PgPool, uri: String) -> Response<Body> {
    router(AppState::core(pool.clone()), &api_config())
        .oneshot(
            Request::builder()
                .uri(uri)
                .body(Body::empty())
                .expect("export request should be valid"),
        )
        .await
        .expect("export request should be handled")
}

async fn response_bytes(response: Response<Body>) -> Vec<u8> {
    to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("export body should be readable")
        .to_vec()
}

#[tokio::test]
async fn postgres_exports_return_every_deterministic_attachment_and_boundary_statuses() {
    let Some(pool) = database().await else {
        return;
    };

    let project_id = Uuid::new_v4();
    let report_id = Uuid::new_v4();
    let report_identifier_id = Uuid::new_v4();
    let record_id = Uuid::new_v4();
    let acquisition_run_id = Uuid::new_v4();
    let provenance_event_id = Uuid::new_v4();
    let protocol_id = Uuid::new_v4();
    let screening_event_id = Uuid::new_v4();
    let dedupe_event_id = Uuid::new_v4();
    let doi = format!("10.5555/deepref-export-{}", Uuid::new_v4());

    sqlx::query("INSERT INTO projects (id,name) VALUES ($1,'export integration project')")
        .bind(project_id)
        .execute(&pool)
        .await
        .expect("project should insert");
    sqlx::query(
        "INSERT INTO reports
         (id,title,abstract_text,publication_year,journal,container_title,publisher,url,work_type,authors,raw,total_citations,references_count)
         VALUES ($1,'Export & report','An export fixture',2024,'Journal of Tests','Container of Tests','DeepRef Press','https://example.test/export','article',$2,$3,7,3)",
    )
    .bind(report_id)
    .bind(serde_json::json!([{"given":"Ada","family":"Lovelace"}]))
    .bind(serde_json::json!({"source":"http-test","citation_count":7}))
    .execute(&pool)
    .await
    .expect("report should insert");
    sqlx::query(
        "INSERT INTO report_identifiers (id,report_id,scheme,value,normalized_value)
         VALUES ($1,$2,'doi',$3,$3)",
    )
    .bind(report_identifier_id)
    .bind(report_id)
    .bind(&doi)
    .execute(&pool)
    .await
    .expect("report identifier should insert");
    sqlx::query(
        "INSERT INTO acquisition_runs
         (id,project_id,legacy_ingestion_id,source,status,max_depth,seed_count,queued_count,fetched_count,failed_count,metadata_provider,citation_provider,created_at)
         VALUES ($1,$2,NULL,'export-fixture','completed',1,1,1,1,0,'fixture-metadata','fixture-citations',now())",
    )
    .bind(acquisition_run_id)
    .bind(project_id)
    .execute(&pool)
    .await
    .expect("acquisition run should insert");
    sqlx::query(
        "INSERT INTO records
         (id,project_id,report_id,source,source_key,title,publication_year,raw,acquisition_run_id)
         VALUES ($1,$2,$3,'fixture','export-record','Export & report',2024,$4,$5)",
    )
    .bind(record_id)
    .bind(project_id)
    .bind(report_id)
    .bind(serde_json::json!({"source":"fixture"}))
    .bind(acquisition_run_id)
    .execute(&pool)
    .await
    .expect("source record should insert");
    sqlx::query(
        "INSERT INTO record_provenance
         (record_id,acquisition_run_id,canonical_doi,depth,parent_doi,status,attempts,queued_at,fetched_at,work_event_id)
         VALUES ($1,$2,$3,0,NULL,'fetched',1,now(),now(),$4)",
    )
    .bind(record_id)
    .bind(acquisition_run_id)
    .bind(&doi)
    .bind(provenance_event_id)
    .execute(&pool)
    .await
    .expect("record provenance should insert");
    sqlx::query(
        "INSERT INTO project_reports (project_id,report_id,first_seen_record_id,lifecycle_status)
         VALUES ($1,$2,$3,'screening')",
    )
    .bind(project_id)
    .bind(report_id)
    .bind(record_id)
    .execute(&pool)
    .await
    .expect("project report should insert");
    sqlx::query(
        "INSERT INTO protocol_versions
         (id,project_id,version,name,status,criteria,framework_kind,framework_fields,objective,question,revision,published_at,created_by_kind,created_by_id,updated_by_kind,updated_by_id,published_by_kind,published_by_id)
         VALUES ($1,$2,1,'Export protocol','published','[]'::jsonb,'custom','{}'::jsonb,'Export objective','Export question',1,now(),'user','export-test','user','export-test','user','export-test')",
    )
    .bind(protocol_id)
    .bind(project_id)
    .execute(&pool)
    .await
    .expect("published protocol should insert");
    sqlx::query(
        "INSERT INTO screening_events
         (id,project_id,report_id,stage,decision,notes,protocol_version_id,actor_kind,actor_id,event_kind,
          previous_title_abstract_status,previous_full_text_status,previous_final_status,
          result_title_abstract_status,result_full_text_status,result_final_status)
         VALUES ($1,$2,$3,'title_abstract','include','accepted by export fixture',$4,'user','export-test','decision',
                 'unscreened','not_required','unscreened','include','not_required','pending_full_text')",
    )
    .bind(screening_event_id)
    .bind(project_id)
    .bind(report_id)
    .bind(protocol_id)
    .execute(&pool)
    .await
    .expect("screening event should insert");
    sqlx::query(
        "INSERT INTO screening_state
         (project_id,report_id,title_abstract_status,full_text_status,final_status,revision,last_event_id)
         VALUES ($1,$2,'include','not_required','pending_full_text',1,$3)",
    )
    .bind(project_id)
    .bind(report_id)
    .bind(screening_event_id)
    .execute(&pool)
    .await
    .expect("screening state should insert");
    sqlx::query(
        "INSERT INTO dedupe_resolution_events
         (id,project_id,record_id,prior_report_id,resolved_report_id,action,reason,actor_kind,actor_id)
         VALUES ($1,$2,$3,$4,$4,'link','canonical export fixture','user','export-test')",
    )
    .bind(dedupe_event_id)
    .bind(project_id)
    .bind(record_id)
    .bind(report_id)
    .execute(&pool)
    .await
    .expect("dedupe audit event should insert");

    let artifacts = [
        ("reports.csv", "text/csv", "reports.csv"),
        ("reports.json", "application/json", "reports.json"),
        (
            "reports.ris",
            "application/x-research-info-systems",
            "reports.ris",
        ),
        ("reports.bib", "application/x-bibtex", "reports.bib"),
        ("prisma.json", "application/json", "prisma.json"),
        ("prisma.svg", "image/svg+xml", "prisma.svg"),
        ("audit.csv", "text/csv", "audit.csv"),
        ("protocol.json", "application/json", "protocol.json"),
    ];
    for (kind, content_type, filename) in artifacts {
        let response = request(&pool, format!("/projects/{project_id}/exports/{kind}")).await;
        assert_eq!(response.status(), StatusCode::OK, "{kind} status");
        assert_eq!(
            response
                .headers()
                .get(header::CONTENT_TYPE)
                .and_then(|value| value.to_str().ok())
                .map(|value| value.split(';').next().unwrap_or(value)),
            Some(content_type),
            "{kind} content type"
        );
        assert_eq!(
            response
                .headers()
                .get(header::CONTENT_DISPOSITION)
                .and_then(|value| value.to_str().ok()),
            Some(format!("attachment; filename=\"deepref-{project_id}-{filename}\"").as_str()),
            "{kind} disposition"
        );
        let bytes = response_bytes(response).await;
        match kind {
            "reports.csv" => {
                let body = String::from_utf8(bytes).expect("CSV should be UTF-8");
                assert!(body.starts_with("report_id,doi,title"));
                assert!(body.contains(&report_id.to_string()));
                assert!(body.contains("Export & report"));
            }
            "reports.json" => {
                let body: Value = serde_json::from_slice(&bytes).expect("reports JSON");
                assert_eq!(body.as_array().expect("reports array").len(), 1);
                assert_eq!(body[0]["report_id"], report_id.to_string());
                assert_eq!(body[0]["screening_status"], "pending_full_text");
            }
            "reports.ris" => {
                let body = String::from_utf8(bytes).expect("RIS should be UTF-8");
                assert!(body.contains("TY  - JOUR"));
                assert!(body.contains("TI  - Export & report"));
                assert!(body.contains(&format!("DO  - {doi}")));
            }
            "reports.bib" => {
                let body = String::from_utf8(bytes).expect("BibTeX should be UTF-8");
                assert!(body.starts_with("@article{"));
                assert!(body.contains("journal = {Journal of Tests}"));
                assert!(body.contains("title = {Export \\& report}"));
            }
            "prisma.json" => {
                let body: Value = serde_json::from_slice(&bytes).expect("PRISMA JSON");
                assert_eq!(body["identified_records"], 1);
                assert_eq!(body["reports_sought"], 1);
                assert_eq!(body["reports_not_retrieved"], 1);
            }
            "prisma.svg" => {
                let body = String::from_utf8(bytes).expect("SVG should be UTF-8");
                assert!(body.starts_with("<svg"));
                assert!(body.contains("Reports sought"));
            }
            "audit.csv" => {
                let body = String::from_utf8(bytes).expect("audit CSV should be UTF-8");
                assert!(body.starts_with("id,created_at,event_type"));
                assert!(body.contains("dedupe_resolution"));
                assert!(body.contains("canonical export fixture"));
                assert!(body.contains(&dedupe_event_id.to_string()));
            }
            "protocol.json" => {
                let body: Value = serde_json::from_slice(&bytes).expect("protocol JSON");
                assert_eq!(body["name"], "Export protocol");
                assert_eq!(body["status"], "published");
                assert_eq!(body["project_id"], project_id.to_string());
            }
            _ => unreachable!(),
        }
    }

    let unknown = request(&pool, format!("/projects/{project_id}/exports/unknown")).await;
    assert_eq!(unknown.status(), StatusCode::BAD_REQUEST);
    let missing_project = Uuid::new_v4();
    let missing = request(
        &pool,
        format!("/projects/{missing_project}/exports/reports.csv"),
    )
    .await;
    assert_eq!(missing.status(), StatusCode::NOT_FOUND);

    sqlx::query("DELETE FROM projects WHERE id=$1")
        .bind(project_id)
        .execute(&pool)
        .await
        .expect("test project should clean up");
    sqlx::query("DELETE FROM reports WHERE id=$1")
        .bind(report_id)
        .execute(&pool)
        .await
        .expect("test report should clean up");
}
