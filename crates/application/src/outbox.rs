//! Broker-independent outbox contracts (handoff 04a).
//!
//! PostgreSQL says what is true. A producer inserts an [`NewOutboxEvent`] in
//! the same transaction as its authoritative state change; an in-process
//! dispatcher later claims the row and invokes typed handlers. A row is
//! `delivered` only when every expected consumer's receipt has committed —
//! claiming or leasing never marks it delivered.
//!
//! This module is pure contract: message identity (type + version), the frozen
//! expected-consumer set, typed handler registration and retry vocabulary. It
//! has no database types. The persistence API lives in `deepref-app::outbox`
//! and takes the caller's transaction where the plan requires atomicity;
//! application handlers see typed messages and [`DeliveryContext`], never the
//! database claim rows (lease owner, lease expiry, claim attempts).
//!
//! Delivery is at least once. A crash after a handler's effect commits but
//! before its receipt commits can redeliver, so every handler must be
//! idempotent (unique idempotency keys, aggregate state checks, or durable job
//! enqueue with a stable dedupe key).

use std::{collections::BTreeMap, collections::BTreeSet, sync::Arc, time::Duration};

use futures::future::BoxFuture;
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use thiserror::Error;
use uuid::Uuid;

/// Maximum attempts recorded by [`retry_backoff`]'s doubling schedule before
/// the delay stops growing (30 s doubling, capped at 15 minutes — the same
/// recovery schedule the job queue uses).
const BACKOFF_CAP_SECONDS: u64 = 900;
const BACKOFF_BASE_SECONDS: u64 = 30;

/// Retry delay after `attempts` failed delivery attempts (1-based).
pub fn retry_backoff(attempts: i32) -> Duration {
    let exponent = u32::try_from(attempts.saturating_sub(1)).unwrap_or(u32::MAX);
    let seconds = BACKOFF_BASE_SECONDS
        .saturating_mul(2u64.saturating_pow(exponent.min(16)))
        .min(BACKOFF_CAP_SECONDS);
    Duration::from_secs(seconds)
}

/// One outbox message contract: the stable identity handlers register under
/// and the expected recipient set. The set is part of the contract and is
/// stable for a given type + version; adding, removing or renaming a consumer
/// requires a new message version or an explicit backfill/replay decision.
pub trait OutboxMessage: Serialize + DeserializeOwned + Send + Sync + 'static {
    /// Stable message type name (the routing key consumers subscribe by).
    const TYPE: &'static str;
    /// Contract version. Handlers register per (type, version).
    const VERSION: u32;
    /// Consumer names that must each commit a receipt before an event of this
    /// type + version is `delivered`.
    const CONSUMERS: &'static [&'static str];
}

/// A new outbox row, inserted by the persistence API in the caller's
/// transaction alongside the authoritative change it describes.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NewOutboxEvent {
    pub event_id: Uuid,
    pub message_type: String,
    pub message_version: u32,
    /// Stable identifiers and routing metadata, not a copy of aggregate state.
    pub payload: serde_json::Value,
    /// Frozen at insert from the message contract.
    pub expected_consumers: Vec<String>,
    /// Per-consumer attempt budget before a recipient is quarantined.
    pub max_attempts: i32,
}

impl NewOutboxEvent {
    pub const DEFAULT_MAX_ATTEMPTS: i32 = 8;

    /// Build the row for one typed message: the payload is the serialized
    /// message and the expected consumers come from the contract. The set is
    /// sorted and deduplicated so the frozen row is canonical.
    pub fn for_message<T: OutboxMessage>(
        event_id: Uuid,
        message: &T,
    ) -> Result<Self, serde_json::Error> {
        Ok(Self {
            event_id,
            message_type: T::TYPE.to_owned(),
            message_version: T::VERSION,
            payload: serde_json::to_value(message)?,
            expected_consumers: canonical_consumers(T::CONSUMERS),
            max_attempts: Self::DEFAULT_MAX_ATTEMPTS,
        })
    }

    /// An untyped row for a message contract that exists only in routing
    /// metadata (for example a replay of a historical event).
    pub fn untyped(
        event_id: Uuid,
        message_type: impl Into<String>,
        message_version: u32,
        payload: serde_json::Value,
        expected_consumers: &[&str],
    ) -> Self {
        Self {
            event_id,
            message_type: message_type.into(),
            message_version,
            payload,
            expected_consumers: canonical_consumers(expected_consumers),
            max_attempts: Self::DEFAULT_MAX_ATTEMPTS,
        }
    }

    pub fn with_max_attempts(mut self, max_attempts: i32) -> Self {
        self.max_attempts = max_attempts;
        self
    }
}

