use std::collections::HashMap;
use std::sync::OnceLock;

use axum::{
    body::{Body, to_bytes},
    http::{Request, Response, StatusCode},
};
use chrono::Utc;
use deepref_ai::{
    AiError, AiFuture, AiGateway, AiRunRecord, AiRunStatus, AiRunStore, AiTaskKind,
    GatewayCompletion, ModelParameters, ModelProfile, ResolvedModel, sha256_bytes,
};
use deepref_http_api::{config::ApiConfig, routes::router, state::AppState};
use sqlx::{PgPool, Row, postgres::PgPoolOptions};
use tower::ServiceExt;
use uuid::Uuid;

static TEST_LOCK: OnceLock<tokio::sync::Mutex<()>> = OnceLock::new();

fn test_lock() -> &'static tokio::sync::Mutex<()> {
    TEST_LOCK.get_or_init(tokio::sync::Mutex::default)
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
        "deepref-api-pr13-test",
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
    report_id: Uuid,
    study_id: Uuid,
}

#[derive(Debug, Clone, Copy)]
struct EvidenceFixture {
    document_id: Uuid,
    block_id: Uuid,
}

#[derive(Debug, Clone, Copy)]
struct ExtractionFields {
    sample_size: Uuid,
    blinded: Uuid,
    publication_date: Uuid,
    intervention: Uuid,
    notes: Uuid,
}

struct ProposalSeed {
    run_id: Uuid,
    proposal_type: &'static str,
    entity_type: &'static str,
    entity_id: Option<Uuid>,
    operation: &'static str,
    authority_tier: &'static str,
    task_kind: &'static str,
    target_report_id: Option<Uuid>,
    target_study_id: Option<Uuid>,
    payload: serde_json::Value,
}

async fn fixture(pool: &PgPool) -> Fixture {
    let fixture = Fixture {
        project_id: Uuid::new_v4(),
        report_id: Uuid::new_v4(),
        study_id: Uuid::new_v4(),
    };
    sqlx::query("INSERT INTO projects (id,name) VALUES ($1,'PR13 HTTP test')")
        .bind(fixture.project_id)
        .execute(pool)
        .await
        .expect("project inserts");
    sqlx::query(
        "INSERT INTO reports (id,title,abstract_text)
         VALUES ($1,'PR13 HTTP report','A report for PR13 integration tests')",
    )
    .bind(fixture.report_id)
    .execute(pool)
    .await
    .expect("report inserts");
    sqlx::query("INSERT INTO project_reports (project_id,report_id) VALUES ($1,$2)")
        .bind(fixture.project_id)
        .bind(fixture.report_id)
        .execute(pool)
        .await
        .expect("project report inserts");
    sqlx::query(
        "INSERT INTO studies
         (id,project_id,title,design_context,study_revision,updated_by_actor_kind,updated_by_actor_id)
         VALUES ($1,$2,'PR13 HTTP study','{}'::jsonb,0,'system','pr13-http-test')",
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
    .expect("study report inserts");
    fixture
}

async fn cleanup(pool: &PgPool, fixture: Fixture) {
    sqlx::query("DELETE FROM extraction_events WHERE project_id=$1")
        .bind(fixture.project_id)
        .execute(pool)
        .await
        .expect("extraction event cleanup");
    sqlx::query("DELETE FROM extraction_values WHERE project_id=$1")
        .bind(fixture.project_id)
        .execute(pool)
        .await
        .expect("extraction value cleanup");
    sqlx::query("DELETE FROM projects WHERE id=$1")
        .bind(fixture.project_id)
        .execute(pool)
        .await
        .expect("project cleanup");
    sqlx::query("DELETE FROM reports WHERE id=$1")
        .bind(fixture.report_id)
        .execute(pool)
        .await
        .expect("report cleanup");
}

async fn response_json(response: Response<Body>) -> serde_json::Value {
    let body = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("response body should be readable");
    serde_json::from_slice(&body).expect("response should be JSON")
}

fn route(profile: ModelProfile, provider: &str) -> ResolvedModel {
    ResolvedModel {
        profile,
        provider: provider.to_owned(),
        model: "pr13-test-model".to_owned(),
        model_version: "2026-pr13".to_owned(),
        parameters: ModelParameters::default(),
        route_id: Some(Uuid::new_v4()),
    }
}

fn source_hash() -> String {
    "b".repeat(64)
}

fn source_evidence(
    report_id: Uuid,
    evidence: EvidenceFixture,
    content_hash: impl Into<String>,
) -> serde_json::Value {
    serde_json::json!({
        "report_id": report_id,
        "document_id": evidence.document_id,
        "document_block_id": evidence.block_id,
        "page": 2,
        "parser_version": "parser.v2",
        "content_hash": content_hash.into()
    })
}

fn appraisal_evidence(
    evidence: EvidenceFixture,
    content_hash: impl Into<String>,
) -> serde_json::Value {
    serde_json::json!({
        "document_id": evidence.document_id,
        "document_block_id": evidence.block_id,
        "page": 2,
        "parser_version": "parser.v2",
        "content_hash": content_hash.into()
    })
}

async fn seed_document(pool: &PgPool, fixture: Fixture) -> EvidenceFixture {
    let evidence = EvidenceFixture {
        document_id: Uuid::new_v4(),
        block_id: Uuid::new_v4(),
    };
    sqlx::query(
        "INSERT INTO documents
         (id,project_id,report_id,object_key,content_hash,mime_type,byte_size,source,status,
          actor_kind,actor_id,active_parser_version,parser_version)
         VALUES ($1,$2,$3,$4,$5,'application/pdf',10,'upload','available',
                 'system','pr13-test','parser.v2','parser.v2')",
    )
    .bind(evidence.document_id)
    .bind(fixture.project_id)
    .bind(fixture.report_id)
    .bind(format!("documents/{}", evidence.document_id))
    .bind("a".repeat(64))
    .execute(pool)
    .await
    .expect("document inserts");
    sqlx::query(
        "INSERT INTO document_pages(document_id,parser_version,page_number,width,height,active)
         VALUES ($1,'parser.v2',2,600,800,true)",
    )
    .bind(evidence.document_id)
    .execute(pool)
    .await
    .expect("document page inserts");
    sqlx::query(
        "INSERT INTO document_blocks
         (id,document_id,parser_version,page_number,kind,section_path,ordinal,text,content_hash,active)
         VALUES ($1,$2,'parser.v2',2,'text',ARRAY['Results'],0,
                 'The trial reported the reviewed evidence.', $3, true)",
    )
    .bind(evidence.block_id)
    .bind(evidence.document_id)
    .bind(source_hash())
    .execute(pool)
    .await
    .expect("document block inserts");
    evidence
}

async fn save_run(
    pool: &PgPool,
    fixture: Fixture,
    task_kind: AiTaskKind,
    prompt_version: &str,
) -> Uuid {
    let run_id = Uuid::new_v4();
    let profile = match task_kind {
        AiTaskKind::StudyGrouping => ModelProfile::Reasoning,
        AiTaskKind::AppraisalPrefill | AiTaskKind::DataExtraction => {
            ModelProfile::LongContextReasoning
        }
        _ => ModelProfile::Reasoning,
    };
    let store = deepref_postgres::PostgresAiStore::new(pool);
    store
        .save_run(AiRunRecord {
            id: run_id,
            project_id: Some(fixture.project_id.into()),
            task_kind,
            route: route(profile, "pr13-decision-provider"),
            prompt_version: prompt_version.to_owned(),
            prompt_hash: sha256_bytes(format!("prompt:{run_id}").as_bytes()),
            schema_version: format!("{prompt_version}.schema"),
            schema_hash: sha256_bytes(format!("schema:{run_id}").as_bytes()),
            input_hash: sha256_bytes(format!("input:{run_id}").as_bytes()),
            reuse_hash: sha256_bytes(format!("reuse:{run_id}").as_bytes()),
            protocol_hash: None,
            document_hash: None,
            evidence_hash: Some(sha256_bytes(format!("evidence:{run_id}").as_bytes())),
            evidence_refs: Vec::new(),
            usage: Default::default(),
            cost_micros: Some(1),
            output: Some(serde_json::json!({"status":"completed"})),
            status: AiRunStatus::Completed,
            error: None,
            parent_automation_run_id: None,
            created_at: Utc::now(),
            completed_at: Some(Utc::now()),
        })
        .await
        .expect("AI run inserts");
    run_id
}

fn with_kind(kind: &str, payload: serde_json::Value) -> serde_json::Value {
    let mut object = payload
        .as_object()
        .expect("typed proposal payload is an object")
        .clone();
    object.insert(
        "kind".to_owned(),
        serde_json::Value::String(kind.to_owned()),
    );
    serde_json::Value::Object(object)
}

async fn insert_proposal(pool: &PgPool, fixture: Fixture, seed: ProposalSeed) -> Uuid {
    let proposal_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO ai_proposals
         (id,project_id,ai_run_id,proposal_type,payload,status,entity_type,entity_id,operation,
          model_run_id,authority_tier,task_kind,target_report_id,target_study_id)
         VALUES ($1,$2,$3,$4,$5,'pending',$6,$7,$8,$3,$9,$10,$11,$12)",
    )
    .bind(proposal_id)
    .bind(fixture.project_id)
    .bind(seed.run_id)
    .bind(seed.proposal_type)
    .bind(seed.payload)
    .bind(seed.entity_type)
    .bind(seed.entity_id)
    .bind(seed.operation)
    .bind(seed.authority_tier)
    .bind(seed.task_kind)
    .bind(seed.target_report_id)
    .bind(seed.target_study_id)
    .execute(pool)
    .await
    .expect("AI proposal inserts");
    proposal_id
}

