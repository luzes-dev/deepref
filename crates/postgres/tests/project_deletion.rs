#![allow(
    clippy::expect_used,
    clippy::panic,
    clippy::unwrap_used,
    clippy::string_slice
)]

use std::collections::BTreeMap;

use deepref_postgres::delete_project_in_transaction;
use sqlx::{PgPool, postgres::PgPoolOptions};
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

/// Every table named by REV-08, plus the migration 0026-0039 tables and the
/// core project tables. Each one must hold rows before the delete, so the test
/// shows the cascade actually ran over real data.
const SEEDED_TABLES: &[&str] = &[
    "projects",
    "project_reports",
    "records",
    "studies",
    "study_reports",
    "study_events",
    "documents",
    "document_pages",
    "document_blocks",
    "document_sections",
    "document_references",
    "protocol_versions",
    "eligibility_criteria",
    "exclusion_reasons",
    "screening_state",
    "screening_events",
    "extraction_field_definitions",
    "extraction_values",
    "extraction_events",
    "ai_runs",
    "ai_proposals",
    "ai_proposal_evidence",
    "ai_proposal_criterion_judgments",
    "ai_run_evidence",
    "ai_activity",
    "ai_reviewer_decisions",
    "ai_usage_ledger",
    "project_ai_autonomy",
    "appraisal_assessments",
    "appraisal_assessment_evidence",
    "appraisal_events",
    "assistant_conversations",
    "assistant_plans",
    "workflows",
    "workflow_versions",
    "workflow_runs",
    "workflow_node_runs",
    "workflow_files",
    "jobs",
    "automation_definitions",
    "automation_definition_steps",
    "automation_runs",
    "automation_step_runs",
    "dedupe_proposals",
    "dedupe_resolution_events",
    "review_artifacts",
    "notifications",
    "metric_snapshots",
    "prisma_snapshots",
    "projection_state",
];

/// Placeholders are written as `@name@` and replaced with fresh UUIDs, so the
/// test can run on a database that already holds other projects.
const TOKENS: &[&str] = &[
    "proj", "rep1", "rep2", "rec", "pver", "crit", "exr", "stu1", "stu2", "doc", "blk", "fdef",
    "ai", "prop", "scev", "exev", "appr", "act", "conv", "wf", "wfv", "wfr", "job", "def", "arun",
    "ded", "dre",
];

const SEED_SQL: &str = r#"
INSERT INTO projects (id, name) VALUES ('@proj@', 'Deletion fixture');
INSERT INTO reports (id, title, publication_year, journal) VALUES
  ('@rep1@', 'Included report', 2007, 'Stroke'),
  ('@rep2@', 'Grouped report', 2010, 'Stroke');
INSERT INTO project_reports (project_id, report_id, lifecycle_status) VALUES
  ('@proj@', '@rep1@', 'included'),
  ('@proj@', '@rep2@', 'included');
INSERT INTO records (id, project_id, source, report_id, title)
  VALUES ('@rec@', '@proj@', 'ris', '@rep1@', 'Included report');
INSERT INTO protocol_versions (id, project_id, version, name, status)
  VALUES ('@pver@', '@proj@', 1, 'Protocol v1', 'draft');
INSERT INTO eligibility_criteria (id, protocol_version_id, criterion_type, label, description)
  VALUES ('@crit@', '@pver@', 'include', 'Adults', 'Adults with stroke');
INSERT INTO exclusion_reasons (id, project_id, code, label, stage)
  VALUES ('@exr@', '@proj@', 'fixture_reason', 'Fixture reason', 'full_text');
INSERT INTO studies (id, project_id, title, design_context, study_revision, updated_by_actor_kind, updated_by_actor_id) VALUES
  ('@stu1@', '@proj@', 'Study one', '{}'::jsonb, 0, 'user', 'fixture'),
  ('@stu2@', '@proj@', 'Study two', '{}'::jsonb, 0, 'user', 'fixture');
INSERT INTO study_reports (project_id, study_id, report_id, relationship)
  VALUES ('@proj@', '@stu1@', '@rep2@', 'report_of_study');
INSERT INTO study_events (id, project_id, study_id, event_type, before_revision, result_revision, actor_kind, actor_id, before_snapshot, result_snapshot, payload)
  VALUES (gen_random_uuid(), '@proj@', '@stu1@', 'study_created', 0, 1, 'user', 'fixture', '{}'::jsonb, '{}'::jsonb, '{}'::jsonb);
INSERT INTO documents (id, project_id, report_id, mime_type, byte_size, source, status)
  VALUES ('@doc@', '@proj@', '@rep1@', 'application/pdf', 0, 'upload', 'missing');
INSERT INTO document_pages (document_id, parser_version, page_number, width, height)
  VALUES ('@doc@', 'fixture-v1', 1, 612, 792);
