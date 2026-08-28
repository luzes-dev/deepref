use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

use axum::{
    body::{Body, to_bytes},
    http::{Request, Response, StatusCode},
};
use chrono::{Duration, Utc};
use deepref_ai::{
    AiError, AiFuture, AiGateway, CompletionRequest, GatewayCompletion, ModelParameters,
    ModelProfile, ResolvedModel, sha256_bytes,
};
use deepref_config::RuntimeConfig;
use deepref_http_api::{config::ApiConfig, routes::router, state::AppState};
use serde_json::{Value, json};
use sqlx::{PgPool, postgres::PgPoolOptions};
use tower::ServiceExt;
use uuid::Uuid;

static TEST_LOCK: OnceLock<tokio::sync::Mutex<()>> = OnceLock::new();

fn test_lock() -> &'static tokio::sync::Mutex<()> {
    TEST_LOCK.get_or_init(tokio::sync::Mutex::default)
}

async fn database() -> Option<PgPool> {
    let url = std::env::var("TEST_DATABASE_URL")
        .or_else(|_| std::env::var("DATABASE_URL"))
        .ok()?;
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
    let runtime = RuntimeConfig::from_map(
        "deepref-api-assistant-test",
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

#[derive(Debug, Clone, Copy)]
struct Fixture {
    project_id: Uuid,
    other_project_id: Uuid,
    report_id: Uuid,
    other_report_id: Uuid,
    record_id: Uuid,
    study_id: Uuid,
    protocol_id: Uuid,
    criterion_id: Uuid,
    document_id: Uuid,
    block_id: Uuid,
    field_id: Uuid,
}

async fn seed(pool: &PgPool) -> Fixture {
    let fixture = Fixture {
        project_id: Uuid::new_v4(),
        other_project_id: Uuid::new_v4(),
        report_id: Uuid::new_v4(),
        other_report_id: Uuid::new_v4(),
        record_id: Uuid::new_v4(),
        study_id: Uuid::new_v4(),
        protocol_id: Uuid::new_v4(),
        criterion_id: Uuid::new_v4(),
        document_id: Uuid::new_v4(),
        block_id: Uuid::new_v4(),
        field_id: Uuid::new_v4(),
    };
    sqlx::query(
        "INSERT INTO projects (id,name) VALUES ($1,'assistant project'),($2,'other project')",
    )
    .bind(fixture.project_id)
    .bind(fixture.other_project_id)
    .execute(pool)
    .await
    .expect("project inserts");
    let long_abstract = "bounded assistant abstract ".repeat(400);
    sqlx::query(
        "INSERT INTO reports (id,title,abstract_text,publication_year,journal,url)
         VALUES ($1,'Assistant report',$3,2026,'Assistant journal','https://example.test/report'),
                ($2,'Other project report','other project abstract',2025,'Other journal',NULL)",
    )
    .bind(fixture.report_id)
    .bind(fixture.other_report_id)
    .bind(long_abstract)
    .execute(pool)
    .await
    .expect("report inserts");
    sqlx::query("INSERT INTO project_reports (project_id,report_id) VALUES ($1,$2),($3,$4)")
        .bind(fixture.project_id)
        .bind(fixture.report_id)
        .bind(fixture.other_project_id)
        .bind(fixture.other_report_id)
        .execute(pool)
        .await
        .expect("project report inserts");
    sqlx::query(
        "INSERT INTO records (id,project_id,source,source_key,title,publication_year,authors)
         VALUES ($1,$2,'assistant-test',$3,'Assistant report',2026,'[]'::jsonb)",
    )
    .bind(fixture.record_id)
    .bind(fixture.project_id)
    .bind(format!("source-{}", fixture.record_id))
    .execute(pool)
    .await
    .expect("record inserts");
    sqlx::query(
        "INSERT INTO studies
         (id,project_id,title,design_context,study_revision,updated_by_actor_kind,updated_by_actor_id)
         VALUES ($1,$2,'Assistant study','{}'::jsonb,0,'system','assistant-test')",
    )
    .bind(fixture.study_id)
    .bind(fixture.project_id)
    .execute(pool)
    .await
    .expect("study inserts");
    sqlx::query(
        "INSERT INTO study_reports (project_id,study_id,report_id,relationship)
         VALUES ($1,$2,$3,'report_of_study')",
    )
    .bind(fixture.project_id)
    .bind(fixture.study_id)
    .bind(fixture.report_id)
    .execute(pool)
    .await
    .expect("study membership inserts");
    sqlx::query(
        "INSERT INTO protocol_versions
         (id,project_id,version,name,status,criteria,framework_kind,framework_fields,objective,question,published_at)
         VALUES ($1,$2,1,'Assistant protocol','draft','[]'::jsonb,'custom','{}'::jsonb,
                 'Assistant objective','Assistant question',NULL)",
    )
    .bind(fixture.protocol_id)
    .bind(fixture.project_id)
    .execute(pool)
    .await
    .expect("protocol inserts");
    sqlx::query(
        "INSERT INTO eligibility_criteria
         (id,protocol_version_id,criterion_type,stage,dimension,label,description,ordinal)
         VALUES ($1,$2,'include','title_abstract','population','Assistant population',
                 'The assistant test population.',0)",
    )
    .bind(fixture.criterion_id)
    .bind(fixture.protocol_id)
    .execute(pool)
    .await
    .expect("criterion inserts");
    sqlx::query(
        "UPDATE protocol_versions SET status='published',published_at=now() WHERE id=$1 AND project_id=$2",
    )
    .bind(fixture.protocol_id)
    .bind(fixture.project_id)
    .execute(pool)
    .await
    .expect("protocol publication");
    let document_hash = "a".repeat(64);
    sqlx::query(
        "INSERT INTO documents
         (id,project_id,report_id,object_key,content_hash,mime_type,byte_size,source,status,
          actor_kind,actor_id,active_parser_version,parser_version)
         VALUES ($1,$2,$3,$4,$5,'application/pdf',10,'upload','available',
                 'system','assistant-test','assistant.parser.v1','assistant.parser.v1')",
    )
    .bind(fixture.document_id)
    .bind(fixture.project_id)
    .bind(fixture.report_id)
    .bind(format!("documents/{}", fixture.document_id))
    .bind(document_hash)
    .execute(pool)
    .await
    .expect("document inserts");
    sqlx::query(
        "INSERT INTO document_pages(document_id,parser_version,page_number,width,height,active)
         VALUES ($1,'assistant.parser.v1',1,600,800,true)",
    )
    .bind(fixture.document_id)
    .execute(pool)
    .await
    .expect("document page inserts");
    sqlx::query(
        "INSERT INTO document_blocks
         (id,document_id,parser_version,page_number,kind,section_path,ordinal,text,content_hash,active)
         VALUES ($1,$2,'assistant.parser.v1',1,'text',ARRAY['Results'],0,$3,$4,true)",
    )
    .bind(fixture.block_id)
    .bind(fixture.document_id)
    .bind(
        "Allocation process Record whether the allocation process is sufficiently described. \
         Outcome reporting Was the outcome measure prespecified? sample size Assistant evidence ",
    )
    .bind("b".repeat(64))
    .execute(pool)
    .await
    .expect("document block inserts");
    sqlx::query(
        "INSERT INTO screening_state
         (project_id,report_id,title_abstract_status,full_text_status,final_status,revision)
         VALUES ($1,$2,'maybe','not_required','maybe',3)",
    )
    .bind(fixture.project_id)
    .bind(fixture.report_id)
    .execute(pool)
    .await
    .expect("screening state inserts");
    sqlx::query(
        "INSERT INTO appraisal_assessments
         (id,project_id,report_id,definition_id,definition_version,responses,judgments,actor_kind,actor_id,completed_at)
         VALUES ($1,$2,$3,'deepref-rct-generic',1,'{}'::jsonb,'{}'::jsonb,'system','assistant-test',now())",
    )
    .bind(Uuid::new_v4())
    .bind(fixture.project_id)
    .bind(fixture.report_id)
    .execute(pool)
    .await
    .expect("appraisal inserts");
    sqlx::query(
        "INSERT INTO extraction_field_definitions
         (id,project_id,version,field_key,label,value_type,required)
         VALUES ($1,$2,1,'sample','Sample size','number',false)",
    )
    .bind(fixture.field_id)
    .bind(fixture.project_id)
    .execute(pool)
    .await
    .expect("extraction field inserts");
    fixture
}

async fn cleanup(pool: &PgPool, fixture: Fixture) {
    sqlx::query("DELETE FROM ai_proposal_criterion_judgments WHERE project_id IN ($1,$2)")
        .bind(fixture.project_id)
        .bind(fixture.other_project_id)
        .execute(pool)
        .await
        .expect("proposal criterion cleanup");
    sqlx::query("DELETE FROM ai_proposal_evidence WHERE project_id IN ($1,$2)")
        .bind(fixture.project_id)
        .bind(fixture.other_project_id)
        .execute(pool)
        .await
        .expect("proposal evidence cleanup");
    sqlx::query("DELETE FROM ai_proposals WHERE project_id IN ($1,$2)")
        .bind(fixture.project_id)
        .bind(fixture.other_project_id)
        .execute(pool)
        .await
        .expect("proposal cleanup");
    sqlx::query("DELETE FROM ai_run_evidence WHERE project_id IN ($1,$2)")
        .bind(fixture.project_id)
        .bind(fixture.other_project_id)
        .execute(pool)
        .await
        .expect("run evidence cleanup");
    sqlx::query("DELETE FROM ai_runs WHERE project_id IN ($1,$2)")
        .bind(fixture.project_id)
        .bind(fixture.other_project_id)
        .execute(pool)
        .await
        .expect("run cleanup");
    // Published protocol rows are immutable when deleted directly; the
    // project cascade is the supported cleanup path for this fixture data.
    sqlx::query("DELETE FROM projects WHERE id IN ($1,$2)")
        .bind(fixture.project_id)
        .bind(fixture.other_project_id)
        .execute(pool)
        .await
        .expect("project cleanup");
    sqlx::query("DELETE FROM reports WHERE id IN ($1,$2)")
        .bind(fixture.report_id)
        .bind(fixture.other_report_id)
        .execute(pool)
        .await
        .expect("report cleanup");
}

async fn response_json(response: Response<Body>) -> Value {
    let body = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("response body should be readable");
    serde_json::from_slice(&body).expect("response should be JSON")
}

async fn execute(pool: &PgPool, project_id: Uuid, tool: &str, args: Value) -> (StatusCode, Value) {
    let response = router(AppState::new(pool.clone()), &api_config())
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/projects/{project_id}/assistant/tools/execute"))
                .header("content-type", "application/json")
                .body(Body::from(json!({"tool": tool, "args": args}).to_string()))
                .expect("assistant request should be valid"),
        )
        .await
        .expect("assistant request should be handled");
    let status = response.status();
    (status, response_json(response).await)
}

async fn execute_with_state(
    state: AppState,
    project_id: Uuid,
    tool: &str,
    args: Value,
) -> (StatusCode, Value) {
    let response = router(state, &api_config())
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/projects/{project_id}/assistant/tools/execute"))
                .header("content-type", "application/json")
                .body(Body::from(json!({"tool": tool, "args": args}).to_string()))
                .expect("assistant request should be valid"),
        )
        .await
        .expect("assistant request should be handled");
    let status = response.status();
    (status, response_json(response).await)
}