fn appraisal_payload(
    fixture: Fixture,
    evidence: EvidenceFixture,
    allocation: &str,
    evidence_hash: impl Into<String>,
) -> serde_json::Value {
    serde_json::json!({
        "report_id": fixture.report_id,
        "definition_id": "deepref-rct-generic",
        "definition_version": 1,
        "answers": [
            {
                "question_id": "allocation_description",
                "answer": {"kind": "enum", "value": allocation},
                "rationale": "The reviewed allocation description supports this answer.",
                "evidence": [appraisal_evidence(evidence, evidence_hash)]
            },
            {
                "question_id": "outcome_measure_prespecified",
                "answer": {"kind": "boolean", "value": false},
                "rationale": "The report does not state that the outcome was prespecified.",
                "evidence": []
            }
        ],
        "domain_judgments": {
            "allocation": "low_concern",
            "outcome_reporting": "some_concern"
        },
        "overall_judgment": "some_concern"
    })
}

async fn insert_appraisal_proposal(
    pool: &PgPool,
    fixture: Fixture,
    payload: serde_json::Value,
) -> Uuid {
    let run_id = save_run(
        pool,
        fixture,
        AiTaskKind::AppraisalPrefill,
        "appraisal.prefill.v1",
    )
    .await;
    insert_proposal(
        pool,
        fixture,
        ProposalSeed {
            run_id,
            proposal_type: "appraisal_prefill",
            entity_type: "appraisal_report",
            entity_id: Some(fixture.report_id),
            operation: "appraisal_prefill",
            authority_tier: "scientific_conclusion",
            task_kind: "appraisal_prefill",
            target_report_id: Some(fixture.report_id),
            target_study_id: None,
            payload,
        },
    )
    .await
}

async fn seed_extraction_fields(pool: &PgPool, fixture: Fixture) -> ExtractionFields {
    let fields = ExtractionFields {
        sample_size: Uuid::new_v4(),
        blinded: Uuid::new_v4(),
        publication_date: Uuid::new_v4(),
        intervention: Uuid::new_v4(),
        notes: Uuid::new_v4(),
    };
    for (id, key, label, value_type, required) in [
        (
            fields.sample_size,
            "sample_size",
            "Sample size",
            "number",
            true,
        ),
        (fields.blinded, "blinded", "Blinded", "boolean", true),
        (
            fields.publication_date,
            "publication_date",
            "Publication date",
            "date",
            true,
        ),
        (
            fields.intervention,
            "intervention",
            "Intervention",
            "text",
            false,
        ),
        (fields.notes, "notes", "Notes", "text", false),
    ] {
        sqlx::query(
            "INSERT INTO extraction_field_definitions
             (id,project_id,version,field_key,label,value_type,required)
             VALUES ($1,$2,1,$3,$4,$5,$6)",
        )
        .bind(id)
        .bind(fixture.project_id)
        .bind(key)
        .bind(label)
        .bind(value_type)
        .bind(required)
        .execute(pool)
        .await
        .expect("extraction field definition inserts");
    }
    fields
}

