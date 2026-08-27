use chrono::Utc;
use deepref_ai::{
    AiError, AiRunRecord, AiRunStatus, AiRunStore, AiTaskKind, AuthorityTier, Embedding,
    EvidenceRef, EvidenceRetriever, ModelParameters, ModelProfile, ModelRouter, ProposalDraft,
    ProposalStatus, ProposalStore, ResolvedModel, RetrievalRequest, safe_error_metadata,
};
use deepref_domain::{DocumentBlockId, DocumentId, ProjectId};
use deepref_postgres::{
    PostgresAiStore, get_ai_study_grouping_target, insert_model_route, migrate,
    persist_document_block_embedding, resolve_ai_proposal,
};
use sqlx::{PgPool, postgres::PgPoolOptions};
use uuid::Uuid;

async fn database() -> Option<PgPool> {
    let url = std::env::var("DATABASE_URL").ok()?;
    let pool = PgPoolOptions::new()
        .max_connections(8)
        .connect(&url)
        .await
        .expect("DATABASE_URL database must be reachable");
    migrate(&pool)
        .await
        .expect("DATABASE_URL migrations must apply");
    Some(pool)
}

async fn fixture(pool: &PgPool) -> (ProjectId, Uuid, DocumentBlockId) {
    let project_id = ProjectId::new(Uuid::new_v4());
    let report_id = Uuid::new_v4();
    let document_id = DocumentId::new(Uuid::new_v4());
    let block_id = DocumentBlockId::new(Uuid::new_v4());
    sqlx::query("INSERT INTO projects (id,name) VALUES ($1,'AI fixture')")
        .bind(project_id.as_uuid())
        .execute(pool)
        .await
        .expect("project inserts");
    sqlx::query(
        "INSERT INTO reports (id,title,abstract_text) VALUES ($1,'Alpha trial','Alpha population')",
    )
    .bind(report_id)
    .execute(pool)
    .await
    .expect("report inserts");
    sqlx::query("INSERT INTO project_reports (project_id,report_id) VALUES ($1,$2)")
        .bind(project_id.as_uuid())
        .bind(report_id)
        .execute(pool)
        .await
        .expect("membership inserts");
    sqlx::query(
        "INSERT INTO documents
           (id,project_id,report_id,object_key,content_hash,mime_type,byte_size,source,status,
            actor_kind,actor_id,active_parser_version,parser_version)
         VALUES ($1,$2,$3,$4,$5,'application/pdf',10,'upload','available','system','ai-test','parser.v1','parser.v1')",
    )
    .bind(document_id.as_uuid())
    .bind(project_id.as_uuid())
    .bind(report_id)
    .bind(format!("documents/{}", document_id.as_uuid()))
    .bind("a".repeat(64))
    .execute(pool)
    .await
    .expect("document inserts");
    sqlx::query(
        "INSERT INTO document_pages(document_id,parser_version,page_number,width,height,active)
         VALUES ($1,'parser.v1',1,600,800,true)",
    )
    .bind(document_id.as_uuid())
    .execute(pool)
    .await
    .expect("page inserts");
    sqlx::query(
        "INSERT INTO document_blocks
           (id,document_id,parser_version,page_number,page_width,page_height,kind,section_path,
            ordinal,text,content_hash,active)
         VALUES ($1,$2,'parser.v1',1,600,800,'text',ARRAY['Results'],0,
                 'Alpha population and outcome', $3,true)",
    )
    .bind(block_id.as_uuid())
    .bind(document_id.as_uuid())
    .bind("b".repeat(64))
    .execute(pool)
    .await
    .expect("block inserts");
    (project_id, document_id.as_uuid(), block_id)
}

async fn cleanup(pool: &PgPool, project_id: ProjectId) {
    sqlx::query("DELETE FROM projects WHERE id=$1")
        .bind(project_id.as_uuid())
        .execute(pool)
        .await
        .expect("fixture cleanup");
}

#[allow(clippy::too_many_arguments)]
async fn extra_block(
    pool: &PgPool,
    document_id: Uuid,
    block_id: Uuid,
    parser_version: &str,
    active: bool,
    ordinal: i32,
    section_path: &[&str],
    text: &str,
) {
    if parser_version != "parser.v1" {
        sqlx::query(
            "INSERT INTO document_pages(document_id,parser_version,page_number,width,height,active)
             VALUES ($1,$2,1,600,800,false)",
        )
        .bind(document_id)
        .bind(parser_version)
        .execute(pool)
        .await
        .expect("extra parser page");
    }
    sqlx::query(
        "INSERT INTO document_blocks
         (id,document_id,parser_version,page_number,page_width,page_height,kind,section_path,ordinal,text,content_hash,active)
         VALUES ($1,$2,$3,1,600,800,'text',$4,$5,$6,$7,$8)",
    )
    .bind(block_id)
    .bind(document_id)
    .bind(parser_version)
    .bind(section_path)
    .bind(ordinal)
    .bind(text)
    .bind(format!("{:064x}", ordinal + 100))
    .bind(active)
    .execute(pool)
    .await
    .expect("extra block");
}

