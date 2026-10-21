use sqlx::PgPool;
use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
use tokio::sync::Notify;
use uuid::Uuid;

#[derive(Default)]
pub struct BlockedHandler {
    pub entered: Notify,
    pub resume: Notify,
    pub dropped: AtomicBool,
}

struct DropMarker(Arc<BlockedHandler>);
impl Drop for DropMarker {
    fn drop(&mut self) {
        self.0.dropped.store(true, Ordering::SeqCst);
    }
}

pub async fn block(handler: Option<Arc<BlockedHandler>>) {
    if let Some(handler) = handler {
        let _guard = DropMarker(Arc::clone(&handler));
        handler.entered.notify_one();
        handler.resume.notified().await;
    }
}

pub async fn database() -> Option<PgPool> {
    deepref_app::test_isolation::isolated_pool_from_env("DATABASE_URL", 8).await
}

pub async fn wait_for_project(pool: &PgPool, id: Uuid) {
    tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            let exists: bool =
                sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM projects WHERE id=$1)")
                    .bind(id)
                    .fetch_one(pool)
                    .await
                    .expect("project query");
            if exists {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("processor eventually commits its effect");
}

pub async fn wait_for_drop(handler: &BlockedHandler) {
    tokio::time::timeout(Duration::from_secs(10), async {
        while !handler.dropped.load(Ordering::SeqCst) {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("lost handler is cancelled");
}

pub async fn assert_no_project(pool: &PgPool, id: Uuid) {
    let exists: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM projects WHERE id=$1)")
        .bind(id)
        .fetch_one(pool)
        .await
        .expect("project query");
    assert!(!exists, "cancelled handler must not commit its transaction");
}