fn extraction_payload(
    fixture: Fixture,
    evidence: EvidenceFixture,
    fields: ExtractionFields,
    sample_size: f64,
    blinded_hash: impl Into<String>,
    required_insufficient: bool,
) -> serde_json::Value {
    let sample_field = if required_insufficient {
        serde_json::json!({
            "kind": "insufficient_evidence",
            "field_id": fields.sample_size,
            "field_version": 1,
            "rationale": "The report does not provide a usable sample size."
        })
    } else {
        serde_json::json!({
            "kind": "value",
            "field_id": fields.sample_size,
            "field_version": 1,
            "value": {"kind": "number", "value": sample_size},
            "rationale": "The reviewed report states the sample size.",
            "source": source_evidence(fixture.report_id, evidence, source_hash())
        })
    };
    serde_json::json!({
        "study_id": fixture.study_id,
        "fields": [
            sample_field,
            {
                "kind": "value",
                "field_id": fields.blinded,
                "field_version": 1,
                "value": {"kind": "boolean", "value": true},
                "rationale": "The methods state that allocation was blinded.",
                "source": source_evidence(fixture.report_id, evidence, blinded_hash.into())
            },
            {
                "kind": "value",
                "field_id": fields.publication_date,
                "field_version": 1,
                "value": {"kind": "date", "value": "2024-01-02"},
                "rationale": "The report identifies its publication date.",
                "source": source_evidence(fixture.report_id, evidence, source_hash())
            },
            {
                "kind": "value",
                "field_id": fields.intervention,
                "field_version": 1,
                "value": {"kind": "text", "value": "standard care"},
                "rationale": "The report names the intervention.",
                "source": source_evidence(fixture.report_id, evidence, source_hash())
            },
            {
                "kind": "insufficient_evidence",
                "field_id": fields.notes,
                "field_version": 1,
                "rationale": "The report does not provide a usable note for this optional field."
            }
        ]
    })
}

async fn insert_extraction_proposal(
    pool: &PgPool,
    fixture: Fixture,
    payload: serde_json::Value,
) -> Uuid {
    let run_id = save_run(
        pool,
        fixture,
        AiTaskKind::DataExtraction,
        "extraction.data.v1",
    )
    .await;
    insert_proposal(
        pool,
        fixture,
        ProposalSeed {
            run_id,
            proposal_type: "data_extraction",
            entity_type: "extraction_study",
            entity_id: Some(fixture.study_id),
            operation: "data_extraction",
            authority_tier: "scientific_conclusion",
            task_kind: "data_extraction",
            target_report_id: None,
            target_study_id: Some(fixture.study_id),
            payload,
        },
    )
    .await
}

#[derive(Clone)]
struct FailingGateway;

impl AiGateway for FailingGateway {
    fn complete<'a>(
        &'a self,
        _request: deepref_ai::CompletionRequest,
    ) -> AiFuture<'a, GatewayCompletion> {
        Box::pin(async {
            Err(AiError::Gateway(
                "PR13 test provider unavailable".to_owned(),
            ))
        })
    }
}

async fn insert_grouping_proposal(pool: &PgPool, fixture: Fixture) -> Uuid {
    let payload = grouping_payload(
        fixture,
        fixture.study_id,
        Some(0),
        serde_json::json!({"kind": "new_study", "title": "A new study"}),
        serde_json::json!([{
            "kind": "report_metadata",
            "report_id": fixture.report_id,
            "field": "title",
            "content_hash": sha256_bytes(b"PR13 HTTP report")
        }]),
    );
    insert_grouping_proposal_with_payload(pool, fixture, payload).await
}

fn grouping_payload(
    fixture: Fixture,
    expected_previous_study_id: Uuid,
    expected_previous_study_revision: Option<i64>,
    choice: serde_json::Value,
    provenance: serde_json::Value,
) -> serde_json::Value {
    serde_json::json!({
        "report_id": fixture.report_id,
        "expected_previous_study_id": expected_previous_study_id,
        "expected_previous_study_revision": expected_previous_study_revision,
        "choice": choice,
        "rationale": "The reviewer-confirmed metadata supports this grouping decision.",
        "provenance": provenance,
        "uncertainties": []
    })
}

async fn insert_grouping_proposal_with_payload(
    pool: &PgPool,
    fixture: Fixture,
    payload: serde_json::Value,
) -> Uuid {
    let run_id = save_run(
        pool,
        fixture,
        AiTaskKind::StudyGrouping,
        "study.grouping.v1",
    )
    .await;
    let target_study_id = payload
        .get("choice")
        .and_then(|choice| choice.get("study_id"))
        .and_then(serde_json::Value::as_str)
        .and_then(|value| Uuid::parse_str(value).ok());
    insert_proposal(
        pool,
        fixture,
        ProposalSeed {
            run_id,
            proposal_type: "study_grouping_suggestion",
            entity_type: "study_grouping_report",
            entity_id: Some(fixture.report_id),
            operation: "study_grouping_suggestion",
            authority_tier: "workflow_suggestion",
            task_kind: "study_grouping",
            target_report_id: Some(fixture.report_id),
            target_study_id,
            payload,
        },
    )
    .await
}

async fn decision_request(
    pool: &PgPool,
    project_id: Uuid,
    proposal_id: Uuid,
    body: serde_json::Value,
) -> Response<Body> {
    router(AppState::core(pool.clone()), &api_config())
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!(
                    "/projects/{project_id}/ai/proposals/{proposal_id}/decision"
                ))
                .header("content-type", "application/json")
                .header("x-actor-kind", "user")
                .header("x-actor-id", "pr13-http-reviewer")
                .body(Body::from(body.to_string()))
                .expect("decision request should be valid"),
        )
        .await
        .expect("decision request should be handled")
}

async fn membership_request(
    pool: &PgPool,
    project_id: Uuid,
    report_id: Uuid,
    body: serde_json::Value,
) -> Response<Body> {
    router(AppState::core(pool.clone()), &api_config())
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri(format!("/projects/{project_id}/reports/{report_id}/study"))
                .header("content-type", "application/json")
                .header("x-actor-kind", "user")
                .header("x-actor-id", "pr13-http-reviewer")
                .body(Body::from(body.to_string()))
                .expect("membership request should be valid"),
        )
        .await
        .expect("membership request should be handled")
}

async fn seed_screening_state(pool: &PgPool, fixture: Fixture) {
    sqlx::query(
        "INSERT INTO screening_state
         (project_id,report_id,title_abstract_status,full_text_status,final_status,revision)
         VALUES ($1,$2,'include','not_required','include',7)",
    )
    .bind(fixture.project_id)
    .bind(fixture.report_id)
    .execute(pool)
    .await
    .expect("screening state inserts");
}