#[tokio::test]
async fn study_grouping_target_executes_without_candidates_and_with_current_candidate() {
    let Some(pool) = database().await else { return };
    let project_id = Uuid::new_v4();
    let report_id = Uuid::new_v4();
    sqlx::query("INSERT INTO projects (id,name) VALUES ($1,'grouping SQL bind test')")
        .bind(project_id)
        .execute(&pool)
        .await
        .expect("project inserts");
    sqlx::query(
        "INSERT INTO reports (id,title,abstract_text,publication_year)
         VALUES ($1,'Current grouping report','A grouping test',2026)",
    )
    .bind(report_id)
    .execute(&pool)
    .await
    .expect("report inserts");
    sqlx::query("INSERT INTO project_reports (project_id,report_id) VALUES ($1,$2)")
        .bind(project_id)
        .bind(report_id)
        .execute(&pool)
        .await
        .expect("project report inserts");

    let empty = get_ai_study_grouping_target(&pool, project_id, report_id)
        .await
        .expect("empty-candidate grouping target");
    assert!(empty.current_study_id.is_none());
    assert!(empty.current_study_revision.is_none());
    assert!(empty.studies.is_empty());

    let study_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO studies
         (id,project_id,title,design_context,study_revision,updated_by_actor_kind,updated_by_actor_id)
         VALUES ($1,$2,'Current study','{}'::jsonb,0,'system','grouping-test')",
    )
    .bind(study_id)
    .bind(project_id)
    .execute(&pool)
    .await
    .expect("study inserts");
    sqlx::query(
        "INSERT INTO study_reports (project_id,study_id,report_id,relationship)
         VALUES ($1,$2,$3,'report_of_study')",
    )
    .bind(project_id)
    .bind(study_id)
    .bind(report_id)
    .execute(&pool)
    .await
    .expect("study membership inserts");

    let current = get_ai_study_grouping_target(&pool, project_id, report_id)
        .await
        .expect("current-candidate grouping target");
    assert_eq!(current.current_study_id, Some(study_id));
    assert_eq!(current.current_study_revision, Some(0));
    assert_eq!(current.studies.len(), 1);
    assert_eq!(current.studies[0].study_id, study_id);
    assert_eq!(current.studies[0].reports.len(), 1);
    assert_eq!(current.studies[0].reports[0].report_id, report_id);

    sqlx::query("DELETE FROM projects WHERE id=$1")
        .bind(project_id)
        .execute(&pool)
        .await
        .expect("grouping SQL bind test cleanup");
}

fn route(provider: &str) -> ResolvedModel {
    ResolvedModel {
        profile: ModelProfile::FastClassifier,
        provider: provider.to_owned(),
        model: "classifier".to_owned(),
        model_version: "2026-08".to_owned(),
        parameters: ModelParameters::default(),
        route_id: Some(Uuid::new_v4()),
    }
}

fn run(project_id: ProjectId, route: ResolvedModel, id: Uuid, hash: &str) -> AiRunRecord {
    AiRunRecord {
        id,
        project_id: Some(project_id),
        task_kind: AiTaskKind::StudyDesignClassification,
        route,
        prompt_version: "classification.v1".to_owned(),
        prompt_hash: "5".repeat(64),
        schema_version: "classification.schema.v1".to_owned(),
        schema_hash: "6".repeat(64),
        input_hash: "1".repeat(64),
        reuse_hash: hash.to_owned(),
        protocol_hash: Some("2".repeat(64)),
        document_hash: Some("3".repeat(64)),
        evidence_hash: None,
        evidence_refs: Vec::new(),
        usage: Default::default(),
        cost_micros: Some(10),
        output: Some(serde_json::json!({"label":"rct"})),
        status: AiRunStatus::Completed,
        error: None,
        parent_automation_run_id: None,
        created_at: Utc::now(),
        completed_at: Some(Utc::now()),
    }
}

