#![allow(
    clippy::expect_used,
    clippy::panic,
    clippy::unwrap_used,
    clippy::string_slice
)]
use std::collections::HashSet;

use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use chrono::{Duration, Utc};
use deepref_app::screening::{
    ScreeningError, get_screening_history, get_screening_queue, screen_report, undo_screening,
};
use deepref_application::{
    GetScreeningQueueQuery, ScreenReportCommand, ScreeningQueueSort, ScreeningQueueStatus,
    UndoScreeningCommand,
};
use deepref_domain::{Actor, ActorKind, ScreeningDecision, ScreeningStage, ScreeningStatus};
use sqlx::PgPool;
use uuid::Uuid;

async fn database() -> Option<PgPool> {
    deepref_app::test_isolation::isolated_pool_from_env("DATABASE_URL", 20).await
}

async fn fixture(pool: &PgPool, report_count: usize) -> (Uuid, Uuid, Vec<Uuid>) {
    let project_id = Uuid::new_v4();
    let protocol_id = Uuid::new_v4();
    sqlx::query("INSERT INTO projects (id,name) VALUES ($1,'screening persistence fixture')")
        .bind(project_id)
        .execute(pool)
        .await
        .expect("project should insert");
    sqlx::query(
        "INSERT INTO protocol_versions (id,project_id,version,name,status,criteria) VALUES ($1,$2,1,'fixture','published','[]')",
    )
    .bind(protocol_id)
    .bind(project_id)
    .execute(pool)
    .await
    .expect("protocol should insert");

    let mut report_ids = Vec::with_capacity(report_count);
    for index in 0..report_count {
        let report_id = Uuid::new_v4();
        report_ids.push(report_id);
        sqlx::query(
            "INSERT INTO reports (id,title,abstract_text,publication_year) VALUES ($1,$2,$3,$4)",
        )
        .bind(report_id)
        .bind(if index % 3 == 0 {
            None
        } else {
            Some("Tied title")
        })
        .bind(format!("Abstract {index}"))
        .bind(if index % 4 == 0 { None } else { Some(2024) })
        .execute(pool)
        .await
        .expect("report should insert");
        sqlx::query("INSERT INTO project_reports (project_id,report_id) VALUES ($1,$2)")
            .bind(project_id)
            .bind(report_id)
            .execute(pool)
            .await
            .expect("project report should insert");
    }
    (project_id, protocol_id, report_ids)
}

async fn cleanup(pool: &PgPool, project_id: Uuid) {
    sqlx::query("DELETE FROM jobs WHERE payload->>'project_id'=$1")
        .bind(project_id.to_string())
        .execute(pool)
        .await
        .expect("jobs should clean up");
    sqlx::query("DELETE FROM projects WHERE id=$1")
        .bind(project_id)
        .execute(pool)
        .await
        .expect("project should clean up");
}

fn actor() -> Actor {
    Actor::new(ActorKind::User, "screening-test").expect("actor should be valid")
}

fn decision_command(
    project_id: Uuid,
    report_id: Uuid,
    protocol_id: Uuid,
    stage: ScreeningStage,
    decision: ScreeningDecision,
    exclusion_reason_id: Option<Uuid>,
    expected_revision: i64,
) -> ScreenReportCommand {
    ScreenReportCommand {
        project_id: project_id.into(),
        report_id: report_id.into(),
        stage,
        decision,
        exclusion_reason_id: exclusion_reason_id.map(Into::into),
        protocol_version_id: protocol_id.into(),
        expected_revision,
        notes: None,
        actor: actor(),
    }
}

