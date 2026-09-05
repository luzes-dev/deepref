#![allow(
    clippy::expect_used,
    clippy::panic,
    clippy::unwrap_used,
    clippy::string_slice
)]
use deepref_application::{
    ProposalDecision, RawAuthor, RawIdentifier, RawRecord, RecordResolutionAction,
    ResolveRecordCommand,
};
use deepref_domain::IdentifierScheme;
use deepref_postgres::{
    DedupeError, DedupeRunRequest, ProposalDecisionRequest, decide_proposal, get_prisma_projection,
    list_proposals, migrate, persist_import, resolve_record, run_deduplication,
};
use serde_json::json;
use sqlx::{PgPool, Row, postgres::PgPoolOptions};
use uuid::Uuid;

async fn database() -> Option<PgPool> {
    let url = std::env::var("DATABASE_URL").ok()?;
    let pool = PgPoolOptions::new()
        .max_connections(12)
        .connect(&url)
        .await
        .ok()?;
    migrate(&pool).await.ok()?;
    Some(pool)
}

async fn project(pool: &PgPool, name: &str) -> Uuid {
    let id = Uuid::new_v4();
    sqlx::query("INSERT INTO projects (id,name) VALUES ($1,$2)")
        .bind(id)
        .bind(name)
        .execute(pool)
        .await
        .unwrap();
    id
}

async fn import(pool: &PgPool, project_id: Uuid, key: &str, record: RawRecord) -> Uuid {
    persist_import(
        pool,
        &deepref_postgres::ImportPersistRequest {
            project_id,
            source: "test-import".to_owned(),
            strategy: "fixture".to_owned(),
            format: deepref_domain::ImportFormat::Csv,
            idempotency_key: Some(key.to_owned()),
            config: json!({"key": key}),
            metadata: json!({"test": true}),
        },
        &[record],
    )
    .await
    .unwrap()
    .run_id
}

fn record(title: &str, doi: Option<&str>, year: Option<i32>, author: Option<&str>) -> RawRecord {
    RawRecord {
        source_identifiers: doi
            .into_iter()
            .map(|value| RawIdentifier {
                scheme: IdentifierScheme::Doi,
                value: value.to_owned(),
                normalized_value: value.to_lowercase(),
            })
            .collect(),
        title: Some(title.to_owned()),
        abstract_text: Some("fixture abstract".to_owned()),
        authors: author
            .map(|family| vec![RawAuthor::named(None, Some(family.to_owned()))])
            .unwrap_or_default(),
        publication_year: year,
        journal: Some("Fixture Journal".to_owned()),
        raw: json!({"fixture": title}),
    }
}

fn run_request(project_id: Uuid, limit: i64) -> DedupeRunRequest {
    DedupeRunRequest {
        project_id,
        limit,
        actor_kind: "system".to_owned(),
        actor_id: "dedupe-test".to_owned(),
    }
}

async fn records_for_run(pool: &PgPool, run_id: Uuid) -> Vec<(Uuid, Option<Uuid>)> {
    sqlx::query("SELECT id,report_id FROM records WHERE acquisition_run_id=$1 ORDER BY id")
        .bind(run_id)
        .fetch_all(pool)
        .await
        .unwrap()
        .into_iter()
        .map(|row| (row.get("id"), row.get("report_id")))
        .collect()
}