#[tokio::test]
async fn postgres_ai_adapters_persist_routes_runs_embeddings_hybrid_results_and_cas_proposals() {
    let Some(pool) = database().await else { return };
    let store = PostgresAiStore::new(&pool);
    let (project_id, document_id, block_id) = fixture(&pool).await;
    let route = route("deterministic-provider");
    insert_model_route(&pool, &route, Utc::now())
        .await
        .expect("route persists");
    let resolved = store
        .resolve(ModelProfile::FastClassifier)
        .await
        .expect("route resolves");
    assert_eq!(resolved.provider, "deterministic-provider");

    let embedding = Embedding::new(vec![1.0, 0.0, 0.0]).expect("embedding is valid");
    assert!(
        persist_document_block_embedding(
            &pool,
            block_id.as_uuid(),
            &"b".repeat(64),
            "test-embedding",
            "generation-1",
            &embedding,
        )
        .await
        .expect("embedding persists")
    );
    let results = store
        .retrieve(RetrievalRequest {
            project_id,
            study_id: None,
            report_id: None,
            document_id: Some(DocumentId::new(document_id)),
            query: "alpha".to_owned(),
            embedding: Some(embedding),
            section_prefix: Some(vec!["Results".to_owned()]),
            kind: Some("text".to_owned()),
            limit: 10,
        })
        .await
        .expect("hybrid retrieval succeeds");
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].evidence.document_block_id, block_id);

    let run_id = Uuid::new_v4();
    let reuse_hash = "4".repeat(64);
    let run = run(project_id, route, run_id, &reuse_hash);
    store.save_run(run.clone()).await.expect("run persists");
    let reused = store
        .find_reusable(&reuse_hash)
        .await
        .expect("run reuse query succeeds")
        .expect("completed run is reusable");
    assert_eq!(reused.id, run_id);

    let proposal = deepref_ai::AiProposal {
        id: Uuid::new_v4(),
        draft: ProposalDraft {
            project_id,
            entity_type: "report".to_owned(),
            entity_id: Some(Uuid::new_v4()),
            operation: "classify".to_owned(),
            payload: serde_json::json!({"label":"rct"}),
            authority: AuthorityTier::WorkflowSuggestion,
        },
        model_run_id: run_id,
        status: ProposalStatus::Pending,
        resolved_at: None,
        resolved_by_actor_id: None,
    };
    let proposal_id = proposal.id;
    store.create(proposal).await.expect("proposal persists");
    assert!(
        resolve_ai_proposal(&pool, proposal_id, true, "user", "reviewer", None)
            .await
            .expect("first CAS resolution succeeds")
    );
    assert!(
        !resolve_ai_proposal(&pool, proposal_id, false, "user", "reviewer", None)
            .await
            .expect("second CAS resolution is harmless")
    );
    let status: String = sqlx::query_scalar("SELECT status FROM ai_proposals WHERE id=$1")
        .bind(proposal_id)
        .fetch_one(&pool)
        .await
        .expect("proposal status loads");
    assert_eq!(status, "accepted");
    cleanup(&pool, project_id).await;
}

#[tokio::test]
async fn postgres_ai_retry_and_proposal_recovery_keep_every_attempt_audited() {
    let Some(pool) = database().await else { return };
    let store = PostgresAiStore::new(&pool);
    let (project_id, _document_id, _block_id) = fixture(&pool).await;
    let route = route("retry-provider");
    let reuse_hash = "5".repeat(64);

    let mut running = run(project_id, route.clone(), Uuid::new_v4(), &reuse_hash);
    running.status = AiRunStatus::Running;
    running.completed_at = None;
    running.output = None;
    store
        .save_run(running.clone())
        .await
        .expect("running attempt persists");
    assert!(
        store
            .find_reusable(&reuse_hash)
            .await
            .expect("running lookup")
            .is_none()
    );

    let mut failed = run(project_id, route.clone(), Uuid::new_v4(), &reuse_hash);
    failed.status = AiRunStatus::Failed;
    failed.completed_at = Some(Utc::now());
    failed.output = None;
    failed.error = Some(safe_error_metadata(&AiError::Gateway(
        "test failure".to_owned(),
    )));
    store
        .save_run(failed.clone())
        .await
        .expect("failed attempt persists");
    assert!(
        store
            .find_reusable(&reuse_hash)
            .await
            .expect("failed lookup")
            .is_none()
    );

    let completed = run(project_id, route, Uuid::new_v4(), &reuse_hash);
    store
        .save_run(completed.clone())
        .await
        .expect("retry persists");
    let reusable = store
        .find_reusable(&reuse_hash)
        .await
        .expect("completed lookup")
        .expect("completed retry is reusable");
    assert_eq!(reusable.id, completed.id);

    let proposal = deepref_ai::AiProposal {
        id: Uuid::new_v4(),
        draft: ProposalDraft {
            project_id,
            entity_type: "report".to_owned(),
            entity_id: Some(Uuid::new_v4()),
            operation: "screen".to_owned(),
            payload: serde_json::json!({"decision":"maybe"}),
            authority: AuthorityTier::ScientificConclusion,
        },
        model_run_id: completed.id,
        status: ProposalStatus::Pending,
        resolved_at: None,
        resolved_by_actor_id: None,
    };
    let first = store
        .create(proposal.clone())
        .await
        .expect("proposal creates");
    let duplicate = deepref_ai::AiProposal {
        id: Uuid::new_v4(),
        ..proposal
    };
    let second = store
        .create(duplicate)
        .await
        .expect("proposal create is idempotent");
    assert_eq!(first.id, second.id);
    let divergent = deepref_ai::AiProposal {
        id: Uuid::new_v4(),
        draft: ProposalDraft {
            payload: serde_json::json!({"decision":"include"}),
            ..first.draft.clone()
        },
        ..first.clone()
    };
    assert!(matches!(
        store.create(divergent).await,
        Err(AiError::Proposal(message)) if message.contains("idempotency")
    ));
    assert!(
        resolve_ai_proposal(&pool, first.id, true, "user", "reviewer", Some("approved"))
            .await
            .expect("resolution")
    );
    assert!(
        !resolve_ai_proposal(&pool, first.id, false, "user", "reviewer", None)
            .await
            .expect("CAS resolution")
    );
    assert!(
        resolve_ai_proposal(&pool, first.id, true, "invalid", "secret", None)
            .await
            .is_err()
    );
    cleanup(&pool, project_id).await;
}

