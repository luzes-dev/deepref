//! The central policy decision point (handoff 03b).
//!
//! Policy Enforcement Points (the HTTP middleware, SSE re-checks, handlers
//! that learn their resource only from a body) ask one question through
//! [`Authorizer::require`]; [`PolicyDecisionPoint`] answers it from the facts
//! an [`AuthorizationStore`] reads in a single query. Nothing here caches: a
//! membership removal, a role downgrade or a user deactivation is visible to
//! the very next call.
//!
//! A missing resource, a resource in a workspace the principal does not belong
//! to and a plain lack of permission are one outcome, [`AuthzError::Denied`].
//! It carries nothing, so no caller can tell them apart.

use std::{future::Future, pin::Pin};

use deepref_domain::{
    Principal, UserId, WorkspaceId,
    authorization::{Grants, Permission, Resource},
};
use thiserror::Error;

pub type AuthorizationFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// The one answer for "no". Deliberately fieldless.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
#[error("access denied")]
pub struct AuthzDenied;

#[derive(Debug, Error)]
pub enum AuthzError {
    #[error(transparent)]
    Denied(#[from] AuthzDenied),
    /// The facts could not be read. Never a denial and never an allowance:
    /// the caller answers with a retryable failure.
    #[error("authorization data is unavailable: {0}")]
    Store(String),
}

/// What the store learned about one principal and one resource.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResourceAccess {
    /// The workspace the resource belongs to. `None` when the resource does
    /// not exist or cannot be resolved to a workspace.
    pub workspace_id: Option<WorkspaceId>,
    pub grants: Grants,
}

impl ResourceAccess {
    pub const fn unresolved() -> Self {
        Self {
            workspace_id: None,
            grants: Grants::none(),
        }
    }
}

/// Reads, in one query, the resource's workspace together with the user's
/// status, active membership and project grant.
pub trait AuthorizationStore: Send + Sync {
    fn access<'a>(
        &'a self,
        user: UserId,
        resource: Resource,
    ) -> AuthorizationFuture<'a, Result<ResourceAccess, AuthzError>>;
}

pub trait Authorizer: Send + Sync {
    fn require<'a>(
        &'a self,
        principal: &'a Principal,
        permission: Permission,
        resource: Resource,
    ) -> AuthorizationFuture<'a, Result<(), AuthzError>>;
}

pub struct PolicyDecisionPoint<S> {
    store: S,
}

impl<S: AuthorizationStore> PolicyDecisionPoint<S> {
    pub const fn new(store: S) -> Self {
        Self { store }
    }
}

impl<S: AuthorizationStore> Authorizer for PolicyDecisionPoint<S> {
    fn require<'a>(
        &'a self,
        principal: &'a Principal,
        permission: Permission,
        resource: Resource,
    ) -> AuthorizationFuture<'a, Result<(), AuthzError>> {
        Box::pin(async move {
            match principal {
                // Work DeepRef starts itself (webhook application, job
                // handlers): trusted after the enqueue was authorized.
                Principal::System(_) => Ok(()),
                // API keys land with handoff 11; until then a service
                // principal holds nothing.
                Principal::Service(_) => Err(AuthzDenied.into()),
                Principal::User(user) => {
                    if !permission_applies(permission, resource) {
                        return Err(AuthzDenied.into());
                    }
                    let access = self.store.access(user.user_id, resource).await?;
                    if access.workspace_id.is_some()
                        && access.grants.permits(permission, resource.kind())
                    {
                        Ok(())
                    } else {
                        Err(AuthzDenied.into())
                    }
                }
            }
        })
    }
}

fn permission_applies(permission: Permission, resource: Resource) -> bool {
    permission.scope() == resource.kind()
}

#[cfg(test)]
mod tests {
    use deepref_domain::{ProjectId, ProjectRole, WorkspaceRole};
    use uuid::Uuid;

    use super::*;

    struct Fixed(ResourceAccess);

