//! The organization-directory port and the pure rules around workspaces.
//!
//! WorkOS is authoritative for external authentication and organization
//! provisioning; DeepRef PostgreSQL is authoritative for runtime access
//! decisions. Nothing in a normal request calls the provider: reconciliation
//! pushes provider state into local projections through this port, and every
//! provider DTO stops in the adapter that implements it.

use std::{future::Future, pin::Pin};

use deepref_domain::{ExternalOrganizationId, ExternalUserId, UserId, WorkspaceId, WorkspaceRole};
use thiserror::Error;
use uuid::Uuid;

pub type DirectoryFuture<'a, T> =
    Pin<Box<dyn Future<Output = Result<T, DirectoryError>> + Send + 'a>>;

/// Failures an organization-directory adapter reports. None carries provider
/// payloads or identifiers beyond the object IDs themselves.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum DirectoryError {
    #[error("the organization directory is unavailable")]
    Unavailable,
    #[error("the organization directory rejected the request")]
    Rejected,
    #[error("organization directory operation failed: {0}")]
    Internal(String),
}

/// One WorkOS Organization as DeepRef reads it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirectoryOrganization {
    pub external_organization_id: ExternalOrganizationId,
    pub name: String,
}

/// The provider's view of a membership's status. `Pending` is an invitation at
/// the provider: it grants nothing locally until it becomes `Active`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DirectoryMembershipStatus {
    Active,
    Inactive,
    Pending,
}

/// One WorkOS organization membership as DeepRef reads it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirectoryMembership {
    pub external_membership_id: String,
    pub external_user_id: ExternalUserId,
    /// The provider's role slug. `None` or an unmapped slug grants no role.
    pub role_slug: Option<String>,
    pub status: DirectoryMembershipStatus,
}

/// Listing Organizations and their memberships. Reconciliation is the only
/// caller; normal project requests never reach the provider.
pub trait OrganizationDirectory: Send + Sync {
    fn list_organizations<'a>(&'a self) -> DirectoryFuture<'a, Vec<DirectoryOrganization>>;

    fn list_memberships<'a>(
        &'a self,
        organization: &'a ExternalOrganizationId,
    ) -> DirectoryFuture<'a, Vec<DirectoryMembership>>;
}

/// Maps a provider role slug to a workspace role. Unknown slugs map to
/// `None`: an unknown role is a reconciliation finding, never a guess.
pub fn map_directory_role(role_slug: Option<&str>) -> Option<WorkspaceRole> {
    match role_slug? {
        "owner" => Some(WorkspaceRole::Owner),
        "admin" => Some(WorkspaceRole::Admin),
        "member" => Some(WorkspaceRole::Member),
        _ => None,
    }
}

/// What reconciliation found that it did not resolve silently. Findings are
/// reported to the operator, never written into audit metadata: they carry no
/// free text and no caller-supplied body.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum ReconciliationFinding {
    /// A provider membership names a person DeepRef has never provisioned.
    /// No membership row can reference a user that does not exist.
    ProviderMemberWithoutLocalUser { external_user_id: ExternalUserId },
    /// The provider assigned a role slug that maps to no workspace role
    /// (`None` is a membership with no role at all).
    ProviderRoleUnknown {
        external_user_id: ExternalUserId,
        role_slug: Option<String>,
    },
    /// The provider membership is still pending; it grants nothing locally.
    ProviderMembershipPending { external_user_id: ExternalUserId },
    /// A DeepRef-only grant the provider does not list. The grant is
    /// preserved, not revoked.
    LocalGrantWithoutProviderMembership { user_id: UserId },
    /// A DeepRef-only grant whose role disagrees with the provider. The grant
    /// is preserved and the disagreement is surfaced.
    LocalGrantRoleConflict {
        user_id: UserId,
        local_role: WorkspaceRole,
        provider_role: WorkspaceRole,
    },
    /// Provider state (not a voluntary DeepRef change) left a workspace with
    /// no active owner. Ownership must be transferred deliberately.
    WorkspaceWithoutOwner { workspace_id: WorkspaceId },
}

/// Normalizes an invitation address: trimmed and lowercase, with an `@` that
/// is neither first nor last and no whitespace. Both the invitation and the
/// signed-in user's email pass through this, so matching is exact. Returns
/// `None` for anything that cannot be an address.
pub fn normalize_invitation_email(raw: &str) -> Option<String> {
    let email = raw.trim().to_lowercase();
    let (local, domain) = email.split_once('@')?;
    let valid = !local.is_empty()
        && !domain.is_empty()
        && !domain.contains('@')
        && email.len() <= 320
        && !email.chars().any(char::is_whitespace);
    valid.then_some(email)
}