#[tokio::test]
async fn postgres_ai_rejects_malformed_first_terminal_insert_without_writing_a_row() {
    let Some(pool) = database().await else { return };
    let store = PostgresAiStore::new(&pool);
    let (project_id, _document_id, _block_id) = fixture(&pool).await;
    let run_id = Uuid::new_v4();
    let mut malformed = run(
        project_id,
        route("malformed-terminal-provider"),
        run_id,
        &"9".repeat(64),
    );
    malformed.status = AiRunStatus::Failed;

    assert!(matches!(
        store.save_run(malformed).await,
        Err(AiError::Persistence(message))
            if message.contains("invalid completion shape")
    ));
    let rows: i64 = sqlx::query_scalar("SELECT count(*) FROM ai_runs WHERE id=$1")
        .bind(run_id)
        .fetch_one(&pool)
        .await
        .expect("malformed run row count");
    assert_eq!(rows, 0);
    cleanup(&pool, project_id).await;
}

#[tokio::test]
async fn ai_run_rows_allow_only_immutable_replays_and_running_to_terminal_transition() {
    let Some(pool) = database().await else { return };
    let store = PostgresAiStore::new(&pool);
    let (project_id, _document_id, block_id) = fixture(&pool).await;
    let run_id = Uuid::new_v4();
    let mut running = run(
        project_id,
        route("immutable-provider"),
        run_id,
        &"8".repeat(64),
    );
    running.status = AiRunStatus::Running;
    running.completed_at = None;
    running.output = None;
    store.save_run(running.clone()).await.expect("running run");
    store
        .save_run(running.clone())
        .await
        .expect("exact running replay");

    let mut identity_mutation = running.clone();
    identity_mutation.route.provider = "changed-provider".to_owned();
    assert!(matches!(
        store.save_run(identity_mutation).await,
        Err(AiError::Persistence(message)) if message.contains("immutable")
    ));

    let mut evidence_mutation = running.clone();
    evidence_mutation.evidence_refs = vec![
        EvidenceRef::new(block_id, 1, "b".repeat(64))
            .expect("evidence")
            .with_retrieval(1, 0.5)
            .expect("retrieval metadata"),
    ];
    assert!(matches!(
        store.save_run(evidence_mutation).await,
        Err(AiError::Persistence(message)) if message.contains("immutable")
    ));

    let mut completed = running.clone();
    completed.status = AiRunStatus::Completed;
    completed.completed_at = Some(Utc::now());
    completed.output = Some(serde_json::json!({"label":"completed"}));
    completed.usage.input_tokens = 4;
    completed.usage.output_tokens = 7;
    store
        .save_run(completed.clone())
        .await
        .expect("running to completed");
    store
        .save_run(completed.clone())
        .await
        .expect("exact terminal replay");

    let mut terminal_mutation = completed.clone();
    terminal_mutation.output = Some(serde_json::json!({"label":"rewritten"}));
    assert!(matches!(
        store.save_run(terminal_mutation).await,
        Err(AiError::Persistence(message)) if message.contains("immutable")
    ));

    let mut reversion = completed.clone();
    reversion.status = AiRunStatus::Running;
    reversion.completed_at = None;
    assert!(matches!(
        store.save_run(reversion).await,
        Err(AiError::Persistence(message))
            if message.contains("immutable") || message.contains("invalid completion shape")
    ));

    let (status, output): (String, serde_json::Value) =
        sqlx::query_as("SELECT status,output FROM ai_runs WHERE id=$1")
            .bind(run_id)
            .fetch_one(&pool)
            .await
            .expect("terminal row remains unchanged");
    assert_eq!(status, "completed");
    assert_eq!(output, serde_json::json!({"label":"completed"}));
    cleanup(&pool, project_id).await;
}

