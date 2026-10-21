//! Permissions and the pure rules that grant them (handoff 03b).
//!
//! `Permission` is a closed set. Handlers never compare role strings: they
//! name a permission and a resource, and the policy decision point answers
//! from the facts here. Everything below is a pure function of its inputs.
//!
//! # How roles grant permissions
//!
//! A principal's access to a resource is the union of two independent grants:
//!
//! * the **workspace role** of an *active* membership in the resource's
//!   workspace (`owner`, `admin`, `member`), which grants workspace
//!   permissions and, for every project in that workspace, project
//!   permissions;
//! * a **project grant** (`project_admin`, `member`, `viewer`) on that one
//!   project, which grants project permissions only. It is how a person
//!   without workspace membership (a guest) reaches one project.
//!
//! A project grant never narrows a workspace role: a workspace `member` who
//! also holds a `viewer` grant keeps the member permissions. Access narrower
//! than workspace membership means "less than the whole workspace", not "less
//! than the role".
//!
//! # Failing closed
//!
//! A deactivated user, a deactivated membership, an unknown role string, an
//! unknown status string, a missing membership and a resource that cannot be
//! resolved to a workspace all yield no permission at all.

use crate::{MembershipStatus, ProjectId, ProjectRole, UserStatus, WorkspaceId, WorkspaceRole};

/// What a permission is checked against.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ResourceKind {
    Workspace,
    Project,
}

/// The resource a permission is required on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Resource {
    Workspace(WorkspaceId),
    Project(ProjectId),
}

impl ResourceKind {
    const fn matches(self, other: Self) -> bool {
        matches!(
            (self, other),
            (Self::Workspace, Self::Workspace) | (Self::Project, Self::Project)
        )
    }
}

impl Resource {
    pub const fn kind(self) -> ResourceKind {
        match self {
            Self::Workspace(_) => ResourceKind::Workspace,
            Self::Project(_) => ResourceKind::Project,
        }
    }
}

/// The closed permission set. The wire form (`as_str`) is stable: it appears
/// in the coverage matrix and in audit metadata, never in a handler.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Permission {
    WorkspaceView,
    WorkspaceManage,
    WorkspaceBilling,
    WorkspaceMembersInvite,
    ProjectCreate,
    ProjectView,
    ProjectEdit,
    ProjectDelete,
    ScreeningPerform,
    ScreeningResolve,
    ExtractionPerform,
    AppraisalPerform,
    AutomationManage,
    AiUse,
    ApiKeyManage,
}

impl Permission {
    pub const ALL: [Self; 15] = [
        Self::WorkspaceView,
        Self::WorkspaceManage,
        Self::WorkspaceBilling,
        Self::WorkspaceMembersInvite,
        Self::ProjectCreate,
        Self::ProjectView,
        Self::ProjectEdit,
        Self::ProjectDelete,
        Self::ScreeningPerform,
        Self::ScreeningResolve,
        Self::ExtractionPerform,
        Self::AppraisalPerform,
        Self::AutomationManage,
        Self::AiUse,
        Self::ApiKeyManage,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::WorkspaceView => "workspace:view",
            Self::WorkspaceManage => "workspace:manage",
            Self::WorkspaceBilling => "workspace:billing",
            Self::WorkspaceMembersInvite => "workspace:members:invite",
            Self::ProjectCreate => "project:create",
            Self::ProjectView => "project:view",
            Self::ProjectEdit => "project:edit",
            Self::ProjectDelete => "project:delete",
            Self::ScreeningPerform => "screening:perform",
            Self::ScreeningResolve => "screening:resolve",
            Self::ExtractionPerform => "extraction:perform",
            Self::AppraisalPerform => "appraisal:perform",
            Self::AutomationManage => "automation:manage",
            Self::AiUse => "ai:use",
            Self::ApiKeyManage => "api_key:manage",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|permission| permission.as_str() == value)
    }

    /// The kind of resource this permission is checked on. `project:create`
    /// is checked on the workspace the project will live in.
    pub const fn scope(self) -> ResourceKind {
        match self {
            Self::WorkspaceView
            | Self::WorkspaceManage
            | Self::WorkspaceBilling
            | Self::WorkspaceMembersInvite
            | Self::ProjectCreate
            | Self::ApiKeyManage => ResourceKind::Workspace,
            Self::ProjectView
            | Self::ProjectEdit
            | Self::ProjectDelete
            | Self::ScreeningPerform
            | Self::ScreeningResolve
            | Self::ExtractionPerform
            | Self::AppraisalPerform
            | Self::AutomationManage
            | Self::AiUse => ResourceKind::Project,
        }
    }
}

