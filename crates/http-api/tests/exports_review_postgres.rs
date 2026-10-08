//! Postgres-backed checks for the review exports: extraction, appraisal, included studies, and
//! the richer report columns. Runs only when `TEST_DATABASE_URL` points at an empty database.

#![allow(
    clippy::expect_used,
    clippy::panic,
    clippy::unwrap_used,
    clippy::string_slice
)]

use std::collections::HashMap;
use std::sync::OnceLock;

use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode, header},
};
use chrono::{Duration, Utc};
use deepref_http_api::{config::ApiConfig, routes::router, state::AppState};
use serde_json::{Value, json};
use sqlx::{PgPool, postgres::PgPoolOptions};
use tower::ServiceExt;
use uuid::Uuid;

static DATABASE_TEST_MUTEX: OnceLock<tokio::sync::Mutex<()>> = OnceLock::new();

fn test_lock() -> &'static tokio::sync::Mutex<()> {
    DATABASE_TEST_MUTEX.get_or_init(tokio::sync::Mutex::default)
}

async fn database() -> Option<PgPool> {
    let url = std::env::var("TEST_DATABASE_URL").ok()?;
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
        "deepref-api-review-exports-test",
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

async fn export_text(pool: &PgPool, project_id: Uuid, kind: &str) -> String {
    let response = router(AppState::core(pool.clone()), &api_config())
        .oneshot(
            Request::builder()
                .uri(format!("/projects/{project_id}/exports/{kind}"))
                .body(Body::empty())
                .expect("export request should be valid"),
        )
        .await
        .expect("export request should be handled");
    let status = response.status();
    let content_type = response
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_owned();
    let bytes = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("export body should be readable");
    assert_eq!(status, StatusCode::OK, "{kind} status");
    assert!(!content_type.is_empty(), "{kind} content type");
    String::from_utf8(bytes.to_vec()).expect("export should be UTF-8")
}

/// Parses an RFC 4180 table into rows of cells, keyed by the first row's headers.
fn table(text: &str) -> Vec<HashMap<String, String>> {
    let rows: Vec<Vec<String>> = ::csv::ReaderBuilder::new()
        .has_headers(false)
        .from_reader(text.as_bytes())
        .records()
        .map(|row| row.map(|row| row.iter().map(str::to_owned).collect()))
        .collect::<Result<_, _>>()
        .expect("export should be a valid CSV table");
    let Some((headers, body)) = rows.split_first() else {
        return Vec::new();
    };
    body.iter()
        .map(|row| headers.iter().cloned().zip(row.iter().cloned()).collect())
        .collect()
}

fn find<'a>(
    rows: &'a [HashMap<String, String>],
    column: &str,
    value: &str,
) -> &'a HashMap<String, String> {
    rows.iter()
        .find(|row| row.get(column).map(String::as_str) == Some(value))
        .unwrap_or_else(|| panic!("no row with {column} = {value}"))
}

fn cell<'a>(row: &'a HashMap<String, String>, column: &str) -> &'a str {
    row.get(column)
        .map(String::as_str)
        .unwrap_or("<missing column>")
}

#[allow(clippy::too_many_arguments)]
async fn insert_report(
    pool: &PgPool,
    id: Uuid,
    title: &str,
    abstract_text: Option<&str>,
    journal: Option<&str>,
    work_type: Option<&str>,
    authors: Value,
    raw: Value,
) {
    sqlx::query(
        "INSERT INTO reports (id,title,abstract_text,publication_year,journal,work_type,authors,raw)
         VALUES ($1,$2,$3,2015,$4,$5,$6,$7)",
    )
    .bind(id)
    .bind(title)
    .bind(abstract_text)
    .bind(journal)
    .bind(work_type)
    .bind(authors)
    .bind(raw)
    .execute(pool)
    .await
    .expect("report should insert");
}