#[tokio::test]
async fn appraisal_reviewed_acceptance_persists_edited_answer_and_audit_without_screening_changes()
{
    let _guard = test_lock().lock().await;
    let Some(pool) = database().await else { return };
    let fixture = fixture(&pool).await;
    let evidence = seed_document(&pool, fixture).await;
    seed_screening_state(&pool, fixture).await;
    let before_screening: (String, String, String, i64) = sqlx::query_as(
        "SELECT title_abstract_status,full_text_status,final_status,revision
         FROM screening_state WHERE project_id=$1 AND report_id=$2",
    )
    .bind(fixture.project_id)
    .bind(fixture.report_id)
    .fetch_one(&pool)
    .await
    .expect("screening state before appraisal");
    let original = appraisal_payload(fixture, evidence, "no", source_hash());
    let proposal_id = insert_appraisal_proposal(&pool, fixture, original).await;
    let edited = appraisal_payload(fixture, evidence, "yes", source_hash());

    let response = decision_request(
        &pool,
        fixture.project_id,
        proposal_id,
        serde_json::json!({
            "decision": "accept",
            "reason": "The reviewer corrected the allocation answer.",
            "reviewed_payload": with_kind("appraisal_prefill", edited.clone())
        }),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = response_json(response).await;
    assert_eq!(body["proposal"]["status"], "accepted");

    let assessment = sqlx::query(
        "SELECT id,responses,actor_kind,actor_id
         FROM appraisal_assessments WHERE project_id=$1 AND report_id=$2",
    )
    .bind(fixture.project_id)
    .bind(fixture.report_id)
    .fetch_one(&pool)
    .await
    .expect("accepted appraisal assessment");
    let assessment_id: Uuid = assessment.get("id");
    let responses: serde_json::Value = assessment.get("responses");
    assert_eq!(
        responses,
        serde_json::json!({
            "allocation_description": "yes",
            "outcome_measure_prespecified": false
        })
    );
    assert_eq!(assessment.get::<String, _>("actor_kind"), "user");
    assert_eq!(
        assessment.get::<String, _>("actor_id"),
        "pr13-http-reviewer"
    );

    let appraisal_event: serde_json::Value =
        sqlx::query_scalar("SELECT payload FROM appraisal_events WHERE assessment_id=$1")
            .bind(assessment_id)
            .fetch_one(&pool)
            .await
            .expect("appraisal completion event");
    assert_eq!(appraisal_event["report_id"], fixture.report_id.to_string());
    assert_eq!(appraisal_event["definition_id"], "deepref-rct-generic");
    let evidence_row = sqlx::query(
        "SELECT e.document_id,e.block_id,b.page_number,b.parser_version,b.content_hash,
                d.active_parser_version,p.active
         FROM appraisal_assessment_evidence e
         JOIN document_blocks b ON b.document_id=e.document_id AND b.id=e.block_id
         JOIN documents d ON d.id=e.document_id
         JOIN document_pages p ON p.document_id=b.document_id
           AND p.parser_version=b.parser_version AND p.page_number=b.page_number
         WHERE e.assessment_id=$1",
    )
    .bind(assessment_id)
    .fetch_one(&pool)
    .await
    .expect("appraisal evidence provenance");
    assert_eq!(
        evidence_row.get::<Uuid, _>("document_id"),
        evidence.document_id
    );
    assert_eq!(evidence_row.get::<Uuid, _>("block_id"), evidence.block_id);
    assert_eq!(evidence_row.get::<i32, _>("page_number"), 2);
    assert_eq!(evidence_row.get::<String, _>("parser_version"), "parser.v2");
    assert_eq!(evidence_row.get::<String, _>("content_hash"), source_hash());
    assert_eq!(
        evidence_row.get::<String, _>("active_parser_version"),
        "parser.v2"
    );
    assert!(evidence_row.get::<bool, _>("active"));

    let review_event: serde_json::Value = sqlx::query_scalar(
        "SELECT payload FROM review_events
         WHERE project_id=$1 AND aggregate_type='ai_proposal' AND aggregate_id=$2",
    )
    .bind(fixture.project_id)
    .bind(proposal_id)
    .fetch_one(&pool)
    .await
    .expect("appraisal proposal review audit");
    assert_eq!(review_event["status"], "accepted");
    assert_eq!(review_event["applied_payload"], edited);
    let review_events: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM review_events
         WHERE project_id=$1 AND aggregate_type='ai_proposal' AND aggregate_id=$2",
    )
    .bind(fixture.project_id)
    .bind(proposal_id)
    .fetch_one(&pool)
    .await
    .expect("appraisal proposal review audit count");
    assert_eq!(review_events, 1);

    let after_screening: (String, String, String, i64) = sqlx::query_as(
        "SELECT title_abstract_status,full_text_status,final_status,revision
         FROM screening_state WHERE project_id=$1 AND report_id=$2",
    )
    .bind(fixture.project_id)
    .bind(fixture.report_id)
    .fetch_one(&pool)
    .await
    .expect("screening state after appraisal");
    assert_eq!(after_screening, before_screening);
    let screening_events: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM screening_events WHERE project_id=$1 AND report_id=$2",
    )
    .bind(fixture.project_id)
    .bind(fixture.report_id)
    .fetch_one(&pool)
    .await
    .expect("screening event count after appraisal");
    assert_eq!(screening_events, 0);

    let response = decision_request(
        &pool,
        fixture.project_id,
        proposal_id,
        serde_json::json!({"decision": "accept", "reason": "Replay must conflict."}),
    )
    .await;
    assert_eq!(response.status(), StatusCode::CONFLICT);
    cleanup(&pool, fixture).await;
}

