//! Operational concurrency admission for AI dispatch.
//!
//! Admission caps simultaneous provider calls where the limit is operational
//! rather than financial:
//!
//! ```text
//! a workspace may have at most N simultaneous AI calls
//! a provider/model route may have at most M simultaneous calls
//! ```
//!
//! It is the application port behind the PostgreSQL leased-slot table
//! (`ai_concurrency_slots`) that exists before `T-VALKEY` and remains the
//! conservative fallback whenever Valkey is unavailable (spec 06/09). A later
//! Valkey adapter replaces the PostgreSQL adapter with the same semantics:
//! both grant, renew and release through this port, and neither may ever change
//! a reservation, a settlement or a ledger number.
//!
//! ## Lease contract
//!
//! - [`AiConcurrencyAdmission::acquire`] is non-blocking: it either grants both
//!   keys of a scope or refuses with [`AdmissionError::Saturated`]. Both keys
//!   are granted in one unit and one fixed order (workspace, then route), so a
//!   caller never holds one without the other.
//! - A grant is owned by the exact `(holder, generation)` identity of its
//!   lease. `generation` is drawn from a sequence on every grant and is never
//!   reused, so a lease whose slot was freed or swept is always distinguishable
//!   from a newer grant of the same slot.
//! - [`AiConcurrencyAdmission::renew`] and [`AiConcurrencyAdmission::release`]
//!   are all-or-nothing and fenced by that identity: while the lease still owns
//!   both slots they apply; the moment either slot is owned by another
//!   generation (or is gone) the write is a no-op that reports
//!   [`AdmissionError::LeaseLost`]. The holder must then stop before further
//!   provider liability, the same way a fenced reservation top-up stops: it no
//!   longer owns capacity it is about to spend.
//! - A crashed holder's slots become reusable when their lease expires; no
//!   explicit release is required.
//!
//! ## Nested calls
//!
//! Every provider call takes its own slot, including nested tool/model calls
//! inside agent runs. Acquire never blocks, so a nested call that cannot be
//! admitted fails fast with [`AdmissionError::Saturated`] instead of waiting on
//! its parent: a parent holding the workspace's last slot can never deadlock
//! against a child that needs another.

use std::{
    fmt,
    future::Future,
    pin::Pin,
    sync::{
        Arc, OnceLock,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

use chrono::{DateTime, Utc};
use deepref_domain::WorkspaceId;
use serde::Serialize;
use uuid::Uuid;

/// How long one grant stays live without renewal. Long calls and streams
/// renew at [`SLOT_LEASE_RENEW_INTERVAL`]; a holder that crashes simply stops
/// renewing and its slots are reclaimable at expiry.
pub const SLOT_LEASE_TTL: Duration = Duration::from_secs(60);
/// Renewal cadence: one third of the TTL, the same ratio the 05a/04a leased
/// handlers use, so one missed renewal never consumes the whole lease.
pub const SLOT_LEASE_RENEW_INTERVAL: Duration = Duration::from_secs(20);

/// Future shape of the admission port.
pub type AdmissionFuture<'a, T> =
    Pin<Box<dyn Future<Output = Result<T, AdmissionError>> + Send + 'a>>;

/// The two keys an admission scope is made of.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AiConcurrencyScopeKind {
    /// Max N simultaneous AI calls for one workspace.
    Workspace,
    /// Max M simultaneous calls for one provider/model route.
    Route,
}

impl AiConcurrencyScopeKind {
    /// In the fixed acquisition order: workspace first, then route.
    pub const ORDER: [Self; 2] = [Self::Workspace, Self::Route];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Workspace => "workspace",
            Self::Route => "route",
        }
    }
}

impl fmt::Display for AiConcurrencyScopeKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// Configured simultaneous-call ceilings. The route ceiling may be narrowed
/// per route by the reviewed capability table; a lowered limit stops granting
/// slots at or above the new value while leases already granted there drain.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AiConcurrencyLimits {
    pub workspace: u32,
    pub route: u32,
}

impl Default for AiConcurrencyLimits {
    fn default() -> Self {
        Self {
            workspace: 4,
            route: 16,
        }
    }
}

/// One call's admission scope: the two keys that must both be granted, each
/// with the limit it is admitted under.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AiConcurrencyScope {
    pub workspace_id: WorkspaceId,
    pub provider: String,
    pub model: String,
    pub limits: AiConcurrencyLimits,
}

impl AiConcurrencyScope {
    pub fn new(
        workspace_id: WorkspaceId,
        provider: impl Into<String>,
        model: impl Into<String>,
        limits: AiConcurrencyLimits,
    ) -> Self {
        Self {
            workspace_id,
            provider: provider.into(),
            model: model.into(),
            limits,
        }
    }