fn canonical_consumers(consumers: &[&str]) -> Vec<String> {
    let mut names: Vec<String> = consumers.iter().map(|name| (*name).to_owned()).collect();
    names.sort();
    names.dedup();
    names
}

/// Everything an application handler receives: the typed message plus the
/// stable identity of this delivery. Handlers record their receipt through the
/// persistence API inside their own transaction, together with their effect.
/// Claim rows (lease owner, lease expiry, claim attempts) are never exposed.
#[derive(Debug, Clone, PartialEq)]
pub struct DeliveryContext {
    pub event_id: Uuid,
    pub consumer: String,
    pub message_type: String,
    pub message_version: u32,
    /// Opaque fencing capability for recording this consumer's receipt in the
    /// same transaction as its effect. A claim generation changes on every
    /// reclaim, even when the same worker owner string is reused.
    pub claim_fence: OutboxClaimFence,
}

/// Capability proving which active outbox claim a consumer is completing.
/// Persistence uses both values and an unexpired-lease predicate; callers
/// must not persist or reuse a fence beyond this delivery attempt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutboxClaimFence {
    owner: String,
    generation: i64,
}

impl OutboxClaimFence {
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
pub enum OutboxHandlerError {
    /// The effect did not commit; retry this consumer with backoff.
    #[error("transient handler failure: {0}")]
    Transient(String),
    /// The effect can never commit for this message; quarantine the recipient
    /// now instead of spending the attempt budget.
    #[error("terminal handler failure: {0}")]
    Terminal(String),
}

/// A typed handler for one consumer of one message type + version. The handler
/// reports success only after its effect (and its receipt) have committed.
pub type TypedOutboxHandler<T> = Arc<
    dyn Fn(T, DeliveryContext) -> BoxFuture<'static, Result<(), OutboxHandlerError>> + Send + Sync,
>;

/// The type-erased form the dispatcher invokes: the registry deserializes the
/// stored payload into the registered typed message before calling the typed
/// handler. Named publicly so the persistence layer can hold and invoke it;
/// application code registers through [`OutboxRegistry::register`].
pub type ErasedOutboxHandler = Arc<
    dyn Fn(DeliveryContext, serde_json::Value) -> BoxFuture<'static, Result<(), OutboxHandlerError>>
        + Send
        + Sync,
>;

/// What the dispatcher failed to register.
#[derive(Debug, Clone, Error, PartialEq, Eq)]
pub enum RegistryError {
    #[error("message contract {0} v{1} is already registered")]
    DuplicateContract(String, u32),
    #[error(
        "handlers for {0} v{1} must cover exactly its declared consumers; declared {2:?}, got {3:?}"
    )]
    ConsumerSetMismatch(String, u32, Vec<String>, Vec<String>),
}

/// Typed handler registry keyed by message type + version. Registration
/// declares the expected consumers of the contract; the dispatcher uses that
/// declaration (frozen onto each outbox row at insert) to know when an event
/// is complete.
#[derive(Clone, Default)]
pub struct OutboxRegistry {
    contracts: BTreeMap<(String, u32), OutboxContract>,
}

#[derive(Clone)]
struct OutboxContract {
    consumers: BTreeSet<String>,
    handlers: BTreeMap<String, ErasedOutboxHandler>,
}