#[tokio::test]
async fn exact_identifier_resolution_preserves_source_records_and_prisma_counts() {
    let Some(pool) = database().await else { return };
    let project_id = project(&pool, "exact identifier resolution").await;
    let first_run = import(
        &pool,
        project_id,
        "exact-first",
        record(
            "A durable DOI record",
            Some("10.5555/exact"),
            Some(2024),
            Some("Smith"),
        ),
    )
    .await;
    let second_run = import(
        &pool,
        project_id,
        "exact-second",
        record(
            "The same DOI from another source",
            Some("10.5555/exact"),
            Some(2024),
            None,
        ),
    )
    .await;

    let summary = run_deduplication(&pool, run_request(project_id, 100))
        .await
        .unwrap();
    assert_eq!(summary.auto_linked, 1);
    assert_eq!(summary.created_reports, 1);
    assert_eq!(summary.proposals_created, 0);
    let first = records_for_run(&pool, first_run).await;
    let second = records_for_run(&pool, second_run).await;
    assert_eq!(first.len(), 1);
    assert_eq!(second.len(), 1);
    assert_eq!(first[0].1, second[0].1);
    let report_count: i64 = sqlx::query_scalar("SELECT count(*) FROM reports WHERE id=$1")
        .bind(first[0].1.unwrap())
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(report_count, 1);

    let prisma = get_prisma_projection(&pool, project_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(prisma.identified_records.get(), 2);
    assert_eq!(prisma.linked_records.get(), 2);
    assert_eq!(prisma.duplicates_removed.get(), 1);
    assert_eq!(prisma.source_canonical_reports.get(), 1);
    assert_eq!(prisma.screened_records.get(), 1);
    assert_eq!(prisma.unresolved_records.get(), 0);
    assert!(prisma.validate().is_ok());
    sqlx::query("DELETE FROM projects WHERE id=$1")
        .bind(project_id)
        .execute(&pool)
        .await
        .unwrap();
}

#[tokio::test]
async fn exact_link_propagates_new_identifiers_for_later_convergence() {
    let Some(pool) = database().await else { return };
    let project_id = project(&pool, "identifier propagation").await;
    let first_run = import(
        &pool,
        project_id,
        "propagation-first",
        record(
            "Propagation report",
            Some("10.5555/propagation"),
            None,
            None,
        ),
    )
    .await;
    run_deduplication(&pool, run_request(project_id, 100))
        .await
        .unwrap();

    let mut second = record(
        "Propagation report from another source",
        Some("10.5555/propagation"),
        None,
        None,
    );
    second.source_identifiers.push(RawIdentifier {
        scheme: IdentifierScheme::Pmid,
        value: "24680135".to_owned(),
        normalized_value: "24680135".to_owned(),
    });
    let second_run = import(&pool, project_id, "propagation-second", second).await;
    run_deduplication(&pool, run_request(project_id, 100))
        .await
        .unwrap();

    let third_run = import(
        &pool,
        project_id,
        "propagation-third",
        RawRecord {
            source_identifiers: vec![RawIdentifier {
                scheme: IdentifierScheme::Pmid,
                value: "24680135".to_owned(),
                normalized_value: "24680135".to_owned(),
            }],
            title: Some("PMID-only source".to_owned()),
            abstract_text: None,
            authors: Vec::new(),
            publication_year: None,
            journal: None,
            raw: json!({"fixture": "pmid-only"}),
        },
    )
    .await;
    run_deduplication(&pool, run_request(project_id, 100))
        .await
        .unwrap();

    let first_report = records_for_run(&pool, first_run).await[0].1;
    assert_eq!(first_report, records_for_run(&pool, second_run).await[0].1);
    assert_eq!(first_report, records_for_run(&pool, third_run).await[0].1);
    let propagated: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM report_identifiers
         WHERE report_id=$1 AND scheme='pmid' AND normalized_value='24680135'",
    )
    .bind(first_report.unwrap())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(propagated, 1);
    sqlx::query("DELETE FROM projects WHERE id=$1")
        .bind(project_id)
        .execute(&pool)
        .await
        .unwrap();
}

#[tokio::test]
async fn fuzzy_accept_propagates_new_identifiers_for_later_convergence() {
    let Some(pool) = database().await else { return };
    let project_id = project(&pool, "fuzzy identifier propagation").await;
    let first_run = import(
        &pool,
        project_id,
        "fuzzy-propagation-first",
        record("Fuzzy propagation report", None, Some(2024), Some("Smith")),
    )
    .await;
    run_deduplication(&pool, run_request(project_id, 100))
        .await
        .unwrap();

    let second_run = import(
        &pool,
        project_id,
        "fuzzy-propagation-second",
        RawRecord {
            source_identifiers: vec![RawIdentifier {
                scheme: IdentifierScheme::Pmid,
                value: "13572468".to_owned(),
                normalized_value: "13572468".to_owned(),
            }],
            title: Some("Fuzzy propagation report".to_owned()),
            abstract_text: None,
            authors: vec![RawAuthor::named(None, Some("Smith".to_owned()))],
            publication_year: Some(2024),
            journal: None,
            raw: json!({"fixture": "fuzzy-propagation"}),
        },
    )
    .await;
    run_deduplication(&pool, run_request(project_id, 100))
        .await
        .unwrap();
    let second_record_id = records_for_run(&pool, second_run).await[0].0;
    let proposal = list_proposals(&pool, project_id, "pending", None, 100)
        .await
        .unwrap()
        .into_iter()
        .find(|proposal| proposal.record_id == second_record_id)
        .unwrap();
    let canonical_report = records_for_run(&pool, first_run).await[0].1.unwrap();
    assert_eq!(proposal.candidate_report_id, Some(canonical_report));
    decide_proposal(
        &pool,
        ProposalDecisionRequest {
            project_id,
            proposal_id: proposal.id,
            decision: ProposalDecision::Accept,
            reason: "same title, author, year, and reviewed source identifier".to_owned(),
            actor_kind: "user".to_owned(),
            actor_id: "tester".to_owned(),
        },
    )
    .await
    .unwrap();

    let third_run = import(
        &pool,
        project_id,
        "fuzzy-propagation-third",
        RawRecord {
            source_identifiers: vec![RawIdentifier {
                scheme: IdentifierScheme::Pmid,
                value: "13572468".to_owned(),
                normalized_value: "13572468".to_owned(),
            }],
            title: Some("PMID-only fuzzy follow-up".to_owned()),
            abstract_text: None,
            authors: Vec::new(),
            publication_year: None,
            journal: None,
            raw: json!({"fixture": "fuzzy-propagation-follow-up"}),
        },
    )
    .await;
    run_deduplication(&pool, run_request(project_id, 100))
        .await
        .unwrap();
    assert_eq!(
        records_for_run(&pool, third_run).await[0].1,
        Some(canonical_report)
    );
    sqlx::query("DELETE FROM projects WHERE id=$1")
        .bind(project_id)
        .execute(&pool)
        .await
        .unwrap();
}