    impl AuthorizationStore for Fixed {
        fn access<'a>(
            &'a self,
            _: UserId,
            _: Resource,
        ) -> AuthorizationFuture<'a, Result<ResourceAccess, AuthzError>> {
            Box::pin(async move { Ok(self.0) })
        }
    }

    struct Broken;

    impl AuthorizationStore for Broken {
        fn access<'a>(
            &'a self,
            _: UserId,
            _: Resource,
        ) -> AuthorizationFuture<'a, Result<ResourceAccess, AuthzError>> {
            Box::pin(async { Err(AuthzError::Store("down".to_owned())) })
        }
    }

    fn user() -> Principal {
        Principal::user(UserId::new(Uuid::new_v4()))
    }

    fn project() -> Resource {
        Resource::Project(ProjectId::new(Uuid::new_v4()))
    }

    fn resolved(grants: Grants) -> ResourceAccess {
        ResourceAccess {
            workspace_id: Some(WorkspaceId::new(Uuid::new_v4())),
            grants,
        }
    }

    fn require<S: AuthorizationStore>(
        store: S,
        principal: &Principal,
        permission: Permission,
        resource: Resource,
    ) -> Result<(), AuthzError> {
        futures::executor::block_on(
            PolicyDecisionPoint::new(store).require(principal, permission, resource),
        )
    }

    #[test]
    fn a_member_is_allowed_what_the_role_grants_and_denied_the_rest() {
        let grants = Grants::new(true, Some(WorkspaceRole::Member), None);
        let principal = user();
        assert!(
            require(
                Fixed(resolved(grants)),
                &principal,
                Permission::ProjectEdit,
                project()
            )
            .is_ok()
        );
        assert!(matches!(
            require(
                Fixed(resolved(grants)),
                &principal,
                Permission::ProjectDelete,
                project()
            ),
            Err(AuthzError::Denied(_))
        ));
    }

    #[test]
    fn a_missing_resource_is_a_denial_even_for_an_owner() {
        let access = ResourceAccess {
            workspace_id: None,
            grants: Grants::new(true, Some(WorkspaceRole::Owner), None),
        };
        assert!(matches!(
            require(Fixed(access), &user(), Permission::ProjectView, project()),
            Err(AuthzError::Denied(_))
        ));
        assert!(matches!(
            require(
                Fixed(ResourceAccess::unresolved()),
                &user(),
                Permission::ProjectView,
                project()
            ),
            Err(AuthzError::Denied(_))
        ));
    }

    #[test]
    fn a_permission_checked_on_the_wrong_resource_kind_is_denied() {
        let grants = Grants::new(true, Some(WorkspaceRole::Owner), None);
        assert!(matches!(
            require(
                Fixed(resolved(grants)),
                &user(),
                Permission::ProjectCreate,
                project()
            ),
            Err(AuthzError::Denied(_))
        ));
    }

    #[test]
    fn a_store_failure_is_neither_allow_nor_deny() {
        assert!(matches!(
            require(Broken, &user(), Permission::ProjectView, project()),
            Err(AuthzError::Store(_))
        ));
    }

    #[test]
    fn system_is_trusted_and_service_principals_hold_nothing() {
        let system = Principal::system();
        assert!(
            require(Broken, &system, Permission::ProjectDelete, project()).is_ok(),
            "system principals never reach the store"
        );
        let service = Principal::service();
        assert!(matches!(
            require(Broken, &service, Permission::ProjectView, project()),
            Err(AuthzError::Denied(_))
        ));
    }

    #[test]
    fn a_guest_with_a_project_grant_is_limited_to_it() {
        let grants = Grants::new(true, None, Some(ProjectRole::Viewer));
        let principal = user();
        assert!(
            require(
                Fixed(resolved(grants)),
                &principal,
                Permission::ProjectView,
                project()
            )
            .is_ok()
        );
        assert!(matches!(
            require(
                Fixed(resolved(grants)),
                &principal,
                Permission::ProjectEdit,
                project()
            ),
            Err(AuthzError::Denied(_))
        ));
    }
}