#[tokio::test]
async fn catalog_and_all_reads_are_project_scoped_and_bounded() {
    let _guard = test_lock().lock().await;
    let Some(pool) = database().await else { return };
    let fixture = seed(&pool).await;
    let list_response = router(AppState::new(pool.clone()), &api_config())
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(format!("/projects/{}/assistant/tools", fixture.project_id))
                .body(Body::empty())
                .expect("catalog request should be valid"),
        )
        .await
        .expect("catalog request should be handled");
    assert_eq!(list_response.status(), StatusCode::OK);
    let catalog = response_json(list_response).await;
    let entries = catalog.as_array().expect("catalog should be an array");
    assert_eq!(entries.len(), 14);
    assert_eq!(
        entries
            .iter()
            .filter(|entry| entry["kind"] == "read")
            .count(),
        8
    );
    assert_eq!(
        entries
            .iter()
            .filter(|entry| entry["kind"] == "proposal")
            .count(),
        6
    );

    let cases = [
        (
            "get_project_protocol",
            json!({"project_id": fixture.project_id}),
            "id",
        ),
        (
            "get_report",
            json!({"project_id": fixture.project_id, "report_id": fixture.report_id}),
            "id",
        ),
        (
            "read_document_blocks",
            json!({"project_id": fixture.project_id, "document_id": fixture.document_id, "block_ids": [fixture.block_id]}),
            "data",
        ),
        (
            "search_document",
            json!({"project_id": fixture.project_id, "document_id": fixture.document_id, "query": "allocation", "limit": 10}),
            "data",
        ),
        (
            "search_project_reports",
            json!({"project_id": fixture.project_id, "query": "assistant", "limit": 10}),
            "data",
        ),
        (
            "get_screening_state",
            json!({"project_id": fixture.project_id, "report_id": fixture.report_id}),
            "final_status",
        ),
        (
            "get_study",
            json!({"project_id": fixture.project_id, "study_id": fixture.study_id}),
            "id",
        ),
        (
            "get_appraisal",
            json!({"project_id": fixture.project_id, "report_id": fixture.report_id, "definition_id": "deepref-rct-generic", "definition_version": 1}),
            "definition_id",
        ),
    ];
    for (tool, args, expected_field) in cases {
        let (status, body) = execute(&pool, fixture.project_id, tool, args).await;
        assert_eq!(status, StatusCode::OK, "{tool} response: {body}");
        assert_eq!(body["kind"], "read", "{tool} response: {body}");
        if expected_field == "data" {
            assert!(body["data"].is_array(), "{tool} should return an array");
        } else {
            assert!(
                body["data"][expected_field].is_string(),
                "{tool} should return {expected_field}"
            );
        }
    }
    let report_abstract_len = execute(
        &pool,
        fixture.project_id,
        "get_report",
        json!({"project_id": fixture.project_id, "report_id": fixture.report_id}),
    )
    .await
    .1["data"]["abstract_text"]
        .as_str()
        .expect("report abstract should be present")
        .chars()
        .count();
    assert_eq!(report_abstract_len, 4_000);
    let block_len = execute(
        &pool,
        fixture.project_id,
        "read_document_blocks",
        json!({"project_id": fixture.project_id, "document_id": fixture.document_id, "block_ids": [fixture.block_id]}),
    )
    .await
    .1["data"][0]["text"]
        .as_str()
        .expect("block text should be present")
        .chars()
        .count();
    assert!(block_len <= 2_000);

    let (status, body) = execute(
        &pool,
        fixture.project_id,
        "get_report",
        json!({"project_id": fixture.project_id, "report_id": fixture.other_report_id}),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "cross-project read: {body}");
    let (status, body) = execute(
        &pool,
        fixture.project_id,
        "search_document",
        json!({"project_id": fixture.project_id, "document_id": Uuid::new_v4(), "query": "allocation", "limit": 10}),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "missing document search: {body}"
    );
    cleanup(&pool, fixture).await;
}