#[tokio::test]
async fn review_exports_carry_decisions_values_judgments_and_included_studies() {
    let _guard = test_lock().lock().await;
    let Some(pool) = database().await else {
        return;
    };

    let project = Uuid::new_v4();
    let report_included = Uuid::new_v4();
    let report_excluded = Uuid::new_v4();
    let report_ungrouped = Uuid::new_v4();
    let study_gait = Uuid::new_v4();
    let study_unlinked = Uuid::new_v4();
    let reason_adults = Uuid::new_v4();
    let field_sample = Uuid::new_v4();
    let field_device = Uuid::new_v4();
    let field_weeks = Uuid::new_v4();
    let document = Uuid::new_v4();
    let block_sample = Uuid::new_v4();
    let block_weeks = Uuid::new_v4();
    let block_allocation = Uuid::new_v4();
    let hash_sample = "a".repeat(64);
    let hash_weeks = "b".repeat(64);
    let hash_allocation = "c".repeat(64);
    let parser = "parser-v1";
    let now = Utc::now();

    sqlx::query("INSERT INTO projects (id,name) VALUES ($1,'review exports project')")
        .bind(project)
        .execute(&pool)
        .await
        .expect("project should insert");

    insert_report(
        &pool,
        report_included,
        "Gait \"self-monitoring\" pilot, 2015",
        Some("Background {x} & y.\nResults: 50% of 2213 participants; _p_ = .01."),
        Some("Journal of Gait"),
        Some("article"),
        json!([
            {"given": "Ana Maria", "family": "O'Neil & Co.", "literal": null},
            {"given": "J", "family": "Smith", "literal": null}
        ]),
        json!({"volume": "49", "issue": "3", "page": "414-418", "source": "fixture"}),
    )
    .await;
    sqlx::query("INSERT INTO report_identifiers (id,report_id,scheme,value,normalized_value) VALUES ($1,$2,'doi','10.1000/gait.2015.1','10.1000/gait.2015.1')")
        .bind(Uuid::new_v4())
        .bind(report_included)
        .execute(&pool)
        .await
        .expect("DOI should insert");
    insert_report(
        &pool,
        report_excluded,
        "Adult trial that was excluded",
        None,
        Some("Lancet Neurol"),
        None,
        json!([{"given": null, "family": null, "literal": "World Health Organization"}]),
        json!({"fields": {"VL": ["12"], "SP": ["e40"]}}),
    )
    .await;
    insert_report(
        &pool,
        report_ungrouped,
        "Included report not yet grouped",
        None,
        None,
        None,
        json!([]),
        json!({}),
    )
    .await;

    for report in [report_included, report_excluded, report_ungrouped] {
        sqlx::query("INSERT INTO project_reports (project_id,report_id) VALUES ($1,$2)")
            .bind(project)
            .bind(report)
            .execute(&pool)
            .await
            .expect("project report should insert");
    }
    sqlx::query(
        "INSERT INTO exclusion_reasons (id,project_id,code,label,stage)
         VALUES ($1,$2,'wrong-population','Wrong population, adults only','full_text')",
    )
    .bind(reason_adults)
    .bind(project)
    .execute(&pool)
    .await
    .expect("exclusion reason should insert");
    for (report, title_abstract, full_text, reason, final_status) in [
        (report_included, "include", "include", None, "include"),
        (
            report_excluded,
            "include",
            "exclude",
            Some(reason_adults),
            "exclude",
        ),
        (report_ungrouped, "include", "include", None, "include"),
    ] {
        sqlx::query(
            "INSERT INTO screening_state
             (project_id,report_id,title_abstract_status,full_text_status,full_text_exclusion_reason_id,final_status,revision)
             VALUES ($1,$2,$3,$4,$5,$6,1)",
        )
        .bind(project)
        .bind(report)
        .bind(title_abstract)
        .bind(full_text)
        .bind(reason)
        .bind(final_status)
        .execute(&pool)
        .await
        .expect("screening state should insert");
    }

    sqlx::query(
        "INSERT INTO studies (id,project_id,title,design) VALUES ($1,$2,'Gait pilot 2015','rct')",
    )
    .bind(study_gait)
    .bind(project)
    .execute(&pool)
    .await
    .expect("study should insert");
    sqlx::query("INSERT INTO studies (id,project_id,title) VALUES ($1,$2,'Unlinked study')")
        .bind(study_unlinked)
        .bind(project)
        .execute(&pool)
        .await
        .expect("second study should insert");
    for (report, relationship) in [
        (report_included, "report_of_study"),
        (report_excluded, "follow_up"),
    ] {
        sqlx::query(
            "INSERT INTO study_reports (study_id,report_id,project_id,relationship) VALUES ($1,$2,$3,$4)",
        )
        .bind(study_gait)
        .bind(report)
        .bind(project)
        .bind(relationship)
        .execute(&pool)
        .await
        .expect("study report link should insert");
    }

    for (field, key, label, value_type, created_offset) in [
        (field_sample, "sample_size", "Sample size (n)", "number", 3),
        (field_device, "device", "Device used", "text", 2),
        (
            field_weeks,
            "duration_weeks",
            "Duration (weeks)",
            "number",
            1,
        ),
    ] {
        sqlx::query(
            "INSERT INTO extraction_field_definitions (id,project_id,version,field_key,label,value_type,required,created_at)
             VALUES ($1,$2,1,$3,$4,$5,false,$6)",
        )
        .bind(field)
        .bind(project)
        .bind(key)
        .bind(label)
        .bind(value_type)
        .bind(now - Duration::minutes(created_offset))
        .execute(&pool)
        .await
        .expect("field definition should insert");
    }

    sqlx::query(
        "INSERT INTO documents (id,report_id,project_id,mime_type,byte_size,status)
         VALUES ($1,$2,$3,'application/pdf',0,'missing')",
    )
    .bind(document)
    .bind(report_included)
    .bind(project)
    .execute(&pool)
    .await
    .expect("document should insert");
    sqlx::query(
        "INSERT INTO document_pages (document_id,parser_version,page_number,width,height,ocr_required,active)
         VALUES ($1,$2,3,612,792,false,true), ($1,$2,4,612,792,false,true)",
    )
    .bind(document)
    .bind(parser)
    .execute(&pool)
    .await
    .expect("document pages should insert");
    for (block, page, ordinal, text, hash) in [
        (
            block_sample,
            3,
            1,
            "Fifty-one women were randomised.",
            &hash_sample,
        ),
        (
            block_weeks,
            4,
            2,
            "Sixteen weeks of \"self-monitoring\", pilot.",
            &hash_weeks,
        ),
        (
            block_allocation,
            4,
            3,
            "Participants were randomised by a computer, 1:1.",
            &hash_allocation,
        ),
    ] {
        sqlx::query(
            "INSERT INTO document_blocks (id,document_id,parser_version,page_number,kind,ordinal,text,content_hash,active)
             VALUES ($1,$2,$3,$4,'text',$5,$6,$7,true)",
        )
        .bind(block)
        .bind(document)
        .bind(parser)
        .bind(page)
        .bind(ordinal)
        .bind(text)
        .bind(hash)
        .execute(&pool)
        .await
        .expect("document block should insert");
    }

    let ai = "ai:glm-5.3-flash";
    // An AI value waiting for a person.
    sqlx::query(
        "INSERT INTO extraction_values
         (id,project_id,study_id,report_id,field_definition_id,field_definition_version,value_type,number_value,
          rationale,source_document_id,source_block_id,source_page,source_parser_version,source_content_hash,
          approved_by_actor_kind,approved_by_actor_id,needs_verification)
         VALUES ($1,$2,$3,$4,$5,1,'number',51,'Stated in the methods.',$6,$7,3,$8,$9,'automation',$10,true)",
    )
    .bind(Uuid::new_v4())
    .bind(project)
    .bind(study_gait)
    .bind(report_included)
    .bind(field_sample)
    .bind(document)
    .bind(block_sample)
    .bind(parser)
    .bind(&hash_sample)
    .bind(ai)
    .execute(&pool)
    .await
    .expect("AI sample size should insert");
    // A value that a person entered directly.
    sqlx::query(
        "INSERT INTO extraction_values
         (id,project_id,study_id,field_definition_id,field_definition_version,value_type,text_value,rationale,
          approved_by_actor_kind,approved_by_actor_id,needs_verification)
         VALUES ($1,$2,$3,$4,1,'text','Fitbit One','Reviewer entry, \"manual\".','user','reviewer-1',false)",
    )
    .bind(Uuid::new_v4())
    .bind(project)
    .bind(study_gait)
    .bind(field_device)
    .execute(&pool)
    .await
    .expect("manual device should insert");
    // An AI value a person has confirmed.
    sqlx::query(
        "INSERT INTO extraction_values
         (id,project_id,study_id,report_id,field_definition_id,field_definition_version,value_type,number_value,
          rationale,source_document_id,source_block_id,source_page,source_parser_version,source_content_hash,
          approved_by_actor_kind,approved_by_actor_id,needs_verification,verified_at,verified_by_actor_kind,verified_by_actor_id)
         VALUES ($1,$2,$3,$4,$5,1,'number',16,'Intervention length.',$6,$7,4,$8,$9,'automation',$10,false,$11,'user','reviewer-1')",
    )
    .bind(Uuid::new_v4())
    .bind(project)
    .bind(study_gait)
    .bind(report_included)
    .bind(field_weeks)
    .bind(document)
    .bind(block_weeks)
    .bind(parser)
    .bind(&hash_weeks)
    .bind(ai)
    .bind(now)
    .execute(&pool)
    .await
    .expect("confirmed duration should insert");

    let latest_assessment = Uuid::new_v4();
    let older_assessment = Uuid::new_v4();
    for (assessment, completed, responses, judgments) in [
        (
            older_assessment,
            now - Duration::days(1),
            json!({"outcome_measure_prespecified": false}),
            json!({"domains": {"outcome_reporting": "high_concern"}, "overall": "high_concern"}),
        ),
        (
            latest_assessment,
            now - Duration::minutes(1),
            json!({"allocation_description": "yes", "outcome_measure_prespecified": true}),
            json!({
                "domains": {"allocation": "low_concern", "outcome_reporting": "some_concern"},
                "overall": "some_concern"
            }),
        ),
    ] {
        sqlx::query(
            "INSERT INTO appraisal_assessments
             (id,project_id,report_id,definition_id,definition_version,responses,judgments,actor_kind,actor_id,completed_at)
             VALUES ($1,$2,$3,'deepref-rct-generic',1,$4,$5,'user','reviewer-1',$6)",
        )
        .bind(assessment)
        .bind(project)
        .bind(report_included)
        .bind(responses)
        .bind(judgments)
        .bind(completed)
        .execute(&pool)
        .await
        .expect("appraisal assessment should insert");
    }
    sqlx::query(
        "INSERT INTO appraisal_assessment_evidence
         (id,assessment_id,project_id,report_id,question_id,document_id,block_id)
         VALUES ($1,$2,$3,$4,'allocation_description',$5,$6)",
    )
    .bind(Uuid::new_v4())
    .bind(latest_assessment)
    .bind(project)
    .bind(report_included)
    .bind(document)
    .bind(block_allocation)
    .execute(&pool)
    .await
    .expect("appraisal evidence should insert");

    // Reports: decisions, reason, abstract, authors, and volume/issue/pages from both sources.
    let reports = table(&export_text(&pool, project, "reports.csv").await);
    assert_eq!(reports.len(), 3);
    let included = find(&reports, "report_id", &report_included.to_string());
    assert_eq!(
        cell(included, "authors"),
        "O'Neil & Co., Ana Maria; Smith, J"
    );
    assert_eq!(cell(included, "title_abstract_decision"), "include");
    assert_eq!(cell(included, "full_text_decision"), "include");
    assert_eq!(cell(included, "study_title"), "Gait pilot 2015");
    assert_eq!(cell(included, "volume"), "49");
    assert_eq!(cell(included, "pages"), "414-418");
    assert_eq!(
        cell(included, "abstract"),
        "Background {x} & y.\nResults: 50% of 2213 participants; _p_ = .01."
    );
    let excluded = find(&reports, "report_id", &report_excluded.to_string());
    assert_eq!(cell(excluded, "full_text_decision"), "exclude");
    assert_eq!(cell(excluded, "exclusion_reason_code"), "wrong-population");
    assert_eq!(
        cell(excluded, "exclusion_reason_label"),
        "Wrong population, adults only"
    );
    assert_eq!(cell(excluded, "volume"), "12");
    assert_eq!(cell(excluded, "pages"), "e40");
    assert_eq!(cell(excluded, "authors"), "World Health Organization");

    let reports_ris = export_text(&pool, project, "reports.ris").await;
    let included_ris = reports_ris
        .split("ER  -")
        .find(|record| record.contains("TI  - Gait"))
        .expect("the included report should be exported");
    assert!(included_ris.contains("TY  - JOUR"), "{included_ris}");
    assert!(included_ris.contains("AB  - Background {x} & y. Results: 50% of 2213"));
    assert!(included_ris.contains("VL  - 49"));
    assert!(included_ris.contains("SP  - 414"));
    assert!(included_ris.contains("EP  - 418"));
    assert!(included_ris.contains("DO  - 10.1000/gait.2015.1"));
    let ungrouped_ris = reports_ris
        .split("ER  -")
        .find(|record| record.contains("TI  - Included report not yet grouped"))
        .expect("the ungrouped report should be exported");
    assert!(ungrouped_ris.contains("TY  - GEN"), "{ungrouped_ris}");

    let reports_bib = export_text(&pool, project, "reports.bib").await;
    assert!(reports_bib.contains("@article{report-"));
    assert!(reports_bib.contains("  abstract = {Background \\textbraceleft{}x\\textbraceright{}"));
    assert!(reports_bib.contains("  doi = {10.1000/gait.2015.1},"));
    assert!(reports_bib.contains("@misc{report-"));

    // Extraction: one row per study and field, with status, actors, source page and quote.
    let extraction = export_text(&pool, project, "extraction.csv").await;
    let extraction_rows = table(&extraction);
    assert_eq!(extraction_rows.len(), 6, "two studies times three fields");
    let sample = extraction_rows
        .iter()
        .find(|row| {
            cell(row, "study_title") == "Gait pilot 2015" && cell(row, "field_key") == "sample_size"
        })
        .expect("AI sample size row");
    assert_eq!(cell(sample, "value"), "51");
    assert_eq!(cell(sample, "status"), "to_verify");
    assert_eq!(cell(sample, "ai_proposed"), "true");
    assert_eq!(cell(sample, "entered_by"), "AI model glm-5.3-flash");
    assert_eq!(cell(sample, "confirmed_by"), "");
    assert_eq!(cell(sample, "source_page"), "3");
    assert_eq!(
        cell(sample, "source_quote"),
        "Fifty-one women were randomised."
    );
    assert_eq!(cell(sample, "field_type"), "number");
    let device = extraction_rows
        .iter()
        .find(|row| {
            cell(row, "study_title") == "Gait pilot 2015" && cell(row, "field_key") == "device"
        })
        .expect("manual device row");
    assert_eq!(cell(device, "value"), "Fitbit One");
    assert_eq!(cell(device, "status"), "entered");
    assert_eq!(cell(device, "entered_by"), "User reviewer-1");
    assert_eq!(cell(device, "rationale"), "Reviewer entry, \"manual\".");
    let weeks = extraction_rows
        .iter()
        .find(|row| {
            cell(row, "study_title") == "Gait pilot 2015"
                && cell(row, "field_key") == "duration_weeks"
        })
        .expect("confirmed duration row");
    assert_eq!(cell(weeks, "value"), "16");
    assert_eq!(cell(weeks, "status"), "confirmed");
    assert_eq!(cell(weeks, "confirmed_by"), "User reviewer-1");
    assert!(cell(weeks, "confirmed_at").starts_with(&now.format("%Y-%m-%d").to_string()));
    assert_eq!(
        cell(weeks, "source_quote"),
        "Sixteen weeks of \"self-monitoring\", pilot."
    );
    let unlinked = extraction_rows
        .iter()
        .filter(|row| cell(row, "study_title") == "Unlinked study")
        .collect::<Vec<_>>();
    assert_eq!(unlinked.len(), 3);
    assert!(
        unlinked
            .iter()
            .all(|row| cell(row, "status") == "not_extracted")
    );

    // Appraisal: only the latest assessment, with labels, judgments, tool and evidence.
    let appraisal = export_text(&pool, project, "appraisal.csv").await;
    assert!(
        !appraisal.contains("high_concern"),
        "superseded judgments must not be exported"
    );
    let appraisal_rows = table(&appraisal);
    assert_eq!(
        appraisal_rows.len(),
        5,
        "two questions, two domains and overall"
    );
    let question = find(&appraisal_rows, "item_id", "allocation_description");
    assert_eq!(cell(question, "tool_id"), "deepref-rct-generic");
    assert_eq!(
        cell(question, "tool_name"),
        "DeepRef generic intervention appraisal"
    );
    assert_eq!(cell(question, "tool_version"), "1");
    assert_eq!(cell(question, "assessor"), "User reviewer-1");
    assert_eq!(cell(question, "answer"), "yes");
    assert_eq!(cell(question, "answer_label"), "Yes");
    assert_eq!(cell(question, "evidence_pages"), "4");
    assert_eq!(
        cell(question, "evidence_quotes"),
        "p. 4: Participants were randomised by a computer, 1:1."
    );
    let overall = find(&appraisal_rows, "item_type", "overall_judgment");
    assert_eq!(cell(overall, "answer"), "some_concern");
    assert_eq!(cell(overall, "answer_label"), "Some concern");
    assert_eq!(
        cell(overall, "report_title"),
        "Gait \"self-monitoring\" pilot, 2015"
    );

    // Included studies: every linked report of the included study, then ungrouped included reports.
    let included_studies = table(&export_text(&pool, project, "included_studies.csv").await);
    assert_eq!(included_studies.len(), 3);
    let grouped = find(&included_studies, "report_id", &report_included.to_string());
    assert_eq!(cell(grouped, "study_id"), study_gait.to_string());
    assert_eq!(cell(grouped, "study_design"), "rct");
    assert_eq!(cell(grouped, "relationship"), "report_of_study");
    assert_eq!(cell(grouped, "doi"), "10.1000/gait.2015.1");
    let follow_up = find(&included_studies, "report_id", &report_excluded.to_string());
    assert_eq!(cell(follow_up, "relationship"), "follow_up");
    assert_eq!(cell(follow_up, "full_text_decision"), "exclude");
    let ungrouped = find(
        &included_studies,
        "report_id",
        &report_ungrouped.to_string(),
    );
    assert_eq!(cell(ungrouped, "study_id"), "");
    assert_eq!(cell(ungrouped, "relationship"), "");
    assert_eq!(cell(ungrouped, "title_abstract_decision"), "include");

    let audit = export_text(&pool, project, "audit.csv").await;
    let audit_header = audit.lines().next().unwrap_or_default();
    assert!(audit_header.starts_with(
        "id,created_at,event_type,aggregate_type,aggregate_id,actor_kind,actor_id,actor_label,"
    ));
    assert!(audit_header.ends_with(",previous_snapshot,result_snapshot,payload,provenance"));

    sqlx::query("DELETE FROM projects WHERE id=$1")
        .bind(project)
        .execute(&pool)
        .await
        .expect("test project should clean up");
    for report in [report_included, report_excluded, report_ungrouped] {
        sqlx::query("DELETE FROM reports WHERE id=$1")
            .bind(report)
            .execute(&pool)
            .await
            .expect("test report should clean up");
    }
}