impl OutboxRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Register one handler per declared consumer of `T`. The handler names
    /// must cover `T::CONSUMERS` exactly: the expected recipient set is part
    /// of the message contract and may not drift silently.
    pub fn register<T: OutboxMessage>(
        &mut self,
        handlers: &[(&str, TypedOutboxHandler<T>)],
    ) -> Result<(), RegistryError> {
        let key = (T::TYPE.to_owned(), T::VERSION);
        if self.contracts.contains_key(&key) {
            return Err(RegistryError::DuplicateContract(key.0, key.1));
        }
        let declared: BTreeSet<String> =
            T::CONSUMERS.iter().map(|name| (*name).to_owned()).collect();
        let registered: BTreeSet<String> = handlers
            .iter()
            .map(|(name, _)| (*name).to_owned())
            .collect();
        if declared != registered {
            return Err(RegistryError::ConsumerSetMismatch(
                key.0,
                key.1,
                declared.into_iter().collect(),
                registered.into_iter().collect(),
            ));
        }
        let handlers: BTreeMap<String, ErasedOutboxHandler> = handlers
            .iter()
            .map(|(name, handler)| {
                let handler: TypedOutboxHandler<T> = Arc::clone(handler);
                let erased: ErasedOutboxHandler = Arc::new(
                    move |context: DeliveryContext, payload: serde_json::Value| {
                        let handler = Arc::clone(&handler);
                        Box::pin(async move {
                            let message: T = serde_json::from_value(payload).map_err(|error| {
                                OutboxHandlerError::Terminal(format!(
                                    "malformed payload for {} v{}: {error}",
                                    T::TYPE,
                                    T::VERSION
                                ))
                            })?;
                            handler(message, context).await
                        })
                    },
                );
                ((*name).to_owned(), erased)
            })
            .collect();
        self.contracts.insert(
            key,
            OutboxContract {
                consumers: declared,
                handlers,
            },
        );
        Ok(())
    }

    /// Contracts this worker can safely claim, used by persistence filtering.
    pub fn contracts(&self) -> impl Iterator<Item = (&str, u32)> {
        self.contracts
            .keys()
            .map(|(kind, version)| (kind.as_str(), *version))
    }

    /// The frozen expected consumers of this contract, when it is registered.
    pub fn expected_consumers(
        &self,
        message_type: &str,
        message_version: u32,
    ) -> Option<&BTreeSet<String>> {
        self.contracts
            .get(&(message_type.to_owned(), message_version))
            .map(|contract| &contract.consumers)
    }

    /// The handler one consumer of this contract is dispatched to.
    pub fn handler(
        &self,
        message_type: &str,
        message_version: u32,
        consumer: &str,
    ) -> Option<&ErasedOutboxHandler> {
        self.contracts
            .get(&(message_type.to_owned(), message_version))
            .and_then(|contract| contract.handlers.get(consumer))
    }

    /// False when no contract is registered for this type + version. Workers
    /// leave unsupported contracts pending for a compatible deployment.
    pub fn knows(&self, message_type: &str, message_version: u32) -> bool {
        self.contracts
            .contains_key(&(message_type.to_owned(), message_version))
    }
}

/// What a failed delivery attempt asks the dispatcher to do next.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutboxRetry {
    /// Retry this consumer after the delay, quarantining it once its attempt
    /// budget is spent.
    After(Duration),
    /// Quarantine this consumer now, whatever budget is left.
    Terminal,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, Clone, Serialize, serde::Deserialize)]
    struct TestMessage {
        note: String,
    }

    impl OutboxMessage for TestMessage {
        const TYPE: &'static str = "test.message";
        const VERSION: u32 = 1;
        const CONSUMERS: &'static [&'static str] = &["beta", "alpha"];
    }

    fn handler<T: OutboxMessage + Clone>() -> TypedOutboxHandler<T> {
        Arc::new(|_message, _context| Box::pin(async { Ok(()) }))
    }

    #[test]
    fn for_message_freezes_a_canonical_consumer_set() {
        let event = NewOutboxEvent::for_message(
            Uuid::new_v4(),
            &TestMessage {
                note: "hello".to_owned(),
            },
        )
        .expect("serializes");
        assert_eq!(event.message_type, "test.message");
        assert_eq!(event.message_version, 1);
        assert_eq!(event.expected_consumers, vec!["alpha", "beta"]);
    }

    #[test]
    fn registration_requires_exactly_the_declared_consumers() {
        let mut registry = OutboxRegistry::new();
        let mismatch = registry.register::<TestMessage>(&[("alpha", handler())]);
        assert!(matches!(
            mismatch,
            Err(RegistryError::ConsumerSetMismatch(..))
        ));
        let ok = registry.register::<TestMessage>(&[("alpha", handler()), ("beta", handler())]);
        assert!(ok.is_ok());
        // The same contract may not be registered twice.
        let duplicate =
            registry.register::<TestMessage>(&[("alpha", handler()), ("beta", handler())]);
        assert!(matches!(
            duplicate,
            Err(RegistryError::DuplicateContract(_, _))
        ));
    }

    #[test]
    fn handlers_are_keyed_by_type_and_version() {
        let mut registry = OutboxRegistry::new();
        registry
            .register::<TestMessage>(&[("alpha", handler()), ("beta", handler())])
            .expect("registers");
        assert!(registry.knows("test.message", 1));
        assert!(!registry.knows("test.message", 2));
        assert!(!registry.knows("test.other", 1));
        assert!(registry.handler("test.message", 1, "alpha").is_some());
        assert!(registry.handler("test.message", 1, "gamma").is_none());
    }

    #[test]
    fn retry_backoff_doubles_and_caps_like_the_job_queue() {
        assert_eq!(retry_backoff(1), Duration::from_secs(30));
        assert_eq!(retry_backoff(2), Duration::from_secs(60));
        assert_eq!(retry_backoff(5), Duration::from_secs(480));
        assert_eq!(retry_backoff(6), Duration::from_secs(900));
        assert_eq!(retry_backoff(50), Duration::from_secs(900));
    }
}
