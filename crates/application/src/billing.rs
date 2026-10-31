//! Application-owned billing boundary; no provider SDK/wire types.
use chrono::{DateTime, Utc};
use deepref_domain::{
    WorkspaceId,
    billing::{Entitlements, SubscriptionState},
};
use serde::{Deserialize, Serialize};
use std::{future::Future, pin::Pin};
use thiserror::Error;
use uuid::Uuid;

pub type BillingFuture<'a, T> = Pin<Box<dyn Future<Output = Result<T, BillingError>> + Send + 'a>>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Offering {
    Primary,
    Secondary,
}

/// Opaque adapter/persistence reference. Never enters domain, public API or logs.
#[derive(Clone, PartialEq, Eq)]
pub struct CustomerReference(String);
impl CustomerReference {
    pub fn new(value: String) -> Result<Self, BillingError> {
        if value.is_empty()
            || value.len() > 128
            || !value
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'_')
        {
            return Err(BillingError::Contract);
        }
        Ok(Self(value))
    }
    pub fn expose(&self) -> &str {
        &self.0
    }
}

/// Bearer capability: intentionally has no Debug/Serialize implementation.
pub struct HostedLink(String);
impl HostedLink {
    /// Only adapters that validated scheme, host and mode may construct this.
    pub fn validated(value: String) -> Self {
        Self(value)
    }
    pub fn expose(&self) -> &str {
        &self.0
    }
}

/// Authorized, deliberately collected billing contact. No member roster.
pub struct BillingContact {
    email: String,
    name: String,
}
impl BillingContact {
    pub fn parse(email: String, name: String) -> Result<Self, BillingError> {
        if email.len() > 320
            || !email.contains('@')
            || email.trim() != email
            || email.chars().any(char::is_control)
            || name.trim().is_empty()
            || name.len() > 200
            || name.chars().any(char::is_control)
        {
            return Err(BillingError::Refused);
        }
        Ok(Self { email, name })
    }
    pub fn email(&self) -> &str {
        &self.email
    }
    pub fn name(&self) -> &str {
        &self.name
    }
}

#[derive(Debug, Error, Clone, Copy, PartialEq, Eq)]
pub enum BillingError {
    #[error("billing provider unavailable")]
    Unavailable,
    #[error("billing operation requires reconciliation")]
    Ambiguous,
    #[error("billing provider contract rejected")]
    Contract,
    #[error("billing request refused")]
    Refused,
    #[error("billing offering unavailable")]
    OfferingUnavailable,
}

pub struct CustomerIntent<'a> {
    pub workspace: WorkspaceId,
    pub operation: Uuid,
    pub contact: &'a BillingContact,
}

pub enum CustomerResolution {
    Found(CustomerReference),
    Unresolved,
}

/// Restricted adapter/persistence references; absent from domain and public views.
pub struct SubscriptionReference {
    subscription: CustomerReference,
    product: CustomerReference,
}
impl SubscriptionReference {
    pub fn new(subscription: String, product: String) -> Result<Self, BillingError> {
        Ok(Self {
            subscription: CustomerReference::new(subscription)?,
            product: CustomerReference::new(product)?,
        })
    }
    pub fn expose(&self) -> (&str, &str) {
        (self.subscription.expose(), self.product.expose())
    }
}
pub struct SubscriptionObservation {
    pub reference: Option<SubscriptionReference>,
    pub state: SubscriptionState,
    pub entitlements: Entitlements,
    pub observed_at: DateTime<Utc>,
    /// Opaque identity of current adverse commercial evidence; no payment IDs leak.
    pub review: Option<Uuid>,
}

pub struct VerifiedBillingWebhook {
    pub identity: String,
    pub event_type: String,
    pub customer: Option<CustomerReference>,
    /// Parsed only after raw-byte authentication; existing inbox owns retention.
    pub payload: serde_json::Value,
    pub adverse: bool,
}

pub struct WebhookHeaders<'a> {
    pub id: &'a str,
    pub timestamp: &'a str,
    pub signatures: &'a str,
}

/// Result of asking the provider to end a detached account's subscription.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SubscriptionCancellation {
    /// The provider reports the subscription cancelled, expired or failed.
    Cancelled,
    /// The provider refused, or the subscription does not belong to the
    /// customer: an operator has to finish closure by hand.
    ManualRequired,
}