#[tokio::test]
async fn model_route_ids_are_immutable_and_changed_routes_get_new_ids() {
    let Some(pool) = database().await else { return };
    let route_id = Uuid::new_v4();
    let effective_from = Utc::now();
    let mut original = route("immutable-route-provider");
    original.route_id = Some(route_id);
    let first = insert_model_route(&pool, &original, effective_from)
        .await
        .expect("route inserts");
    assert_eq!(first, route_id);
    assert_eq!(
        insert_model_route(&pool, &original, effective_from)
            .await
            .expect("exact route replay"),
        route_id
    );

    let mut conflicting = original.clone();
    conflicting.provider = "different-provider".to_owned();
    assert!(
        insert_model_route(&pool, &conflicting, effective_from)
            .await
            .is_err()
    );

    let mut new_route = conflicting;
    new_route.route_id = None;
    let new_id = insert_model_route(&pool, &new_route, effective_from)
        .await
        .expect("changed route gets a new id");
    assert_ne!(new_id, route_id);
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM ai_model_routes WHERE id IN ($1,$2)")
        .bind(route_id)
        .bind(new_id)
        .fetch_one(&pool)
        .await
        .expect("route history count");
    assert_eq!(count, 2);
}

#[tokio::test]
async fn hybrid_retrieval_is_dimension_safe_parser_scoped_and_prefix_ordered() {
    let Some(pool) = database().await else { return };
    let store = PostgresAiStore::new(&pool);
    let (project_id, document_id, first_block) = fixture(&pool).await;
    let original_report_id: Uuid =
        sqlx::query_scalar("SELECT report_id FROM documents WHERE id=$1")
            .bind(document_id)
            .fetch_one(&pool)
            .await
            .expect("original report");
    let other_report_id = Uuid::new_v4();
    let other_document_id = Uuid::new_v4();
    let other_block_id = Uuid::new_v4();
    sqlx::query("INSERT INTO reports (id,title) VALUES ($1,'Other report in same project')")
        .bind(other_report_id)
        .execute(&pool)
        .await
        .expect("other report");
    sqlx::query("INSERT INTO project_reports (project_id,report_id) VALUES ($1,$2)")
        .bind(project_id.as_uuid())
        .bind(other_report_id)
        .execute(&pool)
        .await
        .expect("other report membership");
    sqlx::query(
        "INSERT INTO documents
         (id,project_id,report_id,object_key,content_hash,mime_type,byte_size,source,status,
          actor_kind,actor_id,active_parser_version,parser_version)
         VALUES ($1,$2,$3,$4,$5,'application/pdf',10,'upload','available','system','ai-test','parser.v1','parser.v1')",
    )
    .bind(other_document_id)
    .bind(project_id.as_uuid())
    .bind(other_report_id)
    .bind(format!("documents/{other_document_id}"))
    .bind("c".repeat(64))
    .execute(&pool)
    .await
    .expect("other document");
    sqlx::query(
        "INSERT INTO document_pages(document_id,parser_version,page_number,width,height,active)
         VALUES ($1,'parser.v1',1,600,800,true)",
    )
    .bind(other_document_id)
    .execute(&pool)
    .await
    .expect("other page");
    sqlx::query(
        "INSERT INTO document_blocks
         (id,document_id,parser_version,page_number,page_width,page_height,kind,section_path,
          ordinal,text,content_hash,active)
         VALUES ($1,$2,'parser.v1',1,600,800,'text',ARRAY['Results'],0,
                 'Alpha evidence from another report', $3,true)",
    )
    .bind(other_block_id)
    .bind(other_document_id)
    .bind("c".repeat(64))
    .execute(&pool)
    .await
    .expect("other block");
    let report_scoped = store
        .retrieve(RetrievalRequest {
            project_id,
            study_id: None,
            report_id: Some(original_report_id),
            document_id: None,
            query: "alpha".to_owned(),
            embedding: None,
            section_prefix: None,
            kind: None,
            limit: 10,
        })
        .await
        .expect("report-scoped retrieval");
    assert_eq!(report_scoped.len(), 1);
    assert!(
        report_scoped
            .iter()
            .all(|block| block.evidence.document_block_id.as_uuid() != other_block_id)
    );
    let second_block = DocumentBlockId::new(Uuid::new_v4());
    let stale_block = DocumentBlockId::new(Uuid::new_v4());
    extra_block(
        &pool,
        document_id,
        second_block.as_uuid(),
        "parser.v1",
        true,
        1,
        &["Results", "Deep"],
        "Alpha deep result",
    )
    .await;
    extra_block(
        &pool,
        document_id,
        stale_block.as_uuid(),
        "parser.old",
        true,
        2,
        &["Results", "Deep"],
        "Alpha stale result",
    )
    .await;
    let first_embedding = Embedding::new(vec![1.0, 0.0, 0.0]).expect("first vector");
    let second_embedding = Embedding::new(vec![1.0, 0.0]).expect("second vector");
    persist_document_block_embedding(
        &pool,
        first_block.as_uuid(),
        &"b".repeat(64),
        "model-3d",
        "generation-1",
        &first_embedding,
    )
    .await
    .expect("first embedding");
    persist_document_block_embedding(
        &pool,
        second_block.as_uuid(),
        &format!("{:064x}", 101),
        "model-2d",
        "generation-1",
        &second_embedding,
    )
    .await
    .expect("second embedding");

    let lexical = store
        .retrieve(RetrievalRequest {
            project_id,
            study_id: None,
            report_id: None,
            document_id: Some(DocumentId::new(document_id)),
            query: "alpha".to_owned(),
            embedding: Some(second_embedding.clone()),
            section_prefix: Some(vec!["Results".to_owned()]),
            kind: Some("text".to_owned()),
            limit: 10,
        })
        .await
        .expect("lexical fallback");
    assert_eq!(lexical.len(), 2);
    assert!(
        lexical
            .iter()
            .all(|block| block.evidence.document_block_id != stale_block)
    );
    assert_eq!(lexical[0].retrieval_rank, 1);
    assert!(lexical[0].retrieval_score >= lexical[1].retrieval_score);
    let vector_only = store
        .retrieve(RetrievalRequest {
            project_id,
            study_id: None,
            report_id: None,
            document_id: Some(DocumentId::new(document_id)),
            query: String::new(),
            embedding: Some(second_embedding),
            section_prefix: None,
            kind: None,
            limit: 10,
        })
        .await
        .expect("dimension-safe vector lookup");
    assert_eq!(vector_only.len(), 1);
    assert_eq!(vector_only[0].evidence.document_block_id, second_block);
    let prefix = store
        .retrieve(RetrievalRequest {
            project_id,
            study_id: None,
            report_id: None,
            document_id: Some(DocumentId::new(document_id)),
            query: "alpha".to_owned(),
            embedding: None,
            section_prefix: Some(vec!["Results".to_owned(), "Deep".to_owned()]),
            kind: None,
            limit: 10,
        })
        .await
        .expect("prefix filter");
    assert_eq!(prefix.len(), 1);
    assert_eq!(prefix[0].evidence.document_block_id, second_block);
    cleanup(&pool, project_id).await;
}

