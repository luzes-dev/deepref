//! Disposable operational capacity mirrors. A mirror may refuse, but never
//! authorizes work without an authoritative PostgreSQL lease.

use std::{future::Future, pin::Pin, time::Duration};

use deepref_domain::WorkspaceId;
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MirrorScope {
    Workspace(WorkspaceId),
    Route([u8; 32]),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MirrorScopeKind {
    Workspace,
    Route,
}

impl MirrorScope {
    pub const fn kind(self) -> MirrorScopeKind {
        match self {
            Self::Workspace(_) => MirrorScopeKind::Workspace,
            Self::Route(_) => MirrorScopeKind::Route,
        }
    }
}

/// The exact identity of an already-granted database slot. Reusing a slot
/// index never allows a former generation to remove its replacement.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MirrorLeaseId {
    pub holder: Uuid,
    pub generation: i64,
    pub slot: u32,
}

#[derive(Debug, Clone, Copy)]
pub struct MirrorPolicy {
    limit: u32,
    ttl: Duration,
}

impl MirrorPolicy {
    pub fn new(limit: u32, ttl: Duration) -> Option<Self> {
        (limit > 0
            && limit <= 10_000
            && ttl >= Duration::from_millis(1)
            && ttl <= Duration::from_secs(300))
        .then_some(Self { limit, ttl })
    }

    pub const fn limit(self) -> u32 {
        self.limit
    }

    pub const fn ttl(self) -> Duration {
        self.ttl
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MirrorDecision {
    Available,
    Saturated,
}

#[derive(Debug, Clone, Copy, thiserror::Error)]
#[error("concurrency acceleration is unavailable")]
pub struct MirrorUnavailable;

pub type MirrorFuture<'a, T> =
    Pin<Box<dyn Future<Output = Result<T, MirrorUnavailable>> + Send + 'a>>;

pub trait ConcurrencyMirror: Send + Sync {
    fn check(&self, scope: MirrorScope, policy: MirrorPolicy) -> MirrorFuture<'_, MirrorDecision>;

    /// Inserts an exact database grant, or refuses without changing other
    /// holders. A timeout is ambiguous and must not be retried blindly.
    fn hold(
        &self,
        scope: MirrorScope,
        lease: MirrorLeaseId,
        policy: MirrorPolicy,
    ) -> MirrorFuture<'_, MirrorDecision>;

    /// Extends only an existing live member. Reset or release is never undone
    /// by a delayed renewal. Missing entries remain safe PostgreSQL fallbacks.
    fn refresh(
        &self,
        scope: MirrorScope,
        lease: MirrorLeaseId,
        policy: MirrorPolicy,
    ) -> MirrorFuture<'_, ()>;

    fn release(&self, scope: MirrorScope, lease: MirrorLeaseId) -> MirrorFuture<'_, ()>;
}

#[derive(Debug, Clone, Copy)]
pub enum MirrorOperation {
    Check,
    Hold,
    Refresh,
    Release,
}

#[derive(Debug, Clone, Copy)]
pub enum MirrorOutcome {
    Available,
    Saturated,
    Unavailable,
}

/// Closed dimensions only; identities, routes and driver text are excluded.
pub trait ConcurrencyMirrorObserver: Send + Sync {
    fn observe(
        &self,
        scope: MirrorScopeKind,
        operation: MirrorOperation,
        outcome: MirrorOutcome,
        elapsed: Duration,
    );
}

pub struct NoopConcurrencyMirrorObserver;

impl ConcurrencyMirrorObserver for NoopConcurrencyMirrorObserver {
    fn observe(&self, _: MirrorScopeKind, _: MirrorOperation, _: MirrorOutcome, _: Duration) {}
}
