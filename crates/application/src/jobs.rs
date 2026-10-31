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
    /// Monotonic identity for this claim. Unlike `attempts`, it is never
    /// refunded when unstarted work is released.
    pub claim_seq: i64,
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
    /// claimed it). `claim_seq` fences lifecycle writes; `attempt` remains the
    /// refundable business-attempt count used by workflow bookkeeping.
    pub fn lease(&self, owner: impl Into<String>) -> JobLease {
        JobLease {
            id: self.id,
            owner: owner.into(),
            claim_seq: self.claim_seq,
            attempt: self.attempts,
        }
    }
}

/// Fencing token for one claim of a job. The owner names the worker process;
/// the monotonic claim sequence distinguishes claims even when a released
/// claim refunds the business attempt counter.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JobLease {
    pub id: Uuid,
    pub owner: String,
    pub claim_seq: i64,
    /// Business attempt number. It may be refunded for an unstarted claim and
    /// is not a fencing identity.
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

    /// Extend the lease of the exact monotonic claim identity.
    fn renew(
        &self,
        lease: &JobLease,
        duration: Duration,
    ) -> impl Future<Output = anyhow::Result<bool>> + Send;

    /// Terminal success of the exact monotonic claim identity.
    fn complete(&self, lease: &JobLease) -> impl Future<Output = anyhow::Result<bool>> + Send;

    /// Record a failure for the exact monotonic claim identity. A retry persists
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

    /// Release an unstarted claim only if the write can finish before the
    /// absolute shutdown deadline. Implementations with remote or database
    /// writes must enforce the deadline at the side-effect boundary as well
    /// as at the caller, so a cancelled future cannot apply a late release.
    fn release_until(
        &self,
        lease: &JobLease,
        deadline: std::time::Instant,
    ) -> impl Future<Output = anyhow::Result<bool>> + Send;

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

/// Shared intake cancellation. The owner joins intake after stopping it: a
/// partial batch must return its accepted fences rather than disappear.
#[derive(Default)]
pub struct IntakeControl {
    stopped: std::sync::atomic::AtomicBool,
    wake: std::sync::Mutex<Vec<std::task::Waker>>,
    deadline: std::sync::Mutex<Option<std::time::Instant>>,
}
impl IntakeControl {
    pub fn stop_until(&self, deadline: std::time::Instant) {
        *self
            .deadline
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(deadline);
        self.stop();
    }
    pub fn shutdown_deadline(&self) -> Option<std::time::Instant> {
        *self
            .deadline
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
    pub fn stop(&self) {
        self.stopped
            .store(true, std::sync::atomic::Ordering::Release);
        let waiters = std::mem::take(
            &mut *self
                .wake
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
        );
        for wake in waiters {
            wake.wake();
        }
    }
    pub fn is_stopped(&self) -> bool {
        self.stopped.load(std::sync::atomic::Ordering::Acquire)
    }
    pub async fn cancelled(&self) {
        futures::future::poll_fn(|context| {
            let mut waiters = self
                .wake
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if !waiters.iter().any(|w| w.will_wake(context.waker())) {
                waiters.push(context.waker().clone());
            }
            if self.is_stopped() {
                std::task::Poll::Ready(())
            } else {
                std::task::Poll::Pending
            }
        })
        .await
    }
}

/// Delivery mechanics: wake eligible work and hand it to the worker for a
/// workload pool. Delivery decides nothing about authoritative state; the
/// [`DurableExecutionStore`] claim/fence remains the authority.
///
/// PostgreSQL delivery *is* claiming: `next_work` claims and fences the
/// work it returns. JetStream delivery resolves identifier hints through a
/// targeted PostgreSQL claim before returning work. Neither transport grants
/// authority independently of that claim; broker ACK follows settlement.
pub trait WorkTransport: Send + Sync {
    /// PostgreSQL delivery couples recovery to polling; broker delivery uses
    /// the slower independent reconciler.
    fn recover_before_poll(&self) -> bool {
        true
    }

    /// Cooperatively stop and return any earlier claims from a partial batch.
    /// Broker implementations bound each external operation and retain the
    /// exact prepared fence if a claim commit has an ambiguous outcome.
    fn next_work_controlled(
        &self,
        request: WorkRequest<'_>,
        _control: &IntakeControl,
        _budget: Duration,
    ) -> impl Future<Output = anyhow::Result<Vec<ClaimedJob>>> + Send {
        self.next_work(request)
    }

    /// Confirm transport delivery only after persisted lifecycle settlement.
    /// PostgreSQL transport has no broker acknowledgement.
    fn settled(&self, _lease: &JobLease) -> impl Future<Output = anyhow::Result<()>> + Send {
        async { Ok(()) }
    }

    /// The next eligible work for `request.pool`, at most `request.max_jobs`
    /// items, each claimed and fenced for `request.owner`. Never claims in an
    /// unbounded loop: the caller bounds the batch and re-checks capacity on
    /// its next tick.
    fn next_work(
        &self,
        request: WorkRequest<'_>,
    ) -> impl Future<Output = anyhow::Result<Vec<ClaimedJob>>> + Send;
}