#[tokio::test]
async fn pr12_migration_installs_project_scoped_typed_proposal_fields() {
    let Some(pool) = database().await else { return };
    let columns: Vec<String> = sqlx::query_scalar(
        "SELECT column_name FROM information_schema.columns
         WHERE table_name='ai_proposals' AND column_name = ANY($1::text[])
         ORDER BY column_name",
    )
    .bind(vec![
        "task_kind",
        "target_report_id",
        "target_record_id",
        "protocol_version_id",
        "expected_revision",
    ])
    .fetch_all(&pool)
    .await
    .expect("PR12 proposal columns");
    assert_eq!(columns.len(), 5);

    for table in ["ai_proposal_criterion_judgments", "ai_proposal_evidence"] {
        let exists: bool = sqlx::query_scalar("SELECT to_regclass($1) IS NOT NULL")
            .bind(table)
            .fetch_one(&pool)
            .await
            .expect("PR12 typed proposal table");
        assert!(exists, "missing {table}");
    }
    let judgment_columns: Vec<String> = sqlx::query_scalar(
        "SELECT column_name FROM information_schema.columns
         WHERE table_name='ai_proposal_criterion_judgments'
           AND column_name = ANY($1::text[])",
    )
    .bind(vec!["protocol_version_id"])
    .fetch_all(&pool)
    .await
    .expect("criterion projection columns");
    assert_eq!(judgment_columns, vec!["protocol_version_id"]);
    let evidence_columns: Vec<String> = sqlx::query_scalar(
        "SELECT column_name FROM information_schema.columns
         WHERE table_name='ai_proposal_evidence'
           AND column_name = ANY($1::text[])
         ORDER BY column_name",
    )
    .bind(vec!["document_id", "report_id"])
    .fetch_all(&pool)
    .await
    .expect("evidence projection columns");
    assert_eq!(evidence_columns, vec!["document_id", "report_id"]);
    let projection_constraints: i64 = sqlx::query_scalar(
        "SELECT count(*)::bigint FROM pg_constraint
         WHERE conname IN ('ai_proposal_criterion_judgments_protocol_version_id_fkey',
                           'ai_proposal_evidence_project_report_fkey',
                           'ai_proposal_evidence_document_project_report_fkey',
                           'ai_proposal_evidence_document_block_fkey')",
    )
    .fetch_one(&pool)
    .await
    .expect("projection constraints");
    assert_eq!(projection_constraints, 4);
    let project_target_fk: i64 = sqlx::query_scalar(
        "SELECT count(*)::bigint FROM pg_constraint
         WHERE conname IN ('ai_proposals_project_report_target_fkey',
                           'ai_proposals_project_record_target_fkey',
                           'ai_proposals_project_protocol_target_fkey')",
    )
    .fetch_one(&pool)
    .await
    .expect("PR12 project target constraints");
    assert_eq!(project_target_fk, 3);
}