pub trait BillingProvider: Send + Sync {
    /// Stable provider/business/mode namespace, supplied by configured adapter.
    fn namespace(&self) -> &str;
    fn create_customer<'a>(
        &'a self,
        intent: CustomerIntent<'a>,
    ) -> BillingFuture<'a, CustomerReference>;
    fn resolve_customer<'a>(&'a self, operation: Uuid) -> BillingFuture<'a, CustomerResolution>;
    fn checkout<'a>(
        &'a self,
        customer: &'a CustomerReference,
        offering: Offering,
        return_path: &'a str,
    ) -> BillingFuture<'a, HostedLink>;
    fn portal<'a>(
        &'a self,
        customer: &'a CustomerReference,
        return_path: &'a str,
    ) -> BillingFuture<'a, HostedLink>;
    fn observe<'a>(
        &'a self,
        customer: &'a CustomerReference,
    ) -> BillingFuture<'a, SubscriptionObservation>;
    fn verify(
        &self,
        body: &[u8],
        headers: WebhookHeaders<'_>,
        now: DateTime<Utc>,
    ) -> Result<VerifiedBillingWebhook, BillingError>;
    /// End the subscription captured when a workspace was deleted. Must be
    /// safe to repeat: an already-ended subscription is `Cancelled`. Transport
    /// failures return an error and are retried later.
    fn cancel_subscription<'a>(
        &'a self,
        customer: &'a CustomerReference,
        subscription: &'a CustomerReference,
    ) -> BillingFuture<'a, SubscriptionCancellation>;
}

/// These signals require relevant commercial evidence before a new grant or
/// grace extension. They are not an unconditional financial revocation rule.
pub fn commercial_review_required(event_type: &str) -> bool {
    event_type.starts_with("refund.")
        || event_type.starts_with("dispute.")
        || event_type.starts_with("credit.")
}

use std::collections::BTreeMap;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DodoUsageEvent {
    pub customer_id: String,
    pub event_id: String,
    pub event_name: String,
    pub timestamp: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<BTreeMap<String, serde_json::Value>>,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct DodoEventRecord {
    pub business_id: String,
    pub customer_id: String,
    pub event_id: String,
    pub event_name: String,
    pub timestamp: String,
    #[serde(default)]
    pub metadata: Option<BTreeMap<String, serde_json::Value>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DodoIngestOutcome {
    Accepted,
    Conflict,
    Refused(String),
    Ambiguous,
}

impl DodoEventRecord {
    /// Compares the complete canonical wire payload against the expected event.
    /// Returns true if and only if business_id, customer_id, event_id, event_name,
    /// timestamp (the exact parsed instant), and all metadata keys/values
    /// are identical.
    pub fn matches_canonical(&self, expected: &DodoUsageEvent, expected_business_id: &str) -> bool {
        if self.business_id != expected_business_id {
            return false;
        }
        if self.customer_id != expected.customer_id {
            return false;
        }
        if self.event_id != expected.event_id {
            return false;
        }
        if self.event_name != expected.event_name {
            return false;
        }
        match (
            DateTime::parse_from_rfc3339(&self.timestamp),
            DateTime::parse_from_rfc3339(&expected.timestamp),
        ) {
            (Ok(rec_ts), Ok(exp_ts)) => {
                if rec_ts != exp_ts {
                    return false;
                }
            }
            _ => return false,
        }
        let empty_map = BTreeMap::new();
        let self_meta = self.metadata.as_ref().unwrap_or(&empty_map);
        let exp_meta = expected.metadata.as_ref().unwrap_or(&empty_map);
        self_meta == exp_meta
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UsageEnvironment {
    Test,
    Live,
}

impl UsageEnvironment {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Test => "test_mode",
            Self::Live => "live_mode",
        }
    }
}

pub trait BillingUsageProvider: Send + Sync {
    fn usage_environment(&self) -> UsageEnvironment;
    fn usage_namespace(&self) -> &str;
    fn business_id(&self) -> &str;
    fn ingest_usage_event<'a>(
        &'a self,
        event: DodoUsageEvent,
    ) -> BillingFuture<'a, DodoIngestOutcome>;
    fn lookup_usage_event<'a>(
        &'a self,
        event_id: &'a str,
    ) -> BillingFuture<'a, Option<DodoEventRecord>>;
}
