use std::{future::Future, time::Duration};

use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnqueueJob {
    pub id: Uuid,
    pub kind: String,
    pub payload: serde_json::Value,
    pub priority: i32,
    pub max_attempts: i32,
    pub dedupe_key: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClaimedJob {
    pub id: Uuid,
    pub kind: String,
    pub payload: serde_json::Value,
    pub attempts: i32,
    pub max_attempts: i32,
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
        owner: &str,
        job_id: Uuid,
        lease: Duration,
    ) -> impl Future<Output = anyhow::Result<bool>> + Send;
    fn complete(
        &self,
        owner: &str,
        job_id: Uuid,
    ) -> impl Future<Output = anyhow::Result<bool>> + Send;
    fn fail(
        &self,
        owner: &str,
        job: &ClaimedJob,
        error: &str,
        retry_after: Duration,
    ) -> impl Future<Output = anyhow::Result<bool>> + Send;
}