#[tokio::test]
async fn extraction_reviewed_acceptance_persists_typed_values_provenance_and_audit() {
    let _guard = test_lock().lock().await;
    let Some(pool) = database().await else { return };
    let fixture = fixture(&pool).await;
    let evidence = seed_document(&pool, fixture).await;
    let fields = seed_extraction_fields(&pool, fixture).await;
    let original = extraction_payload(fixture, evidence, fields, 42.0, source_hash(), false);
    let proposal_id = insert_extraction_proposal(&pool, fixture, original).await;
    let edited = extraction_payload(fixture, evidence, fields, 84.5, source_hash(), false);

    let response = decision_request(
        &pool,
        fixture.project_id,
        proposal_id,
        serde_json::json!({
            "decision": "accept",
            "reason": "The reviewer corrected the sample size.",
            "reviewed_payload": with_kind("data_extraction", edited.clone())
        }),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = response_json(response).await;
    assert_eq!(body["proposal"]["status"], "accepted");

    let values_response = router(AppState::core(pool.clone()), &api_config())
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(format!(
                    "/projects/{}/studies/{}/extraction",
                    fixture.project_id, fixture.study_id
                ))
                .body(Body::empty())
                .expect("extraction values request should be valid"),
        )
        .await
        .expect("extraction values request should be handled");
    assert_eq!(values_response.status(), StatusCode::OK);
    let values = response_json(values_response).await;
    let values = values.as_array().expect("extraction values array");
    assert_eq!(values.len(), 4);
    let sample_value = values
        .iter()
        .find(|value| value["field_definition_id"] == fields.sample_size.to_string())
        .expect("edited sample size value");
    assert_eq!(
        sample_value["value"],
        serde_json::json!({"kind": "number", "value": 84.5})
    );
    assert_eq!(
        sample_value["source_document_id"],
        evidence.document_id.to_string()
    );
    assert_eq!(
        sample_value["source_block_id"],
        evidence.block_id.to_string()
    );
    assert_eq!(sample_value["source_page"], 2);
    assert_eq!(sample_value["source_parser_version"], "parser.v2");
    assert_eq!(sample_value["source_content_hash"], source_hash());
    assert_eq!(sample_value["approved_by_actor_kind"], "user");
    assert_eq!(sample_value["approved_by_actor_id"], "pr13-http-reviewer");
    assert!(values.iter().any(|value| {
        value["field_definition_id"] == fields.blinded.to_string()
            && value["value"] == serde_json::json!({"kind": "boolean", "value": true})
    }));
    assert!(values.iter().any(|value| {
        value["field_definition_id"] == fields.publication_date.to_string()
            && value["value"] == serde_json::json!({"kind": "date", "value": "2024-01-02"})
    }));
    assert!(values.iter().any(|value| {
        value["field_definition_id"] == fields.intervention.to_string()
            && value["value"] == serde_json::json!({"kind": "text", "value": "standard care"})
    }));
    assert!(
        !values
            .iter()
            .any(|value| value["field_definition_id"] == fields.notes.to_string())
    );

    let extraction_event: serde_json::Value = sqlx::query_scalar(
        "SELECT payload FROM extraction_events WHERE project_id=$1 AND proposal_id=$2",
    )
    .bind(fixture.project_id)
    .bind(proposal_id)
    .fetch_one(&pool)
    .await
    .expect("extraction approval event");
    assert_eq!(extraction_event, edited);
    let (event_actor_kind, event_actor_id): (String, String) = sqlx::query_as(
        "SELECT actor_kind,actor_id FROM extraction_events
         WHERE project_id=$1 AND proposal_id=$2",
    )
    .bind(fixture.project_id)
    .bind(proposal_id)
    .fetch_one(&pool)
    .await
    .expect("extraction event actor");
    assert_eq!(event_actor_kind, "user");
    assert_eq!(event_actor_id, "pr13-http-reviewer");
    let review_event: serde_json::Value = sqlx::query_scalar(
        "SELECT payload FROM review_events
         WHERE project_id=$1 AND aggregate_type='ai_proposal' AND aggregate_id=$2",
    )
    .bind(fixture.project_id)
    .bind(proposal_id)
    .fetch_one(&pool)
    .await
    .expect("extraction proposal review audit");
    assert_eq!(review_event["status"], "accepted");
    assert_eq!(review_event["applied_payload"], edited);
    let accepted_values: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM extraction_values WHERE project_id=$1 AND study_id=$2",
    )
    .bind(fixture.project_id)
    .bind(fixture.study_id)
    .fetch_one(&pool)
    .await
    .expect("accepted extraction value count");
    assert_eq!(accepted_values, 4);

    let moved_study_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO studies
         (id,project_id,title,design_context,study_revision,updated_by_actor_kind,updated_by_actor_id)
         VALUES ($1,$2,'PR13 moved extraction study','{}'::jsonb,0,'system','pr13-http-test')",
    )
    .bind(moved_study_id)
    .bind(fixture.project_id)
    .execute(&pool)
    .await
    .expect("target study for extraction move");
    let response = membership_request(
        &pool,
        fixture.project_id,
        fixture.report_id,
        serde_json::json!({
            "study_id": moved_study_id,
            "role": "report_of_study",
            "expected_revision": 0,
            "expected_previous_study_id": fixture.study_id,
            "expected_previous_study_revision": 0
        }),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let response = membership_request(
        &pool,
        fixture.project_id,
        fixture.report_id,
        serde_json::json!({
            "study_id": null,
            "expected_revision": 1,
            "expected_previous_study_id": null,
            "expected_previous_study_revision": null
        }),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let preserved_values: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM extraction_values
         WHERE project_id=$1 AND study_id=$2 AND source_document_id=$3
           AND source_block_id=$4 AND source_page=2
           AND source_parser_version='parser.v2' AND source_content_hash=$5",
    )
    .bind(fixture.project_id)
    .bind(fixture.study_id)
    .bind(evidence.document_id)
    .bind(evidence.block_id)
    .bind(source_hash())
    .fetch_one(&pool)
    .await
    .expect("extraction provenance after membership changes");
    assert_eq!(preserved_values, 4);

    let response = decision_request(
        &pool,
        fixture.project_id,
        proposal_id,
        serde_json::json!({"decision": "accept", "reason": "Replay must conflict."}),
    )
    .await;
    assert_eq!(response.status(), StatusCode::CONFLICT);
    cleanup(&pool, fixture).await;
}

#[tokio::test]
async fn appraisal_acceptance_rolls_back_on_inactive_or_wrong_project_evidence_and_keeps_pending() {
    let _guard = test_lock().lock().await;
    let Some(pool) = database().await else { return };
    let fixture = fixture(&pool).await;
    let evidence = seed_document(&pool, fixture).await;
    let payload = appraisal_payload(fixture, evidence, "no", source_hash());
    let proposal_id = insert_appraisal_proposal(&pool, fixture, payload.clone()).await;
    sqlx::query("UPDATE document_blocks SET active=false WHERE id=$1")
        .bind(evidence.block_id)
        .execute(&pool)
        .await
        .expect("inactivate appraisal evidence block");

    let response = decision_request(
        &pool,
        fixture.project_id,
        proposal_id,
        serde_json::json!({
            "decision": "accept",
            "reason": "Inactive evidence must not be accepted.",
            "reviewed_payload": with_kind("appraisal_prefill", payload)
        }),
    )
    .await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let status: String = sqlx::query_scalar("SELECT status FROM ai_proposals WHERE id=$1")
        .bind(proposal_id)
        .fetch_one(&pool)
        .await
        .expect("pending appraisal proposal status");
    assert_eq!(status, "pending");

    let wrong_project_id = Uuid::new_v4();
    sqlx::query("INSERT INTO projects (id,name) VALUES ($1,'PR13 wrong-project evidence')")
        .bind(wrong_project_id)
        .execute(&pool)
        .await
        .expect("wrong-project evidence project inserts");
    sqlx::query("INSERT INTO project_reports (project_id,report_id) VALUES ($1,$2)")
        .bind(wrong_project_id)
        .bind(fixture.report_id)
        .execute(&pool)
        .await
        .expect("wrong-project evidence report membership inserts");
    sqlx::query("UPDATE documents SET project_id=$2 WHERE id=$1")
        .bind(evidence.document_id)
        .bind(wrong_project_id)
        .execute(&pool)
        .await
        .expect("move evidence document to wrong project");
    sqlx::query("UPDATE document_blocks SET active=true WHERE id=$1")
        .bind(evidence.block_id)
        .execute(&pool)
        .await
        .expect("reactivate wrong-project evidence block");
    let wrong_project_payload = appraisal_payload(fixture, evidence, "yes", source_hash());
    let wrong_project_proposal =
        insert_appraisal_proposal(&pool, fixture, wrong_project_payload.clone()).await;
    let response = decision_request(
        &pool,
        fixture.project_id,
        wrong_project_proposal,
        serde_json::json!({
            "decision": "accept",
            "reason": "Evidence from another project must not be accepted.",
            "reviewed_payload": with_kind("appraisal_prefill", wrong_project_payload)
        }),
    )
    .await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let statuses: Vec<String> = sqlx::query_scalar(
        "SELECT status FROM ai_proposals WHERE project_id=$1 ORDER BY created_at,id",
    )
    .bind(fixture.project_id)
    .fetch_all(&pool)
    .await
    .expect("pending appraisal proposal statuses");
    assert_eq!(statuses, vec!["pending", "pending"]);
    let assessments: i64 =
        sqlx::query_scalar("SELECT count(*) FROM appraisal_assessments WHERE project_id=$1")
            .bind(fixture.project_id)
            .fetch_one(&pool)
            .await
            .expect("appraisal assessment rollback count");
    let appraisal_events: i64 =
        sqlx::query_scalar("SELECT count(*) FROM appraisal_events WHERE project_id=$1")
            .bind(fixture.project_id)
            .fetch_one(&pool)
            .await
            .expect("appraisal event rollback count");
    let review_events: i64 =
        sqlx::query_scalar("SELECT count(*) FROM review_events WHERE project_id=$1")
            .bind(fixture.project_id)
            .fetch_one(&pool)
            .await
            .expect("review event rollback count");
    assert_eq!(assessments, 0);
    assert_eq!(appraisal_events, 0);
    assert_eq!(review_events, 0);
    sqlx::query("DELETE FROM documents WHERE id=$1")
        .bind(evidence.document_id)
        .execute(&pool)
        .await
        .expect("wrong-project evidence document cleanup");
    sqlx::query("DELETE FROM projects WHERE id=$1")
        .bind(wrong_project_id)
        .execute(&pool)
        .await
        .expect("wrong-project evidence project cleanup");
    cleanup(&pool, fixture).await;
}

