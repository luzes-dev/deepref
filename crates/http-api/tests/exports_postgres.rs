use std::collections::{HashMap, HashSet};

use axum::{
    body::{Body, to_bytes},
    http::{Request, Response, StatusCode, header},
};
use chrono::{DateTime, Timelike, Utc};
use deepref_http_api::{config::ApiConfig, routes::router, state::AppState};
use serde_json::Value;
use sqlx::{PgPool, postgres::PgPoolOptions};
use std::sync::OnceLock;
use tower::ServiceExt;
use uuid::Uuid;

static DATABASE_TEST_MUTEX: OnceLock<tokio::sync::Mutex<()>> = OnceLock::new();

fn test_lock() -> &'static tokio::sync::Mutex<()> {
    DATABASE_TEST_MUTEX.get_or_init(tokio::sync::Mutex::default)
}

async fn database() -> Option<PgPool> {
    let url = match std::env::var("TEST_DATABASE_URL") {
        Ok(url) => url,
        Err(_) => return None,
    };
    let pool = PgPoolOptions::new()
        .max_connections(5)
        .connect(&url)
        .await
        .unwrap_or_else(|error| {
            panic!("TEST_DATABASE_URL is set but PostgreSQL is unavailable: {error}")
        });
    deepref_postgres::migrate(&pool)
        .await
        .unwrap_or_else(|error| panic!("TEST_DATABASE_URL migrations failed: {error}"));
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
    let _guard = test_lock().lock().await;
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
    let appraisal_assessment_id = Uuid::new_v4();
    let dedupe_event_id = Uuid::new_v4();
    let ai_run_id = Uuid::new_v4();
    let failed_ai_run_id = Uuid::new_v4();
    let ai_proposal_id = Uuid::new_v4();
    let automation_definition_id = Uuid::new_v4();
    let automation_job_id = Uuid::new_v4();
    let automation_run_id = Uuid::new_v4();
    let automation_step_id = Uuid::new_v4();
    let review_attempt_id = Uuid::new_v4();
    let review_artifact_id = Uuid::new_v4();
    let review_predecessor_artifact_id = Uuid::new_v4();
    let calibration_bundle_id = Uuid::new_v4();
    let other_project_id = Uuid::new_v4();
    let other_ai_run_id = Uuid::new_v4();
    let other_ai_proposal_id = Uuid::new_v4();
    let other_automation_definition_id = Uuid::new_v4();
    let other_automation_job_id = Uuid::new_v4();
    let other_automation_run_id = Uuid::new_v4();
    let other_automation_step_id = Uuid::new_v4();
    let doi = format!("10.5555/deepref-export-{}", Uuid::new_v4());
    let now = Utc::now();
    let tied_audit_created_at = now
        .with_nanosecond((now.nanosecond() / 1_000) * 1_000)
        .expect("truncated timestamp should remain valid");

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
         (id,project_id,report_id,stage,decision,notes,protocol_version_id,actor_kind,actor_id,event_kind,created_at,
          previous_title_abstract_status,previous_full_text_status,previous_final_status,
          result_title_abstract_status,result_full_text_status,result_final_status)
         VALUES ($1,$2,$3,'title_abstract','include','accepted by export fixture',$4,'user','export-test','decision',
                 $5,'unscreened','not_required','unscreened','include','not_required','pending_full_text')",
    )
    .bind(screening_event_id)
    .bind(project_id)
    .bind(report_id)
    .bind(protocol_id)
    .bind(tied_audit_created_at)
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
        "INSERT INTO appraisal_assessments
         (id,project_id,report_id,definition_id,definition_version,responses,judgments,actor_kind,actor_id)
         VALUES ($1,$2,$3,'export-fixture',1,'{}'::jsonb,'{}'::jsonb,'user','export-test')",
    )
    .bind(appraisal_assessment_id)
    .bind(project_id)
    .bind(report_id)
    .execute(&pool)
    .await
    .expect("appraisal assessment should insert");
    sqlx::query(
        "INSERT INTO appraisal_events
         (id,assessment_id,project_id,report_id,event_type,payload,actor_kind,actor_id,created_at)
         VALUES ($1,$2,$3,$4,'appraisal_completed','{}'::jsonb,'user','export-test',$5)",
    )
    .bind(screening_event_id)
    .bind(appraisal_assessment_id)
    .bind(project_id)
    .bind(report_id)
    .bind(tied_audit_created_at)
    .execute(&pool)
    .await
    .expect("appraisal event should insert");
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

    sqlx::query("INSERT INTO projects (id,name) VALUES ($1,'other export project')")
        .bind(other_project_id)
        .execute(&pool)
        .await
        .expect("other project should insert");
    sqlx::query(
        "INSERT INTO automation_definitions
         (id,project_id,name,trigger_kind,recipe_id,recipe_version,status,actor_kind,actor_id,created_at,updated_at)
         VALUES ($1,$2,'Export automation','manual','project_maintenance',1,'active','user','export-automation-initiator',now() - interval '6 minutes',now() - interval '6 minutes')",
    )
    .bind(automation_definition_id)
    .bind(project_id)
    .execute(&pool)
    .await
    .expect("automation definition should insert");
    sqlx::query(
        "INSERT INTO automation_definition_steps
         (project_id,definition_id,ordinal,step_key,step_kind)
         VALUES ($1,$2,0,'screening-assistance','ai_task')",
    )
    .bind(project_id)
    .bind(automation_definition_id)
    .execute(&pool)
    .await
    .expect("automation definition step should insert");
    sqlx::query(
        "INSERT INTO jobs
         (id,project_id,kind,payload,state,attempts,max_attempts,available_at,dedupe_key,created_at,completed_at)
         VALUES ($1,$2,'automation_run',$3,'completed',2,4,now(),$4,now() - interval '5 minutes',now() - interval '4 minutes')",
    )
    .bind(automation_job_id)
    .bind(project_id)
    .bind(serde_json::json!({"secret_job_payload":"SENSITIVE_JOB_PAYLOAD"}))
    .bind(format!("export-automation-job-{project_id}"))
    .execute(&pool)
    .await
    .expect("automation job should insert");
    sqlx::query(
        "INSERT INTO automation_runs
         (id,project_id,definition_id,job_id,recipe_id,recipe_version,trigger_kind,trigger_reference,
          idempotency_key,actor_kind,actor_id,status,created_at,started_at,finished_at)
         VALUES ($1,$2,$3,$4,'project_maintenance',1,'manual','manual-export-reference',
                 'export-automation-run','user','export-automation-initiator','completed',
                 now() - interval '4 minutes',now() - interval '4 minutes',now() - interval '3 minutes')",
    )
    .bind(automation_run_id)
    .bind(project_id)
    .bind(automation_definition_id)
    .bind(automation_job_id)
    .execute(&pool)
    .await
    .expect("automation run should insert");
    sqlx::query(
        "INSERT INTO automation_step_runs
         (id,project_id,automation_run_id,ordinal,step_key,step_kind,status,attempts,started_at,finished_at,output)
         VALUES ($1,$2,$3,0,'screening-assistance','ai_task','completed',2,
                 now() - interval '4 minutes',now() - interval '3 minutes',
                 $4)",
    )
    .bind(automation_step_id)
    .bind(project_id)
    .bind(automation_run_id)
    .bind(serde_json::json!({"raw_model_output":"SENSITIVE_STEP_OUTPUT"}))
    .execute(&pool)
    .await
    .expect("automation step run should insert");
    sqlx::query(
        r#"INSERT INTO ai_runs
         (id,project_id,task_kind,provider,model,prompt_version,input_hash,output,status,created_at,completed_at,
          profile,model_version,parameters,schema_version,prompt_hash,schema_hash,reuse_hash,protocol_hash,
          document_hash,evidence_hash,evidence_refs,input_tokens,output_tokens,cost_micros,error_code,error_message,
          parent_automation_run_id)
         VALUES ($1,$2,'screening_suggestion','fixture-provider','fixture-model','prompt-v5',
                 'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa',$3,'completed',
                 now() - interval '2 minutes',now() - interval '1 minute','screening-profile','fixture-model-2026',
                 $6::jsonb,'schema-v2',
                 'bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb',
                 'cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc',
                 'dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd',
                 'eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee',
                 'ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff',
                 '1111111111111111111111111111111111111111111111111111111111111111',
                 $4,17,5,1234,'provider_timeout','SENSITIVE_ERROR_MESSAGE',$5)"#,
    )
    .bind(ai_run_id)
    .bind(project_id)
    .bind(serde_json::json!({
        "raw_model_output": "SENSITIVE_RAW_OUTPUT",
        "prompt": "SENSITIVE_PROMPT"
    }))
                    .bind(serde_json::json!([{
        "document_block_id": Uuid::new_v4(),
        "snippet": "SENSITIVE_EVIDENCE_TEXT"
    }]))
    .bind(automation_run_id)
    .bind(serde_json::json!({
        "temperature": 0,
                "secret_parameter": "SENSITIVE_PARAMETER"
    }))
    .execute(&pool)
    .await
    .expect("AI run should insert");
    sqlx::query(
        "INSERT INTO ai_proposals
         (id,project_id,ai_run_id,proposal_type,payload,status,decided_by,decided_at,created_at,
          entity_type,entity_id,operation,model_run_id,authority_tier,resolved_at,resolved_by_actor_kind,
          resolved_by_actor_id,resolution_reason,task_kind,target_report_id,protocol_version_id,expected_revision)
         VALUES ($1,$2,$3,'screening_suggestion',$4,'accepted','export-reviewer',now() - interval '30 seconds',now() - interval '90 seconds',
                 'screening_report',$5,'screening_suggestion',$3,'workflow_suggestion',now() - interval '30 seconds',
                 'user','export-reviewer','reviewed by export reviewer','screening_suggestion',$5,$6,1)",
    )
    .bind(ai_proposal_id)
    .bind(project_id)
    .bind(ai_run_id)
    .bind(serde_json::json!({
        "scientific_content": "SENSITIVE_PROPOSAL_PAYLOAD"
    }))
    .bind(report_id)
    .bind(protocol_id)
    .execute(&pool)
    .await
    .expect("AI proposal should insert");
    sqlx::query(
        "INSERT INTO review_calibration_bundles
         (id,project_id,definition_key,semantic_bundle_hash,evaluation_set_id,
          thresholds,metrics,reviewer_metadata,status,evaluated_at)
         VALUES ($1,$2,'screening',$3,'expert-export-v1',$4,$5,$6,'passing',now())",
    )
    .bind(calibration_bundle_id)
    .bind(project_id)
    .bind("6".repeat(64))
    .bind(serde_json::json!({"false_exclusion_rate": 0.01}))
    .bind(serde_json::json!({"false_exclusion_rate": 0.0}))
    .bind(serde_json::json!({"reviewer_id": "expert-export-reviewer"}))
    .execute(&pool)
    .await
    .expect("review calibration should insert");
    sqlx::query(
        "INSERT INTO review_run_manifests
         (project_id,automation_run_id,definition_key,definition_id,definition_version,
          manifest_hash,semantic_bundle_hash,manifest,subject,origin,prepared_task,state,
          candidate_hash,proposal_id,started_at,finished_at)
         VALUES ($1,$2,'screening','screening.v1',1,$3,$4,$5,$6,$7,$8,'completed',$9,$10,
                 now()-interval '2 minutes',now()-interval '1 minute')",
    )
    .bind(project_id)
    .bind(automation_run_id)
    .bind("7".repeat(64))
    .bind("6".repeat(64))
    .bind(serde_json::json!({"manifest_hash": "review-export-manifest"}))
    .bind(serde_json::json!({"kind": "screening", "report_id": report_id}))
    .bind(serde_json::json!({"kind": "automation_triggered", "calibration_bundle_id": calibration_bundle_id}))
    .bind(serde_json::json!({"kind": "screening"}))
    .bind("8".repeat(64))
    .bind(ai_proposal_id)
    .execute(&pool)
    .await
    .expect("review manifest should insert");
    sqlx::query(
        "INSERT INTO review_artifacts (id,project_id,content_hash,media_type,payload)
         VALUES ($1,$3,$4,'application/json',$6),
                ($2,$3,$5,'application/json',$7)",
    )
    .bind(review_artifact_id)
    .bind(review_predecessor_artifact_id)
    .bind(project_id)
    .bind("9".repeat(64))
    .bind("a".repeat(64))
    .bind(serde_json::json!({"candidate": "review-export"}))
    .bind(serde_json::json!({"prepared": "review-export"}))
    .execute(&pool)
    .await
    .expect("review artifacts should insert");
    sqlx::query(
        "INSERT INTO review_artifact_lineage
         (project_id,artifact_id,predecessor_artifact_id) VALUES ($1,$2,$3)",
    )
    .bind(project_id)
    .bind(review_artifact_id)
    .bind(review_predecessor_artifact_id)
    .execute(&pool)
    .await
    .expect("review artifact lineage should insert");
    sqlx::query(
        "INSERT INTO review_step_attempts
         (id,project_id,automation_run_id,node_id,node_version,attempt_number,input_fingerprint,
          status,worker_id,artifact_id,model_run_id,started_at,finished_at,accepted_at)
         VALUES ($1,$2,$3,'finalize',1,1,$4,'completed','export-worker',$5,$6,
                 now()-interval '2 minutes',now()-interval '1 minute',now()-interval '1 minute')",
    )
    .bind(review_attempt_id)
    .bind(project_id)
    .bind(automation_run_id)
    .bind("b".repeat(64))
    .bind(review_artifact_id)
    .bind(ai_run_id)
    .execute(&pool)
    .await
    .expect("review attempt should insert");
    sqlx::query(
        "INSERT INTO ai_runs
         (id,project_id,task_kind,provider,model,prompt_version,input_hash,output,status,created_at,completed_at,
          profile,model_version,parameters,schema_version,prompt_hash,schema_hash,reuse_hash,evidence_refs,
          input_tokens,output_tokens,cost_micros,error_code,error_message)
         VALUES ($1,$2,'appraisal_prefill','fixture-provider','fixture-model','prompt-failed',
                 '2222222222222222222222222222222222222222222222222222222222222222',NULL,'failed',
                 now() - interval '1 minute',now(),'appraisal-profile','fixture-model-2026','{}'::jsonb,'schema-v2',
                 '3333333333333333333333333333333333333333333333333333333333333333',
                 '4444444444444444444444444444444444444444444444444444444444444444',
                 '5555555555555555555555555555555555555555555555555555555555555555','[]'::jsonb,
                 0,0,0,'provider_timeout','SENSITIVE_FAILED_ERROR')",
    )
    .bind(failed_ai_run_id)
    .bind(project_id)
    .execute(&pool)
    .await
    .expect("failed AI run should insert");

    sqlx::query(
        "INSERT INTO automation_definitions
         (id,project_id,name,trigger_kind,recipe_id,recipe_version,status,actor_kind,actor_id)
         VALUES ($1,$2,'Other automation','manual','project_maintenance',1,'paused','system','other-system')",
    )
    .bind(other_automation_definition_id)
    .bind(other_project_id)
    .execute(&pool)
    .await
    .expect("other automation definition should insert");
    sqlx::query(
        "INSERT INTO jobs
         (id,project_id,kind,payload,state,attempts,max_attempts,available_at,dedupe_key)
         VALUES ($1,$2,'automation_run','{}'::jsonb,'queued',0,4,now(),$3)",
    )
    .bind(other_automation_job_id)
    .bind(other_project_id)
    .bind(format!("other-export-automation-job-{other_project_id}"))
    .execute(&pool)
    .await
    .expect("other automation job should insert");
    sqlx::query(
        "INSERT INTO automation_runs
         (id,project_id,definition_id,job_id,recipe_id,recipe_version,trigger_kind,trigger_reference,
          idempotency_key,actor_kind,actor_id,status)
         VALUES ($1,$2,$3,$4,'project_maintenance',1,'manual','other-reference',
                 'other-export-automation-run','system','other-system','queued')",
    )
    .bind(other_automation_run_id)
    .bind(other_project_id)
    .bind(other_automation_definition_id)
    .bind(other_automation_job_id)
    .execute(&pool)
    .await
    .expect("other automation run should insert");
    sqlx::query(
        "INSERT INTO automation_step_runs
         (id,project_id,automation_run_id,ordinal,step_key,step_kind,status)
         VALUES ($1,$2,$3,0,'other-step','deterministic_action','pending')",
    )
    .bind(other_automation_step_id)
    .bind(other_project_id)
    .bind(other_automation_run_id)
    .execute(&pool)
    .await
    .expect("other automation step run should insert");
    sqlx::query(
        "INSERT INTO ai_runs
         (id,project_id,task_kind,provider,model,prompt_version,input_hash,output,status,
          profile,model_version,schema_version,prompt_hash,schema_hash,reuse_hash,evidence_refs)
         VALUES ($1,$2,'screening_suggestion','other-provider','other-model','other-prompt',
                 '6666666666666666666666666666666666666666666666666666666666666666',NULL,'running',
                 'other-profile','other-model-version','other-schema','7777777777777777777777777777777777777777777777777777777777777777',
                 '8888888888888888888888888888888888888888888888888888888888888888',
                 '9999999999999999999999999999999999999999999999999999999999999999','[]'::jsonb)",
    )
    .bind(other_ai_run_id)
    .bind(other_project_id)
    .execute(&pool)
    .await
    .expect("other AI run should insert");
    sqlx::query(
        "INSERT INTO ai_proposals
         (id,project_id,ai_run_id,proposal_type,payload,status,created_at,entity_type,operation,
          model_run_id,authority_tier,task_kind)
         VALUES ($1,$2,$3,'screening_suggestion','{}'::jsonb,'pending',now(),
                 'screening_report','screening_suggestion',$3,'workflow_suggestion','screening_suggestion')",
    )
    .bind(other_ai_proposal_id)
    .bind(other_project_id)
    .bind(other_ai_run_id)
    .execute(&pool)
    .await
    .expect("other AI proposal should insert");

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
                assert_eq!(
                    body.lines().next(),
                    Some(
                        "id,created_at,event_type,aggregate_type,aggregate_id,actor_kind,actor_id,protocol_version_id,stage,decision,reason_id,event_kind,supersedes_event_id,undoes_event_id,previous_snapshot,result_snapshot,notes,payload,provenance"
                    )
                );
                assert!(body.contains("dedupe_resolution"));
                assert!(body.contains("canonical export fixture"));
                assert!(body.contains(&dedupe_event_id.to_string()));
                for event_type in [
                    "ai_run_snapshot",
                    "ai_proposal_snapshot",
                    "automation_definition_snapshot",
                    "automation_run_snapshot",
                    "automation_job_snapshot",
                    "automation_step_snapshot",
                    "review_run_manifest",
                    "review_step_attempt",
                    "review_artifact",
                    "review_calibration_bundle",
                    "reviewer_proposal_decision",
                ] {
                    assert!(
                        body.contains(event_type),
                        "audit event type is missing {event_type}"
                    );
                }
                let exported_ids: Vec<Uuid> = body
                    .lines()
                    .skip(1)
                    .map(|line| {
                        line.split(',')
                            .next()
                            .expect("audit row has an id")
                            .trim_matches('"')
                            .parse()
                            .expect("audit row id should be a UUID")
                    })
                    .collect();
                for (expected_id, expected_count) in [
                    (ai_run_id, 1),
                    (failed_ai_run_id, 1),
                    (ai_proposal_id, 2),
                    (automation_definition_id, 1),
                    (automation_job_id, 1),
                    (automation_run_id, 2),
                    (automation_step_id, 1),
                    (review_attempt_id, 1),
                    (review_artifact_id, 1),
                    (review_predecessor_artifact_id, 1),
                    (calibration_bundle_id, 1),
                    (screening_event_id, 2),
                    (dedupe_event_id, 1),
                ] {
                    assert_eq!(
                        exported_ids
                            .iter()
                            .filter(|exported_id| **exported_id == expected_id)
                            .count(),
                        expected_count,
                        "expected {expected_count} audit rows for {expected_id}"
                    );
                }
                for excluded_id in [
                    other_ai_run_id,
                    other_ai_proposal_id,
                    other_automation_definition_id,
                    other_automation_job_id,
                    other_automation_run_id,
                    other_automation_step_id,
                ] {
                    assert!(
                        !exported_ids.contains(&excluded_id),
                        "cross-project row {excluded_id} leaked"
                    );
                    assert!(
                        !body.contains(&excluded_id.to_string()),
                        "cross-project identifier {excluded_id} leaked"
                    );
                }
                assert_eq!(exported_ids.len(), 16);
                assert_eq!(exported_ids.iter().collect::<HashSet<_>>().len(), 13);
                assert!(body.contains(&review_predecessor_artifact_id.to_string()));
                let ordered_keys: Vec<(DateTime<Utc>, Uuid, String)> = body
                    .lines()
                    .skip(1)
                    .map(|line| {
                        let mut fields = line.split(',');
                        let id = fields
                            .next()
                            .expect("audit row has an id")
                            .trim_matches('"')
                            .parse()
                            .expect("audit row id should be a UUID");
                        let created_at = fields
                            .next()
                            .expect("audit row has a created_at")
                            .trim_matches('"')
                            .parse()
                            .expect("audit created_at should be RFC3339");
                        let event_type = fields
                            .next()
                            .expect("audit row has an event type")
                            .trim_matches('"')
                            .to_owned();
                        (created_at, id, event_type)
                    })
                    .collect();
                assert!(
                    ordered_keys.windows(2).all(|window| window[0] <= window[1]),
                    "audit rows should be ordered by created_at, UUID, then event type"
                );
                let tied_event_types: Vec<&str> = ordered_keys
                    .iter()
                    .filter(|(created_at, id, _)| {
                        *created_at == tied_audit_created_at && *id == screening_event_id
                    })
                    .map(|(_, _, event_type)| event_type.as_str())
                    .collect();
                assert_eq!(
                    tied_event_types,
                    ["appraisal_completed", "screening"],
                    "rows sharing created_at and UUID should use event_type as the final sort key"
                );
                let repeated = String::from_utf8(
                    response_bytes(
                        request(&pool, format!("/projects/{project_id}/exports/audit.csv")).await,
                    )
                    .await,
                )
                .expect("repeated audit CSV should be UTF-8");
                assert_eq!(body, repeated, "audit ordering should be deterministic");
                for sensitive in [
                    "SENSITIVE_PROMPT",
                    "SENSITIVE_RAW_OUTPUT",
                    "SENSITIVE_PROPOSAL_PAYLOAD",
                    "SENSITIVE_ERROR_MESSAGE",
                    "SENSITIVE_FAILED_ERROR",
                    "SENSITIVE_PARAMETER",
                    "SENSITIVE_EVIDENCE_TEXT",
                    "SENSITIVE_JOB_PAYLOAD",
                    "SENSITIVE_STEP_OUTPUT",
                ] {
                    assert!(
                        !body.contains(sensitive),
                        "sensitive value {sensitive} was exported"
                    );
                }
                for provenance in [
                    "screening_suggestion",
                    "fixture-provider",
                    "fixture-model-2026",
                    "prompt-v5",
                    "schema-v2",
                    "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
                    "workflow_suggestion",
                    "accepted",
                    "export-reviewer",
                    "reviewed by export reviewer",
                    "project_maintenance",
                    "manual-export-reference",
                    "job_attempts",
                    "ai_linkage_scope",
                    "automation_run_parent",
                    "input_tokens",
                    "cost_micros",
                ] {
                    assert!(
                        body.contains(provenance),
                        "audit provenance is missing {provenance}"
                    );
                }
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
    sqlx::query("DELETE FROM projects WHERE id=$1")
        .bind(other_project_id)
        .execute(&pool)
        .await
        .expect("other test project should clean up");
    sqlx::query("DELETE FROM reports WHERE id=$1")
        .bind(report_id)
        .execute(&pool)
        .await
        .expect("test report should clean up");
}
