#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]
use deepref_postgres::{
    NotificationDraft, list_notifications, mark_notifications_read, migrate, record_notification,
    record_run_notification_once_in_transaction, unread_summary,
};
use sqlx::{PgPool, postgres::PgPoolOptions};
use uuid::Uuid;

async fn database() -> Option<PgPool> {
    let url = std::env::var("DATABASE_URL").ok()?;
    let pool = PgPoolOptions::new()
        .max_connections(10)
        .connect(&url)
        .await
        .expect("DATABASE_URL is set but PostgreSQL is unavailable");
    migrate(&pool).await.expect("migrations should apply");
    Some(pool)
}

/// The notifications table is shared by every concurrent test, so fixtures are
/// keyed by a unique kind marker and assertions only consider their own rows.
fn unique_kind(prefix: &str) -> String {
    format!("{prefix}.{}", Uuid::new_v4())
}

async fn insert_project(pool: &PgPool, name: &str) -> Uuid {
    let id = Uuid::new_v4();
    sqlx::query("INSERT INTO projects (id,name) VALUES ($1,$2)")
        .bind(id)
        .bind(name)
        .execute(pool)
        .await
        .expect("project fixture");
    id
}

#[tokio::test]
async fn records_are_listed_newest_first_and_paginated() {
    let Some(pool) = database().await else {
        return;
    };
    let kind = unique_kind("test.pagination");
    for index in 0..5 {
        record_notification(
            &pool,
            &NotificationDraft::success(
                &kind,
                None,
                &format!("Notification {index}"),
                None,
                serde_json::json!({ "index": index }),
            ),
        )
        .await
        .unwrap();
    }

    let mut seen: Vec<i64> = Vec::new();
    let mut cursor: Option<i64> = None;
    for _ in 0..20 {
        let page = list_notifications(&pool, None, cursor, 2).await.unwrap();
        let page_revisions: Vec<i64> = page
            .items
            .iter()
            .filter(|item| item.kind == kind)
            .map(|item| item.revision)
            .collect();
        seen.extend(page_revisions);
        match page.next_cursor {
            Some(next) => cursor = Some(next),
            None => break,
        }
    }

    assert_eq!(
        seen.len(),
        5,
        "cursor pagination must eventually surface every row"
    );
    let mut sorted = seen.clone();
    sorted.sort_unstable();
    sorted.reverse();
    assert_eq!(seen, sorted, "rows must appear newest first across pages");
}