/// Whether a workspace role carries `permission`. The workspace role covers
/// the workspace's own permissions and, for every project inside it, the
/// project permissions.
pub const fn workspace_role_grants(role: WorkspaceRole, permission: Permission) -> bool {
    use Permission::{
        AiUse, AppraisalPerform, ExtractionPerform, ProjectCreate, ProjectEdit, ProjectView,
        ScreeningPerform, ScreeningResolve, WorkspaceBilling, WorkspaceView,
    };
    match role {
        WorkspaceRole::Owner => true,
        WorkspaceRole::Admin => !matches!(permission, WorkspaceBilling),
        WorkspaceRole::Member => matches!(
            permission,
            WorkspaceView
                | ProjectCreate
                | ProjectView
                | ProjectEdit
                | ScreeningPerform
                | ScreeningResolve
                | ExtractionPerform
                | AppraisalPerform
                | AiUse
        ),
    }
}

/// Whether a project grant carries `permission`. Project grants never carry a
/// workspace permission.
pub const fn project_role_grants(role: ProjectRole, permission: Permission) -> bool {
    use Permission::{
        AiUse, AppraisalPerform, AutomationManage, ExtractionPerform, ProjectDelete, ProjectEdit,
        ProjectView, ScreeningPerform, ScreeningResolve,
    };
    match role {
        ProjectRole::ProjectAdmin => matches!(
            permission,
            ProjectView
                | ProjectEdit
                | ProjectDelete
                | ScreeningPerform
                | ScreeningResolve
                | ExtractionPerform
                | AppraisalPerform
                | AutomationManage
                | AiUse
        ),
        ProjectRole::Member => matches!(
            permission,
            ProjectView
                | ProjectEdit
                | ScreeningPerform
                | ScreeningResolve
                | ExtractionPerform
                | AppraisalPerform
                | AiUse
        ),
        ProjectRole::Viewer => matches!(permission, ProjectView),
    }
}

/// The stored facts about one principal and one resource's workspace, parsed
/// so that anything unrecognized is *absent*, never a guess.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Grants {
    user_active: bool,
    workspace_role: Option<WorkspaceRole>,
    project_role: Option<ProjectRole>,
}

impl Grants {
    /// Parses stored strings. `user_status` that is not exactly `active`
    /// denies everything. A membership counts only when both its role and its
    /// status parse and the status is `active`; a project grant counts only
    /// when its role parses.
    pub fn from_stored(
        user_status: Option<&str>,
        membership: Option<(&str, &str)>,
        project_role: Option<&str>,
    ) -> Self {
        Self {
            user_active: user_status.and_then(UserStatus::parse) == Some(UserStatus::Active),
            workspace_role: membership.and_then(|(role, status)| {
                (MembershipStatus::parse(status)? == MembershipStatus::Active)
                    .then(|| WorkspaceRole::parse(role))
                    .flatten()
            }),
            project_role: project_role.and_then(ProjectRole::parse),
        }
    }

    /// No access at all: the answer for a missing user, resource or grant.
    pub const fn none() -> Self {
        Self {
            user_active: false,
            workspace_role: None,
            project_role: None,
        }
    }