#[tokio::test]
async fn manual_link_propagates_identifiers_and_conflicts_roll_back() {
    let Some(pool) = database().await else { return };
    let project_id = project(&pool, "manual identifier propagation").await;
    let canonical_run = import(
        &pool,
        project_id,
        "manual-propagation-canonical",
        record("Manual propagation canonical", None, None, None),
    )
    .await;
    run_deduplication(&pool, run_request(project_id, 100))
        .await
        .unwrap();
    let canonical_report = records_for_run(&pool, canonical_run).await[0].1.unwrap();

    let link_run = import(
        &pool,
        project_id,
        "manual-propagation-link",
        RawRecord {
            source_identifiers: vec![RawIdentifier {
                scheme: IdentifierScheme::Pmid,
                value: "86421357".to_owned(),
                normalized_value: "86421357".to_owned(),
            }],
            title: Some("Manually reviewed source".to_owned()),
            abstract_text: None,
            authors: Vec::new(),
            publication_year: None,
            journal: None,
            raw: json!({"fixture": "manual-link"}),
        },
    )
    .await;
    let link_record_id = records_for_run(&pool, link_run).await[0].0;
    resolve_record(
        &pool,
        ResolveRecordCommand {
            project_id: project_id.into(),
            record_id: link_record_id.into(),
            action: RecordResolutionAction::Link,
            report_id: Some(canonical_report.into()),
            proposal_id: None,
            reason: "manual evidence review confirmed identity".to_owned(),
            actor_kind: "user".to_owned(),
            actor_id: "tester".to_owned(),
        },
    )
    .await
    .unwrap();
    let follow_up_run = import(
        &pool,
        project_id,
        "manual-propagation-follow-up",
        RawRecord {
            source_identifiers: vec![RawIdentifier {
                scheme: IdentifierScheme::Pmid,
                value: "86421357".to_owned(),
                normalized_value: "86421357".to_owned(),
            }],
            title: Some("Identifier-only manual follow-up".to_owned()),
            abstract_text: None,
            authors: Vec::new(),
            publication_year: None,
            journal: None,
            raw: json!({"fixture": "manual-follow-up"}),
        },
    )
    .await;
    run_deduplication(&pool, run_request(project_id, 100))
        .await
        .unwrap();
    assert_eq!(
        records_for_run(&pool, follow_up_run).await[0].1,
        Some(canonical_report)
    );

    let conflicting_report_run = import(
        &pool,
        project_id,
        "manual-propagation-owner",
        record("Identifier owner", Some("10.5555/manual-owner"), None, None),
    )
    .await;
    run_deduplication(&pool, run_request(project_id, 100))
        .await
        .unwrap();
    let conflicting_report = records_for_run(&pool, conflicting_report_run).await[0]
        .1
        .unwrap();
    let conflict_run = import(
        &pool,
        project_id,
        "manual-propagation-conflict",
        RawRecord {
            source_identifiers: vec![
                RawIdentifier {
                    scheme: IdentifierScheme::Doi,
                    value: "10.5555/manual-new".to_owned(),
                    normalized_value: "10.5555/manual-new".to_owned(),
                },
                RawIdentifier {
                    scheme: IdentifierScheme::Doi,
                    value: "10.5555/manual-owner".to_owned(),
                    normalized_value: "10.5555/manual-owner".to_owned(),
                },
            ],
            title: Some("Manual conflict source".to_owned()),
            abstract_text: None,
            authors: Vec::new(),
            publication_year: None,
            journal: None,
            raw: json!({"fixture": "manual-conflict"}),
        },
    )
    .await;
    let conflict_record_id = records_for_run(&pool, conflict_run).await[0].0;
    let error = resolve_record(
        &pool,
        ResolveRecordCommand {
            project_id: project_id.into(),
            record_id: conflict_record_id.into(),
            action: RecordResolutionAction::Link,
            report_id: Some(canonical_report.into()),
            proposal_id: None,
            reason: "must not steal an identifier".to_owned(),
            actor_kind: "user".to_owned(),
            actor_id: "tester".to_owned(),
        },
    )
    .await
    .unwrap_err();
    assert!(matches!(error, DedupeError::IdentifierConflict));
    assert!(records_for_run(&pool, conflict_run).await[0].1.is_none());
    let new_identifier_count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM report_identifiers
         WHERE report_id=$1 AND scheme='doi' AND normalized_value='10.5555/manual-new'",
    )
    .bind(canonical_report)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(new_identifier_count, 0);
    assert_eq!(
        sqlx::query_scalar::<_, Uuid>(
            "SELECT report_id FROM report_identifiers
             WHERE scheme='doi' AND normalized_value='10.5555/manual-owner'",
        )
        .fetch_one(&pool)
        .await
        .unwrap(),
        conflicting_report
    );
    sqlx::query("DELETE FROM projects WHERE id=$1")
        .bind(project_id)
        .execute(&pool)
        .await
        .unwrap();
}