#[tokio::test]
async fn concurrent_first_decisions_have_one_commit_and_one_conflict() {
    let Some(pool) = database().await else { return };
    let (project_id, protocol_id, reports) = fixture(&pool, 1).await;
    let report_id = reports[0];

    let first = decision_command(
        project_id,
        report_id,
        protocol_id,
        ScreeningStage::TitleAbstract,
        ScreeningDecision::Include,
        None,
        0,
    );
    let second = decision_command(
        project_id,
        report_id,
        protocol_id,
        ScreeningStage::TitleAbstract,
        ScreeningDecision::Maybe,
        None,
        0,
    );
    let (left, right) = tokio::join!(screen_report(&pool, first), screen_report(&pool, second));
    let results = [left, right];
    assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
    assert_eq!(
        results
            .iter()
            .filter(|result| matches!(result, Err(ScreeningError::RevisionConflict { current }) if current.revision == 1))
            .count(),
        1
    );

    let event_count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM screening_events WHERE project_id=$1 AND report_id=$2",
    )
    .bind(project_id)
    .bind(report_id)
    .fetch_one(&pool)
    .await
    .expect("event count should be readable");
    let revision: i64 = sqlx::query_scalar(
        "SELECT revision FROM screening_state WHERE project_id=$1 AND report_id=$2",
    )
    .bind(project_id)
    .bind(report_id)
    .fetch_one(&pool)
    .await
    .expect("state revision should be readable");
    assert_eq!(event_count, 1);
    assert_eq!(revision, 1);
    cleanup(&pool, project_id).await;
}

#[tokio::test]
async fn queue_cursor_pages_are_total_ordered_for_every_sort_family() {
    let Some(pool) = database().await else { return };
    let (project_id, _protocol_id, report_ids) = fixture(&pool, 11).await;
    let tied_created_at = Utc::now() - Duration::days(2);
    sqlx::query("UPDATE project_reports SET created_at=$2 WHERE project_id=$1")
        .bind(project_id)
        .bind(tied_created_at)
        .execute(&pool)
        .await
        .expect("queue timestamps should update");

    for sort in [
        ScreeningQueueSort::CreatedAscending,
        ScreeningQueueSort::CreatedDescending,
        ScreeningQueueSort::TitleAscending,
        ScreeningQueueSort::TitleDescending,
        ScreeningQueueSort::YearAscending,
        ScreeningQueueSort::YearDescending,
    ] {
        let mut cursor = None;
        let mut seen = Vec::new();
        loop {
            let page = get_screening_queue(
                &pool,
                GetScreeningQueueQuery {
                    reviewer_id: None,
                    project_id: project_id.into(),
                    status: ScreeningQueueStatus::All,
                    search: None,
                    sort,
                    cursor,
                    limit: 2,
                },
            )
            .await
            .expect("queue page should load");
            seen.extend(page.items.iter().map(|item| item.report_id));
            cursor = page.next_cursor;
            if cursor.is_none() {
                break;
            }
        }
        assert_eq!(
            seen.len(),
            report_ids.len(),
            "sort {sort:?} should page all rows"
        );
        let unique: HashSet<_> = seen.iter().copied().collect();
        assert_eq!(
            unique.len(),
            report_ids.len(),
            "sort {sort:?} should not duplicate rows"
        );
        assert_eq!(unique, report_ids.iter().copied().collect());
    }

    let first_page = get_screening_queue(
        &pool,
        GetScreeningQueueQuery {
            reviewer_id: None,
            project_id: project_id.into(),
            status: ScreeningQueueStatus::All,
            search: None,
            sort: ScreeningQueueSort::CreatedAscending,
            cursor: None,
            limit: 2,
        },
    )
    .await
    .expect("first queue page should load");
    let next_cursor = first_page
        .next_cursor
        .expect("fixture should have a next page");
    let foreign_sort = get_screening_queue(
        &pool,
        GetScreeningQueueQuery {
            reviewer_id: None,
            project_id: project_id.into(),
            status: ScreeningQueueStatus::All,
            search: None,
            sort: ScreeningQueueSort::CreatedDescending,
            cursor: Some(next_cursor.clone()),
            limit: 2,
        },
    )
    .await;
    assert!(matches!(foreign_sort, Err(ScreeningError::InvalidCursor)));

    let mut decoded: serde_json::Value = serde_json::from_slice(
        &URL_SAFE_NO_PAD
            .decode(next_cursor)
            .expect("cursor should be base64url"),
    )
    .expect("cursor should be JSON");
    decoded["title"] = serde_json::json!("tampered");
    let tampered =
        URL_SAFE_NO_PAD.encode(serde_json::to_vec(&decoded).expect("JSON should encode"));
    let tampered_result = get_screening_queue(
        &pool,
        GetScreeningQueueQuery {
            reviewer_id: None,
            project_id: project_id.into(),
            status: ScreeningQueueStatus::All,
            search: None,
            sort: ScreeningQueueSort::CreatedAscending,
            cursor: Some(tampered),
            limit: 2,
        },
    )
    .await;
    assert!(matches!(
        tampered_result,
        Err(ScreeningError::InvalidCursor)
    ));
    cleanup(&pool, project_id).await;
}

