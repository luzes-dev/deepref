#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

use std::time::Duration;

use chrono::{TimeDelta, Utc};
use deepref_app::project_counts::article_counts;
use deepref_application::project_count_cache::{
    ArticleCountSnapshot, COUNT_CACHE_MAX_VALUE_BYTES, ProjectCountCache, ProjectCountKey,
};
use deepref_domain::{ProjectId, WorkspaceId};
use deepref_integrations::valkey_fixture::ValkeyFixtureControl;
use uuid::Uuid;

async fn fixture() -> Option<(sqlx::PgPool, ValkeyFixtureControl, ProjectCountKey)> {
    let Ok(address) = std::env::var("TEST_VALKEY_URL") else {
        assert_ne!(std::env::var("DEEPREF_REQUIRE_VALKEY").as_deref(), Ok("1"));
        return None;
    };
    let pool = deepref_app::test_isolation::isolated_pool_from_env("DATABASE_URL", 1)
        .await
        .expect("required disposable database");
    let project = ProjectId::new(Uuid::new_v4());
    let workspace: Uuid = sqlx::query_scalar(
        "INSERT INTO projects(id,name) VALUES($1,'display cache') RETURNING workspace_id",
    )
    .bind(project.as_uuid())
    .fetch_one(&pool)
    .await
    .unwrap();
    let key = ProjectCountKey {
        workspace: WorkspaceId::new(workspace),
        project,
        revision: 0,
    };
    let control = ValkeyFixtureControl::connect(
        &address,
        &format!("contract-{}", Uuid::new_v4()),
        Duration::from_millis(100),
    )
    .await
    .unwrap();
    Some((pool, control, key))
}