    /// The scope key of the provider/model route.
    pub fn route_key(&self) -> String {
        format!("{}/{}", self.provider, self.model)
    }

    /// Both keys in the fixed acquisition order (workspace, then route) with
    /// the limit each key is admitted under. Every adapter must acquire them
    /// in exactly this order so concurrent callers cannot deadlock.
    pub fn keys(&self) -> [(AiConcurrencyScopeKind, String, u32); 2] {
        [
            (
                AiConcurrencyScopeKind::Workspace,
                self.workspace_id.as_uuid().to_string(),
                self.limits.workspace,
            ),
            (
                AiConcurrencyScopeKind::Route,
                self.route_key(),
                self.limits.route,
            ),
        ]
    }
}

/// One granted slot of one scope key, with the fencing identity that later
/// renewals and releases must present.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SlotGrant {
    pub scope_kind: AiConcurrencyScopeKind,
    pub scope_key: String,
    pub slot_index: u32,
    /// Owner identity: the worker or process holding the slot.
    pub holder_id: Uuid,
    /// Fencing generation of this grant. Never reused, so a stale lease can
    /// never impersonate a newer grant of the same slot.
    pub generation: i64,
    pub lease_expires_at: DateTime<Utc>,
}

/// Both slots of one call, granted atomically. The lease is only usable by the
/// holder identity that acquired it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SlotLease {
    pub scope: AiConcurrencyScope,
    pub workspace: SlotGrant,
    pub route: SlotGrant,
}

impl SlotLease {
    /// Both grants in the fixed order used for renewal and release.
    pub fn grants(&self) -> [&SlotGrant; 2] {
        [&self.workspace, &self.route]
    }
}

/// Typed outcomes of the admission port.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum AdmissionError {
    /// No slot is free in this scope. The call is refused before dispatch and
    /// must not be retried against the same saturation.
    #[error("AI concurrency is saturated: {scope_kind}")]
    Saturated { scope_kind: AiConcurrencyScopeKind },
    /// The lease no longer owns its slots (another generation holds them, or
    /// they are gone). The renewal or release was a no-op; the caller must stop
    /// before further provider liability.
    #[error("the AI concurrency lease is no longer owned")]
    LeaseLost,
    /// Admission state could not be read or written. Callers fail closed: an
    /// unproven slot is never treated as a granted one.
    #[error("AI concurrency admission failed: {0}")]
    Persistence(String),
}

impl AdmissionError {
    /// The dispatch-facing error. A refusal keeps its scope kind; a lost lease
    /// becomes its own typed error so callers stop instead of retrying.
    pub(crate) fn into_ai_error(self) -> crate::AiError {
        match self {
            Self::Saturated { scope_kind } => crate::AiError::AdmissionSaturated { scope_kind },
            Self::LeaseLost => crate::AiError::AdmissionLeaseLost,
            Self::Persistence(reason) => crate::AiError::Persistence(reason),
        }
    }
}

/// Leased concurrency admission: grant, renew and release. The PostgreSQL
/// leased-slot table is the first adapter; a later Valkey adapter keeps these
/// exact semantics.
pub trait AiConcurrencyAdmission: Send + Sync {
    /// Grants both keys of `scope` without blocking, or refuses with
    /// [`AdmissionError::Saturated`]. The two keys are acquired in one
    /// transaction in [`AiConcurrencyScope::keys`] order, so a refusal leaves
    /// no slot held.
    fn acquire<'a>(&'a self, scope: AiConcurrencyScope) -> AdmissionFuture<'a, SlotLease>;

    /// Extends both slots of a lease the caller still owns. A renewal that
    /// finds either slot owned by another generation changes nothing and
    /// reports [`AdmissionError::LeaseLost`].
    fn renew<'a>(&'a self, lease: &'a SlotLease) -> AdmissionFuture<'a, ()>;

    /// Frees both slots of a lease the caller still owns. A release that finds
    /// either slot owned by another generation changes nothing and reports
    /// [`AdmissionError::LeaseLost`]; the freed slot then expires on its own.
    fn release<'a>(&'a self, lease: SlotLease) -> AdmissionFuture<'a, ()>;

    /// How often a live lease is renewed: one third of its TTL, the same ratio
    /// the 05a/04a leased handlers use, so one missed renewal never consumes
    /// the whole lease. Adapters with a non-default lease duration scale it.
    fn renew_interval(&self) -> Duration {
        SLOT_LEASE_RENEW_INTERVAL
    }
}