#[tokio::test]
async fn pr12_projection_foreign_keys_reject_cross_project_criterion_and_block_references() {
    let Some(pool) = database().await else { return };
    let (project_a, document_a, _block_a) = fixture(&pool).await;
    let (project_b, document_b, block_b) = fixture(&pool).await;
    let report_a: Uuid = sqlx::query_scalar("SELECT report_id FROM documents WHERE id=$1")
        .bind(document_a)
        .fetch_one(&pool)
        .await
        .expect("project A report");
    let report_b: Uuid = sqlx::query_scalar("SELECT report_id FROM documents WHERE id=$1")
        .bind(document_b)
        .fetch_one(&pool)
        .await
        .expect("project B report");
    let protocol_a = Uuid::new_v4();
    let protocol_b = Uuid::new_v4();
    let criterion_a = Uuid::new_v4();
    let criterion_b = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO protocol_versions (id,project_id,version,name,status,criteria)
         VALUES ($1,$2,1,'A','draft','[]'::jsonb),($3,$4,1,'B','draft','[]'::jsonb)",
    )
    .bind(protocol_a)
    .bind(project_a.as_uuid())
    .bind(protocol_b)
    .bind(project_b.as_uuid())
    .execute(&pool)
    .await
    .expect("protocols");
    sqlx::query(
        "INSERT INTO eligibility_criteria
         (id,protocol_version_id,criterion_type,stage,dimension,label,description,ordinal)
         VALUES ($1,$2,'include','title_abstract','population','A','A',0),
                ($3,$4,'include','title_abstract','population','B','B',0)",
    )
    .bind(criterion_a)
    .bind(protocol_a)
    .bind(criterion_b)
    .bind(protocol_b)
    .execute(&pool)
    .await
    .expect("criteria");
    sqlx::query(
        "UPDATE protocol_versions SET status='published',published_at=now()
         WHERE id IN ($1,$2)",
    )
    .bind(protocol_a)
    .bind(protocol_b)
    .execute(&pool)
    .await
    .expect("protocol publication");
    let run_id = Uuid::new_v4();
    let store = PostgresAiStore::new(&pool);
    store
        .save_run(run(
            project_a,
            route("projection-scope"),
            run_id,
            &"8".repeat(64),
        ))
        .await
        .expect("run");
    let proposal_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO ai_proposals
         (id,project_id,ai_run_id,proposal_type,payload,status,entity_type,operation,
          model_run_id,authority_tier,task_kind,protocol_version_id,target_report_id)
         VALUES ($1,$2,$3,'screening_suggestion','{}'::jsonb,'pending','screening_report',
                 'screening_suggestion',$3,'scientific_conclusion','title_abstract_screening',$4,$5)",
    )
    .bind(proposal_id)
    .bind(project_a.as_uuid())
    .bind(run_id)
    .bind(protocol_a)
    .bind(report_a)
    .execute(&pool)
    .await
    .expect("proposal");

    let cross_project_criterion = sqlx::query(
        "INSERT INTO ai_proposal_criterion_judgments
         (proposal_id,project_id,criterion_id,protocol_version_id,ordinal,judgment,rationale)
         VALUES ($1,$2,$3,$4,0,'meets','cross-project criterion')",
    )
    .bind(proposal_id)
    .bind(project_a.as_uuid())
    .bind(criterion_b)
    .bind(protocol_b)
    .execute(&pool)
    .await;
    assert!(cross_project_criterion.is_err());

    let cross_project_block = sqlx::query(
        "INSERT INTO ai_proposal_evidence
         (proposal_id,project_id,ordinal,evidence_kind,report_id,document_id,document_block_id,
          page,content_hash)
         VALUES ($1,$2,0,'document_block',$3,$4,$5,1,$6)",
    )
    .bind(proposal_id)
    .bind(project_a.as_uuid())
    .bind(report_b)
    .bind(document_b)
    .bind(block_b.as_uuid())
    .bind("b".repeat(64))
    .execute(&pool)
    .await;
    assert!(cross_project_block.is_err());

    cleanup(&pool, project_a).await;
    cleanup(&pool, project_b).await;
}