#[tokio::test]
async fn extraction_acceptance_rolls_back_on_wrong_hash_and_required_insufficiency() {
    let _guard = test_lock().lock().await;
    let Some(pool) = database().await else { return };
    let fixture = fixture(&pool).await;
    let evidence = seed_document(&pool, fixture).await;
    let fields = seed_extraction_fields(&pool, fixture).await;
    let original = extraction_payload(fixture, evidence, fields, 42.0, source_hash(), false);
    let wrong_hash_proposal = insert_extraction_proposal(&pool, fixture, original).await;
    let wrong_hash = extraction_payload(fixture, evidence, fields, 84.5, "f".repeat(64), false);
    let response = decision_request(
        &pool,
        fixture.project_id,
        wrong_hash_proposal,
        serde_json::json!({
            "decision": "accept",
            "reason": "The stale source hash must conflict.",
            "reviewed_payload": with_kind("data_extraction", wrong_hash)
        }),
    )
    .await;
    assert_eq!(response.status(), StatusCode::CONFLICT);

    let required_insufficient =
        extraction_payload(fixture, evidence, fields, 84.5, source_hash(), true);
    let insufficient_proposal =
        insert_extraction_proposal(&pool, fixture, required_insufficient.clone()).await;
    let response = decision_request(
        &pool,
        fixture.project_id,
        insufficient_proposal,
        serde_json::json!({
            "decision": "accept",
            "reason": "A required insufficient result must conflict.",
            "reviewed_payload": with_kind("data_extraction", required_insufficient)
        }),
    )
    .await;
    assert_eq!(response.status(), StatusCode::CONFLICT);

    let statuses: Vec<String> = sqlx::query_scalar(
        "SELECT status FROM ai_proposals WHERE project_id=$1 ORDER BY created_at,id",
    )
    .bind(fixture.project_id)
    .fetch_all(&pool)
    .await
    .expect("pending extraction proposal statuses");
    assert_eq!(statuses, vec!["pending", "pending"]);
    let values: i64 =
        sqlx::query_scalar("SELECT count(*) FROM extraction_values WHERE project_id=$1")
            .bind(fixture.project_id)
            .fetch_one(&pool)
            .await
            .expect("extraction rollback value count");
    let extraction_events: i64 =
        sqlx::query_scalar("SELECT count(*) FROM extraction_events WHERE project_id=$1")
            .bind(fixture.project_id)
            .fetch_one(&pool)
            .await
            .expect("extraction rollback event count");
    let review_events: i64 =
        sqlx::query_scalar("SELECT count(*) FROM review_events WHERE project_id=$1")
            .bind(fixture.project_id)
            .fetch_one(&pool)
            .await
            .expect("extraction review rollback event count");
    assert_eq!(values, 0);
    assert_eq!(extraction_events, 0);
    assert_eq!(review_events, 0);
    cleanup(&pool, fixture).await;
}

#[tokio::test]
async fn extraction_rejection_writes_only_one_review_event_and_no_scientific_rows() {
    let _guard = test_lock().lock().await;
    let Some(pool) = database().await else { return };
    let fixture = fixture(&pool).await;
    let evidence = seed_document(&pool, fixture).await;
    let fields = seed_extraction_fields(&pool, fixture).await;
    let payload = extraction_payload(fixture, evidence, fields, 42.0, source_hash(), false);
    let proposal_id = insert_extraction_proposal(&pool, fixture, payload).await;
    let response = decision_request(
        &pool,
        fixture.project_id,
        proposal_id,
        serde_json::json!({
            "decision": "reject",
            "reason": "The reviewer rejected this extraction proposal."
        }),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = response_json(response).await;
    assert_eq!(body["proposal"]["status"], "rejected");
    let values: i64 =
        sqlx::query_scalar("SELECT count(*) FROM extraction_values WHERE project_id=$1")
            .bind(fixture.project_id)
            .fetch_one(&pool)
            .await
            .expect("rejected extraction value count");
    let extraction_events: i64 =
        sqlx::query_scalar("SELECT count(*) FROM extraction_events WHERE project_id=$1")
            .bind(fixture.project_id)
            .fetch_one(&pool)
            .await
            .expect("rejected extraction event count");
    let review_event: serde_json::Value = sqlx::query_scalar(
        "SELECT payload FROM review_events
         WHERE project_id=$1 AND aggregate_type='ai_proposal' AND aggregate_id=$2",
    )
    .bind(fixture.project_id)
    .bind(proposal_id)
    .fetch_one(&pool)
    .await
    .expect("rejected proposal review event");
    assert_eq!(values, 0);
    assert_eq!(extraction_events, 0);
    assert_eq!(review_event["status"], "rejected");
    assert_eq!(review_event["applied_payload"], serde_json::Value::Null);
    let review_event_count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM review_events
         WHERE project_id=$1 AND aggregate_type='ai_proposal' AND aggregate_id=$2",
    )
    .bind(fixture.project_id)
    .bind(proposal_id)
    .fetch_one(&pool)
    .await
    .expect("rejected proposal review event count");
    assert_eq!(review_event_count, 1);

    let response = decision_request(
        &pool,
        fixture.project_id,
        proposal_id,
        serde_json::json!({"decision": "reject", "reason": "Replay must conflict."}),
    )
    .await;
    assert_eq!(response.status(), StatusCode::CONFLICT);
    cleanup(&pool, fixture).await;
}

