//! Who is making a request: DeepRef users and the principals built from them.
//!
//! DeepRef owns the user ID. The identity provider's ID is an attribute of a
//! user, never a key.

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::{ApiKeyId, UserId, WorkspaceId};

const MAX_EXTERNAL_USER_ID_BYTES: usize = 128;

/// The identity provider's own identifier for a person (a WorkOS user ID).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ExternalUserId(String);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum ExternalUserIdError {
    #[error("external user id must not be blank")]
    Blank,
    #[error(
        "external user id must be at most {MAX_EXTERNAL_USER_ID_BYTES} bytes of printable ASCII"
    )]
    Invalid,
}

impl ExternalUserId {
    pub fn parse(value: impl Into<String>) -> Result<Self, ExternalUserIdError> {
        let value = value.into();
        if value.trim().is_empty() {
            return Err(ExternalUserIdError::Blank);
        }
        if value.len() > MAX_EXTERNAL_USER_ID_BYTES || !value.bytes().all(|b| b.is_ascii_graphic())
        {
            return Err(ExternalUserIdError::Invalid);
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UserStatus {
    Active,
    Deactivated,
}

impl UserStatus {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Deactivated => "deactivated",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "active" => Some(Self::Active),
            "deactivated" => Some(Self::Deactivated),
            _ => None,
        }
    }
}

/// A person signed in through a browser session. Workspace membership is not
/// part of it: authorization looks that up per request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct UserPrincipal {
    pub user_id: UserId,
}

/// A machine credential. It carries no human identity. Live scopes and
/// creator membership are re-read by the central PDP for each decision.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ServicePrincipal {
    pub key_id: ApiKeyId,
    pub workspace_id: WorkspaceId,
}

/// Work DeepRef starts itself, such as applying a provider webhook. Declared
/// for later handoffs; no request produces it yet.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub struct SystemPrincipal;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Principal {
    User(UserPrincipal),
    Service(ServicePrincipal),
    System(SystemPrincipal),
}

impl Principal {
    pub const fn user(user_id: UserId) -> Self {
        Self::User(UserPrincipal { user_id })
    }

    /// Work DeepRef starts itself. No request produces it.
    pub const fn system() -> Self {
        Self::System(SystemPrincipal)
    }

    /// A verified machine credential; never a human session.
    pub const fn service(key_id: ApiKeyId, workspace_id: WorkspaceId) -> Self {
        Self::Service(ServicePrincipal {
            key_id,
            workspace_id,
        })
    }

    pub const fn user_id(&self) -> Option<UserId> {
        match self {
            Self::User(user) => Some(user.user_id),
            Self::Service(_) | Self::System(_) => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use uuid::Uuid;

    use super::*;

    #[test]
    fn external_user_id_rejects_blank_and_unprintable_values() {
        assert!(ExternalUserId::parse("user_01H").is_ok());
        for bad in ["", "  ", "user 01", "user\n01", "usér"] {
            assert!(ExternalUserId::parse(bad).is_err(), "{bad:?}");
        }
        assert!(ExternalUserId::parse("u".repeat(129)).is_err());
    }

    #[test]
    fn status_round_trips() {
        for status in [UserStatus::Active, UserStatus::Deactivated] {
            assert_eq!(UserStatus::parse(status.as_str()), Some(status));
        }
        assert_eq!(UserStatus::parse("deleted"), None);
    }

    #[test]
    fn only_user_principals_carry_a_user_id() {
        let id = UserId::new(Uuid::from_u128(7));
        assert_eq!(Principal::user(id).user_id(), Some(id));
        assert_eq!(Principal::System(SystemPrincipal).user_id(), None);
        assert_eq!(
            Principal::service(
                crate::ApiKeyId::new(Uuid::new_v4()),
                crate::WorkspaceId::new(Uuid::new_v4())
            )
            .user_id(),
            None
        );
    }
}