/// Runs work with bounded lease renewal. Any renewal failure stops work:
/// capacity cannot be assumed during a database outage or a hung renewal.
/// Work is polled concurrently with renewal, so a slow database never stalls
/// cancellation or prevents a provider future from making progress.
pub async fn renew_while<T>(
    admission: &dyn AiConcurrencyAdmission,
    lease: &SlotLease,
    work: impl Future<Output = T>,
) -> Result<T, AdmissionError> {
    tokio::pin!(work);
    let heartbeat = maintain_lease(admission, lease);
    tokio::pin!(heartbeat);
    tokio::select! {
        biased;
        result = &mut heartbeat => { result?; Err(AdmissionError::LeaseLost) }
        result = &mut work => Ok(result),
    }
}

async fn maintain_lease(
    admission: &dyn AiConcurrencyAdmission,
    lease: &SlotLease,
) -> Result<(), AdmissionError> {
    loop {
        tokio::time::sleep(admission.renew_interval()).await;
        tokio::time::timeout(admission.renew_interval(), admission.renew(lease))
            .await
            .map_err(|_| AdmissionError::LeaseLost)??;
    }
}

/// Owns a slot across cancellation points. Normal completion awaits release;
/// dropping a cancelled call schedules bounded cleanup. If the runtime is
/// already shutting down, database expiry remains the crash fallback.
pub(crate) struct OwnedSlot {
    admission: Arc<dyn AiConcurrencyAdmission>,
    lease: Option<SlotLease>,
}

impl OwnedSlot {
    pub(crate) fn new(admission: Arc<dyn AiConcurrencyAdmission>, lease: SlotLease) -> Self {
        Self {
            admission,
            lease: Some(lease),
        }
    }

    pub(crate) fn lease(&self) -> Option<&SlotLease> {
        self.lease.as_ref()
    }

    pub(crate) async fn release(mut self) {
        if let Some(lease) = self.lease.as_ref() {
            release_bounded(self.admission.as_ref(), lease.clone()).await;
            self.lease.take();
        }
    }
}

impl Drop for OwnedSlot {
    fn drop(&mut self) {
        if let Some(lease) = self.lease.take()
            && let Ok(runtime) = tokio::runtime::Handle::try_current()
        {
            let admission = self.admission.clone();
            runtime.spawn(async move {
                release_bounded(admission.as_ref(), lease).await;
            });
        }
    }
}

async fn release_bounded(admission: &dyn AiConcurrencyAdmission, lease: SlotLease) {
    match tokio::time::timeout(Duration::from_secs(5), admission.release(lease)).await {
        Ok(Ok(())) => {}
        Ok(Err(error)) => {
            tracing::warn!(%error, "AI concurrency slot release failed; expiry will recover it")
        }
        Err(_) => tracing::warn!("AI concurrency slot release timed out; expiry will recover it"),
    }
}

/// Background heartbeat for streamed calls. The loss callback cancels the
/// active run, including a stalled stream, rather than just its next dispatch.
pub struct SlotHeartbeat {
    task: Option<tokio::task::JoinHandle<()>>,
}

impl SlotHeartbeat {
    pub fn start(
        admission: Arc<dyn AiConcurrencyAdmission>,
        lease: SlotLease,
        on_lost: impl Fn() + Send + Sync + 'static,
    ) -> Self {
        let Ok(runtime) = tokio::runtime::Handle::try_current() else {
            on_lost();
            return Self { task: None };
        };
        let task = runtime.spawn(async move {
            if let Err(error) =
                renew_while(admission.as_ref(), &lease, std::future::pending::<()>()).await
            {
                tracing::warn!(%error, "AI concurrency heartbeat lost ownership");
                on_lost();
            }
        });
        Self { task: Some(task) }
    }
}

impl Drop for SlotHeartbeat {
    fn drop(&mut self) {
        if let Some(task) = self.task.take() {
            task.abort();
        }
    }
}

/// The fixed upper bounds of the acquire-latency histogram, in microseconds.
const ACQUIRE_BUCKETS_MICROS: [u64; 12] = [
    50, 100, 250, 500, 1_000, 2_500, 5_000, 10_000, 25_000, 50_000, 100_000, 250_000,
];

/// Fixed-bucket histogram of acquire latency. No histogram label carries a
/// workspace id, a route key or a cost.
#[derive(Debug, Default)]
pub struct Histogram {
    /// One counter per bucket upper bound, plus the overflow bucket.
    buckets: [AtomicU64; ACQUIRE_BUCKETS_MICROS.len() + 1],
    count: AtomicU64,
    sum_micros: AtomicU64,
}

impl Histogram {
    fn record(&self, micros: u64) {
        let bucket = ACQUIRE_BUCKETS_MICROS
            .iter()
            .position(|upper| micros <= *upper)
            .unwrap_or(ACQUIRE_BUCKETS_MICROS.len());
        self.buckets[bucket].fetch_add(1, Ordering::Relaxed);
        self.count.fetch_add(1, Ordering::Relaxed);
        self.sum_micros.fetch_add(micros, Ordering::Relaxed);
    }