#[tokio::test]
async fn unicode_compatible_titles_are_authoritatively_shortlisted() {
    let Some(pool) = database().await else { return };
    let project_id = project(&pool, "unicode title shortlist").await;
    let report_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO reports (id,title,publication_year,authors,normalized_title,raw)
         VALUES ($1,$2,$3,$4,$5,$6)",
    )
    .bind(report_id)
    .bind("Ｆｕｌｌｗｉｄｔｈ evidence review")
    .bind(2024_i32)
    .bind(json!([]))
    .bind("ｆｕｌｌｗｉｄｔｈ evidence review")
    .bind(json!({"legacy": true}))
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query("INSERT INTO project_reports (project_id,report_id) VALUES ($1,$2)")
        .bind(project_id)
        .bind(report_id)
        .execute(&pool)
        .await
        .unwrap();
    let run_id = import(
        &pool,
        project_id,
        "unicode-shortlist-source",
        record("Fullwidth evidence review", None, Some(2024), None),
    )
    .await;
    let summary = run_deduplication(&pool, run_request(project_id, 100))
        .await
        .unwrap();
    assert_eq!(summary.proposals_created, 1);
    assert!(records_for_run(&pool, run_id).await[0].1.is_none());
    let normalized: String = sqlx::query_scalar("SELECT normalized_title FROM reports WHERE id=$1")
        .bind(report_id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(normalized, "fullwidth evidence review");
    sqlx::query("DELETE FROM projects WHERE id=$1")
        .bind(project_id)
        .execute(&pool)
        .await
        .unwrap();
}

#[tokio::test]
async fn multiple_identifiers_converge_but_conflicts_become_proposals() {
    let Some(pool) = database().await else { return };
    let project_id = project(&pool, "identifier convergence").await;
    let mut first_record = record(
        "Converging report",
        Some("10.5555/converge"),
        Some(2024),
        None,
    );
    first_record.source_identifiers.push(RawIdentifier {
        scheme: IdentifierScheme::Pmid,
        value: "12345678".to_owned(),
        normalized_value: "12345678".to_owned(),
    });
    let mut second_record = record(
        "Same report with matching identifiers",
        Some("10.5555/converge"),
        Some(2024),
        None,
    );
    second_record.source_identifiers.push(RawIdentifier {
        scheme: IdentifierScheme::Pmid,
        value: "12345678".to_owned(),
        normalized_value: "12345678".to_owned(),
    });
    let first_run = import(&pool, project_id, "converge-first", first_record).await;
    let second_run = import(&pool, project_id, "converge-second", second_record).await;
    run_deduplication(&pool, run_request(project_id, 100))
        .await
        .unwrap();
    assert_eq!(
        records_for_run(&pool, first_run).await[0].1,
        records_for_run(&pool, second_run).await[0].1
    );

    let other_run = import(
        &pool,
        project_id,
        "conflict-other",
        record("Other report", Some("10.5555/other"), Some(2023), None),
    )
    .await;
    run_deduplication(&pool, run_request(project_id, 100))
        .await
        .unwrap();
    let conflict_run = import(
        &pool,
        project_id,
        "conflict-input",
        RawRecord {
            source_identifiers: vec![
                RawIdentifier {
                    scheme: IdentifierScheme::Doi,
                    value: "10.5555/converge".to_owned(),
                    normalized_value: "10.5555/converge".to_owned(),
                },
                RawIdentifier {
                    scheme: IdentifierScheme::Doi,
                    value: "10.5555/other".to_owned(),
                    normalized_value: "10.5555/other".to_owned(),
                },
            ],
            title: Some("Conflicting durable identifiers".to_owned()),
            abstract_text: None,
            authors: Vec::new(),
            publication_year: Some(2024),
            journal: None,
            raw: json!({"conflict": true}),
        },
    )
    .await;
    let summary = run_deduplication(&pool, run_request(project_id, 100))
        .await
        .unwrap();
    assert_eq!(summary.conflicts, 1);
    assert!(records_for_run(&pool, conflict_run).await[0].1.is_none());
    let pending = list_proposals(&pool, project_id, "pending", None, 100)
        .await
        .unwrap();
    assert!(
        pending
            .iter()
            .any(|proposal| proposal.conflicting_identifier)
    );
    assert!(records_for_run(&pool, other_run).await[0].1.is_some());
    sqlx::query("DELETE FROM projects WHERE id=$1")
        .bind(project_id)
        .execute(&pool)
        .await
        .unwrap();
}