#[derive(Clone)]
struct ProposalGateway {
    fixture: Fixture,
    calls: Arc<Mutex<usize>>,
}

impl AiGateway for ProposalGateway {
    fn complete<'a>(&'a self, request: CompletionRequest) -> AiFuture<'a, GatewayCompletion> {
        let fixture = self.fixture;
        let calls = Arc::clone(&self.calls);
        Box::pin(async move {
            *calls.lock().expect("gateway calls lock") += 1;
            let input: Value = serde_json::from_str(&request.user_prompt)
                .map_err(|error| AiError::Gateway(format!("test input: {error}")))?;
            Ok(GatewayCompletion {
                output_json: proposal_output(input, fixture).to_string(),
                input_tokens: 1,
                output_tokens: 1,
                cost_micros: Some(1),
            })
        })
    }
}

fn proposal_output(input: Value, fixture: Fixture) -> Value {
    if input.get("stage").is_some() {
        let criterion = &input["criteria"][0];
        return json!({
            "report_id": fixture.report_id,
            "expected_revision": input["expected_revision"],
            "stage": "title_abstract",
            "protocol_version_id": fixture.protocol_id,
            "criteria": [{
                "criterion_id": criterion["id"],
                "judgment": "unclear",
                "rationale": "The reviewer should inspect the grounded metadata.",
                "evidence": [{"kind": "report_metadata", "report_id": fixture.report_id, "field": "title", "content_hash": sha256_bytes(b"Assistant report")}]
            }],
            "suggested_decision": {"kind": "maybe"},
            "uncertainties": []
        });
    }
    if input.get("source_record_id").is_some() {
        return json!({
            "candidate": {"source_record_id": fixture.record_id, "candidate_report_id": fixture.report_id},
            "decision": "no_match",
            "rationale": [{"code": "metadata_review", "explanation": "The metadata should be reviewed by a human."}],
            "signals": input["grounded_signals"],
            "provenance": input["grounded_provenance"],
            "uncertainties": []
        });
    }
    if input.get("candidates").is_some() {
        return json!({
            "report_id": fixture.report_id,
            "expected_previous_study_id": fixture.study_id,
            "expected_previous_study_revision": 0,
            "choice": {"kind": "existing_study", "study_id": fixture.study_id, "expected_revision": 0},
            "rationale": "The report remains grouped with the grounded study.",
            "provenance": input["grounded_evidence"],
            "uncertainties": []
        });
    }
    if input.get("allowed_designs").is_some() {
        return json!({
            "study_id": fixture.study_id,
            "suggested_design": "rct",
            "rationale": "The study metadata is compatible with this reviewer suggestion.",
            "evidence": input["grounded_evidence"],
            "uncertainties": []
        });
    }
    if input.get("fields").is_some() {
        return json!({
            "study_id": fixture.study_id,
            "fields": [{
                "kind": "value",
                "field_id": fixture.field_id,
                "field_version": 1,
                "value": {"kind": "number", "value": 42.0},
                "rationale": "The grounded block reports the sample size.",
                "source": input["grounded_evidence"][0]
            }]
        });
    }
    if input.get("questions").is_some() {
        let answers = input["questions"]
            .as_array()
            .expect("appraisal questions")
            .iter()
            .map(|question| {
                let answer = match question["answer_schema"]["kind"].as_str() {
                    Some("enum") => json!({"kind": "enum", "value": "yes"}),
                    Some("boolean") => json!({"kind": "boolean", "value": true}),
                    Some("scale") => json!({"kind": "scale", "value": 1}),
                    Some("text") => json!({"kind": "text", "value": "reviewed"}),
                    _ => json!({"kind": "boolean", "value": true}),
                };
                json!({
                    "question_id": question["id"],
                    "answer": answer,
                    "rationale": "The reviewer should verify this grounded answer.",
                    "evidence": if question["requires_evidence"].as_bool().unwrap_or(false) {
                        json!([input["grounded_evidence"][0]])
                    } else {
                        json!([])
                    }
                })
            })
            .collect::<Vec<_>>();
        let domains = input["domains"]
            .as_array()
            .expect("appraisal domains")
            .iter()
            .map(|domain| {
                (
                    domain["id"].as_str().expect("domain id").to_owned(),
                    domain["allowed_judgments"][0].clone(),
                )
            })
            .collect::<serde_json::Map<_, _>>();
        return json!({
            "report_id": fixture.report_id,
            "definition_id": "deepref-rct-generic",
            "definition_version": 1,
            "answers": answers,
            "domain_judgments": domains,
            "overall_judgment": input["overall_allowed_judgments"][0]
        });
    }
    panic!("unexpected assistant task input: {input}");
}

