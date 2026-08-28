use std::collections::HashMap;
use std::sync::OnceLock;
use std::sync::{Arc, Mutex};

use axum::{
    body::{Body, to_bytes},
    http::{Request, Response, StatusCode},
};
use chrono::Utc;
use deepref_ai::{
    AiError, AiFuture, AiGateway, CompletionRequest, GatewayCompletion, ModelParameters,
    ModelProfile, ResolvedModel, sha256_bytes,
};
use deepref_http_api::{config::ApiConfig, routes::router, state::AppState};
use sqlx::{PgPool, postgres::PgPoolOptions};
use tower::ServiceExt;
use uuid::Uuid;

static TEST_LOCK: OnceLock<tokio::sync::Mutex<()>> = OnceLock::new();

fn test_lock() -> &'static tokio::sync::Mutex<()> {
    TEST_LOCK.get_or_init(tokio::sync::Mutex::default)
}

#[derive(Clone)]
struct TestGateway {
    output: String,
    fail: bool,
    calls: Arc<Mutex<u32>>,
    evidence_block_ids: Arc<Mutex<Vec<Uuid>>>,
}

impl AiGateway for TestGateway {
    fn complete<'a>(&'a self, request: CompletionRequest) -> AiFuture<'a, GatewayCompletion> {
        let output = self.output.clone();
        let fail = self.fail;
        let calls = Arc::clone(&self.calls);
        let evidence_block_ids = Arc::clone(&self.evidence_block_ids);
        let retrieved = request
            .evidence
            .iter()
            .map(|block| block.evidence.document_block_id.as_uuid())
            .collect::<Vec<_>>();
        Box::pin(async move {
            *calls.lock().expect("gateway call counter") += 1;
            *evidence_block_ids.lock().expect("gateway evidence") = retrieved;
            if fail {
                return Err(AiError::Gateway("test provider unavailable".to_owned()));
            }
            Ok(GatewayCompletion {
                output_json: output,
                input_tokens: 7,
                output_tokens: 11,
                cost_micros: Some(13),
            })
        })
    }
}

async fn database() -> Option<PgPool> {
    let url = std::env::var("DATABASE_URL").ok()?;
    let pool = PgPoolOptions::new()
        .max_connections(8)
        .connect(&url)
        .await
        .expect("DATABASE_URL database must be reachable");
    deepref_postgres::migrate(&pool)
        .await
        .expect("DATABASE_URL migrations must apply");
    Some(pool)
}