#[tokio::test]
async fn interleaved_stages_have_truthful_supersession_and_undo_snapshots() {
    let Some(pool) = database().await else { return };
    let (project_id, protocol_id, reports) = fixture(&pool, 1).await;
    let report_id = reports[0];
    let reason_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO exclusion_reasons (id,project_id,code,label,stage) VALUES ($1,$2,'wrong','Wrong','full_text')",
    )
    .bind(reason_id)
    .bind(project_id)
    .execute(&pool)
    .await
    .expect("reason should insert");

    let title = screen_report(
        &pool,
        decision_command(
            project_id,
            report_id,
            protocol_id,
            ScreeningStage::TitleAbstract,
            ScreeningDecision::Include,
            None,
            0,
        ),
    )
    .await
    .expect("title decision should persist");
    let title_event = title.last_event_id.expect("title event should exist");
    let full = screen_report(
        &pool,
        decision_command(
            project_id,
            report_id,
            protocol_id,
            ScreeningStage::FullText,
            ScreeningDecision::Exclude,
            Some(reason_id),
            1,
        ),
    )
    .await
    .expect("full-text decision should persist");
    let full_event = full.last_event_id.expect("full-text event should exist");
    let title_change = screen_report(
        &pool,
        decision_command(
            project_id,
            report_id,
            protocol_id,
            ScreeningStage::TitleAbstract,
            ScreeningDecision::Maybe,
            None,
            2,
        ),
    )
    .await
    .expect("title change should persist");
    let title_change_event = title_change
        .last_event_id
        .expect("title change event should exist");

    let links: Vec<(Uuid, Option<Uuid>, String)> = sqlx::query_as(
        "SELECT id,supersedes_event_id,stage FROM screening_events WHERE project_id=$1 AND report_id=$2 ORDER BY created_at,id",
    )
    .bind(project_id)
    .bind(report_id)
    .fetch_all(&pool)
    .await
    .expect("event links should be readable");
    assert_eq!(links[0].1, None);
    assert_eq!(
        links[1].1, None,
        "full-text event must not supersede title event"
    );
    assert_eq!(
        links[2].1,
        Some(title_event),
        "title event must supersede title stage only"
    );
    assert_eq!(links[1].0, full_event);
    assert_eq!(links[2].0, title_change_event);

    let undo = undo_screening(
        &pool,
        UndoScreeningCommand {
            project_id: project_id.into(),
            report_id: report_id.into(),
            stage: ScreeningStage::TitleAbstract,
            protocol_version_id: protocol_id.into(),
            expected_revision: 3,
            notes: Some("restore previous title decision".to_owned()),
            actor: actor(),
        },
    )
    .await
    .expect("latest title decision should be undoable");
    assert_eq!(
        undo.title_abstract_status,
        deepref_domain::ScreeningStatus::Include
    );
    assert_eq!(
        undo.full_text_status,
        deepref_domain::ScreeningStatus::Exclude
    );
    assert_eq!(undo.full_text_exclusion_reason_id, Some(reason_id));

    let history = get_screening_history(&pool, project_id, report_id)
        .await
        .expect("history should load");
    let undo_item = history.items.last().expect("undo event should be present");
    assert_eq!(undo_item.event_kind, "undo");
    assert_eq!(undo_item.undoes_event_id, Some(title_change_event));
    assert_eq!(
        undo_item.previous_title_abstract_status,
        ScreeningStatus::Maybe
    );
    assert_eq!(
        undo_item.result_title_abstract_status,
        ScreeningStatus::Include
    );
    cleanup(&pool, project_id).await;
}

