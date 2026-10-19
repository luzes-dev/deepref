//! The tenant: a workspace owns projects, and users reach them through
//! workspace memberships.
//!
//! Every project belongs to exactly one workspace. Roles here are account and
//! product roles; scientific roles and blinding policy are domain rules
//! elsewhere, never workspace roles.

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::{ProjectId, UserId};

const MAX_EXTERNAL_ORGANIZATION_ID_BYTES: usize = 128;

/// The identity provider's own identifier for an organization (a WorkOS
/// Organization ID). Like [`crate::ExternalUserId`] it is an attribute of a
/// workspace, never a key.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ExternalOrganizationId(String);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum ExternalOrganizationIdError {
    #[error("external organization id must not be blank")]
    Blank,
    #[error(
        "external organization id must be at most {MAX_EXTERNAL_ORGANIZATION_ID_BYTES} bytes of printable ASCII"
    )]
    Invalid,
}

impl ExternalOrganizationId {
    pub fn parse(value: impl Into<String>) -> Result<Self, ExternalOrganizationIdError> {
        let value = value.into();
        if value.trim().is_empty() {
            return Err(ExternalOrganizationIdError::Blank);
        }
        if value.len() > MAX_EXTERNAL_ORGANIZATION_ID_BYTES
            || !value.bytes().all(|b| b.is_ascii_graphic())
        {
            return Err(ExternalOrganizationIdError::Invalid);
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// What a workspace is for. The migration workspace holds the projects that
/// existed before workspaces and is never a user's own workspace.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkspaceKind {
    Personal,
    Organization,
    Migration,
}

impl WorkspaceKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Personal => "personal",
            Self::Organization => "organization",
            Self::Migration => "migration",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "personal" => Some(Self::Personal),
            "organization" => Some(Self::Organization),
            "migration" => Some(Self::Migration),
            _ => None,
        }
    }
}

/// Account and product roles inside a workspace. Keep the set small; do not
/// encode every capability as a role (permissions are decided centrally in
/// later handoffs).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkspaceRole {
    Owner,
    Admin,
    Member,
}

impl WorkspaceRole {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Owner => "owner",
            Self::Admin => "admin",
            Self::Member => "member",
        }
    }

    /// Orders roles by authority. A role may be granted only by someone whose
    /// own rank is at least as high.
    pub const fn rank(self) -> u8 {
        match self {
            Self::Owner => 3,
            Self::Admin => 2,
            Self::Member => 1,
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "owner" => Some(Self::Owner),
            "admin" => Some(Self::Admin),
            "member" => Some(Self::Member),
            _ => None,
        }
    }
}

/// Access narrower than workspace membership, for the few flows that need it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProjectRole {
    ProjectAdmin,
    Member,
    Viewer,
}

impl ProjectRole {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ProjectAdmin => "project_admin",
            Self::Member => "member",
            Self::Viewer => "viewer",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "project_admin" => Some(Self::ProjectAdmin),
            "member" => Some(Self::Member),
            "viewer" => Some(Self::Viewer),
            _ => None,
        }
    }
}

/// Where a grant came from. WorkOS-mirrored grants follow the provider;
/// DeepRef-only (`local`) grants are never touched by reconciliation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GrantSource {
    WorkosMirrored,
    Local,
}

impl GrantSource {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::WorkosMirrored => "workos_mirrored",
            Self::Local => "local",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "workos_mirrored" => Some(Self::WorkosMirrored),
            "local" => Some(Self::Local),
            _ => None,
        }
    }
}

/// A membership is deactivated, not deleted, when access ends through the
/// provider. Removal deletes the row and keeps its history in the audit.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MembershipStatus {
    Active,
    Deactivated,
}

impl MembershipStatus {
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

/// One user's place in one workspace. Membership is looked up per request; it
/// is never embedded in a session or identity token.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceMembership {
    pub workspace_id: crate::WorkspaceId,
    pub user_id: UserId,
    pub role: WorkspaceRole,
    pub status: MembershipStatus,
    pub grant_source: GrantSource,
}

impl WorkspaceMembership {
    /// Whether this membership may act in its workspace right now. An unknown
    /// role or status parses to no membership at all, so this stays closed.
    pub fn is_active(&self) -> bool {
        self.status == MembershipStatus::Active
    }
}

/// A grant of project access narrower than workspace membership.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectAccess {
    pub project_id: ProjectId,
    pub user_id: UserId,
    pub role: ProjectRole,
    pub grant_source: GrantSource,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn external_organization_id_rejects_blank_and_unprintable_values() {
        assert!(ExternalOrganizationId::parse("org_01H").is_ok());
        for bad in ["", "  ", "org 01", "org\n01", "orgánización"] {
            assert!(ExternalOrganizationId::parse(bad).is_err(), "{bad:?}");
        }
        assert!(ExternalOrganizationId::parse("o".repeat(129)).is_err());
    }

    #[test]
    fn role_and_status_values_round_trip() {
        for role in [
            WorkspaceRole::Owner,
            WorkspaceRole::Admin,
            WorkspaceRole::Member,
        ] {
            assert_eq!(WorkspaceRole::parse(role.as_str()), Some(role));
        }
        assert_eq!(WorkspaceRole::parse("root"), None);
        for role in [
            ProjectRole::ProjectAdmin,
            ProjectRole::Member,
            ProjectRole::Viewer,
        ] {
            assert_eq!(ProjectRole::parse(role.as_str()), Some(role));
        }
        assert_eq!(ProjectRole::parse("owner"), None);
        for source in [GrantSource::WorkosMirrored, GrantSource::Local] {
            assert_eq!(GrantSource::parse(source.as_str()), Some(source));
        }
        for status in [MembershipStatus::Active, MembershipStatus::Deactivated] {
            assert_eq!(MembershipStatus::parse(status.as_str()), Some(status));
        }
        for kind in [
            WorkspaceKind::Personal,
            WorkspaceKind::Organization,
            WorkspaceKind::Migration,
        ] {
            assert_eq!(WorkspaceKind::parse(kind.as_str()), Some(kind));
        }
    }
}
