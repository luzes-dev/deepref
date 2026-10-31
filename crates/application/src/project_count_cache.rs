//! Short-lived display snapshots. These counts never authorize a command,
//! financial allowance or scientific decision.

use std::{future::Future, pin::Pin, time::Duration};

use chrono::{DateTime, Utc};
use deepref_domain::{ProjectId, WorkspaceId};
use serde::{Deserialize, Serialize};

pub const COUNT_CACHE_TTL: Duration = Duration::from_secs(5);
pub const COUNT_CACHE_MAX_BATCH: usize = 101;
pub const COUNT_CACHE_MAX_VALUE_BYTES: usize = 512;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectCountKey {
    pub workspace: WorkspaceId,
    pub project: ProjectId,
    pub revision: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArticleCountSnapshot {
    pub key: ProjectCountKey,
    pub count: i64,
    pub observed_at: DateTime<Utc>,
}

impl ArticleCountSnapshot {
    /// Validate data crossing the disposable cache boundary. Clock skew or
    /// malformed values cause misses, never a weaker authorization check.
    pub fn matches_at(&self, expected: ProjectCountKey, now: DateTime<Utc>) -> bool {
        self.key == expected
            && self.key.revision >= 0
            && self.count >= 0
            && self.observed_at <= now
            && now
                .signed_duration_since(self.observed_at)
                .to_std()
                .is_ok_and(|age| age <= COUNT_CACHE_TTL)
    }
}

#[derive(Debug, Clone, Copy, thiserror::Error)]
#[error("project count cache is unavailable")]
pub struct CountCacheUnavailable;

pub type CountCacheFuture<'a, T> =
    Pin<Box<dyn Future<Output = Result<T, CountCacheUnavailable>> + Send + 'a>>;

/// Batch operations avoid one network round trip per visible project. The
/// caller resolves current authorized identities on primary PostgreSQL first.
pub trait ProjectCountCache: Send + Sync {
    fn get<'a>(
        &'a self,
        keys: &'a [ProjectCountKey],
    ) -> CountCacheFuture<'a, Vec<Option<ArticleCountSnapshot>>>;
    fn put<'a>(&'a self, snapshots: &'a [ArticleCountSnapshot]) -> CountCacheFuture<'a, ()>;
}
