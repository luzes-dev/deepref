//! Broker-independent integration inbox contracts (handoff 04a).
//!
//! Provider webhooks (WorkOS, Polar) arrive through a PostgreSQL-backed inbox.
//! The signature is verified before any insert, the provider-scoped
//! `(provider, provider_event_id)` key makes a duplicate delivery a committed
//! no-op, and the caller may only answer `2xx` after the insert or duplicate
//! check commits. Provider event ordering is never assumed: handlers reload
//! authoritative state and stay safe for duplicates and out-of-order events.
//!
//! This module is pure contract: the verifier port, the typed processing
//! registry and the reconciliation hook. Provider adapters land in handoffs 02
//! and 08; until then only [`HmacSha256Verifier`] (the test/development
//! verifier) implements [`WebhookVerifier`]. The persistence API and the
//! processor shell live in `deepref-app::inbox`.

use std::{collections::BTreeMap, sync::Arc};

use futures::future::BoxFuture;
use serde::de::DeserializeOwned;
use thiserror::Error;

/// Signature verification for one provider request. Implementations decide
/// how a provider authenticates; the receive API calls them before any
/// database write and rejects the request when they fail.
pub trait WebhookVerifier: Send + Sync {
    fn verify(&self, body: &[u8], signature: Option<&str>) -> Result<(), WebhookVerificationError>;
}

#[derive(Debug, Clone, Error, PartialEq, Eq)]
pub enum WebhookVerificationError {
    #[error("request carried no signature")]
    MissingSignature,
    #[error("signature did not match the request body")]
    InvalidSignature,
    #[error("signature verification failed: {0}")]
    Other(String),
}

/// HMAC-SHA256 verifier for the `sha256=<hex>` header format used by the
/// repository's webhook signing (`deepref-application::workflows::security`).
///
/// This is the test/development verifier for the inbox pipeline. Real provider
/// adapters (WorkOS, Polar) arrive in handoffs 02 and 08 with their own
/// [`WebhookVerifier`] implementations and secret handling.
#[derive(Clone)]
pub struct HmacSha256Verifier {
    secret: String,
}

impl HmacSha256Verifier {
    pub fn new(secret: impl Into<String>) -> Self {
        Self {
            secret: secret.into(),
        }
    }
}

impl WebhookVerifier for HmacSha256Verifier {
    fn verify(&self, body: &[u8], signature: Option<&str>) -> Result<(), WebhookVerificationError> {
        let signature = signature.ok_or(WebhookVerificationError::MissingSignature)?;
        if crate::workflows::security::verify_signature(&self.secret, body, signature) {
            Ok(())
        } else {
            Err(WebhookVerificationError::InvalidSignature)
        }
    }
}

/// The processing state machine of one inbox row:
/// `received -> processing -> processed`, or `failed` (retryable, with backoff)
/// until bounded retries quarantine the event for operational review and
/// replay.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InboxProcessingState {
    Received,
    Processing,
    Processed,
    Failed,
    Quarantined,
}

/// Identity of one delivered provider event. Handlers see this plus the typed
/// payload, never claim rows (lease owner, lease expiry).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InboxContext {
    pub inbox_id: i64,
    pub provider: String,
    pub provider_event_id: String,
    pub event_type: String,
    /// Fencing capability for processor state changes and for handlers that
    /// apply provider effects inside a database transaction.
    pub claim_fence: InboxClaimFence,
}

/// Capability proving which active inbox claim a handler is processing.
/// Persistence compares the monotonically increasing generation as well as
/// the owner and lease expiry, so an old handler cannot complete a reclaimed
/// event even if its worker owner string is reused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InboxClaimFence {
    owner: String,
    generation: i64,
}

impl InboxClaimFence {
    pub fn new(owner: impl Into<String>, generation: i64) -> Self {
        Self {
            owner: owner.into(),
            generation,
        }
    }

    pub fn owner(&self) -> &str {
        &self.owner
    }

    pub const fn generation(&self) -> i64 {
        self.generation
    }
}

/// What a handler reports after attempting its effect.
#[derive(Debug, Clone, Error)]
pub enum InboxHandlerError {
    /// The effect did not commit; retry with backoff.
    #[error("transient inbox handler failure: {0}")]
    Transient(String),
    /// The event can never be processed; quarantine it now.
    #[error("terminal inbox handler failure: {0}")]
    Terminal(String),
}