INSERT INTO document_blocks (id, document_id, parser_version, page_number, page_width, page_height, kind, ordinal, text, content_hash, active)
  VALUES ('@blk@', '@doc@', 'fixture-v1', 1, 612, 792, 'text', 0, 'Fixture block text', repeat('a', 64), true);
INSERT INTO document_sections (id, document_id, parser_version, ordinal, title, depth, source)
  VALUES (gen_random_uuid(), '@doc@', 'fixture-v1', 1, 'Methods', 1, 'native');
INSERT INTO document_references (id, document_id, parser_version, ordinal, raw, source)
  VALUES (gen_random_uuid(), '@doc@', 'fixture-v1', 1, 'Smith 2001', 'native');
INSERT INTO extraction_field_definitions (project_id, id, version, field_key, label, value_type, required)
  VALUES ('@proj@', '@fdef@', 1, 'populacao', 'População', 'text', false);
INSERT INTO ai_runs (id, project_id, task_kind, provider, model, prompt_version, input_hash, status, profile, model_version, schema_version, prompt_hash, schema_hash, reuse_hash, input_tokens, output_tokens)
  VALUES ('@ai@', '@proj@', 'data_extraction', 'fixture', 'fixture-model', 'v1', repeat('b', 64), 'completed', 'standard', 'v1', 'v1', repeat('c', 64), repeat('d', 64), repeat('e', 64), 0, 0);
INSERT INTO ai_proposals (id, project_id, ai_run_id, model_run_id, proposal_type, payload, entity_type, operation, authority_tier, task_kind, status, protocol_version_id, target_study_id, target_report_id, target_record_id)
  VALUES ('@prop@', '@proj@', '@ai@', '@ai@', 'extraction', '{}'::jsonb, 'extraction_value', 'create', 'scientific_conclusion', 'data_extraction', 'pending', '@pver@', '@stu1@', '@rep1@', '@rec@');
INSERT INTO ai_proposal_criterion_judgments (proposal_id, project_id, criterion_id, protocol_version_id, ordinal, judgment, rationale, evidence)
  VALUES ('@prop@', '@proj@', '@crit@', '@pver@', 0, 'meets', 'Fixture rationale', '[]'::jsonb);
INSERT INTO ai_proposal_evidence (proposal_id, project_id, ordinal, evidence_kind, document_id, document_block_id, page, report_id, content_hash)
  VALUES ('@prop@', '@proj@', 0, 'document_block', '@doc@', '@blk@', 1, '@rep1@', repeat('f', 64));
INSERT INTO ai_run_evidence (ai_run_id, project_id, document_id, document_block_id, rank, retrieval_score, content_hash)
  VALUES ('@ai@', '@proj@', '@doc@', '@blk@', 1, 0.5, repeat('f', 64));
INSERT INTO extraction_values (id, project_id, study_id, report_id, field_definition_id, field_definition_version, value_type, text_value, source_document_id, source_block_id, source_page, source_parser_version, source_content_hash, approved_by_actor_kind, approved_by_actor_id)
  VALUES (gen_random_uuid(), '@proj@', '@stu1@', '@rep1@', '@fdef@', 1, 'text', 'Adults', '@doc@', '@blk@', 1, 'fixture-v1', repeat('a', 64), 'user', 'fixture');
INSERT INTO extraction_events (id, project_id, study_id, proposal_id, event_type, payload, actor_kind, actor_id)
  VALUES ('@exev@', '@proj@', '@stu1@', '@prop@', 'extraction_values_approved', '{}'::jsonb, 'user', 'fixture');
INSERT INTO screening_events (id, project_id, report_id, stage, protocol_version_id, actor_kind, actor_id, decision, result_final_status, result_title_abstract_status, result_full_text_status)
  VALUES ('@scev@', '@proj@', '@rep1@', 'full_text', '@pver@', 'user', 'fixture', 'include', 'include', 'include', 'include');
INSERT INTO screening_state (project_id, report_id, title_abstract_status, full_text_status, final_status, last_event_id)
  VALUES ('@proj@', '@rep1@', 'include', 'include', 'include', '@scev@');
INSERT INTO ai_activity (id, project_id, actor_type, actor_label, actor_kind, actor_id, task, action, summary, affected, evidence)
  VALUES ('@act@', '@proj@', 'ai', 'Fixture model', 'system', 'fixture', 'title_abstract_screening', 'proposal_created', 'Fixture activity', '[]'::jsonb, '[]'::jsonb);
INSERT INTO ai_reviewer_decisions (id, project_id, report_id, stage, decision, rationale, source, activity_id, evidence)
  VALUES (gen_random_uuid(), '@proj@', '@rep1@', 'title_abstract', 'include', 'Fixture decision', 'ai', '@act@', '[]'::jsonb);
