//! Operational acceleration around authoritative PostgreSQL AI leases.

use std::{sync::Arc, time::Instant};

use deepref_ai::{
    AdmissionError, AdmissionFuture, AiConcurrencyAdmission, AiConcurrencyScope,
    AiConcurrencyScopeKind, SlotGrant, SlotLease,
};
use deepref_application::concurrency_mirror::{
    ConcurrencyMirror, ConcurrencyMirrorObserver, MirrorDecision, MirrorFuture, MirrorLeaseId,
    MirrorOperation, MirrorOutcome, MirrorPolicy, MirrorScope, MirrorUnavailable,
    NoopConcurrencyMirrorObserver,
};
use sha2::{Digest, Sha256};

pub struct MirroredAiConcurrencyAdmission {
    canonical: Arc<dyn AiConcurrencyAdmission>,
    mirror: Arc<dyn ConcurrencyMirror>,
    observer: Arc<dyn ConcurrencyMirrorObserver>,
}

impl MirroredAiConcurrencyAdmission {
    pub fn new(
        canonical: Arc<dyn AiConcurrencyAdmission>,
        mirror: Arc<dyn ConcurrencyMirror>,
    ) -> Self {
        Self {
            canonical,
            mirror,
            observer: Arc::new(NoopConcurrencyMirrorObserver),
        }
    }

    pub fn with_observer(mut self, observer: Arc<dyn ConcurrencyMirrorObserver>) -> Self {
        self.observer = observer;
        self
    }

    fn policies(&self, scope: &AiConcurrencyScope) -> Option<[(MirrorScope, MirrorPolicy); 2]> {
        let ttl = self.canonical.renew_interval().saturating_mul(3);
        Some([
            (
                MirrorScope::Workspace(scope.workspace_id),
                MirrorPolicy::new(scope.limits.workspace, ttl)?,
            ),
            (
                MirrorScope::Route(Sha256::digest(scope.route_key().as_bytes()).into()),
                MirrorPolicy::new(scope.limits.route, ttl)?,
            ),
        ])
    }

    async fn decision(
        &self,
        scope: MirrorScope,
        operation: MirrorOperation,
        future: MirrorFuture<'_, MirrorDecision>,
    ) -> Result<MirrorDecision, MirrorUnavailable> {
        let started = Instant::now();
        let result = future.await;
        let outcome = match result {
            Ok(MirrorDecision::Available) => MirrorOutcome::Available,
            Ok(MirrorDecision::Saturated) => MirrorOutcome::Saturated,
            Err(_) => MirrorOutcome::Unavailable,
        };
        self.observer
            .observe(scope.kind(), operation, outcome, started.elapsed());
        result
    }

    async fn release_mirror(&self, lease: &SlotLease) {
        if let Some(scopes) = self.policies(&lease.scope) {
            for ((scope, _), grant) in scopes.into_iter().zip(lease.grants()) {
                let _ = self
                    .decision(
                        scope,
                        MirrorOperation::Release,
                        Box::pin(async {
                            self.mirror.release(scope, identity(grant)).await?;
                            Ok(MirrorDecision::Available)
                        }),
                    )
                    .await;
            }
        }
    }
}

fn identity(grant: &SlotGrant) -> MirrorLeaseId {
    MirrorLeaseId {
        holder: grant.holder_id,
        generation: grant.generation,
        slot: grant.slot_index,
    }
}

fn saturated(scope: MirrorScope) -> AdmissionError {
    AdmissionError::Saturated {
        scope_kind: match scope {
            MirrorScope::Workspace(_) => AiConcurrencyScopeKind::Workspace,
            MirrorScope::Route(_) => AiConcurrencyScopeKind::Route,
        },
    }
}

impl AiConcurrencyAdmission for MirroredAiConcurrencyAdmission {
    fn acquire<'a>(&'a self, scope: AiConcurrencyScope) -> AdmissionFuture<'a, SlotLease> {
        Box::pin(async move {
            // Unsupported optional mirror policy never weakens the database
            // ceiling. Valid application policy still uses the canonical port.
            let Some(scopes) = self.policies(&scope) else {
                return self.canonical.acquire(scope).await;
            };
            for (key, policy) in scopes {
                if matches!(
                    self.decision(key, MirrorOperation::Check, self.mirror.check(key, policy))
                        .await,
                    Ok(MirrorDecision::Saturated)
                ) {
                    return Err(saturated(key));
                }
            }
            let lease = self.canonical.acquire(scope).await?;
            for ((key, policy), grant) in scopes.into_iter().zip(lease.grants()) {
                if matches!(
                    self.decision(
                        key,
                        MirrorOperation::Hold,
                        self.mirror.hold(key, identity(grant), policy)
                    )
                    .await,
                    Ok(MirrorDecision::Saturated)
                ) {
                    // Refusal is truthful only after the database grant is
                    // proven released. An ambiguous release is not saturation.
                    self.canonical.release(lease.clone()).await?;
                    self.release_mirror(&lease).await;
                    return Err(saturated(key));
                }
            }
            Ok(lease)
        })
    }

    fn renew<'a>(&'a self, lease: &'a SlotLease) -> AdmissionFuture<'a, ()> {
        Box::pin(async move {
            let started = Instant::now();
            self.canonical.renew(lease).await?;
            let Some(scopes) = self.policies(&lease.scope) else {
                return Ok(());
            };
            // Optional work cannot consume the enclosing canonical heartbeat
            // deadline. No Valkey result invalidates a proven database renewal.
            let remaining = self.renew_interval().saturating_sub(started.elapsed());
            for ((key, policy), grant) in scopes.into_iter().zip(lease.grants()) {
                let _ = self
                    .decision(
                        key,
                        MirrorOperation::Refresh,
                        Box::pin(async {
                            tokio::time::timeout(
                                remaining / 8,
                                self.mirror.refresh(key, identity(grant), policy),
                            )
                            .await
                            .map_err(|_| MirrorUnavailable)??;
                            Ok(MirrorDecision::Available)
                        }),
                    )
                    .await;
            }
            Ok(())
        })
    }

    fn release<'a>(&'a self, lease: SlotLease) -> AdmissionFuture<'a, ()> {
        Box::pin(async move {
            self.canonical.release(lease.clone()).await?;
            self.release_mirror(&lease).await;
            Ok(())
        })
    }

    fn renew_interval(&self) -> std::time::Duration {
        self.canonical.renew_interval()
    }
}