async fn insert_report(pool: &sqlx::PgPool, project: ProjectId) {
    let report = Uuid::new_v4();
    sqlx::query("INSERT INTO reports(id,title,raw) VALUES($1,'cache report','{}')")
        .bind(report)
        .execute(pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO project_reports(project_id,report_id) VALUES($1,$2)")
        .bind(project.as_uuid())
        .bind(report)
        .execute(pool)
        .await
        .unwrap();
}

#[tokio::test]
async fn miss_hit_strict_bypass_revision_expiry_and_unavailable_cache_use_one_pg_connection() {
    let Some((pool, mut control, mut key)) = fixture().await else {
        return;
    };
    let cache = control.project_count_cache().unwrap();
    let requested = [(key.workspace, key.project)];
    let first = article_counts(&pool, &requested, Some(&cache), false)
        .await
        .unwrap();
    assert_eq!(first[&key.project].count, 0);
    assert!((1..=5_000).contains(&control.project_count_ttl(key).await.unwrap()));
    insert_report(&pool, key.project).await;
    let hit = article_counts(&pool, &requested, Some(&cache), false)
        .await
        .unwrap();
    assert_eq!(
        hit[&key.project], first[&key.project],
        "legacy/raw mutation has honest bounded staleness"
    );
    let strict = article_counts(&pool, &requested, Some(&cache), true)
        .await
        .unwrap();
    assert_eq!(strict[&key.project].count, 1);
    assert_eq!(
        cache.get(&[key]).await.unwrap()[0].as_ref().unwrap().count,
        0,
        "strict bypass does not update cache"
    );
    sqlx::query(
        "UPDATE projects SET metric_requested_revision=metric_requested_revision+1 WHERE id=$1",
    )
    .bind(key.project.as_uuid())
    .execute(&pool)
    .await
    .unwrap();
    key.revision += 1;
    let revised = article_counts(&pool, &requested, Some(&cache), false)
        .await
        .unwrap();
    assert_eq!(revised[&key.project].count, 1);
    assert_eq!(revised[&key.project].key, key);
    // Exercise real server expiry without a five-second fixed sleep.
    control
        .set_project_count(
            key,
            &serde_json::to_vec(&revised[&key.project]).unwrap(),
            Duration::from_millis(1),
        )
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(2), async {
        while control.project_count_ttl(key).await.unwrap() != -2 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert!(cache.get(&[key]).await.unwrap()[0].is_none());
    let unavailable = ValkeyFixtureControl::unavailable_project_count_cache().unwrap();
    assert_eq!(
        article_counts(&pool, &requested, Some(&unavailable), false)
            .await
            .unwrap()[&key.project]
            .count,
        1
    );
    sqlx::query("DELETE FROM projects WHERE id=$1")
        .bind(key.project.as_uuid())
        .execute(&pool)
        .await
        .unwrap();
    assert!(
        article_counts(&pool, &requested, Some(&cache), false)
            .await
            .unwrap()
            .is_empty(),
        "cached identity cannot resurrect deleted project"
    );
}

#[tokio::test]
async fn corrupt_oversize_wrong_scope_future_and_expired_values_are_misses() {
    let Some((pool, mut control, key)) = fixture().await else {
        return;
    };
    let cache = control.project_count_cache().unwrap();
    let valid = ArticleCountSnapshot {
        key,
        count: 9,
        observed_at: Utc::now(),
    };
    let mut wrong = valid.clone();
    wrong.key.workspace = WorkspaceId::new(Uuid::new_v4());
    let mut future = valid.clone();
    future.observed_at += TimeDelta::seconds(10);
    let mut stale = valid.clone();
    stale.observed_at -= TimeDelta::seconds(10);
    let mut negative = valid.clone();
    negative.count = -1;
    let mut unknown = serde_json::to_value(&valid).unwrap();
    unknown["unrecognized"] = true.into();
    for bytes in [
        b"not-json".to_vec(),
        vec![b'x'; COUNT_CACHE_MAX_VALUE_BYTES + 1],
        serde_json::to_vec(&wrong).unwrap(),
        serde_json::to_vec(&future).unwrap(),
        serde_json::to_vec(&stale).unwrap(),
        serde_json::to_vec(&negative).unwrap(),
        serde_json::to_vec(&unknown).unwrap(),
    ] {
        control
            .set_project_count(key, &bytes, Duration::from_secs(5))
            .await
            .unwrap();
        assert!(cache.get(&[key]).await.unwrap()[0].is_none());
        assert_eq!(
            article_counts(&pool, &[(key.workspace, key.project)], Some(&cache), false)
                .await
                .unwrap()[&key.project]
                .count,
            0
        );
    }
    control
        .set_project_count(
            key,
            &serde_json::to_vec(&valid).unwrap(),
            Duration::from_secs(5),
        )
        .await
        .unwrap();
    let foreign = WorkspaceId::new(Uuid::new_v4());
    assert!(
        article_counts(&pool, &[(foreign, key.project)], Some(&cache), false)
            .await
            .unwrap()
            .is_empty()
    );
    let other = ValkeyFixtureControl::connect(
        &std::env::var("TEST_VALKEY_URL").unwrap(),
        &format!("contract-{}", Uuid::new_v4()),
        Duration::from_millis(100),
    )
    .await
    .unwrap();
    assert!(
        other
            .project_count_cache()
            .unwrap()
            .get(&[key])
            .await
            .unwrap()[0]
            .is_none(),
        "deployment namespaces remain isolated"
    );
}

struct StalledCache;
impl ProjectCountCache for StalledCache {
    fn get<'a>(
        &'a self,
        _: &'a [ProjectCountKey],
    ) -> deepref_application::project_count_cache::CountCacheFuture<
        'a,
        Vec<Option<ArticleCountSnapshot>>,
    > {
        Box::pin(std::future::pending())
    }
    fn put<'a>(
        &'a self,
        _: &'a [ArticleCountSnapshot],
    ) -> deepref_application::project_count_cache::CountCacheFuture<'a, ()> {
        Box::pin(std::future::pending())
    }
}

#[tokio::test]
async fn stalled_optional_reads_and_writes_leave_a_bounded_primary_fallback() {
    let Some((pool, _control, key)) = fixture().await else {
        return;
    };
    let result = tokio::time::timeout(
        Duration::from_secs(1),
        article_counts(
            &pool,
            &[(key.workspace, key.project)],
            Some(&StalledCache),
            false,
        ),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(result[&key.project].count, 0);
    // The held read transaction was rolled back before the optional write.
    assert_eq!(
        sqlx::query_scalar::<_, i32>("SELECT 1")
            .fetch_one(&pool)
            .await
            .unwrap(),
        1
    );
}