/// The display name of the personal workspace first sign-in creates: the
/// person it belongs to, never an email-as-key. Falls back to the email's
/// local part so the name is never blank.
pub fn personal_workspace_name(display_name: Option<&str>, email: &str) -> String {
    let person = display_name
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| {
            let local = email.split_once('@').map_or(email, |(local, _)| local);
            let local = local.trim();
            if local.is_empty() { "Personal" } else { local }
        });
    let mut name = format!("{person}'s workspace");
    if name.chars().count() > 200 {
        name = name.chars().take(200).collect();
    }
    name
}

/// A stable, URL-safe slug for a workspace: the slugified name plus a short
/// suffix of the workspace ID, so renaming never moves a URL and two
/// workspaces with the same name never collide.
pub fn workspace_slug(name: &str, workspace_id: Uuid) -> String {
    let mut base = String::with_capacity(name.len());
    let mut pending_dash = false;
    for character in name.chars() {
        if character.is_ascii_alphanumeric() {
            if pending_dash && !base.is_empty() {
                base.push('-');
            }
            pending_dash = false;
            base.push(character.to_ascii_lowercase());
        } else {
            pending_dash = true;
        }
    }
    while base.len() > 40 {
        base.pop();
    }
    while base.ends_with('-') {
        base.pop();
    }
    if base.is_empty() {
        base.push_str("workspace");
    }
    let suffix: String = workspace_id.simple().to_string().chars().take(8).collect();
    let slug = format!("{base}-{suffix}");
    if slug.chars().count() > 80 {
        slug.chars().take(80).collect()
    } else {
        slug
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn directory_roles_map_only_known_slugs() {
        assert_eq!(
            map_directory_role(Some("owner")),
            Some(WorkspaceRole::Owner)
        );
        assert_eq!(
            map_directory_role(Some("admin")),
            Some(WorkspaceRole::Admin)
        );
        assert_eq!(
            map_directory_role(Some("member")),
            Some(WorkspaceRole::Member)
        );
        assert_eq!(map_directory_role(Some("janitor")), None);
        assert_eq!(map_directory_role(None), None);
    }

    #[test]
    fn invitation_emails_are_normalized_or_refused() {
        assert_eq!(
            normalize_invitation_email("  Ada@Example.ORG "),
            Some("ada@example.org".to_owned())
        );
        for bad in [
            "",
            "ada",
            "@example.org",
            "ada@",
            "a b@example.org",
            "a@b@c",
        ] {
            assert_eq!(normalize_invitation_email(bad), None, "{bad}");
        }
    }

    #[test]
    fn personal_workspace_names_the_person() {
        assert_eq!(
            personal_workspace_name(Some("Ada Lovelace"), "ada@example.org"),
            "Ada Lovelace's workspace"
        );
        assert_eq!(
            personal_workspace_name(None, "ada@example.org"),
            "ada's workspace"
        );
        assert_eq!(
            personal_workspace_name(Some("   "), "  @example.org"),
            "Personal's workspace"
        );
        let long = "x".repeat(300);
        assert_eq!(
            personal_workspace_name(Some(&long), "a@b.org")
                .chars()
                .count(),
            200
        );
    }

    #[test]
    fn workspace_slugs_are_stable_and_distinct() {
        let id = Uuid::from_u128(0x1234_5678);
        assert_eq!(
            workspace_slug("  Ada's Workspace!  ", id),
            "ada-s-workspace-00000000"
        );
        assert_ne!(
            workspace_slug("Same name", id),
            workspace_slug("Same name", Uuid::new_v4())
        );
        assert_eq!(
            workspace_slug("###", id),
            format!("workspace-{}", short(id))
        );
        let long_slug = workspace_slug(&"a".repeat(300), id);
        assert!(long_slug.chars().count() <= 80, "{long_slug}");
        for candidate in [
            workspace_slug("Ada", id),
            long_slug,
            workspace_slug("###", id),
        ] {
            assert!(
                candidate
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-'),
                "{candidate}"
            );
        }
    }

    fn short(id: Uuid) -> String {
        id.simple().to_string().chars().take(8).collect()
    }
}
