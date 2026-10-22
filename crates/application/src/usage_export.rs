//! Provider-neutral AI usage fact export contract (handoff 09 slice B).
//!
//! PostgreSQL says what is true about usage: the immutable usage fact in the AI
//! usage ledger is the commercial event identity, and DeepRef exports it as one
//! outbox event per fact. This module is pure contract — the message shape, the
//! delivery port and the outcome vocabulary. It names no billing vendor: the
//! adapter behind [`UsageExportSink`] is chosen with handoff 08.
//!
//! Payload minimization is part of the contract. An exported event carries
//! facts only (identity, attribution, counters, price and period); it never
//! carries email addresses, names, prompt text or secrets.

use std::sync::Mutex;

use chrono::{DateTime, NaiveDate, Utc};
use futures::future::BoxFuture;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::outbox::OutboxMessage;

/// The single consumer of [`AiUsageFact`]: the billing usage export.
pub const USAGE_EXPORT_CONSUMER: &str = "billing_usage_export";

/// Delivery attempts one usage fact event budgets before an operator reviews
/// it. Usage facts are never dropped while a provider is merely unavailable:
/// retryable and ambiguous outcomes keep retrying on the outbox's capped
/// backoff for roughly ten days before the budget is spent. A terminal sink
/// outcome quarantines the recipient immediately, whatever budget is left.
pub const USAGE_EXPORT_MAX_ATTEMPTS: i32 = 1000;

/// Attribution of a usage fact: a human user or a service principal. Only the
/// kind crosses the export boundary; the principal's identity never does.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UsagePrincipalKind {
    User,
    Service,
}

impl UsagePrincipalKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::User => "user",
            Self::Service => "service",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "user" => Some(Self::User),
            "service" => Some(Self::Service),
            _ => None,
        }
    }
}

/// One immutable usage fact as a commercial export event (contract
/// `ai_usage.fact` v1).
///
/// `fact_id` is the immutable identity of the local usage fact (the ledger row
/// id, or a correction record id). It is the outbox event id and the provider
/// idempotency key; it is never reissued or rewritten. A correction record
/// exports as its own fact and references the original in `correction_of`; the
/// original event is never edited.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AiUsageFact {
    /// Immutable identity of this usage fact; also its event id.
    pub fact_id: Uuid,
    /// External customer identity.
    pub workspace_id: Uuid,
    pub project_id: Option<Uuid>,
    /// Which kind of principal incurred the usage, never which principal.
    pub principal_kind: Option<UsagePrincipalKind>,
    pub provider: String,
    pub model: String,
    pub input_tokens: Option<i64>,
    pub output_tokens: Option<i64>,
    pub reasoning_tokens: Option<i64>,
    /// Provider cost in micro-dollars; commercial units stay separate.
    pub provider_cost_micros: i64,
    pub commercial_units: Option<i64>,
    /// Price-table version that priced this fact. Null only for facts settled
    /// before price versions existed.
    pub price_version: Option<String>,
    /// The one UTC month this fact is charged to.
    pub period_start: NaiveDate,
    /// When the immutable fact was committed.
    pub settled_at: DateTime<Utc>,
    /// The fact this one corrects, for correction records only.
    pub correction_of: Option<Uuid>,
}

impl OutboxMessage for AiUsageFact {
    const TYPE: &'static str = "ai_usage.fact";
    const VERSION: u32 = 1;
    const CONSUMERS: &'static [&'static str] = &[USAGE_EXPORT_CONSUMER];
}

/// What a sink reports after attempting to deliver one usage fact event.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UsageExportOutcome {
    /// The provider accepted the event.
    Accepted,
    /// The provider already has this event id; treat it as delivered.
    Duplicate,
    /// The provider is unavailable or refused the request temporarily. Retry
    /// with the same event id.
    RetryableFailure(String),
    /// The event can never be accepted; quarantine it for review instead of
    /// spending the attempt budget.
    PermanentFailure(String),
    /// The client timed out, so the provider may have accepted the event. This
    /// is **not** success: the event is retried with the same event id and is
    /// marked delivered only after [`UsageExportOutcome::Accepted`] or
    /// [`UsageExportOutcome::Duplicate`].
    Ambiguous(String),
}

impl UsageExportOutcome {
    /// The complete label vocabulary of export counters and traces. Counter
    /// dimensions are fixed: there is no free-form label where a workspace id
    /// or a cost could become a metric label.
    pub const fn label(&self) -> &'static str {
        match self {
            Self::Accepted => "accepted",
            Self::Duplicate => "duplicate",
            Self::RetryableFailure(_) => "retryable_failure",
            Self::PermanentFailure(_) => "permanent_failure",
            Self::Ambiguous(_) => "ambiguous",
        }
    }

    /// True only for [`UsageExportOutcome::Accepted`] and
    /// [`UsageExportOutcome::Duplicate`].
    pub fn is_delivered(&self) -> bool {
        matches!(self, Self::Accepted | Self::Duplicate)
    }

    fn message(&self) -> &str {
        match self {
            Self::Accepted | Self::Duplicate => "",
            Self::RetryableFailure(message)
            | Self::PermanentFailure(message)
            | Self::Ambiguous(message) => message,
        }
    }
}

