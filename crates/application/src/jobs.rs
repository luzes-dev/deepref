use std::{future::Future, time::Duration};

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use deepref_domain::{JobKind, ProjectId, WorkloadClass, WorkspaceId};

/// Typed scope of one durable job: the tenant level the work belongs to. A job
/// never fabricates a project to carry workspace- or platform-level work, and
/// authorization/tenancy checks follow the scope kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "scope", content = "id", rename_all = "snake_case")]
pub enum JobScope {
    Project(ProjectId),
    Workspace(WorkspaceId),
    Platform,
}

impl JobScope {
    /// Persisted spelling of the scope kind.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Project(_) => "project",
            Self::Workspace(_) => "workspace",
            Self::Platform => "platform",
        }
    }

    pub const fn project_id(self) -> Option<ProjectId> {
        match self {
            Self::Project(project_id) => Some(project_id),
            Self::Workspace(_) | Self::Platform => None,
        }
    }

    pub const fn workspace_id(self) -> Option<WorkspaceId> {
        match self {
            Self::Workspace(workspace_id) => Some(workspace_id),
            Self::Project(_) | Self::Platform => None,
        }
    }
}

impl From<ProjectId> for JobScope {
    fn from(project_id: ProjectId) -> Self {
        Self::Project(project_id)
    }
}

impl std::fmt::Display for JobScope {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Project(project_id) => write!(formatter, "project:{}", project_id.as_uuid()),
            Self::Workspace(workspace_id) => {
                write!(formatter, "workspace:{}", workspace_id.as_uuid())
            }
            Self::Platform => formatter.write_str("platform"),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnqueueJob {
    pub id: Uuid,
    pub scope: JobScope,
    pub kind: JobKind,
    pub payload: serde_json::Value,
    pub priority: i32,
    pub max_attempts: i32,
    /// Logical identity for deduplication. The key is global to the jobs
    /// table; scope-specific callers must encode the scope in it.
    pub dedupe_key: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClaimedJob {
    pub id: Uuid,
    pub scope: JobScope,
    pub kind: JobKind,
    pub payload: serde_json::Value,
    pub attempts: i32,
    pub max_attempts: i32,
}

impl ClaimedJob {
    /// The project this job serves, when it is project-scoped. Workspace- and
    /// platform-scoped jobs have none.
    pub const fn project_id(&self) -> Option<ProjectId> {
        self.scope.project_id()
    }

    /// The project this job serves, for handlers that only exist for
    /// project-scoped work. A non-project scope fails visibly instead of
    /// borrowing a project that does not own the work.
    pub fn require_project(&self) -> anyhow::Result<ProjectId> {
        self.project_id().ok_or_else(|| {
            anyhow::anyhow!(
                "job {} ({}) requires a project-scoped job, got {}",
                self.id,
                self.kind,
                self.scope
            )
        })
    }

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
    /// The delay is persisted as the job's due time (`available_at`); the
    /// claim loop picks the job up when it is due.
    After(Duration),
    /// Dead-letter now, whatever budget is left.
    DeadLetter,
}

/// Persisted lifecycle of durable execution: the authoritative job state,
/// its claims/fences, attempts, leases, terminal transitions and recovery.
///
/// Business attempts increment exactly once, when a claim/fence is accepted
/// (`claim`); nothing else — not recovery, not retries, not terminal
/// transitions — increments them. `fail` persists the retry's due time and
/// `release` hands an accepted-but-unstarted claim back without spending its
/// attempt, so repeated delivery can never multiply business retries.
pub trait DurableExecutionStore: Send + Sync {
    /// Persist new work in its scoped shape. The workload class is derived
    /// from `job.kind`, never chosen by the caller.
    fn enqueue(&self, job: EnqueueJob) -> impl Future<Output = anyhow::Result<Uuid>> + Send;

    /// Claim and fence up to `limit` eligible jobs in `pools` (an empty slice
    /// claims across every workload class) in one statement. Eligibility is
    /// the persisted state: `queued`, due (`available_at <= now()`), and
    /// within the attempt budget. A row claimed by a live lease, a completed
    /// or dead row, and work due later are never returned.
    fn claim(
        &self,
        owner: &str,
        lease: Duration,
        pools: &[WorkloadClass],
        limit: usize,
    ) -> impl Future<Output = anyhow::Result<Vec<ClaimedJob>>> + Send;

    /// Extend the lease of the exact fenced attempt.
    fn renew(
        &self,
        lease: &JobLease,
        duration: Duration,
    ) -> impl Future<Output = anyhow::Result<bool>> + Send;

    /// Terminal success of the exact fenced attempt.
    fn complete(&self, lease: &JobLease) -> impl Future<Output = anyhow::Result<bool>> + Send;

    /// Record a failed attempt of the exact fenced attempt. A retry persists
    /// its due time before the job becomes claimable again.
    fn fail(
        &self,
        lease: &JobLease,
        error: &str,
        retry: JobRetry,
    ) -> impl Future<Output = anyhow::Result<bool>> + Send;

    /// Hand an accepted but not yet started claim back to the queue (used
    /// when the worker drains mid-batch). The row returns to `queued` and is
    /// immediately claimable; the claim's attempt is refunded because no
    /// business execution was accepted. Only valid for claims whose handler
    /// never started; the released token must then be discarded.
    fn release(&self, lease: &JobLease) -> impl Future<Output = anyhow::Result<bool>> + Send;

    /// Requeue or dead-letter jobs whose lease expired without a graceful
    /// completion or failure. Recovery never increments attempts: the expired
    /// claim already spent its attempt.
    fn recover_expired(&self) -> impl Future<Output = anyhow::Result<u64>> + Send;
}

/// One delivery request: give the worker the next eligible work for a pool.
#[derive(Debug, Clone, Copy)]
pub struct WorkRequest<'a> {
    pub owner: &'a str,
    pub lease: Duration,
    pub pool: WorkloadClass,
    pub max_jobs: usize,
}

/// Delivery mechanics: wake eligible work and hand it to the worker for a
/// workload pool. Delivery decides nothing about authoritative state; the
/// [`DurableExecutionStore`] claim/fence remains the authority.
///
/// Under the PostgreSQL transport (the first and, until 05b, only
/// implementation) delivery *is* claiming: `next_work` claims and fences the
/// work it returns, so the claim doubles as wakeup and authority. A broker
/// transport would return wakeup hints only, resolved through
/// [`DurableExecutionStore::claim`].
pub trait WorkTransport: Send + Sync {
    /// The next eligible work for `request.pool`, at most `request.max_jobs`
    /// items, each claimed and fenced for `request.owner`. Never claims in an
    /// unbounded loop: the caller bounds the batch and re-checks capacity on
    /// its next tick.
    fn next_work(
        &self,
        request: WorkRequest<'_>,
    ) -> impl Future<Output = anyhow::Result<Vec<ClaimedJob>>> + Send;
}