fn model_route(profile: ModelProfile) -> ResolvedModel {
    ResolvedModel {
        profile,
        provider: format!("assistant-test-{}", Uuid::new_v4()),
        model: "assistant-test-model".to_owned(),
        model_version: "assistant-test-v1".to_owned(),
        parameters: ModelParameters::default(),
        route_id: Some(Uuid::new_v4()),
    }
}

#[tokio::test]
async fn all_proposal_tools_persist_pending_proposals_without_domain_writes() {
    let _guard = test_lock().lock().await;
    let Some(pool) = database().await else { return };
    let fixture = seed(&pool).await;
    for profile in [
        ModelProfile::FastClassifier,
        ModelProfile::Reasoning,
        ModelProfile::LongContextReasoning,
    ] {
        deepref_postgres::insert_model_route(
            &pool,
            &model_route(profile),
            Utc::now() - Duration::milliseconds(1),
        )
        .await
        .expect("model route inserts");
    }
    let calls = Arc::new(Mutex::new(0));
    let state = AppState::new(pool.clone()).with_ai_gateway(ProposalGateway {
        fixture,
        calls: Arc::clone(&calls),
    });
    let args = [
        (
            "propose_screening_decision",
            json!({"project_id": fixture.project_id, "report_id": fixture.report_id, "stage": "title_abstract"}),
        ),
        (
            "propose_duplicate_merge",
            json!({"project_id": fixture.project_id, "source_record_id": fixture.record_id, "candidate_report_id": fixture.report_id}),
        ),
        (
            "propose_study_grouping",
            json!({"project_id": fixture.project_id, "report_id": fixture.report_id}),
        ),
        (
            "propose_classification",
            json!({"project_id": fixture.project_id, "study_id": fixture.study_id}),
        ),
        (
            "propose_extraction",
            json!({"project_id": fixture.project_id, "study_id": fixture.study_id}),
        ),
        (
            "propose_appraisal_answer",
            json!({"project_id": fixture.project_id, "report_id": fixture.report_id, "definition_id": "deepref-rct-generic", "definition_version": 1}),
        ),
    ];
    for (tool, tool_args) in args {
        let (status, body) =
            execute_with_state(state.clone(), fixture.project_id, tool, tool_args).await;
        assert_eq!(status, StatusCode::OK, "{tool} response: {body}");
        assert_eq!(body["kind"], "proposal", "{tool} response: {body}");
        assert!(body["proposal_id"].as_str().is_some());
    }
    assert_eq!(*calls.lock().expect("gateway calls lock"), 6);
    let pending: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM ai_proposals WHERE project_id=$1 AND status='pending'",
    )
    .bind(fixture.project_id)
    .fetch_one(&pool)
    .await
    .expect("pending proposal count");
    assert_eq!(pending, 6);
    let (design, revision): (Option<String>, i64) =
        sqlx::query_as("SELECT design,study_revision FROM studies WHERE project_id=$1 AND id=$2")
            .bind(fixture.project_id)
            .bind(fixture.study_id)
            .fetch_one(&pool)
            .await
            .expect("study state");
    assert!(design.is_none());
    assert_eq!(revision, 0);
    cleanup(&pool, fixture).await;
}