/// A typed handler for one provider event type. Handlers must be idempotent
/// and order-safe: duplicates and out-of-order delivery are normal.
pub type TypedInboxHandler<T> =
    Arc<dyn Fn(T, InboxContext) -> BoxFuture<'static, Result<(), InboxHandlerError>> + Send + Sync>;

/// The type-erased form the processor shell invokes: the registry
/// deserializes the stored payload into the registered typed message before
/// calling the typed handler. Named publicly so the persistence layer can hold
/// and invoke it; application code registers through
/// [`InboxRegistry::register`].
pub type ErasedInboxHandler = Arc<
    dyn Fn(InboxContext, serde_json::Value) -> BoxFuture<'static, Result<(), InboxHandlerError>>
        + Send
        + Sync,
>;

#[derive(Debug, Clone, Error, PartialEq, Eq)]
pub enum InboxRegistryError {
    #[error("inbox handler for {0}/{1} is already registered")]
    DuplicateHandler(String, String),
}

/// Typed handler registry keyed by `(provider, event_type)`. An event type no
/// handler is registered for is quarantined terminally (visible for review and
/// replay), never silently discarded.
#[derive(Clone, Default)]
pub struct InboxRegistry {
    handlers: BTreeMap<(String, String), ErasedInboxHandler>,
}

impl InboxRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register<T: DeserializeOwned + 'static>(
        &mut self,
        provider: &str,
        event_type: &str,
        handler: TypedInboxHandler<T>,
    ) -> Result<(), InboxRegistryError> {
        let key = (provider.to_owned(), event_type.to_owned());
        if self.handlers.contains_key(&key) {
            return Err(InboxRegistryError::DuplicateHandler(key.0, key.1));
        }
        let erased: ErasedInboxHandler =
            Arc::new(move |context: InboxContext, payload: serde_json::Value| {
                let handler = Arc::clone(&handler);
                Box::pin(async move {
                    let message: T = serde_json::from_value(payload).map_err(|error| {
                        InboxHandlerError::Terminal(format!(
                            "malformed payload for {}/{}/{}: {error}",
                            context.provider, context.event_type, context.provider_event_id
                        ))
                    })?;
                    handler(message, context).await
                })
            });
        self.handlers.insert(key, erased);
        Ok(())
    }

    pub fn handler(&self, provider: &str, event_type: &str) -> Option<&ErasedInboxHandler> {
        self.handlers
            .get(&(provider.to_owned(), event_type.to_owned()))
    }
}

/// What a failed processing attempt asks the processor to do next.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InboxRetry {
    /// Retry after the delay, quarantining the event once its attempt budget
    /// is spent.
    After(std::time::Duration),
    /// Quarantine the event now, whatever budget is left.
    Terminal,
}

/// Result of one reconciliation pass.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ReconciliationOutcome {
    /// Provider records inspected.
    pub checked: u64,
    /// Missed, delayed or quarantined events repaired (re-received or
    /// re-applied).
    pub repaired: u64,
}

#[derive(Debug, Clone, Error)]
pub enum ReconciliationError {
    #[error("reconciliation for {provider} failed: {message}")]
    Failed { provider: String, message: String },
}

/// Periodic repair hook for one provider. Provider adapters (handoffs 02 and
/// 08) implement it to re-fetch provider state so missed, delayed or
/// quarantined events are repaired even when the webhook stream lost them; the
/// worker reconciler invokes the registered hooks. No provider implements it
/// yet.
pub trait ProviderReconciler: Send + Sync {
    fn provider(&self) -> &str;
    fn reconcile(&self) -> BoxFuture<'static, Result<ReconciliationOutcome, ReconciliationError>>;
}

/// Registered provider reconciliation hooks. The worker runs these from its
/// existing periodic reconciler; a failing provider does not prevent other
/// providers from reconciling during the same pass.
#[derive(Clone, Default)]
pub struct ProviderReconcilerRegistry {
    providers: BTreeMap<String, Arc<dyn ProviderReconciler>>,
}

