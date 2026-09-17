#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]
use deepref_postgres::{
    NotificationDraft, list_notifications, mark_notifications_read, migrate, record_notification,
    unread_summary,
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
        let page = list_notifications(&pool, cursor, 2).await.unwrap();
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
    let kind = unique_kind("test.markread");
    let before = unread_summary(&pool).await.unwrap();

    let mut ids = Vec::new();
    for index in 0..3 {
        record_notification(
            &pool,
            &NotificationDraft::warning(
                &kind,
                None,
                &format!("Unread {index}"),
                Some("body".to_owned()),
                serde_json::json!({}),
            ),
        )
        .await
        .unwrap();
    }
    let listed = list_notifications(&pool, None, 100).await.unwrap();
    ids.extend(
        listed
            .items
            .iter()
            .filter(|item| item.kind == kind)
            .map(|item| item.id),
    );
    assert_eq!(ids.len(), 3);

    let after_insert = unread_summary(&pool).await.unwrap();
    assert!(
        after_insert.count >= before.count + 3,
        "inserting unread rows must raise the unread count"
    );
    assert!(after_insert.latest_revision >= before.latest_revision);

    let updated = mark_notifications_read(
        &pool,
        &deepref_postgres::MarkNotificationsRead {
            ids: Some(ids.clone()),
            all: false,
        },
    )
    .await
    .unwrap();
    assert_eq!(updated, 3, "only the requested ids transition to read");

    let re_mark = mark_notifications_read(
        &pool,
        &deepref_postgres::MarkNotificationsRead {
            ids: Some(ids.clone()),
            all: false,
        },
    )
    .await
    .unwrap();
    assert_eq!(re_mark, 0, "mark-read is idempotent for read rows");

    let all = mark_notifications_read(
        &pool,
        &deepref_postgres::MarkNotificationsRead {
            ids: None,
            all: true,
        },
    )
    .await
    .unwrap();
    assert!(all >= 3);

    let listed = list_notifications(&pool, None, 100).await.unwrap();
    let marked: Vec<bool> = listed
        .items
        .iter()
        .filter(|item| ids.contains(&item.id))
        .map(|item| item.read_at.is_some())
        .collect();
    assert_eq!(marked, vec![true, true, true]);
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

    let listed = list_notifications(&pool, None, 100).await.unwrap();
    let item = listed
        .items
        .iter()
        .find(|item| item.title == title)
        .expect("recorded notification must be listed");
    assert_eq!(item.body, None);
    assert_eq!(item.severity, "info");
}
