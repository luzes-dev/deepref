//! Shared operational rate limits. Authorization and financial quotas remain
//! separate, authoritative PostgreSQL decisions.

use std::{future::Future, pin::Pin, time::Duration};

use deepref_domain::ApiKeyId;
use uuid::Uuid;

/// Construct subjects only after verifying a credential or resolving a private
/// workflow endpoint. Raw bearer tokens and arbitrary request values are not keys.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RateLimitSubject {
    ApiKey(ApiKeyId),
    WorkflowWebhook(Uuid),
    WorkflowEmail(Uuid),
}

/// Closed telemetry dimensions. Observations carry no subject IDs or payloads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RateLimitPurpose {
    ApiKey,
    WorkflowWebhook,
    WorkflowEmail,
}

impl RateLimitPurpose {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ApiKey => "api_key",
            Self::WorkflowWebhook => "workflow_webhook",
            Self::WorkflowEmail => "workflow_email",
        }
    }
}

impl RateLimitSubject {
    pub const fn kind(self) -> &'static str {
        self.purpose().as_str()
    }

    pub const fn purpose(self) -> RateLimitPurpose {
        match self {
            Self::ApiKey(_) => RateLimitPurpose::ApiKey,
            Self::WorkflowWebhook(_) => RateLimitPurpose::WorkflowWebhook,
            Self::WorkflowEmail(_) => RateLimitPurpose::WorkflowEmail,
        }
    }

    pub const fn id(self) -> Uuid {
        match self {
            Self::ApiKey(id) => id.as_uuid(),
            Self::WorkflowWebhook(id) | Self::WorkflowEmail(id) => id,
        }
    }
}

/// A bounded number of requests in a sixty-second window anchored at its first
/// request. A lower policy applies immediately; increases wait for a new window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RateLimitPolicy(u32);

impl RateLimitPolicy {
    pub const API_KEY: Self = Self(600);
    pub const WORKFLOW_HOOK: Self = Self(60);

    pub const fn per_minute(limit: u32) -> Option<Self> {
        if limit > 0 && limit <= 10_000 {
            Some(Self(limit))
        } else {
            None
        }
    }

    pub const fn limit(self) -> u32 {
        self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RateLimitDecision {
    Allowed,
    Limited,
}

/// A static error: driver messages can contain connection URLs or credentials.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("shared rate limiting is unavailable")]
pub struct RateLimitUnavailable;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RateLimitBackend {
    Valkey,
    Postgres,
}

/// The runtime installs an exporter bridge. Tests can inspect observations
/// without depending on a particular metrics SDK.
pub trait RateLimitObserver: Send + Sync {
    fn decision(
        &self,
        purpose: RateLimitPurpose,
        backend: RateLimitBackend,
        outcome: Result<RateLimitDecision, RateLimitUnavailable>,
        elapsed: Duration,
    );

    fn fallback(&self, purpose: RateLimitPurpose);
}

#[derive(Default)]
pub struct NoopRateLimitObserver;

impl RateLimitObserver for NoopRateLimitObserver {
    fn decision(
        &self,
        _: RateLimitPurpose,
        _: RateLimitBackend,
        _: Result<RateLimitDecision, RateLimitUnavailable>,
        _: Duration,
    ) {
    }
    fn fallback(&self, _: RateLimitPurpose) {}
}

pub type RateLimitFuture<'a> =
    Pin<Box<dyn Future<Output = Result<RateLimitDecision, RateLimitUnavailable>> + Send + 'a>>;

pub trait RateLimiter: Send + Sync {
    fn check(&self, subject: RateLimitSubject, policy: RateLimitPolicy) -> RateLimitFuture<'_>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn policies_are_bounded_before_an_adapter_receives_them() {
        assert!(RateLimitPolicy::per_minute(0).is_none());
        assert!(RateLimitPolicy::per_minute(10_001).is_none());
        assert_eq!(
            RateLimitPolicy::per_minute(1).map(RateLimitPolicy::limit),
            Some(1)
        );
    }
}
