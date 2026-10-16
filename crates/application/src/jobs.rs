use std::{future::Future, time::Duration};

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use deepref_domain::{JobKind, ProjectId};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnqueueJob {
    pub id: Uuid,
    pub project_id: ProjectId,
    pub kind: JobKind,
    pub payload: serde_json::Value,
    pub priority: i32,
    pub max_attempts: i32,
    pub dedupe_key: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClaimedJob {
    pub id: Uuid,
    pub project_id: ProjectId,
    pub kind: JobKind,
    pub payload: serde_json::Value,
    pub attempts: i32,
    pub max_attempts: i32,
}

impl ClaimedJob {
    /// The claim token for this attempt, held by `owner` (the worker that
    /// claimed it). Renew, complete and fail take effect only while this exact
    /// attempt still holds the lease.
    pub fn lease(&self, owner: impl Into<String>) -> JobLease {
        JobLease {
            id: self.id,
            owner: owner.into(),
            attempt: self.attempts,
        }
    }
}

/// Fencing token for one claim of a job. The owner names the worker process;
/// the attempt tells this claim apart from an earlier claim by the same
/// process whose lease expired and was reclaimed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JobLease {
    pub id: Uuid,
    pub owner: String,
    pub attempt: i32,
}

/// What a failed attempt asks the queue to do next.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JobRetry {
    /// Retry after the delay, unless the attempt budget is spent (then dead).
    After(Duration),
    /// Dead-letter now, whatever budget is left.
    DeadLetter,
}

pub trait JobQueue: Send + Sync {
    fn enqueue(&self, job: EnqueueJob) -> impl Future<Output = anyhow::Result<Uuid>> + Send;
    fn claim(
        &self,
        owner: &str,
        lease: Duration,
    ) -> impl Future<Output = anyhow::Result<Option<ClaimedJob>>> + Send;
    fn renew(
        &self,
        lease: &JobLease,
        duration: Duration,
    ) -> impl Future<Output = anyhow::Result<bool>> + Send;
    fn complete(&self, lease: &JobLease) -> impl Future<Output = anyhow::Result<bool>> + Send;
    fn fail(
        &self,
        lease: &JobLease,
        error: &str,
        retry: JobRetry,
    ) -> impl Future<Output = anyhow::Result<bool>> + Send;
}