#[tokio::test]
async fn accepted_study_grouping_creates_membership_and_audited_study_events() {
    let _guard = test_lock().lock().await;
    let Some(pool) = database().await else { return };
    let fixture = fixture(&pool).await;
    let payload = grouping_payload(
        fixture,
        fixture.study_id,
        Some(0),
        serde_json::json!({"kind": "new_study", "title": "Accepted PR13 study"}),
        serde_json::json!([{
            "kind": "report_metadata",
            "report_id": fixture.report_id,
            "field": "title",
            "content_hash": sha256_bytes(b"PR13 HTTP report")
        }]),
    );
    let proposal_id = insert_grouping_proposal_with_payload(&pool, fixture, payload.clone()).await;
    let response = decision_request(
        &pool,
        fixture.project_id,
        proposal_id,
        serde_json::json!({"decision": "accept", "reason": "The reviewer accepted the grouping."}),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = response_json(response).await;
    assert_eq!(body["proposal"]["status"], "accepted");
    let assigned_study_id: Uuid = sqlx::query_scalar(
        "SELECT study_id FROM study_reports WHERE project_id=$1 AND report_id=$2",
    )
    .bind(fixture.project_id)
    .bind(fixture.report_id)
    .fetch_one(&pool)
    .await
    .expect("accepted grouping membership");
    assert_ne!(assigned_study_id, fixture.study_id);
    let study_created: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM study_events WHERE project_id=$1 AND study_id=$2
         AND event_type='study_created'",
    )
    .bind(fixture.project_id)
    .bind(assigned_study_id)
    .fetch_one(&pool)
    .await
    .expect("accepted grouping study creation event");
    let report_moved: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM study_events WHERE project_id=$1 AND study_id=$2
         AND report_id=$3 AND event_type='report_moved'",
    )
    .bind(fixture.project_id)
    .bind(assigned_study_id)
    .bind(fixture.report_id)
    .fetch_one(&pool)
    .await
    .expect("accepted grouping move event");
    assert_eq!(study_created, 1);
    assert_eq!(report_moved, 1);
    let review_event: serde_json::Value = sqlx::query_scalar(
        "SELECT payload FROM review_events
         WHERE project_id=$1 AND aggregate_type='ai_proposal' AND aggregate_id=$2",
    )
    .bind(fixture.project_id)
    .bind(proposal_id)
    .fetch_one(&pool)
    .await
    .expect("accepted grouping review audit");
    assert_eq!(review_event["status"], "accepted");
    assert_eq!(review_event["applied_payload"], payload);
    let review_events: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM review_events
         WHERE project_id=$1 AND aggregate_type='ai_proposal' AND aggregate_id=$2",
    )
    .bind(fixture.project_id)
    .bind(proposal_id)
    .fetch_one(&pool)
    .await
    .expect("accepted grouping review audit count");
    assert_eq!(review_events, 1);

    let response = decision_request(
        &pool,
        fixture.project_id,
        proposal_id,
        serde_json::json!({"decision": "accept", "reason": "Replay must conflict."}),
    )
    .await;
    assert_eq!(response.status(), StatusCode::CONFLICT);
    cleanup(&pool, fixture).await;
}

#[tokio::test]
async fn stale_grouping_previous_or_target_revision_rolls_back_and_keeps_proposal_pending() {
    let _guard = test_lock().lock().await;
    let Some(pool) = database().await else { return };
    let fixture = fixture(&pool).await;
    let target_study_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO studies
         (id,project_id,title,design_context,study_revision,updated_by_actor_kind,updated_by_actor_id)
         VALUES ($1,$2,'Stale target study','{}'::jsonb,1,'system','pr13-http-test')",
    )
    .bind(target_study_id)
    .bind(fixture.project_id)
    .execute(&pool)
    .await
    .expect("stale target study inserts");
    let target_payload = grouping_payload(
        fixture,
        fixture.study_id,
        Some(0),
        serde_json::json!({
            "kind": "existing_study",
            "study_id": target_study_id,
            "expected_revision": 0
        }),
        serde_json::json!([
            {
                "kind": "report_metadata",
                "report_id": fixture.report_id,
                "field": "title",
                "content_hash": sha256_bytes(b"PR13 HTTP report")
            },
            {
                "kind": "study_metadata",
                "study_id": target_study_id,
                "field": "title",
                "content_hash": sha256_bytes(b"Stale target study")
            }
        ]),
    );
    let target_proposal =
        insert_grouping_proposal_with_payload(&pool, fixture, target_payload).await;
    let response = decision_request(
        &pool,
        fixture.project_id,
        target_proposal,
        serde_json::json!({"decision": "accept", "reason": "Stale target must conflict."}),
    )
    .await;
    assert_eq!(response.status(), StatusCode::CONFLICT);

    let previous_payload = grouping_payload(
        fixture,
        fixture.study_id,
        Some(1),
        serde_json::json!({"kind": "new_study", "title": "Rolled back study"}),
        serde_json::json!([{
            "kind": "report_metadata",
            "report_id": fixture.report_id,
            "field": "title",
            "content_hash": sha256_bytes(b"PR13 HTTP report")
        }]),
    );
    let previous_proposal =
        insert_grouping_proposal_with_payload(&pool, fixture, previous_payload).await;
    let response = decision_request(
        &pool,
        fixture.project_id,
        previous_proposal,
        serde_json::json!({"decision": "accept", "reason": "Stale previous must conflict."}),
    )
    .await;
    assert_eq!(response.status(), StatusCode::CONFLICT);

    let statuses: Vec<String> = sqlx::query_scalar(
        "SELECT status FROM ai_proposals WHERE project_id=$1 ORDER BY created_at,id",
    )
    .bind(fixture.project_id)
    .fetch_all(&pool)
    .await
    .expect("stale grouping proposal statuses");
    assert_eq!(statuses, vec!["pending", "pending"]);
    let membership: Uuid = sqlx::query_scalar(
        "SELECT study_id FROM study_reports WHERE project_id=$1 AND report_id=$2",
    )
    .bind(fixture.project_id)
    .bind(fixture.report_id)
    .fetch_one(&pool)
    .await
    .expect("membership after stale grouping");
    assert_eq!(membership, fixture.study_id);
    let target_revision: i64 =
        sqlx::query_scalar("SELECT study_revision FROM studies WHERE project_id=$1 AND id=$2")
            .bind(fixture.project_id)
            .bind(target_study_id)
            .fetch_one(&pool)
            .await
            .expect("stale target revision");
    assert_eq!(target_revision, 1);
    let rolled_back_studies: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM studies WHERE project_id=$1 AND title='Rolled back study'",
    )
    .bind(fixture.project_id)
    .fetch_one(&pool)
    .await
    .expect("rolled back study count");
    let study_events: i64 =
        sqlx::query_scalar("SELECT count(*) FROM study_events WHERE project_id=$1")
            .bind(fixture.project_id)
            .fetch_one(&pool)
            .await
            .expect("stale grouping study event count");
    assert_eq!(rolled_back_studies, 0);
    assert_eq!(study_events, 0);
    cleanup(&pool, fixture).await;
}