#[tokio::test]
async fn assistant_envelope_and_unsupported_actions_fail_closed() {
    let _guard = test_lock().lock().await;
    let Some(pool) = database().await else { return };
    let fixture = seed(&pool).await;
    let state = AppState::new(pool.clone());
    for body in [
        json!({"tool":"unknown_tool","args":{"project_id":fixture.project_id}}),
        json!({"tool":"get_report","args":{"project_id":fixture.project_id,"report_id":fixture.report_id},"extra":true}),
        json!({"tool":"get_report","args":{"project_id":fixture.project_id,"report_id":fixture.report_id,"sql":"DROP TABLE projects"}}),
        json!({"tool":"final_exclusion","args":{"project_id":fixture.project_id}}),
    ] {
        let response = router(state.clone(), &api_config())
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!(
                        "/projects/{}/assistant/tools/execute",
                        fixture.project_id
                    ))
                    .header("content-type", "application/json")
                    .body(Body::from(body.to_string()))
                    .expect("assistant request should be valid"),
            )
            .await
            .expect("assistant request should be handled");
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }
    let response = router(state, &api_config())
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!(
                    "/projects/{}/assistant/tools/execute",
                    fixture.project_id
                ))
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({
                        "tool":"get_report",
                        "args":{"project_id":fixture.other_project_id,"report_id":fixture.report_id}
                    })
                    .to_string(),
                ))
                .expect("assistant request should be valid"),
        )
        .await
        .expect("assistant request should be handled");
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    cleanup(&pool, fixture).await;
}