    pub const fn new(
        user_active: bool,
        workspace_role: Option<WorkspaceRole>,
        project_role: Option<ProjectRole>,
    ) -> Self {
        Self {
            user_active,
            workspace_role,
            project_role,
        }
    }

    /// Whether these grants permit `permission` on a resource of `kind`.
    /// A permission checked on the wrong kind of resource is denied.
    pub const fn permits(self, permission: Permission, kind: ResourceKind) -> bool {
        if !self.user_active || !kind.matches(permission.scope()) {
            return false;
        }
        if let Some(role) = self.workspace_role
            && workspace_role_grants(role, permission)
        {
            return true;
        }
        match self.project_role {
            Some(role) => project_role_grants(role, permission),
            None => false,
        }
    }

    /// Whether the principal can view projects at all through these grants.
    pub const fn can_view_project(self) -> bool {
        self.permits(Permission::ProjectView, ResourceKind::Project)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const WORKSPACE_ROLES: [WorkspaceRole; 3] = [
        WorkspaceRole::Owner,
        WorkspaceRole::Admin,
        WorkspaceRole::Member,
    ];
    const PROJECT_ROLES: [ProjectRole; 3] = [
        ProjectRole::ProjectAdmin,
        ProjectRole::Member,
        ProjectRole::Viewer,
    ];

    fn granted(role: WorkspaceRole) -> Vec<&'static str> {
        Permission::ALL
            .into_iter()
            .filter(|permission| workspace_role_grants(role, *permission))
            .map(Permission::as_str)
            .collect()
    }