INSERT INTO ai_usage_ledger (id, project_id, profile, provider, model, purpose, input_tokens, output_tokens, cost_micros)
  VALUES (gen_random_uuid(), '@proj@', 'standard', 'fixture', 'fixture-model', 'structured', 10, 5, 100);
INSERT INTO project_ai_autonomy (project_id, task, level, updated_by_kind, updated_by_id)
  VALUES ('@proj@', 'title_abstract_screening', 'suggest', 'user', 'fixture');
INSERT INTO appraisal_assessments (id, project_id, report_id, definition_id, definition_version, responses, judgments, actor_kind, actor_id)
  VALUES ('@appr@', '@proj@', '@rep1@', 'deepref-rct-generic', 1, '{}'::jsonb, '{}'::jsonb, 'user', 'fixture');
INSERT INTO appraisal_assessment_evidence (id, assessment_id, project_id, report_id, question_id, document_id, block_id)
  VALUES (gen_random_uuid(), '@appr@', '@proj@', '@rep1@', 'q1', '@doc@', '@blk@');
INSERT INTO appraisal_events (id, assessment_id, project_id, report_id, event_type, payload, actor_kind, actor_id)
  VALUES (gen_random_uuid(), '@appr@', '@proj@', '@rep1@', 'appraisal_completed', '{}'::jsonb, 'user', 'fixture');
INSERT INTO assistant_conversations (id, project_id, title) VALUES ('@conv@', '@proj@', 'Fixture chat');
INSERT INTO assistant_plans (id, project_id, conversation_id, summary, actions, created_by_kind, created_by_id, model, prompt_version, status, evidence)
  VALUES (gen_random_uuid(), '@proj@', '@conv@', 'Fixture plan', '[]'::jsonb, 'user', 'fixture', 'fixture-model', 'v1', 'pending', '[]'::jsonb);
INSERT INTO workflows (id, project_id, name, webhook_token, webhook_secret, email_token, created_by_kind, created_by_id)
  VALUES ('@wf@', '@proj@', 'Fixture workflow', gen_random_uuid()::text, gen_random_uuid()::text, gen_random_uuid()::text, 'user', 'fixture');
INSERT INTO workflow_versions (id, project_id, workflow_id, version, graph, trigger_kind, published_by_kind, published_by_id)
  VALUES ('@wfv@', '@proj@', '@wf@', 1, '{}'::jsonb, 'manual', 'user', 'fixture');
UPDATE workflows SET published_version_id = '@wfv@' WHERE id = '@wf@';
INSERT INTO workflow_runs (id, project_id, workflow_id, version_id, graph, trigger_kind, idempotency_key, actor_kind, actor_id)
  VALUES ('@wfr@', '@proj@', '@wf@', '@wfv@', '{}'::jsonb, 'manual', gen_random_uuid()::text, 'user', 'fixture');
INSERT INTO workflow_node_runs (id, project_id, run_id, node_id, node_type)
  VALUES (gen_random_uuid(), '@proj@', '@wfr@', 'node-1', 'trigger');
INSERT INTO workflow_files (id, project_id, run_id, node_id, name, content_type, content)
  VALUES (gen_random_uuid(), '@proj@', '@wfr@', 'node-1', 'fixture.txt', 'text/plain', '\x6869'::bytea);
INSERT INTO jobs (id, project_id, kind, state) VALUES ('@job@', '@proj@', 'automation_run', 'queued');
INSERT INTO automation_definitions (id, project_id, name, trigger_kind, recipe_id, recipe_version, status, actor_kind, actor_id)
  VALUES ('@def@', '@proj@', 'Fixture automation', 'manual', 'review_screening', 1, 'active', 'user', 'fixture');
INSERT INTO automation_definition_steps (project_id, definition_id, ordinal, step_key, step_kind)
  VALUES ('@proj@', '@def@', 0, 'screen', 'ai_task');
INSERT INTO automation_runs (id, project_id, definition_id, job_id, recipe_id, recipe_version, trigger_kind, idempotency_key, actor_kind, actor_id)
  VALUES ('@arun@', '@proj@', '@def@', '@job@', 'review_screening', 1, 'manual', gen_random_uuid()::text, 'user', 'fixture');
INSERT INTO automation_step_runs (project_id, automation_run_id, ordinal, step_key, step_kind, status, attempts)
  VALUES ('@proj@', '@arun@', 0, 'screen', 'ai_task', 'pending', 0);
INSERT INTO dedupe_proposals (id, project_id, record_id, proposal_kind, status, score, title_similarity)
  VALUES ('@ded@', '@proj@', '@rec@', 'fuzzy', 'pending', 0.9, 0.9);
