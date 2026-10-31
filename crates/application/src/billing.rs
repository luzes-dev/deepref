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
}

/// These signals require relevant commercial evidence before a new grant or
/// grace extension. They are not an unconditional financial revocation rule.
pub fn commercial_review_required(event_type: &str) -> bool {
    event_type.starts_with("refund.")
        || event_type.starts_with("dispute.")
        || event_type.starts_with("credit.")
}