    fn granted_to_project_role(role: ProjectRole) -> Vec<&'static str> {
        Permission::ALL
            .into_iter()
            .filter(|permission| project_role_grants(role, *permission))
            .map(Permission::as_str)
            .collect()
    }

    #[test]
    fn permission_wire_names_round_trip_and_are_unique() {
        for permission in Permission::ALL {
            assert_eq!(Permission::parse(permission.as_str()), Some(permission));
        }
        assert_eq!(Permission::parse("project:everything"), None);
        let mut names: Vec<_> = Permission::ALL.map(Permission::as_str).to_vec();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), Permission::ALL.len());
    }

    #[test]
    fn owner_holds_every_permission() {
        assert_eq!(granted(WorkspaceRole::Owner).len(), Permission::ALL.len());
    }

    #[test]
    fn admin_holds_everything_except_billing() {
        let admin = granted(WorkspaceRole::Admin);
        assert_eq!(admin.len(), Permission::ALL.len() - 1);
        assert!(!admin.contains(&"workspace:billing"));
        assert!(admin.contains(&"workspace:manage"));
        assert!(admin.contains(&"workspace:members:invite"));
        assert!(admin.contains(&"project:delete"));
    }

    #[test]
    fn workspace_member_works_but_does_not_administer() {
        assert_eq!(
            granted(WorkspaceRole::Member),
            [
                "workspace:view",
                "project:create",
                "project:view",
                "project:edit",
                "screening:perform",
                "screening:resolve",
                "extraction:perform",
                "appraisal:perform",
                "ai:use",
            ]
        );
    }

    #[test]
    fn project_roles_map_to_their_table() {
        assert_eq!(
            granted_to_project_role(ProjectRole::ProjectAdmin),
            [
                "project:view",
                "project:edit",
                "project:delete",
                "screening:perform",
                "screening:resolve",
                "extraction:perform",
                "appraisal:perform",
                "automation:manage",
                "ai:use",
            ]
        );
        assert_eq!(
            granted_to_project_role(ProjectRole::Member),
            [
                "project:view",
                "project:edit",
                "screening:perform",
                "screening:resolve",
                "extraction:perform",
                "appraisal:perform",
                "ai:use",
            ]
        );
        assert_eq!(
            granted_to_project_role(ProjectRole::Viewer),
            ["project:view"]
        );
    }

    #[test]
    fn project_grants_never_carry_workspace_permissions() {
        for role in PROJECT_ROLES {
            for permission in Permission::ALL {
                if permission.scope() == ResourceKind::Workspace {
                    assert!(
                        !project_role_grants(role, permission),
                        "{role:?} {permission:?}"
                    );
                }
            }
        }
    }

    #[test]
    fn every_role_can_view_projects() {
        for role in WORKSPACE_ROLES {
            assert!(workspace_role_grants(role, Permission::ProjectView));
        }
        for role in PROJECT_ROLES {
            assert!(project_role_grants(role, Permission::ProjectView));
        }
    }

    #[test]
    fn a_permission_on_the_wrong_kind_of_resource_is_denied() {
        let owner = Grants::new(true, Some(WorkspaceRole::Owner), None);
        assert!(owner.permits(Permission::ProjectView, ResourceKind::Project));
        assert!(!owner.permits(Permission::ProjectView, ResourceKind::Workspace));
        assert!(owner.permits(Permission::ProjectCreate, ResourceKind::Workspace));
        assert!(!owner.permits(Permission::ProjectCreate, ResourceKind::Project));
    }

    #[test]
    fn workspace_and_project_grants_union_and_a_viewer_grant_does_not_narrow() {
        let member_and_viewer =
            Grants::new(true, Some(WorkspaceRole::Member), Some(ProjectRole::Viewer));
        assert!(member_and_viewer.permits(Permission::ProjectEdit, ResourceKind::Project));
        let guest_admin = Grants::new(true, None, Some(ProjectRole::ProjectAdmin));
        assert!(guest_admin.permits(Permission::ProjectDelete, ResourceKind::Project));
        assert!(!guest_admin.permits(Permission::WorkspaceView, ResourceKind::Workspace));
    }

    #[test]
    fn a_deactivated_user_holds_nothing() {
        let grants = Grants::new(
            false,
            Some(WorkspaceRole::Owner),
            Some(ProjectRole::ProjectAdmin),
        );
        for permission in Permission::ALL {
            assert!(!grants.permits(permission, permission.scope()));
        }
    }

    #[test]
    fn unknown_or_missing_facts_fail_closed() {
        let denies_everything = |grants: Grants| {
            Permission::ALL
                .into_iter()
                .all(|permission| !grants.permits(permission, permission.scope()))
        };
        assert!(denies_everything(Grants::none()));
        // No user row.
        assert!(denies_everything(Grants::from_stored(
            None,
            Some(("owner", "active")),
            Some("project_admin")
        )));
        // Unknown user status.
        assert!(denies_everything(Grants::from_stored(
            Some("suspended"),
            Some(("owner", "active")),
            None
        )));
        // Missing membership and grant.
        assert!(denies_everything(Grants::from_stored(
            Some("active"),
            None,
            None
        )));
        // Unknown role, unknown membership status, deactivated membership.
        assert!(denies_everything(Grants::from_stored(
            Some("active"),
            Some(("superuser", "active")),
            None
        )));
        assert!(denies_everything(Grants::from_stored(
            Some("active"),
            Some(("owner", "pending")),
            None
        )));
        assert!(denies_everything(Grants::from_stored(
            Some("active"),
            Some(("owner", "deactivated")),
            None
        )));
        // Unknown project role.
        assert!(denies_everything(Grants::from_stored(
            Some("active"),
            None,
            Some("root")
        )));
        // Casing is not normalized: a mangled value is unknown.
        assert!(denies_everything(Grants::from_stored(
            Some("Active"),
            Some(("Owner", "Active")),
            None
        )));
    }

    #[test]
    fn a_deactivated_membership_leaves_a_project_grant_working() {
        let grants = Grants::from_stored(
            Some("active"),
            Some(("owner", "deactivated")),
            Some("viewer"),
        );
        assert!(grants.permits(Permission::ProjectView, ResourceKind::Project));
        assert!(!grants.permits(Permission::ProjectEdit, ResourceKind::Project));
    }
}