INSERT INTO dedupe_resolution_events (id, project_id, record_id, action, reason, actor_kind, actor_id, proposal_id)
  VALUES ('@dre@', '@proj@', '@rec@', 'accept_proposal', 'Fixture resolution', 'user', 'fixture', '@ded@');
INSERT INTO review_artifacts (id, project_id, content_hash, media_type, payload)
  VALUES (gen_random_uuid(), '@proj@', repeat('a', 64), 'application/json', '{}'::jsonb);
INSERT INTO notifications (id, project_id, kind, severity, title, payload)
  VALUES (gen_random_uuid(), '@proj@', 'fixture', 'info', 'Fixture notification', '{}'::jsonb);
INSERT INTO metric_snapshots (project_id, revision, metrics_as_of, work_count, edge_count)
  VALUES ('@proj@', 1, now(), 0, 0);
INSERT INTO prisma_snapshots (project_id) VALUES ('@proj@');
INSERT INTO projection_state (projection_name, project_id, state, revision, watermark, lag, updated_at)
  VALUES ('fixture_projection', '@proj@', 'ready', 0, 0, 0, now());
"#;

fn render(sql: &str, ids: &BTreeMap<&'static str, Uuid>) -> String {
    let mut rendered = sql.to_owned();
    for (name, id) in ids {
        rendered = rendered.replace(&format!("@{name}@"), &id.to_string());
    }
    rendered
}

/// Rows in `table` that belong to `project_id`. Most tables carry a `project_id`
/// column. Document-level tables and criterion rows are counted through their
/// parent, and `projects` is keyed by its own id. `table` is a constant from
/// this file or a name from information_schema, so it is not user input.
async fn count_rows(pool: &PgPool, table: &str, project_id: Uuid) -> i64 {
    let sql = match table {
        "projects" => String::from("SELECT count(*) FROM projects WHERE id = $1"),
        "document_pages" | "document_blocks" | "document_sections" | "document_references" => {
            format!(
                "SELECT count(*) FROM \"{table}\" WHERE document_id IN \
                 (SELECT id FROM documents WHERE project_id = $1)"
            )
        }
        "eligibility_criteria" => format!(
            "SELECT count(*) FROM \"{table}\" WHERE protocol_version_id IN \
             (SELECT id FROM protocol_versions WHERE project_id = $1)"
        ),
        _ => format!("SELECT count(*) FROM \"{table}\" WHERE project_id = $1"),
    };
    sqlx::query_scalar(sqlx::AssertSqlSafe(sql))
        .bind(project_id)
        .fetch_one(pool)
        .await
        .expect("row count runs")
}

#[tokio::test]
async fn deleting_a_populated_project_removes_every_row_that_belongs_to_it() {
    let Some(pool) = database().await else {
        return;
    };
    let ids: BTreeMap<&'static str, Uuid> =
        TOKENS.iter().map(|name| (*name, Uuid::new_v4())).collect();
    let project_id = ids["proj"];

    let mut tx = pool.begin().await.expect("fixture transaction starts");
    // The seed is a constant template; only generated UUIDs are substituted.
    sqlx::raw_sql(sqlx::AssertSqlSafe(render(SEED_SQL, &ids)))
        .execute(&mut *tx)
        .await
        .expect("fixture rows insert");
    tx.commit().await.expect("fixture commits");

    for table in SEEDED_TABLES {
        assert!(
            count_rows(&pool, table, project_id).await > 0,
            "fixture has no rows in {table}"
        );
    }

    let mut tx = pool.begin().await.expect("delete transaction starts");
    assert!(
        delete_project_in_transaction(&mut tx, project_id)
            .await
            .expect("project delete succeeds"),
        "the project should exist before the delete"
    );
    tx.commit().await.expect("delete commits");

    assert_eq!(
        count_rows(&pool, "projects", project_id).await,
        0,
        "the project row itself is deleted"
    );
    let project_tables: Vec<String> = sqlx::query_scalar(
        "SELECT table_name::text FROM information_schema.columns
         WHERE table_schema = 'public' AND column_name = 'project_id'
         ORDER BY table_name",
    )
    .fetch_all(&pool)
    .await
    .expect("project tables list");
    for table in SEEDED_TABLES {
        assert_eq!(
            count_rows(&pool, table, project_id).await,
            0,
            "{table} still holds rows for the deleted project"
        );
    }
    for table in &project_tables {
        assert_eq!(
            count_rows(&pool, table, project_id).await,
            0,
            "{table} still holds rows for the deleted project"
        );
    }

    let mut tx = pool
        .begin()
        .await
        .expect("second delete transaction starts");
    assert!(
        !delete_project_in_transaction(&mut tx, project_id)
            .await
            .expect("second delete runs"),
        "a deleted project is not found again"
    );
    tx.rollback().await.expect("rollback");
}