#[tokio::test]
async fn queue_page_is_bounded_and_required_indexes_exist_for_large_fixture() {
    let Some(pool) = database().await else { return };
    let (project_id, _protocol_id, _reports) = fixture(&pool, 0).await;
    let marker = format!("screening-10k-{project_id}");
    sqlx::query(
        "INSERT INTO reports (id,title,abstract_text,publication_year) SELECT gen_random_uuid(), $2 || '-' || n, 'searchable abstract ' || n, 2024 FROM generate_series(1,10000) AS n",
    )
    .bind(project_id)
    .bind(&marker)
    .execute(&pool)
    .await
    .expect("large report fixture should insert");
    sqlx::query(
        "INSERT INTO project_reports (project_id,report_id) SELECT $1,id FROM reports WHERE title LIKE $2 || '-%'",
    )
    .bind(project_id)
    .bind(&marker)
    .execute(&pool)
    .await
    .expect("large project report fixture should insert");

    let page = get_screening_queue(
        &pool,
        GetScreeningQueueQuery {
            reviewer_id: None,
            project_id: project_id.into(),
            status: ScreeningQueueStatus::Unscreened,
            search: Some("searchable".to_owned()),
            sort: ScreeningQueueSort::CreatedDescending,
            cursor: None,
            limit: 25,
        },
    )
    .await
    .expect("large queue page should load");
    assert_eq!(page.items.len(), 25);
    assert!(page.next_cursor.is_some());
    assert_eq!(page.total, 10000);

    let indexes: HashSet<String> = sqlx::query_scalar(
        "SELECT indexname FROM pg_indexes WHERE schemaname=current_schema() AND tablename IN ('project_reports','screening_state','reports')",
    )
    .fetch_all(&pool)
    .await
    .expect("index catalog should load")
    .into_iter()
    .collect();
    assert!(indexes.contains("project_reports_screening_queue_idx"));
    assert!(indexes.contains("screening_state_title_status_idx"));
    assert!(indexes.contains("reports_title_abstract_trgm_idx"));

    let mut tx = pool
        .begin()
        .await
        .expect("explain transaction should start");
    sqlx::query("SET LOCAL enable_seqscan=off")
        .execute(&mut *tx)
        .await
        .expect("planner setting should apply");
    let plan: Vec<String> = sqlx::query_scalar(
        "EXPLAIN SELECT id FROM reports WHERE lower(coalesce(title,'') || ' ' || coalesce(abstract_text,'')) LIKE '%searchable%'",
    )
    .fetch_all(&mut *tx)
    .await
    .expect("search plan should load");
    assert!(
        plan.iter()
            .any(|line| line.contains("reports_title_abstract_trgm_idx"))
    );
    tx.rollback()
        .await
        .expect("explain transaction should roll back");
    cleanup(&pool, project_id).await;
    sqlx::query("DELETE FROM reports WHERE title LIKE $1 || '-%'")
        .bind(&marker)
        .execute(&pool)
        .await
        .expect("large fixture reports should clean up");
}

#[tokio::test]
async fn screening_events_record_a_constrained_origin() {
    let Some(pool) = database().await else { return };
    let (project_id, protocol_id, reports) = fixture(&pool, 1).await;
    let report_id = reports[0];
    let insert = |origin: Option<&'static str>| {
        let pool = pool.clone();
        async move {
            sqlx::query(
                "INSERT INTO screening_events (id,project_id,report_id,stage,decision,protocol_version_id,actor_kind,actor_id,origin)
                 VALUES ($1,$2,$3,'title_abstract','include',$4,'user','origin-test',$5)",
            )
            .bind(Uuid::new_v4())
            .bind(project_id)
            .bind(report_id)
            .bind(protocol_id)
            .bind(origin)
            .execute(&pool)
            .await
        }
    };
    insert(None).await.expect("a direct decision has no origin");
    insert(Some("direct"))
        .await
        .expect("direct is an explicit origin");
    insert(Some("assistant_plan"))
        .await
        .expect("assistant plans are a recorded origin");
    let error = insert(Some("model"))
        .await
        .expect_err("unknown origins are refused");
    assert!(
        error.to_string().contains("screening_events_origin_check"),
        "{error}"
    );
    cleanup(&pool, project_id).await;
}