/// The provider-neutral delivery port for usage fact events.
///
/// A real adapter must verify the provider's event-ingestion idempotency and
/// deduplication semantics before retries are enabled (spec "Idempotency"):
/// every attempt after an [`UsageExportOutcome::Ambiguous`] outcome re-sends
/// the same event id, so a provider that does not deduplicate by that key can
/// double-charge. DeepRef always sends `fact_id` as the idempotency key and
/// never aggregates facts into fewer events until the provider's ingestion,
/// deduplication, correction and billing-period semantics are verified.
pub trait UsageExportSink: Send + Sync {
    /// Deliver exactly one fact event. Implementations send `event.fact_id` as
    /// the provider idempotency key.
    fn deliver<'a>(&'a self, event: &'a AiUsageFact) -> BoxFuture<'a, UsageExportOutcome>;
}

/// The default sink while no billing provider is configured. It never claims
/// success and never drops an event as delivered: every event stays pending
/// and retries through the outbox's backoff until an adapter accepts it.
#[derive(Debug, Clone, Copy, Default)]
pub struct DisabledUsageExportSink;

impl UsageExportSink for DisabledUsageExportSink {
    fn deliver<'a>(&'a self, _event: &'a AiUsageFact) -> BoxFuture<'a, UsageExportOutcome> {
        Box::pin(async {
            UsageExportOutcome::RetryableFailure(
                "usage export has no billing provider adapter configured".to_owned(),
            )
        })
    }
}

/// Test double: scripted outcomes in order and a record of received event ids.
/// When the script is exhausted the last outcome repeats, so a test states only
/// the transitions it cares about.
#[derive(Debug, Default)]
pub struct ScriptedUsageExportSink {
    script: Mutex<Vec<UsageExportOutcome>>,
    deliveries: Mutex<Vec<AiUsageFact>>,
}

impl ScriptedUsageExportSink {
    pub fn new(script: Vec<UsageExportOutcome>) -> Self {
        Self {
            script: Mutex::new(script),
            deliveries: Mutex::new(Vec::new()),
        }
    }

    /// A poisoned lock still holds the delivery log; a test double recovers it
    /// instead of panicking.
    fn delivery_log(&self) -> std::sync::MutexGuard<'_, Vec<AiUsageFact>> {
        self.deliveries
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Every delivery attempt, in order, including duplicates and retries.
    pub fn deliveries(&self) -> Vec<AiUsageFact> {
        self.delivery_log().clone()
    }

    /// Every delivery attempt count.
    pub fn delivery_count(&self) -> usize {
        self.delivery_log().len()
    }

    /// Received fact ids deduplicated by id, in first-seen order: the fake
    /// counts each fact once no matter how often it is delivered.
    pub fn recorded_fact_ids(&self) -> Vec<Uuid> {
        let deliveries = self.delivery_log();
        let mut seen = Vec::new();
        for fact in deliveries.iter() {
            if !seen.contains(&fact.fact_id) {
                seen.push(fact.fact_id);
            }
        }
        seen
    }
}

impl UsageExportSink for ScriptedUsageExportSink {
    fn deliver<'a>(&'a self, event: &'a AiUsageFact) -> BoxFuture<'a, UsageExportOutcome> {
        Box::pin(async move {
            self.delivery_log().push(event.clone());
            let mut script = self
                .script
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if script.len() > 1 {
                script.remove(0)
            } else {
                script
                    .first()
                    .cloned()
                    .unwrap_or(UsageExportOutcome::PermanentFailure(
                        "scripted sink has no outcome left".to_owned(),
                    ))
            }
        })
    }
}

/// Export telemetry counters. The fields are the only dimensions: counters
/// carry no workspace, project, user or cost labels, and the outcome vocabulary
/// is the fixed [`UsageExportOutcome::label`] set.
#[derive(Debug, Default)]
pub struct UsageExportCounters {
    facts_emitted: std::sync::atomic::AtomicU64,
    facts_delivered: std::sync::atomic::AtomicU64,
    facts_duplicate: std::sync::atomic::AtomicU64,
    facts_ambiguous: std::sync::atomic::AtomicU64,
    facts_retryable_failures: std::sync::atomic::AtomicU64,
    facts_permanent_failures: std::sync::atomic::AtomicU64,
}

impl UsageExportCounters {
    /// Count one sink outcome.
    pub fn record(&self, outcome: &UsageExportOutcome) {
        use std::sync::atomic::Ordering::Relaxed;
        match outcome {
            UsageExportOutcome::Accepted => {
                self.facts_delivered.fetch_add(1, Relaxed);
            }
            UsageExportOutcome::Duplicate => {
                self.facts_duplicate.fetch_add(1, Relaxed);
            }
            UsageExportOutcome::RetryableFailure(_) => {
                self.facts_retryable_failures.fetch_add(1, Relaxed);
            }
            UsageExportOutcome::PermanentFailure(_) => {
                self.facts_permanent_failures.fetch_add(1, Relaxed);
            }
            UsageExportOutcome::Ambiguous(_) => {
                self.facts_ambiguous.fetch_add(1, Relaxed);
            }
        }
    }

