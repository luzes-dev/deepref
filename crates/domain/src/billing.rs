//! Local billing policy. Provider identifiers and network calls do not belong here.
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Feature {
    AiFirst,
    Automation,
    ApiAccess,
    AdvancedExports,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Entitlements {
    pub features: Vec<Feature>,
    pub projects: u32,
    pub members: u32,
    /// Commercial allowance, never a replacement for PostgreSQL hard caps.
    pub monthly_ai_units: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BillingClassification {
    KnownFree,
    Paid,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SubscriptionState {
    Pending,
    Active,
    OnHold,
    Paused,
    Cancelled,
    Failed,
    Expired,
    PastDue,
    Unknown,
}

/// A local projection has explicit evidence. Missing provider subscriptions do
/// not construct Free: only the deliberate local enrollment transaction does.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum BillingProjection {
    Unknown,
    Free {
        enrolled_at: DateTime<Utc>,
        entitlements: Entitlements,
    },
    Subscription {
        state: SubscriptionState,
        entitlements: Entitlements,
        observed_at: DateTime<Utc>,
        /// Explicit bounded outage grace; never extended by a failed fetch.
        valid_until: DateTime<Utc>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EffectiveBilling {
    pub classification: BillingClassification,
    pub entitlements: Entitlements,
}

impl BillingProjection {
    pub fn effective(&self, now: DateTime<Utc>) -> EffectiveBilling {
        let (classification, entitlements) = match self {
            Self::Unknown => (BillingClassification::Unknown, Entitlements::default()),
            Self::Free { entitlements, .. } => {
                (BillingClassification::KnownFree, entitlements.clone())
            }
            Self::Subscription {
                state,
                entitlements,
                observed_at,
                valid_until,
            } if *observed_at <= now && now <= *valid_until => {
                // A negative authoritative observation revokes immediately.
                let grants = if *state == SubscriptionState::Active {
                    entitlements.clone()
                } else {
                    Entitlements::default()
                };
                (BillingClassification::Paid, grants)
            }
            Self::Subscription { .. } => (BillingClassification::Unknown, Entitlements::default()),
        };
        EffectiveBilling {
            classification,
            entitlements,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration;
    #[test]
    fn missing_or_expired_evidence_is_never_free() {
        let now = Utc::now();
        assert_eq!(
            BillingProjection::Unknown.effective(now).classification,
            BillingClassification::Unknown
        );
        let p = BillingProjection::Subscription {
            state: SubscriptionState::Active,
            entitlements: Entitlements {
                features: vec![Feature::AiFirst],
                projects: 4,
                ..Default::default()
            },
            observed_at: now,
            valid_until: now + Duration::minutes(30),
        };
        assert!(
            p.effective(now)
                .entitlements
                .features
                .contains(&Feature::AiFirst)
        );
        assert_eq!(
            p.effective(now + Duration::minutes(31)).classification,
            BillingClassification::Unknown
        );
        assert_eq!(
            p.effective(now + Duration::minutes(31)).entitlements,
            Entitlements::default()
        );
    }
    #[test]
    fn only_deliberate_enrollment_is_known_free_and_downgrades_revoke() {
        let now = Utc::now();
        let e = Entitlements {
            features: vec![Feature::Automation],
            projects: 2,
            ..Default::default()
        };
        assert_eq!(
            BillingProjection::Free {
                enrolled_at: now,
                entitlements: e.clone()
            }
            .effective(now)
            .classification,
            BillingClassification::KnownFree
        );
        for state in [
            SubscriptionState::Pending,
            SubscriptionState::OnHold,
            SubscriptionState::Paused,
            SubscriptionState::Cancelled,
            SubscriptionState::Failed,
            SubscriptionState::Expired,
            SubscriptionState::PastDue,
            SubscriptionState::Unknown,
        ] {
            let p = BillingProjection::Subscription {
                state,
                entitlements: e.clone(),
                observed_at: now,
                valid_until: now + Duration::minutes(30),
            };
            assert_eq!(p.effective(now).entitlements, Entitlements::default());
        }
    }
}