#[tokio::test]
async fn embedding_generations_are_versioned_and_evidence_is_project_scoped_with_rank_score() {
    let Some(pool) = database().await else { return };
    let store = PostgresAiStore::new(&pool);
    let (project_a, document_a, block_a) = fixture(&pool).await;
    let (project_b, _document_b, block_b) = fixture(&pool).await;
    let embedding_a = Embedding::new(vec![1.0, 0.0, 0.0]).expect("embedding");
    persist_document_block_embedding(
        &pool,
        block_a.as_uuid(),
        &"b".repeat(64),
        "model",
        "generation-1",
        &embedding_a,
    )
    .await
    .expect("generation one");
    let created_at: chrono::DateTime<Utc> = sqlx::query_scalar(
        "SELECT created_at FROM document_block_embeddings
         WHERE document_block_id=$1 AND model_identifier='model' AND generation='generation-1'",
    )
    .bind(block_a.as_uuid())
    .fetch_one(&pool)
    .await
    .expect("generation timestamp");
    persist_document_block_embedding(
        &pool,
        block_a.as_uuid(),
        &"b".repeat(64),
        "model",
        "generation-1",
        &embedding_a,
    )
    .await
    .expect("exact generation replay");
    let replayed_created_at: chrono::DateTime<Utc> = sqlx::query_scalar(
        "SELECT created_at FROM document_block_embeddings
         WHERE document_block_id=$1 AND model_identifier='model' AND generation='generation-1'",
    )
    .bind(block_a.as_uuid())
    .fetch_one(&pool)
    .await
    .expect("replayed generation timestamp");
    assert_eq!(created_at, replayed_created_at);
    let changed_vector = Embedding::new(vec![0.0, 1.0, 0.0]).expect("changed embedding");
    assert!(
        persist_document_block_embedding(
            &pool,
            block_a.as_uuid(),
            &"b".repeat(64),
            "model",
            "generation-1",
            &changed_vector,
        )
        .await
        .is_err()
    );
    let changed_dimension = Embedding::new(vec![1.0, 0.0]).expect("changed dimension");
    assert!(
        persist_document_block_embedding(
            &pool,
            block_a.as_uuid(),
            &"b".repeat(64),
            "model",
            "generation-1",
            &changed_dimension,
        )
        .await
        .is_err()
    );
    persist_document_block_embedding(
        &pool,
        block_a.as_uuid(),
        &"b".repeat(64),
        "model",
        "generation-2",
        &embedding_a,
    )
    .await
    .expect("generation two");
    let generations: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM document_block_embeddings WHERE document_block_id=$1",
    )
    .bind(block_a.as_uuid())
    .fetch_one(&pool)
    .await
    .expect("generation count");
    assert_eq!(generations, 2);
    let current_generation: String = sqlx::query_scalar(
        "SELECT generation FROM document_block_embeddings
         WHERE document_block_id=$1 AND is_current",
    )
    .bind(block_a.as_uuid())
    .fetch_one(&pool)
    .await
    .expect("current generation");
    assert_eq!(current_generation, "generation-2");
    let retrieved = store
        .retrieve(RetrievalRequest {
            project_id: project_a,
            study_id: None,
            report_id: None,
            document_id: Some(DocumentId::new(document_a)),
            query: "alpha".to_owned(),
            embedding: Some(embedding_a),
            section_prefix: None,
            kind: None,
            limit: 5,
        })
        .await
        .expect("retrieval");
    let mut scoped_run = run(
        project_a,
        route("scope-provider"),
        Uuid::new_v4(),
        &"6".repeat(64),
    );
    scoped_run.evidence_refs = vec![retrieved[0].evidence.clone()];
    store
        .save_run(scoped_run.clone())
        .await
        .expect("scoped evidence");
    let (stored_rank, stored_score): (i32, f64) =
        sqlx::query_as("SELECT rank,retrieval_score FROM ai_run_evidence WHERE ai_run_id=$1")
            .bind(scoped_run.id)
            .fetch_one(&pool)
            .await
            .expect("stored retrieval metadata");
    assert_eq!(stored_rank, retrieved[0].retrieval_rank as i32);
    assert_eq!(stored_score, retrieved[0].retrieval_score);
    let mut cross_project = run(
        project_a,
        route("scope-provider"),
        Uuid::new_v4(),
        &"7".repeat(64),
    );
    cross_project.evidence_refs = vec![
        EvidenceRef::new(block_b, 1, "b".repeat(64))
            .expect("cross evidence")
            .with_retrieval(1, 1.0)
            .expect("rank"),
    ];
    assert!(store.save_run(cross_project).await.is_err());
    cleanup(&pool, project_a).await;
    cleanup(&pool, project_b).await;
}