fn api_config() -> ApiConfig {
    let runtime = deepref_config::RuntimeConfig::from_map(
        "deepref-api-ai-test",
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

async fn response_json(response: Response<Body>) -> serde_json::Value {
    let body = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("response body should be readable");
    serde_json::from_slice(&body).expect("response should be JSON")
}

async fn setup(pool: &PgPool) -> (Uuid, Uuid, Uuid, String) {
    setup_with_full_text_criteria(pool, false).await
}

async fn setup_full_text(pool: &PgPool) -> (Uuid, Uuid, Uuid, String) {
    setup_with_full_text_criteria(pool, true).await
}

async fn setup_with_full_text_criteria(
    pool: &PgPool,
    with_full_text_criteria: bool,
) -> (Uuid, Uuid, Uuid, String) {
    let project_id = Uuid::new_v4();
    let report_id = Uuid::new_v4();
    let protocol_id = Uuid::new_v4();
    let criterion_id = Uuid::new_v4();
    let title = format!("AI screening test {}", Uuid::new_v4());
    sqlx::query("INSERT INTO projects (id,name) VALUES ($1,'AI HTTP test')")
        .bind(project_id)
        .execute(pool)
        .await
        .expect("project inserts");
    sqlx::query("INSERT INTO reports (id,title,abstract_text) VALUES ($1,$2,'A test abstract')")
        .bind(report_id)
        .bind(&title)
        .execute(pool)
        .await
        .expect("report inserts");
    sqlx::query("INSERT INTO project_reports (project_id,report_id) VALUES ($1,$2)")
        .bind(project_id)
        .bind(report_id)
        .execute(pool)
        .await
        .expect("report membership inserts");
    sqlx::query(
        "INSERT INTO protocol_versions (id,project_id,version,name,status,criteria)
         VALUES ($1,$2,1,'AI test protocol','draft','[]'::jsonb)",
    )
    .bind(protocol_id)
    .bind(project_id)
    .execute(pool)
    .await
    .expect("protocol inserts");
    sqlx::query(
        "INSERT INTO eligibility_criteria
         (id,protocol_version_id,criterion_type,stage,dimension,label,description,ordinal)
         VALUES ($1,$2,'include',$3,'population',$4,$5,0)",
    )
    .bind(criterion_id)
    .bind(protocol_id)
    .bind(if with_full_text_criteria {
        "both"
    } else {
        "title_abstract"
    })
    .bind(if with_full_text_criteria {
        "Adults with the target pulmonary condition"
    } else {
        "Test population"
    })
    .bind(if with_full_text_criteria {
        "Include adults with chronic obstructive pulmonary disease."
    } else {
        "The report has the test population."
    })
    .execute(pool)
    .await
    .expect("criterion inserts");
    if with_full_text_criteria {
        sqlx::query(
            "INSERT INTO eligibility_criteria
             (id,protocol_version_id,criterion_type,stage,dimension,label,description,ordinal)
             VALUES ($1,$2,'include','full_text','intervention',
                     'Virtual neuromuscular teletherapy intervention',
                     'Include virtual neuromuscular teletherapy delivered remotely.',1)",
        )
        .bind(Uuid::new_v4())
        .bind(protocol_id)
        .execute(pool)
        .await
        .expect("second criterion inserts");
    }
    sqlx::query(
        "UPDATE protocol_versions SET status='published',published_at=now()
         WHERE id=$1 AND project_id=$2",
    )
    .bind(protocol_id)
    .bind(project_id)
    .execute(pool)
    .await
    .expect("protocol publication");
    (project_id, report_id, criterion_id, title)
}

fn route(provider: &str) -> ResolvedModel {
    ResolvedModel {
        profile: ModelProfile::Reasoning,
        provider: provider.to_owned(),
        model: "screening-model".to_owned(),
        model_version: "2026-test".to_owned(),
        parameters: ModelParameters::default(),
        route_id: Some(Uuid::new_v4()),
    }
}

fn full_text_route(provider: &str) -> ResolvedModel {
    ResolvedModel {
        profile: ModelProfile::LongContextReasoning,
        provider: provider.to_owned(),
        model: "full-text-screening-model".to_owned(),
        model_version: "2026-test".to_owned(),
        parameters: ModelParameters::default(),
        route_id: Some(Uuid::new_v4()),
    }
}

async fn generate(
    pool: &PgPool,
    project_id: Uuid,
    report_id: Uuid,
    gateway: TestGateway,
) -> Response<Body> {
    generate_stage(pool, project_id, report_id, gateway, "title_abstract").await
}

async fn generate_stage(
    pool: &PgPool,
    project_id: Uuid,
    report_id: Uuid,
    gateway: TestGateway,
    stage: &str,
) -> Response<Body> {
    router(
        AppState::new(pool.clone()).with_ai_gateway(gateway),
        &api_config(),
    )
    .oneshot(
        Request::builder()
            .method("POST")
            .uri(format!(
                "/projects/{project_id}/reports/{report_id}/ai/screening"
            ))
            .header("content-type", "application/json")
            .body(Body::from(format!(r#"{{"stage":"{stage}"}}"#)))
            .expect("AI request should be valid"),
    )
    .await
    .expect("AI request should be handled")
}

async fn cleanup_project(pool: &PgPool, project_id: Uuid, report_id: Uuid) {
    sqlx::query("DELETE FROM ai_proposal_evidence WHERE project_id=$1")
        .bind(project_id)
        .execute(pool)
        .await
        .expect("evidence cleanup");
    sqlx::query("DELETE FROM ai_proposal_criterion_judgments WHERE project_id=$1")
        .bind(project_id)
        .execute(pool)
        .await
        .expect("criterion cleanup");
    sqlx::query("DELETE FROM ai_proposals WHERE project_id=$1")
        .bind(project_id)
        .execute(pool)
        .await
        .expect("proposal cleanup");
    sqlx::query("DELETE FROM ai_runs WHERE project_id=$1")
        .bind(project_id)
        .execute(pool)
        .await
        .expect("run cleanup");
    sqlx::query("DELETE FROM projects WHERE id=$1")
        .bind(project_id)
        .execute(pool)
        .await
        .expect("project cleanup");
    sqlx::query("DELETE FROM reports WHERE id=$1")
        .bind(report_id)
        .execute(pool)
        .await
        .expect("report cleanup");
}

#[tokio::test]
async fn injected_gateway_is_called_and_run_provenance_is_persisted() {
    let _guard = test_lock().lock().await;
    let Some(pool) = database().await else { return };
    let (project_id, report_id, criterion_id, title) = setup(&pool).await;
    let provider = format!("test-provider-{}", Uuid::new_v4());
    let model_route = route(&provider);
    deepref_postgres::insert_model_route(&pool, &model_route, Utc::now())
        .await
        .expect("model route inserts");
    let calls = Arc::new(Mutex::new(0));
    let gateway = TestGateway {
        output: serde_json::json!({
            "report_id": report_id,
            "expected_revision": 0,
            "stage": "title_abstract",
            "protocol_version_id": model_route.route_id.unwrap_or_default(),
            "criteria": [{
                "criterion_id": criterion_id,
                "judgment": "unclear",
                "rationale": "The test evidence is inconclusive.",
                "evidence": [{
                    "kind": "report_metadata",
                    "report_id": report_id,
                    "field": "title",
                    "content_hash": sha256_bytes(title.as_bytes())
                }]
            }],
            "suggested_decision": {"kind": "maybe"},
            "uncertainties": []
        })
        .to_string(),
        fail: false,
        calls: Arc::clone(&calls),
        evidence_block_ids: Arc::new(Mutex::new(Vec::new())),
    };
    let protocol_id: Uuid = sqlx::query_scalar(
        "SELECT id FROM protocol_versions WHERE project_id=$1 AND status='published'",
    )
    .bind(project_id)
    .fetch_one(&pool)
    .await
    .expect("protocol id");
    let mut output: serde_json::Value = serde_json::from_str(&gateway.output).expect("output");
    output["protocol_version_id"] = serde_json::json!(protocol_id);
    let gateway = TestGateway {
        output: output.to_string(),
        ..gateway
    };
    let response = generate(&pool, project_id, report_id, gateway).await;
    let status = response.status();
    let body = response_json(response).await;
    assert_eq!(status, StatusCode::OK, "AI response: {body}");
    assert_eq!(body["payload"]["kind"], "screening");
    assert_eq!(*calls.lock().expect("gateway call counter"), 1);

    let (status, provider_from_run, model, version, prompt_version): (
        String,
        String,
        String,
        String,
        String,
    ) = sqlx::query_as(
        "SELECT r.status,r.provider,r.model,r.model_version,r.prompt_version
         FROM ai_runs r JOIN ai_proposals p ON p.model_run_id=r.id
         WHERE p.project_id=$1 ORDER BY r.created_at DESC LIMIT 1",
    )
    .bind(project_id)
    .fetch_one(&pool)
    .await
    .expect("run provenance");
    assert_eq!(status, "completed");
    assert_eq!(provider_from_run, provider);
    assert_eq!(model, "screening-model");
    assert_eq!(version, "2026-test");
    assert_eq!(prompt_version, "screening.title_abstract.v1");

    cleanup_project(&pool, project_id, report_id).await;
}

#[tokio::test]
async fn full_text_retrieval_uses_or_terms_and_passes_grounding_to_gateway() {
    let _guard = test_lock().lock().await;
    let Some(pool) = database().await else { return };
    let (project_id, report_id, first_criterion_id, _title) = setup_full_text(&pool).await;
    let document_id = Uuid::new_v4();
    let block_id = Uuid::new_v4();
    let title = "Randomized comparative outcomes study";
    let abstract_text = [
        "Background multicenter prospective observational comparative cohort investigation enrolled participants across outpatient tertiary academic centers.",
        "Baseline demographics comorbidity medication adherence exposure allocation followup assessment laboratory imaging biomarkers intervals questionnaire response attrition.",
        "Protocol deviations monitoring recruitment retention endpoints variance subgroup stratification sensitivity calibration imputation missingness covariance regression confidence interval estimate precision heterogeneity interaction moderation.",
        "Implementation fidelity feasibility acceptability safety tolerability utilization resource expenditure workflow training staffing environment seasonality geography socioeconomic insurance transport communication equity dissemination limitations interpretation conclusion manuscript supplement appendix registration funding oversight.",
    ]
    .join(" ");
    sqlx::query("UPDATE reports SET title=$2,abstract_text=$3 WHERE id=$1")
        .bind(report_id)
        .bind(title)
        .bind(abstract_text)
        .execute(&pool)
        .await
        .expect("realistic report metadata");
    let protocol_id: Uuid = sqlx::query_scalar(
        "SELECT id FROM protocol_versions WHERE project_id=$1 AND status='published'",
    )
    .bind(project_id)
    .fetch_one(&pool)
    .await
    .expect("published protocol");
    let second_criterion_id: Uuid = sqlx::query_scalar(
        "SELECT id FROM eligibility_criteria WHERE protocol_version_id=$1 AND ordinal=1",
    )
    .bind(protocol_id)
    .fetch_one(&pool)
    .await
    .expect("second criterion");
    sqlx::query(
        "INSERT INTO documents
         (id,project_id,report_id,object_key,content_hash,mime_type,byte_size,source,status,
          actor_kind,actor_id,active_parser_version,parser_version)
         VALUES ($1,$2,$3,$4,$5,'application/pdf',10,'upload','available','system','ai-test','parser.v1','parser.v1')",
    )
    .bind(document_id)
    .bind(project_id)
    .bind(report_id)
    .bind(format!("documents/{document_id}"))
    .bind("f".repeat(64))
    .execute(&pool)
    .await
    .expect("document");
    sqlx::query(
        "INSERT INTO document_pages(document_id,parser_version,page_number,width,height,active)
         VALUES ($1,'parser.v1',1,600,800,true)",
    )
    .bind(document_id)
    .execute(&pool)
    .await
    .expect("document page");
    let block_text = "Virtual neuromuscular teletherapy was delivered remotely.";
    let block_hash = "e".repeat(64);
    sqlx::query(
        "INSERT INTO document_blocks
         (id,document_id,parser_version,page_number,page_width,page_height,kind,section_path,
          ordinal,text,content_hash,active)
         VALUES ($1,$2,'parser.v1',1,600,800,'text',ARRAY['Methods'],0,$3,$4,true)",
    )
    .bind(block_id)
    .bind(document_id)
    .bind(block_text)
    .bind(&block_hash)
    .execute(&pool)
    .await
    .expect("target evidence block");

    let provider = format!("full-text-provider-{}", Uuid::new_v4());
    deepref_postgres::insert_model_route(&pool, &full_text_route(&provider), Utc::now())
        .await
        .expect("full-text model route");
    let calls = Arc::new(Mutex::new(0));
    let evidence_block_ids = Arc::new(Mutex::new(Vec::new()));
    let gateway = TestGateway {
        output: serde_json::json!({
            "report_id": report_id,
            "expected_revision": 0,
            "stage": "full_text",
            "protocol_version_id": protocol_id,
            "criteria": [
                {
                    "criterion_id": first_criterion_id,
                    "judgment": "unclear",
                    "rationale": "The retrieved block supports the population but not a complete eligibility decision.",
                    "evidence": [{
                        "kind": "document_block",
                        "document_block_id": block_id,
                        "page": 1,
                        "content_hash": block_hash,
                        "section_path": ["Methods"]
                    }]
                },
                {
                    "criterion_id": second_criterion_id,
                    "judgment": "unclear",
                    "rationale": "The retrieved block supports the intervention context but needs reviewer interpretation.",
                    "evidence": [{
                        "kind": "document_block",
                        "document_block_id": block_id,
                        "page": 1,
                        "content_hash": block_hash,
                        "section_path": ["Methods"]
                    }]
                }
            ],
            "suggested_decision": {"kind": "insufficient_evidence"},
            "uncertainties": ["The available block is only a partial eligibility context."]
        })
        .to_string(),
        fail: false,
        calls: Arc::clone(&calls),
        evidence_block_ids: Arc::clone(&evidence_block_ids),
    };
    let response = generate_stage(&pool, project_id, report_id, gateway, "full_text").await;
    let status = response.status();
    let body = response_json(response).await;
    assert_eq!(status, StatusCode::OK, "AI response: {body}");
    assert_eq!(body["payload"]["kind"], "screening");
    assert_eq!(*calls.lock().expect("gateway call counter"), 1);
    assert_eq!(
        *evidence_block_ids.lock().expect("gateway evidence"),
        vec![block_id]
    );
    let prompt_version: String = sqlx::query_scalar(
        "SELECT prompt_version FROM ai_runs WHERE project_id=$1 ORDER BY created_at DESC LIMIT 1",
    )
    .bind(project_id)
    .fetch_one(&pool)
    .await
    .expect("full-text prompt provenance");
    assert_eq!(prompt_version, "screening.full_text.v1");
    let evidence_count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM ai_run_evidence e JOIN ai_runs r ON r.id=e.ai_run_id WHERE r.project_id=$1",
    )
    .bind(project_id)
    .fetch_one(&pool)
    .await
    .expect("stored full-text evidence");
    assert_eq!(evidence_count, 1);

    cleanup_project(&pool, project_id, report_id).await;
}

#[tokio::test]
async fn gateway_failure_is_a_failed_run_without_a_proposal() {
    let _guard = test_lock().lock().await;
    let Some(pool) = database().await else { return };
    let (project_id, report_id, criterion_id, _title) = setup(&pool).await;
    let provider = format!("failing-provider-{}", Uuid::new_v4());
    deepref_postgres::insert_model_route(&pool, &route(&provider), Utc::now())
        .await
        .expect("model route inserts");
    let response = generate(
        &pool,
        project_id,
        report_id,
        TestGateway {
            output: serde_json::json!({"criterion_id": criterion_id}).to_string(),
            fail: true,
            calls: Arc::new(Mutex::new(0)),
            evidence_block_ids: Arc::new(Mutex::new(Vec::new())),
        },
    )
    .await;
    let status = response.status();
    let body = response_json(response).await;
    assert_eq!(
        status,
        StatusCode::SERVICE_UNAVAILABLE,
        "AI response: {body}"
    );
    let proposals: i64 =
        sqlx::query_scalar("SELECT count(*) FROM ai_proposals WHERE project_id=$1")
            .bind(project_id)
            .fetch_one(&pool)
            .await
            .expect("proposal count");
    let failed_runs: i64 =
        sqlx::query_scalar("SELECT count(*) FROM ai_runs WHERE project_id=$1 AND status='failed'")
            .bind(project_id)
            .fetch_one(&pool)
            .await
            .expect("failed run count");
    assert_eq!(proposals, 0);
    assert_eq!(failed_runs, 1);

    cleanup_project(&pool, project_id, report_id).await;
}