#[derive(Debug, Clone, Error, PartialEq, Eq)]
pub enum ProviderReconcilerRegistryError {
    #[error("provider reconciler for {0} is already registered")]
    DuplicateProvider(String),
}

impl ProviderReconcilerRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register(
        &mut self,
        reconciler: Arc<dyn ProviderReconciler>,
    ) -> Result<(), ProviderReconcilerRegistryError> {
        let provider = reconciler.provider().to_owned();
        if self.providers.contains_key(&provider) {
            return Err(ProviderReconcilerRegistryError::DuplicateProvider(provider));
        }
        self.providers.insert(provider, reconciler);
        Ok(())
    }

    pub async fn reconcile_all(
        &self,
    ) -> Vec<(String, Result<ReconciliationOutcome, ReconciliationError>)> {
        let mut outcomes = Vec::with_capacity(self.providers.len());
        for (provider, reconciler) in &self.providers {
            outcomes.push((provider.clone(), reconciler.reconcile().await));
        }
        outcomes
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workflows::security::sign_payload;

    #[test]
    fn hmac_verifier_accepts_its_own_signature_and_rejects_tampering() {
        let verifier = HmacSha256Verifier::new("test-secret");
        let body = br#"{"provider_event_id":"evt_1"}"#;
        let signature = sign_payload("test-secret", body);
        assert!(verifier.verify(body, Some(&signature)).is_ok());
        assert_eq!(
            verifier.verify(b"{}", Some(&signature)),
            Err(WebhookVerificationError::InvalidSignature)
        );
        assert_eq!(
            verifier.verify(body, None),
            Err(WebhookVerificationError::MissingSignature)
        );
    }

    #[derive(Debug, serde::Deserialize)]
    struct ProviderEvent {
        #[allow(dead_code)]
        id: String,
    }

    #[test]
    fn registry_is_keyed_by_provider_and_event_type() {
        let mut registry = InboxRegistry::new();
        let handler: TypedInboxHandler<ProviderEvent> =
            Arc::new(|_message, _context| Box::pin(async { Ok(()) }));
        registry
            .register("workos", "user.created", Arc::clone(&handler))
            .expect("registers");
        assert!(registry.handler("workos", "user.created").is_some());
        assert!(registry.handler("workos", "user.deleted").is_none());
        assert!(registry.handler("polar", "user.created").is_none());
        let duplicate = registry.register("workos", "user.created", handler);
        assert!(matches!(
            duplicate,
            Err(InboxRegistryError::DuplicateHandler(_, _))
        ));
    }

    struct TestReconciler {
        provider: &'static str,
        fails: bool,
    }

    impl ProviderReconciler for TestReconciler {
        fn provider(&self) -> &str {
            self.provider
        }

        fn reconcile(
            &self,
        ) -> BoxFuture<'static, Result<ReconciliationOutcome, ReconciliationError>> {
            let provider = self.provider.to_owned();
            let fails = self.fails;
            Box::pin(async move {
                if fails {
                    Err(ReconciliationError::Failed {
                        provider,
                        message: "provider unavailable".to_owned(),
                    })
                } else {
                    Ok(ReconciliationOutcome {
                        checked: 4,
                        repaired: 1,
                    })
                }
            })
        }
    }

    #[test]
    fn provider_reconciliation_runs_every_registered_hook_and_keeps_failures_visible() {
        let mut registry = ProviderReconcilerRegistry::new();
        registry
            .register(Arc::new(TestReconciler {
                provider: "workos",
                fails: false,
            }))
            .expect("WorkOS reconciler registers");
        registry
            .register(Arc::new(TestReconciler {
                provider: "polar",
                fails: true,
            }))
            .expect("Polar reconciler registers");

        let results = futures::executor::block_on(registry.reconcile_all());
        assert_eq!(results.len(), 2);
        assert!(results.iter().any(|(provider, result)| {
            provider == "workos"
                && matches!(
                    result,
                    Ok(ReconciliationOutcome {
                        checked: 4,
                        repaired: 1
                    })
                )
        }));
        assert!(results.iter().any(|(provider, result)| {
            provider == "polar" && matches!(result, Err(ReconciliationError::Failed { .. }))
        }));
    }
}