    /// Count facts the replay emitted into the outbox.
    pub fn record_emitted(&self, count: u64) {
        self.facts_emitted
            .fetch_add(count, std::sync::atomic::Ordering::Relaxed);
    }

    pub fn snapshot(&self) -> UsageExportCountersSnapshot {
        use std::sync::atomic::Ordering::Relaxed;
        UsageExportCountersSnapshot {
            facts_emitted: self.facts_emitted.load(Relaxed),
            facts_delivered: self.facts_delivered.load(Relaxed),
            facts_duplicate: self.facts_duplicate.load(Relaxed),
            facts_ambiguous: self.facts_ambiguous.load(Relaxed),
            facts_retryable_failures: self.facts_retryable_failures.load(Relaxed),
            facts_permanent_failures: self.facts_permanent_failures.load(Relaxed),
        }
    }
}

/// Point-in-time counter values. Serialized for operator summaries and
/// structured logs; the field set is closed, so no identifier can become a
/// metric label.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct UsageExportCountersSnapshot {
    pub facts_emitted: u64,
    pub facts_delivered: u64,
    pub facts_duplicate: u64,
    pub facts_ambiguous: u64,
    pub facts_retryable_failures: u64,
    pub facts_permanent_failures: u64,
}

impl UsageExportOutcome {
    /// Operator-facing description used in retry and quarantine reasons.
    pub fn describe(&self) -> String {
        let message = self.message();
        if message.is_empty() {
            self.label().to_owned()
        } else {
            format!("{}: {}", self.label(), message)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fact() -> AiUsageFact {
        AiUsageFact {
            fact_id: Uuid::new_v4(),
            workspace_id: Uuid::new_v4(),
            project_id: Some(Uuid::new_v4()),
            principal_kind: Some(UsagePrincipalKind::Service),
            provider: "provider".to_owned(),
            model: "model".to_owned(),
            input_tokens: Some(10),
            output_tokens: Some(20),
            reasoning_tokens: None,
            provider_cost_micros: 30,
            commercial_units: None,
            price_version: Some("v1".to_owned()),
            period_start: NaiveDate::from_ymd_opt(2026, 10, 1).expect("date"),
            settled_at: Utc::now(),
            correction_of: None,
        }
    }

    #[test]
    fn the_contract_is_stable_and_carries_one_consumer() {
        assert_eq!(AiUsageFact::TYPE, "ai_usage.fact");
        assert_eq!(AiUsageFact::VERSION, 1);
        assert_eq!(AiUsageFact::CONSUMERS, &[USAGE_EXPORT_CONSUMER]);
    }

    #[test]
    fn counters_count_each_outcome_in_its_own_field() {
        let counters = UsageExportCounters::default();
        counters.record(&UsageExportOutcome::Accepted);
        counters.record(&UsageExportOutcome::Duplicate);
        counters.record(&UsageExportOutcome::Ambiguous("timeout".to_owned()));
        counters.record(&UsageExportOutcome::RetryableFailure("down".to_owned()));
        counters.record(&UsageExportOutcome::PermanentFailure("bad".to_owned()));
        counters.record_emitted(3);
        assert_eq!(
            counters.snapshot(),
            UsageExportCountersSnapshot {
                facts_emitted: 3,
                facts_delivered: 1,
                facts_duplicate: 1,
                facts_ambiguous: 1,
                facts_retryable_failures: 1,
                facts_permanent_failures: 1,
            }
        );
    }

    #[test]
    fn ambiguous_is_not_delivery() {
        assert!(!UsageExportOutcome::Ambiguous("timeout".to_owned()).is_delivered());
        assert!(!UsageExportOutcome::RetryableFailure("down".to_owned()).is_delivered());
        assert!(!UsageExportOutcome::PermanentFailure("bad".to_owned()).is_delivered());
        assert!(UsageExportOutcome::Accepted.is_delivered());
        assert!(UsageExportOutcome::Duplicate.is_delivered());
    }

    #[test]
    fn the_scripted_sink_repeats_its_last_outcome_and_dedups_by_fact_id() {
        let event = fact();
        let sink = ScriptedUsageExportSink::new(vec![
            UsageExportOutcome::Ambiguous("timeout".to_owned()),
            UsageExportOutcome::Accepted,
        ]);
        let first = futures::executor::block_on(sink.deliver(&event));
        let second = futures::executor::block_on(sink.deliver(&event));
        let third = futures::executor::block_on(sink.deliver(&event));
        assert!(!first.is_delivered());
        assert!(second.is_delivered());
        assert_eq!(third, UsageExportOutcome::Accepted);
        assert_eq!(sink.delivery_count(), 3);
        assert_eq!(sink.recorded_fact_ids(), vec![event.fact_id]);
    }

    #[test]
    fn the_disabled_sink_never_claims_success() {
        let event = fact();
        let outcome = futures::executor::block_on(DisabledUsageExportSink.deliver(&event));
        assert!(!outcome.is_delivered());
        assert_eq!(outcome.label(), "retryable_failure");
    }
}