    fn snapshot(&self) -> HistogramSnapshot {
        HistogramSnapshot {
            buckets: ACQUIRE_BUCKETS_MICROS
                .iter()
                .enumerate()
                .map(|(index, upper)| (*upper, self.buckets[index].load(Ordering::Relaxed)))
                .chain([(
                    u64::MAX,
                    self.buckets[ACQUIRE_BUCKETS_MICROS.len()].load(Ordering::Relaxed),
                )])
                .collect(),
            count: self.count.load(Ordering::Relaxed),
            sum_micros: self.sum_micros.load(Ordering::Relaxed),
        }
    }
}

/// Read-only view of one histogram.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct HistogramSnapshot {
    /// `(upper bound in microseconds, observations in that bucket)`, with
    /// `u64::MAX` as the overflow bucket.
    pub buckets: Vec<(u64, u64)>,
    pub count: u64,
    pub sum_micros: u64,
}

/// Counter pair indexed by [`AiConcurrencyScopeKind`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct ScopeCounters {
    pub workspace: u64,
    pub route: u64,
}

/// Read-only view of the admission counters.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AiConcurrencyMetricsSnapshot {
    /// Slots granted, by scope kind.
    pub grants: ScopeCounters,
    /// Calls refused because a scope had no free slot, by scope kind.
    pub saturations: ScopeCounters,
    /// Renewals or releases that found the lease fenced out.
    pub lost_leases: u64,
    /// Grants that reclaimed a slot from an expired lease (a crashed holder).
    pub expiries_reclaimed: u64,
    /// Acquire latency.
    pub acquire: HistogramSnapshot,
}

/// Process-wide admission metrics. Production adapters record here no matter
/// how many instances a process builds; tests construct their own
/// [`AiConcurrencyMetrics`] for isolated assertions.
pub fn admission_metrics() -> Arc<AiConcurrencyMetrics> {
    static METRICS: OnceLock<Arc<AiConcurrencyMetrics>> = OnceLock::new();
    Arc::clone(METRICS.get_or_init(|| Arc::new(AiConcurrencyMetrics::default())))
}

/// Admission counters and histograms. Labeled by scope kind only: workspace
/// ids, route keys and costs are never metric labels. Adapters record into
/// their own instance; one instance per process is the production shape.
#[derive(Debug, Default)]
pub struct AiConcurrencyMetrics {
    grants: [AtomicU64; 2],
    saturations: [AtomicU64; 2],
    lost_leases: AtomicU64,
    expiries_reclaimed: AtomicU64,
    acquire: Histogram,
}

impl AiConcurrencyMetrics {
    /// One granted slot. `reclaimed` marks a grant that took over a slot from
    /// an expired lease.
    pub fn record_grant(&self, kind: AiConcurrencyScopeKind, reclaimed: bool) {
        self.grants[scope_index(kind)].fetch_add(1, Ordering::Relaxed);
        if reclaimed {
            self.expiries_reclaimed.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// One refused call.
    pub fn record_saturation(&self, kind: AiConcurrencyScopeKind) {
        self.saturations[scope_index(kind)].fetch_add(1, Ordering::Relaxed);
    }

    /// One renewal or release that found the lease fenced out.
    pub fn record_lost_lease(&self) {
        self.lost_leases.fetch_add(1, Ordering::Relaxed);
    }

    /// One acquire, timed from request to grant.
    pub fn record_acquire(&self, latency: Duration) {
        self.acquire
            .record(latency.as_micros().min(u128::from(u64::MAX)) as u64);
    }

    pub fn snapshot(&self) -> AiConcurrencyMetricsSnapshot {
        AiConcurrencyMetricsSnapshot {
            grants: ScopeCounters {
                workspace: self.grants[scope_index(AiConcurrencyScopeKind::Workspace)]
                    .load(Ordering::Relaxed),
                route: self.grants[scope_index(AiConcurrencyScopeKind::Route)]
                    .load(Ordering::Relaxed),
            },
            saturations: ScopeCounters {
                workspace: self.saturations[scope_index(AiConcurrencyScopeKind::Workspace)]
                    .load(Ordering::Relaxed),
                route: self.saturations[scope_index(AiConcurrencyScopeKind::Route)]
                    .load(Ordering::Relaxed),
            },
            lost_leases: self.lost_leases.load(Ordering::Relaxed),
            expiries_reclaimed: self.expiries_reclaimed.load(Ordering::Relaxed),
            acquire: self.acquire.snapshot(),
        }
    }
}

const fn scope_index(kind: AiConcurrencyScopeKind) -> usize {
    match kind {
        AiConcurrencyScopeKind::Workspace => 0,
        AiConcurrencyScopeKind::Route => 1,
    }
}