#[tokio::test]
async fn extraction_values_http_returns_empty_only_for_an_existing_project_study() {
    let _guard = test_lock().lock().await;
    let Some(pool) = database().await else { return };
    let fixture = fixture(&pool).await;

    let response = router(AppState::core(pool.clone()), &api_config())
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(format!(
                    "/projects/{}/studies/{}/extraction",
                    fixture.project_id, fixture.study_id
                ))
                .body(Body::empty())
                .expect("extraction request should be valid"),
        )
        .await
        .expect("extraction request should be handled");
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response_json(response).await, serde_json::json!([]));

    let wrong_project = Uuid::new_v4();
    let response = router(AppState::core(pool.clone()), &api_config())
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(format!(
                    "/projects/{wrong_project}/studies/{}/extraction",
                    fixture.study_id
                ))
                .body(Body::empty())
                .expect("cross-project extraction request should be valid"),
        )
        .await
        .expect("cross-project extraction request should be handled");
    assert_eq!(response.status(), StatusCode::NOT_FOUND);

    let response = router(AppState::core(pool.clone()), &api_config())
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(format!(
                    "/projects/{}/studies/{}/extraction",
                    fixture.project_id,
                    Uuid::new_v4()
                ))
                .body(Body::empty())
                .expect("missing extraction request should be valid"),
        )
        .await
        .expect("missing extraction request should be handled");
    assert_eq!(response.status(), StatusCode::NOT_FOUND);

    cleanup(&pool, fixture).await;
}

#[tokio::test]
async fn decision_rejects_reviewed_grouping_payload_and_allows_audited_rejection_without_one() {
    let _guard = test_lock().lock().await;
    let Some(pool) = database().await else { return };
    let fixture = fixture(&pool).await;
    let proposal_id = insert_grouping_proposal(&pool, fixture).await;

    let response = decision_request(
        &pool,
        fixture.project_id,
        proposal_id,
        serde_json::json!({
            "decision": "reject",
            "reason": "The reviewer needs more evidence.",
            "reviewed_payload": {
                "kind": "data_extraction",
                "study_id": fixture.study_id,
                "fields": []
            }
        }),
    )
    .await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let status: String = sqlx::query_scalar("SELECT status FROM ai_proposals WHERE id=$1")
        .bind(proposal_id)
        .fetch_one(&pool)
        .await
        .expect("proposal status after rejected request");
    assert_eq!(status, "pending");

    let response = decision_request(
        &pool,
        fixture.project_id,
        proposal_id,
        serde_json::json!({
            "decision": "reject",
            "reason": "The reviewer needs more evidence."
        }),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = response_json(response).await;
    assert_eq!(body["proposal"]["status"], "rejected");

    let events: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM review_events
         WHERE project_id=$1 AND aggregate_type='ai_proposal' AND aggregate_id=$2",
    )
    .bind(fixture.project_id)
    .bind(proposal_id)
    .fetch_one(&pool)
    .await
    .expect("AI rejection audit event");
    assert_eq!(events, 1);

    let response = decision_request(
        &pool,
        fixture.project_id,
        proposal_id,
        serde_json::json!({
            "decision": "reject",
            "reason": "Replay should be rejected."
        }),
    )
    .await;
    assert_eq!(response.status(), StatusCode::CONFLICT);

    cleanup(&pool, fixture).await;
}

#[tokio::test]
async fn decision_is_project_scoped_and_reviewed_variant_must_match_operation() {
    let _guard = test_lock().lock().await;
    let Some(pool) = database().await else { return };
    let fixture = fixture(&pool).await;
    let proposal_id = insert_grouping_proposal(&pool, fixture).await;

    let response = decision_request(
        &pool,
        Uuid::new_v4(),
        proposal_id,
        serde_json::json!({
            "decision": "accept",
            "reason": "Cross-project requests must not resolve proposals.",
            "reviewed_payload": {
                "kind": "appraisal_prefill",
                "report_id": fixture.report_id,
                "definition_id": "rob2",
                "definition_version": 1,
                "answers": [],
                "domain_judgments": {},
                "overall_judgment": "low"
            }
        }),
    )
    .await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);

    let response = decision_request(
        &pool,
        fixture.project_id,
        proposal_id,
        serde_json::json!({
            "decision": "accept",
            "reason": "The reviewed variant is not valid for grouping.",
            "reviewed_payload": {
                "kind": "appraisal_prefill",
                "report_id": fixture.report_id,
                "definition_id": "rob2",
                "definition_version": 1,
                "answers": [],
                "domain_judgments": {},
                "overall_judgment": "low"
            }
        }),
    )
    .await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);

    let status: String = sqlx::query_scalar("SELECT status FROM ai_proposals WHERE id=$1")
        .bind(proposal_id)
        .fetch_one(&pool)
        .await
        .expect("proposal status after variant mismatch");
    assert_eq!(status, "pending");
    cleanup(&pool, fixture).await;
}

#[tokio::test]
async fn study_grouping_provider_failure_is_http_503_without_a_proposal() {
    let _guard = test_lock().lock().await;
    let Some(pool) = database().await else { return };
    let fixture = fixture(&pool).await;
    let model_route = route(ModelProfile::Reasoning, "pr13-failing-provider");
    deepref_postgres::insert_model_route(&pool, &model_route, Utc::now())
        .await
        .expect("grouping test model route");

    let response = router(
        AppState::core(pool.clone()).with_ai_gateway(FailingGateway),
        &api_config(),
    )
    .oneshot(
        Request::builder()
            .method("POST")
            .uri(format!(
                "/projects/{}/reports/{}/ai/study-grouping",
                fixture.project_id, fixture.report_id
            ))
            .body(Body::empty())
            .expect("grouping request should be valid"),
    )
    .await
    .expect("grouping request should be handled");
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);

    let proposals: i64 =
        sqlx::query_scalar("SELECT count(*) FROM ai_proposals WHERE project_id=$1")
            .bind(fixture.project_id)
            .fetch_one(&pool)
            .await
            .expect("grouping proposal count");
    assert_eq!(proposals, 0);
    cleanup(&pool, fixture).await;
}