#[tokio::test]
async fn conflict_create_new_is_typed_and_has_no_partial_write() {
    let Some(pool) = database().await else { return };
    let project_id = project(&pool, "conflict create new").await;
    let left_run = import(
        &pool,
        project_id,
        "conflict-create-left",
        record("Conflict left", Some("10.5555/conflict-left"), None, None),
    )
    .await;
    let right_run = import(
        &pool,
        project_id,
        "conflict-create-right",
        record("Conflict right", Some("10.5555/conflict-right"), None, None),
    )
    .await;
    run_deduplication(&pool, run_request(project_id, 100))
        .await
        .unwrap();
    let conflict_run = import(
        &pool,
        project_id,
        "conflict-create-source",
        RawRecord {
            source_identifiers: vec![
                RawIdentifier {
                    scheme: IdentifierScheme::Doi,
                    value: "10.5555/conflict-left".to_owned(),
                    normalized_value: "10.5555/conflict-left".to_owned(),
                },
                RawIdentifier {
                    scheme: IdentifierScheme::Doi,
                    value: "10.5555/conflict-right".to_owned(),
                    normalized_value: "10.5555/conflict-right".to_owned(),
                },
            ],
            title: Some("Conflicting create-new source".to_owned()),
            abstract_text: None,
            authors: Vec::new(),
            publication_year: None,
            journal: None,
            raw: json!({"fixture": "conflict-create"}),
        },
    )
    .await;
    run_deduplication(&pool, run_request(project_id, 100))
        .await
        .unwrap();
    let conflict_record_id = records_for_run(&pool, conflict_run).await[0].0;
    let proposal = list_proposals(&pool, project_id, "pending", None, 100)
        .await
        .unwrap()
        .into_iter()
        .find(|item| item.record_id == conflict_record_id)
        .unwrap();
    let report_count_before: i64 = sqlx::query_scalar("SELECT count(*) FROM reports")
        .fetch_one(&pool)
        .await
        .unwrap();
    let error = decide_proposal(
        &pool,
        ProposalDecisionRequest {
            project_id,
            proposal_id: proposal.id,
            decision: ProposalDecision::CreateNew,
            reason: "conflict cannot create a duplicate identifier owner".to_owned(),
            actor_kind: "user".to_owned(),
            actor_id: "tester".to_owned(),
        },
    )
    .await
    .unwrap_err();
    assert!(matches!(error, DedupeError::ConflictCreateNew));
    let events_before: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM dedupe_resolution_events WHERE project_id=$1 AND record_id=$2",
    )
    .bind(project_id)
    .bind(conflict_record_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    let resolve_error = resolve_record(
        &pool,
        ResolveRecordCommand {
            project_id: project_id.into(),
            record_id: conflict_record_id.into(),
            action: RecordResolutionAction::Create,
            report_id: None,
            proposal_id: Some(proposal.id),
            reason: "conflict proposals cannot create a new report".to_owned(),
            actor_kind: "user".to_owned(),
            actor_id: "tester".to_owned(),
        },
    )
    .await
    .unwrap_err();
    assert!(matches!(resolve_error, DedupeError::ConflictCreateNew));
    let report_count_after: i64 = sqlx::query_scalar("SELECT count(*) FROM reports")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(report_count_after, report_count_before);
    let events_after: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM dedupe_resolution_events WHERE project_id=$1 AND record_id=$2",
    )
    .bind(project_id)
    .bind(conflict_record_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(events_after, events_before);
    let proposal_status: String =
        sqlx::query_scalar("SELECT status FROM dedupe_proposals WHERE project_id=$1 AND id=$2")
            .bind(project_id)
            .bind(proposal.id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(proposal_status, "pending");
    assert!(records_for_run(&pool, conflict_run).await[0].1.is_none());
    assert!(records_for_run(&pool, left_run).await[0].1.is_some());
    assert!(records_for_run(&pool, right_run).await[0].1.is_some());
    sqlx::query("DELETE FROM projects WHERE id=$1")
        .bind(project_id)
        .execute(&pool)
        .await
        .unwrap();
}

#[tokio::test]
async fn accepting_conflict_supersedes_all_sibling_proposals() {
    let Some(pool) = database().await else { return };
    let project_id = project(&pool, "conflict sibling closure").await;
    for (key, title, doi) in [
        ("sibling-left", "Sibling left", "10.5555/sibling-left"),
        ("sibling-right", "Sibling right", "10.5555/sibling-right"),
    ] {
        import(&pool, project_id, key, record(title, Some(doi), None, None)).await;
    }
    run_deduplication(&pool, run_request(project_id, 100))
        .await
        .unwrap();
    let conflict_run = import(
        &pool,
        project_id,
        "sibling-source",
        RawRecord {
            source_identifiers: vec![
                RawIdentifier {
                    scheme: IdentifierScheme::Doi,
                    value: "10.5555/sibling-left".to_owned(),
                    normalized_value: "10.5555/sibling-left".to_owned(),
                },
                RawIdentifier {
                    scheme: IdentifierScheme::Doi,
                    value: "10.5555/sibling-right".to_owned(),
                    normalized_value: "10.5555/sibling-right".to_owned(),
                },
            ],
            title: Some("Sibling conflict".to_owned()),
            abstract_text: None,
            authors: Vec::new(),
            publication_year: None,
            journal: None,
            raw: json!({"fixture": "sibling"}),
        },
    )
    .await;
    run_deduplication(&pool, run_request(project_id, 100))
        .await
        .unwrap();
    let record_id = records_for_run(&pool, conflict_run).await[0].0;
    let proposals = list_proposals(&pool, project_id, "pending", None, 100)
        .await
        .unwrap()
        .into_iter()
        .filter(|item| item.record_id == record_id)
        .collect::<Vec<_>>();
    assert_eq!(proposals.len(), 2);
    let selected = proposals[0].clone();
    let sibling = proposals[1].clone();
    decide_proposal(
        &pool,
        ProposalDecisionRequest {
            project_id,
            proposal_id: selected.id,
            decision: ProposalDecision::Accept,
            reason: "selected candidate after conflict review".to_owned(),
            actor_kind: "user".to_owned(),
            actor_id: "tester".to_owned(),
        },
    )
    .await
    .unwrap();
    let pending = list_proposals(&pool, project_id, "pending", None, 100)
        .await
        .unwrap();
    assert!(!pending.iter().any(|item| item.record_id == record_id));
    let rejected = list_proposals(&pool, project_id, "rejected", None, 100)
        .await
        .unwrap();
    let sibling_state = rejected.iter().find(|item| item.id == sibling.id).unwrap();
    assert_eq!(sibling_state.revision, 1);
    assert_eq!(sibling_state.reviewer_kind.as_deref(), Some("user"));
    assert_eq!(sibling_state.reviewer_id.as_deref(), Some("tester"));
    assert_eq!(
        sibling_state.decision_reason.as_deref(),
        Some("selected candidate after conflict review")
    );
    assert_eq!(sibling_state.metadata["action"], "superseded");
    let error = decide_proposal(
        &pool,
        ProposalDecisionRequest {
            project_id,
            proposal_id: sibling.id,
            decision: ProposalDecision::Accept,
            reason: "stale sibling".to_owned(),
            actor_kind: "user".to_owned(),
            actor_id: "tester".to_owned(),
        },
    )
    .await
    .unwrap_err();
    assert!(matches!(error, DedupeError::ProposalNotPending));
    sqlx::query("DELETE FROM projects WHERE id=$1")
        .bind(project_id)
        .execute(&pool)
        .await
        .unwrap();
}

#[tokio::test]
async fn dedupe_foreign_keys_reject_cross_project_candidate_and_event_rows() {
    let Some(pool) = database().await else { return };
    let project_a = project(&pool, "schema project a").await;
    let project_b = project(&pool, "schema project b").await;
    let record_a_run = import(
        &pool,
        project_a,
        "schema-record-a",
        record("Schema source", None, None, None),
    )
    .await;
    let record_a = records_for_run(&pool, record_a_run).await[0].0;
    let report_b = Uuid::new_v4();
    sqlx::query("INSERT INTO reports (id,title,authors,raw) VALUES ($1,$2,$3,$4)")
        .bind(report_b)
        .bind("Project B report")
        .bind(json!([]))
        .bind(json!({}))
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO project_reports (project_id,report_id) VALUES ($1,$2)")
        .bind(project_b)
        .bind(report_b)
        .execute(&pool)
        .await
        .unwrap();
    let foreign_proposal = sqlx::query(
        "INSERT INTO dedupe_proposals (id,project_id,record_id,candidate_report_id,proposal_kind)
         VALUES ($1,$2,$3,$4,'fuzzy')",
    )
    .bind(Uuid::new_v4())
    .bind(project_a)
    .bind(record_a)
    .bind(report_b)
    .execute(&pool)
    .await;
    assert!(foreign_proposal.is_err());
    let foreign_event = sqlx::query(
        "INSERT INTO dedupe_resolution_events
         (id,project_id,record_id,prior_report_id,action,reason,actor_kind,actor_id)
         VALUES ($1,$2,$3,$4,'reassign','cross-project row','system','schema-test')",
    )
    .bind(Uuid::new_v4())
    .bind(project_a)
    .bind(record_a)
    .bind(report_b)
    .execute(&pool)
    .await;
    assert!(foreign_event.is_err());
    sqlx::query("DELETE FROM projects WHERE id IN ($1,$2)")
        .bind(project_a)
        .bind(project_b)
        .execute(&pool)
        .await
        .unwrap();
}

#[tokio::test]
async fn project_deletion_cleans_dedupe_proposals_and_resolution_events() {
    let Some(pool) = database().await else { return };
    let project_id = project(&pool, "dedupe deletion cleanup").await;
    import(
        &pool,
        project_id,
        "deletion-cleanup-base",
        record("Deletion cleanup study", None, Some(2024), Some("Smith")),
    )
    .await;
    run_deduplication(&pool, run_request(project_id, 100))
        .await
        .unwrap();
    let proposal_run = import(
        &pool,
        project_id,
        "deletion-cleanup-proposal",
        record("Deletion cleanup study", None, Some(2024), Some("Smith")),
    )
    .await;
    run_deduplication(&pool, run_request(project_id, 100))
        .await
        .unwrap();
    let proposal_record_id = records_for_run(&pool, proposal_run).await[0].0;
    let proposal = list_proposals(&pool, project_id, "pending", None, 100)
        .await
        .unwrap()
        .into_iter()
        .find(|proposal| proposal.record_id == proposal_record_id)
        .unwrap();
    decide_proposal(
        &pool,
        ProposalDecisionRequest {
            project_id,
            proposal_id: proposal.id,
            decision: ProposalDecision::Accept,
            reason: "retain a resolution event before project cleanup".to_owned(),
            actor_kind: "user".to_owned(),
            actor_id: "tester".to_owned(),
        },
    )
    .await
    .unwrap();
    let event_count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM dedupe_resolution_events WHERE project_id=$1")
            .bind(project_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(event_count, 2);

    sqlx::query("DELETE FROM projects WHERE id=$1")
        .bind(project_id)
        .execute(&pool)
        .await
        .unwrap();
    let remaining_proposals: i64 =
        sqlx::query_scalar("SELECT count(*) FROM dedupe_proposals WHERE project_id=$1")
            .bind(project_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    let remaining_events: i64 =
        sqlx::query_scalar("SELECT count(*) FROM dedupe_resolution_events WHERE project_id=$1")
            .bind(project_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(remaining_proposals, 0);
    assert_eq!(remaining_events, 0);
}

#[tokio::test]
async fn fuzzy_candidates_are_proposals_and_distinct_titles_create_reports() {
    let Some(pool) = database().await else { return };
    let project_id = project(&pool, "fuzzy candidates").await;
    let base_run = import(
        &pool,
        project_id,
        "fuzzy-base",
        record(
            "Effects of exercise on sleep quality in adults",
            None,
            Some(2024),
            Some("Smith"),
        ),
    )
    .await;
    run_deduplication(&pool, run_request(project_id, 100))
        .await
        .unwrap();
    let fuzzy_run = import(
        &pool,
        project_id,
        "fuzzy-proposal",
        record(
            "Effects of exercise on sleep quality in adult",
            None,
            Some(2024),
            Some("Smith"),
        ),
    )
    .await;
    let distinct_run = import(
        &pool,
        project_id,
        "distinct-report",
        record(
            "A randomized trial of coastal flood defenses",
            None,
            Some(2019),
            Some("Jones"),
        ),
    )
    .await;
    let summary = run_deduplication(&pool, run_request(project_id, 100))
        .await
        .unwrap();
    assert_eq!(summary.proposals_created, 1);
    assert!(records_for_run(&pool, base_run).await[0].1.is_some());
    assert!(records_for_run(&pool, fuzzy_run).await[0].1.is_none());
    assert!(records_for_run(&pool, distinct_run).await[0].1.is_some());
    sqlx::query("DELETE FROM projects WHERE id=$1")
        .bind(project_id)
        .execute(&pool)
        .await
        .unwrap();
}

#[tokio::test]
async fn concurrent_exact_identifier_runs_converge_to_one_report() {
    let Some(pool) = database().await else { return };
    let project_id = project(&pool, "concurrent exact identifiers").await;
    let first_run = import(
        &pool,
        project_id,
        "concurrent-first",
        record(
            "Concurrent DOI one",
            Some("10.5555/concurrent-dedupe"),
            None,
            None,
        ),
    )
    .await;
    let second_run = import(
        &pool,
        project_id,
        "concurrent-second",
        record(
            "Concurrent DOI two",
            Some("10.5555/concurrent-dedupe"),
            None,
            None,
        ),
    )
    .await;
    let (left, right) = tokio::join!(
        run_deduplication(&pool, run_request(project_id, 1)),
        run_deduplication(&pool, run_request(project_id, 1)),
    );
    left.unwrap();
    right.unwrap();
    assert_eq!(
        records_for_run(&pool, first_run).await[0].1,
        records_for_run(&pool, second_run).await[0].1
    );
    let count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM report_identifiers WHERE scheme='doi' AND normalized_value='10.5555/concurrent-dedupe'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(count, 1);
    sqlx::query("DELETE FROM projects WHERE id=$1")
        .bind(project_id)
        .execute(&pool)
        .await
        .unwrap();
}

#[tokio::test]
async fn manual_resolution_is_project_isolated_and_append_only() {
    let Some(pool) = database().await else { return };
    let project_id = project(&pool, "manual resolution").await;
    let other_project_id = project(&pool, "manual isolation").await;
    let _base_run = import(
        &pool,
        project_id,
        "manual-base",
        record("Manual resolution base title", None, Some(2024), None),
    )
    .await;
    run_deduplication(&pool, run_request(project_id, 100))
        .await
        .unwrap();
    let proposal_run = import(
        &pool,
        project_id,
        "manual-proposal",
        record("Manual resolution base title", None, Some(2024), None),
    )
    .await;
    run_deduplication(&pool, run_request(project_id, 100))
        .await
        .unwrap();
    let proposal = list_proposals(&pool, project_id, "pending", None, 100)
        .await
        .unwrap()
        .into_iter()
        .next()
        .unwrap();
    let candidate = proposal.candidate_report_id.unwrap();
    assert_eq!(
        decide_proposal(
            &pool,
            ProposalDecisionRequest {
                project_id: other_project_id,
                proposal_id: proposal.id,
                decision: ProposalDecision::Accept,
                reason: "cross-project attempt".to_owned(),
                actor_kind: "user".to_owned(),
                actor_id: "tester".to_owned(),
            },
        )
        .await
        .unwrap_err()
        .to_string(),
        "deduplication proposal not found in this project"
    );
    decide_proposal(
        &pool,
        ProposalDecisionRequest {
            project_id,
            proposal_id: proposal.id,
            decision: ProposalDecision::Accept,
            reason: "title and year reviewed".to_owned(),
            actor_kind: "user".to_owned(),
            actor_id: "tester".to_owned(),
        },
    )
    .await
    .unwrap();
    assert_eq!(
        records_for_run(&pool, proposal_run).await[0].1,
        Some(candidate)
    );

    let create_new_run = import(
        &pool,
        project_id,
        "manual-create-new-proposal",
        record("Manual resolution base title", None, Some(2024), None),
    )
    .await;
    run_deduplication(&pool, run_request(project_id, 100))
        .await
        .unwrap();
    let create_new_record_id = records_for_run(&pool, create_new_run).await[0].0;
    let create_new_proposal = list_proposals(&pool, project_id, "pending", None, 100)
        .await
        .unwrap()
        .into_iter()
        .find(|item| item.record_id == create_new_record_id)
        .unwrap();
    decide_proposal(
        &pool,
        ProposalDecisionRequest {
            project_id,
            proposal_id: create_new_proposal.id,
            decision: ProposalDecision::CreateNew,
            reason: "keep as a separate report after review".to_owned(),
            actor_kind: "user".to_owned(),
            actor_id: "tester".to_owned(),
        },
    )
    .await
    .unwrap();
    assert!(records_for_run(&pool, create_new_run).await[0].1.is_some());

    let reject_run = import(
        &pool,
        project_id,
        "manual-reject-proposal",
        record("Manual resolution base title", None, Some(2024), None),
    )
    .await;
    run_deduplication(&pool, run_request(project_id, 100))
        .await
        .unwrap();
    let reject_record_id = records_for_run(&pool, reject_run).await[0].0;
    let reject_proposal = list_proposals(&pool, project_id, "pending", None, 100)
        .await
        .unwrap()
        .into_iter()
        .find(|item| item.record_id == reject_record_id)
        .unwrap();
    decide_proposal(
        &pool,
        ProposalDecisionRequest {
            project_id,
            proposal_id: reject_proposal.id,
            decision: ProposalDecision::Reject,
            reason: "not enough evidence to merge".to_owned(),
            actor_kind: "user".to_owned(),
            actor_id: "tester".to_owned(),
        },
    )
    .await
    .unwrap();
    assert!(records_for_run(&pool, reject_run).await[0].1.is_none());

    let events: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM dedupe_resolution_events WHERE project_id=$1 AND record_id=$2",
    )
    .bind(project_id)
    .bind(records_for_run(&pool, proposal_run).await[0].0)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(events, 1);

    let create_run = import(
        &pool,
        project_id,
        "manual-create",
        record("Manual create report", None, None, None),
    )
    .await;
    let create_record_id = records_for_run(&pool, create_run).await[0].0;
    let create_result = resolve_record(
        &pool,
        ResolveRecordCommand {
            project_id: project_id.into(),
            record_id: create_record_id.into(),
            action: RecordResolutionAction::Create,
            report_id: None,
            proposal_id: None,
            reason: "create a reviewed report".to_owned(),
            actor_kind: "user".to_owned(),
            actor_id: "tester".to_owned(),
        },
    )
    .await
    .unwrap();
    resolve_record(
        &pool,
        ResolveRecordCommand {
            project_id: project_id.into(),
            record_id: create_record_id.into(),
            action: RecordResolutionAction::Reassign,
            report_id: Some(candidate.into()),
            proposal_id: None,
            reason: "reassign after review".to_owned(),
            actor_kind: "user".to_owned(),
            actor_id: "tester".to_owned(),
        },
    )
    .await
    .unwrap();
    let revert_result = resolve_record(
        &pool,
        ResolveRecordCommand {
            project_id: project_id.into(),
            record_id: create_record_id.into(),
            action: RecordResolutionAction::Revert,
            report_id: None,
            proposal_id: None,
            reason: "revert mistaken reassign".to_owned(),
            actor_kind: "user".to_owned(),
            actor_id: "tester".to_owned(),
        },
    )
    .await
    .unwrap();
    let action_count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM dedupe_resolution_events WHERE project_id=$1 AND record_id=$2",
    )
    .bind(project_id)
    .bind(create_record_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(action_count, 3);
    assert_eq!(
        records_for_run(&pool, create_run).await[0].1,
        create_result.resolved_report_id
    );
    assert_eq!(
        revert_result.resolved_report_id,
        Some(create_result.resolved_report_id.unwrap())
    );
    let latest_event = sqlx::query(
        "SELECT prior_report_id,resolved_report_id,action,reverted_event_id
         FROM dedupe_resolution_events
         WHERE project_id=$1 AND record_id=$2
         ORDER BY created_at DESC,id DESC LIMIT 1",
    )
    .bind(project_id)
    .bind(create_record_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        latest_event.get::<Option<Uuid>, _>("prior_report_id"),
        Some(candidate)
    );
    assert_eq!(
        latest_event.get::<Option<Uuid>, _>("resolved_report_id"),
        create_result.resolved_report_id
    );
    assert_eq!(latest_event.get::<String, _>("action"), "revert");
    assert!(
        latest_event
            .get::<Option<Uuid>, _>("reverted_event_id")
            .is_some()
    );

    let second_revert = resolve_record(
        &pool,
        ResolveRecordCommand {
            project_id: project_id.into(),
            record_id: create_record_id.into(),
            action: RecordResolutionAction::Revert,
            report_id: None,
            proposal_id: None,
            reason: "undo the earlier create transition".to_owned(),
            actor_kind: "user".to_owned(),
            actor_id: "tester".to_owned(),
        },
    )
    .await
    .unwrap();
    assert_eq!(second_revert.resolved_report_id, None);
    assert!(records_for_run(&pool, create_run).await[0].1.is_none());

    sqlx::query("DELETE FROM projects WHERE id IN ($1,$2)")
        .bind(project_id)
        .bind(other_project_id)
        .execute(&pool)
        .await
        .unwrap();
}