#[tokio::test]
async fn unread_summary_and_mark_read_track_state() {
    let Some(pool) = database().await else {
        return;
    };
    // A project of its own: mark-all in here must never touch rows that other
    // tests are writing into the shared table at the same time.
    let project = insert_project(&pool, &format!("Mark read {}", Uuid::new_v4())).await;
    let kind = unique_kind("test.markread");
    let before = unread_summary(&pool, Some(project)).await.unwrap();
    assert_eq!(before.count, 0);

    for index in 0..3 {
        record_notification(
            &pool,
            &NotificationDraft::warning(
                &kind,
                Some(project),
                &format!("Unread {index}"),
                Some("body".to_owned()),
                serde_json::json!({}),
            ),
        )
        .await
        .unwrap();
    }
    let listed = list_notifications(&pool, Some(project), None, 100)
        .await
        .unwrap();
    let ids: Vec<Uuid> = listed
        .items
        .iter()
        .filter(|item| item.kind == kind)
        .map(|item| item.id)
        .collect();
    assert_eq!(ids.len(), 3);

    let after_insert = unread_summary(&pool, Some(project)).await.unwrap();
    assert_eq!(after_insert.count, 3, "three new rows are unread");
    assert!(after_insert.latest_revision >= before.latest_revision);

    let updated = mark_notifications_read(
        &pool,
        &deepref_postgres::MarkNotificationsRead {
            ids: Some(ids.clone()),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert_eq!(updated, 3, "only the requested ids transition to read");
    assert_eq!(
        unread_summary(&pool, Some(project)).await.unwrap().count,
        0,
        "the unread total follows the mark"
    );

    let re_mark = mark_notifications_read(
        &pool,
        &deepref_postgres::MarkNotificationsRead {
            ids: Some(ids.clone()),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert_eq!(re_mark, 0, "mark-read is idempotent for read rows");

    for index in 0..2 {
        record_notification(
            &pool,
            &NotificationDraft::info(
                &kind,
                Some(project),
                &format!("Later {index}"),
                None,
                serde_json::json!({}),
            ),
        )
        .await
        .unwrap();
    }
    let all = mark_notifications_read(
        &pool,
        &deepref_postgres::MarkNotificationsRead {
            ids: None,
            all: true,
            project_id: Some(project),
        },
    )
    .await
    .unwrap();
    assert_eq!(all, 2, "mark-all touches only this project's unread rows");

    let listed = list_notifications(&pool, Some(project), None, 100)
        .await
        .unwrap();
    assert!(
        listed.items.iter().all(|item| item.read_at.is_some()),
        "every row of the project is read"
    );

    sqlx::query("DELETE FROM projects WHERE id = $1")
        .bind(project)
        .execute(&pool)
        .await
        .expect("cleanup");
}

#[tokio::test]
async fn empty_body_is_normalized_to_null() {
    let Some(pool) = database().await else {
        return;
    };
    let kind = unique_kind("test.body");
    let title = format!("Blank body {}", Uuid::new_v4());
    record_notification(
        &pool,
        &NotificationDraft::info(
            &kind,
            None,
            &title,
            Some("   ".to_owned()),
            serde_json::json!({}),
        ),
    )
    .await
    .unwrap();

    let listed = list_notifications(&pool, None, None, 100).await.unwrap();
    let item = listed
        .items
        .iter()
        .find(|item| item.title == title)
        .expect("recorded notification must be listed");
    assert_eq!(item.body, None);
    assert_eq!(item.severity, "info");
}

#[tokio::test]
async fn listing_unread_and_mark_all_are_scoped_to_one_project() {
    let Some(pool) = database().await else {
        return;
    };
    let kind = unique_kind("test.scope");
    let alpha_name = format!("Alpha {}", Uuid::new_v4());
    let beta_name = format!("Beta {}", Uuid::new_v4());
    let alpha = insert_project(&pool, &alpha_name).await;
    let beta = insert_project(&pool, &beta_name).await;
    let alpha_before = unread_summary(&pool, Some(alpha)).await.unwrap();

    for (project, title) in [
        (Some(alpha), "alpha one"),
        (Some(alpha), "alpha two"),
        (Some(beta), "beta one"),
        (None, "workspace note"),
    ] {
        record_notification(
            &pool,
            &NotificationDraft::error(
                &kind,
                project,
                title,
                None,
                serde_json::json!({ "run_id": Uuid::new_v4() }),
            ),
        )
        .await
        .unwrap();
    }

    let scoped = list_notifications(&pool, Some(alpha), None, 100)
        .await
        .unwrap();
    let alpha_rows: Vec<_> = scoped
        .items
        .iter()
        .filter(|item| item.kind == kind)
        .collect();
    assert_eq!(alpha_rows.len(), 2, "only alpha's rows are listed");
    assert!(
        alpha_rows.iter().all(|item| item.project_id == Some(alpha)
            && item.project_name.as_deref() == Some(alpha_name.as_str())),
        "each row carries its project name"
    );

    let everything = list_notifications(&pool, None, None, 100).await.unwrap();
    let all_rows = everything
        .items
        .iter()
        .filter(|item| item.kind == kind)
        .count();
    assert_eq!(all_rows, 4, "the unscoped list includes every project");

    let alpha_after = unread_summary(&pool, Some(alpha)).await.unwrap();
    assert_eq!(
        alpha_after.count,
        alpha_before.count + 2,
        "the scoped unread total counts only alpha's rows"
    );

    let marked = mark_notifications_read(
        &pool,
        &deepref_postgres::MarkNotificationsRead {
            all: true,
            project_id: Some(beta),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert_eq!(marked, 1, "mark-all for beta touches only beta");
    let still_unread = list_notifications(&pool, Some(alpha), None, 100)
        .await
        .unwrap()
        .items
        .iter()
        .filter(|item| item.kind == kind && item.read_at.is_none())
        .count();
    assert_eq!(still_unread, 2, "alpha stays unread");

    sqlx::query("DELETE FROM projects WHERE id = ANY($1)")
        .bind(vec![alpha, beta])
        .execute(&pool)
        .await
        .expect("cleanup");
}

#[tokio::test]
async fn a_run_has_one_failure_notification_however_often_it_is_recorded() {
    let Some(pool) = database().await else {
        return;
    };
    let kind = unique_kind("workflow_run.failed");
    let run_id = Uuid::new_v4();
    let draft = NotificationDraft::error(
        &kind,
        None,
        "Recent papers failed",
        Some("That address points to a private or local network.".to_owned()),
        serde_json::json!({ "run_id": run_id }),
    );

    let mut first = pool.begin().await.unwrap();
    assert!(
        record_run_notification_once_in_transaction(&mut first, &draft, run_id)
            .await
            .unwrap(),
        "the first record for a run is written"
    );
    first.commit().await.unwrap();

    let mut second = pool.begin().await.unwrap();
    assert!(
        !record_run_notification_once_in_transaction(&mut second, &draft, run_id)
            .await
            .unwrap(),
        "a second record for the same run is dropped"
    );
    second.commit().await.unwrap();

    let rows: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM notifications WHERE kind=$1 AND payload->>'run_id'=$2",
    )
    .bind(&kind)
    .bind(run_id.to_string())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(rows, 1);
}
