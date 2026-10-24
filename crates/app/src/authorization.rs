//! PostgreSQL side of central authorization (handoff 03b): the
//! [`AuthorizationStore`] that reads every fact one decision needs in a single
//! query, the predicate that restricts lists to viewable projects, and the
//! bounded audit of denials.
//!
//! Nothing is cached. Every call reads the user, the membership and the
//! project grant as they are now, so a removal or deactivation takes effect on
//! the next request.

use std::{collections::HashSet, sync::Mutex};

use deepref_application::authorization::{
    AuthorizationFuture, AuthorizationStore, AuthzError, ResourceAccess, ServiceAccess,
};
use deepref_domain::{
    ProjectId, ProjectRole, UserId, WorkspaceId, WorkspaceRole,
    authorization::{Grants, Permission, Resource, project_role_grants, workspace_role_grants},
};
use sqlx::{PgConnection, PgPool, Row};
use uuid::Uuid;

#[derive(Clone)]
pub struct PostgresAuthorizationStore {
    pool: PgPool,
}

impl PostgresAuthorizationStore {
    pub const fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

/// The project row resolved to its workspace together with the principal's
/// facts, in one statement. A project absent from `projects`, or one whose
/// workspace is unset, yields no row or a NULL workspace: both are denials.
const PROJECT_ACCESS_SQL: &str = "\
    SELECT p.workspace_id AS workspace_id, u.status AS user_status, \
           m.role AS membership_role, m.status AS membership_status, \
           pa.role AS project_role \
    FROM projects p \
    LEFT JOIN users u ON u.id = $2 \
    LEFT JOIN workspace_memberships m ON m.workspace_id = p.workspace_id AND m.user_id = $2 \
    LEFT JOIN project_access pa ON pa.project_id = p.id AND pa.user_id = $2 \
    WHERE p.id = $1";

const WORKSPACE_ACCESS_SQL: &str = "\
    SELECT w.id AS workspace_id, u.status AS user_status, \
           m.role AS membership_role, m.status AS membership_status, \
           NULL::text AS project_role \
    FROM workspaces w \
    LEFT JOIN users u ON u.id = $2 \
    LEFT JOIN workspace_memberships m ON m.workspace_id = w.id AND m.user_id = $2 \
    WHERE w.id = $1";

/// One human-access fact query and parser for pool and transaction adapters.
/// Permission/role decisions remain exclusively in the central PDP/domain.
async fn read_access<'e>(
    executor: impl sqlx::Executor<'e, Database = sqlx::Postgres>,
    user: UserId,
    resource: Resource,
) -> Result<ResourceAccess, AuthzError> {
    let (sql, id) = match resource {
        Resource::Project(project) => (PROJECT_ACCESS_SQL, project.as_uuid()),
        Resource::Workspace(workspace) => (WORKSPACE_ACCESS_SQL, workspace.as_uuid()),
    };
    let row = sqlx::query(sql)
        .bind(id)
        .bind(user.as_uuid())
        .fetch_optional(executor)
        .await
        .map_err(|error| AuthzError::Store(error.to_string()))?;
    let Some(row) = row else {
        return Ok(ResourceAccess::unresolved());
    };
    let read = |error: sqlx::Error| AuthzError::Store(error.to_string());
    let workspace: Option<Uuid> = row.try_get("workspace_id").map_err(read)?;
    let user_status: Option<String> = row.try_get("user_status").map_err(read)?;
    let role: Option<String> = row.try_get("membership_role").map_err(read)?;
    let status: Option<String> = row.try_get("membership_status").map_err(read)?;
    let project_role: Option<String> = row.try_get("project_role").map_err(read)?;
    let membership = role.as_deref().zip(status.as_deref());
    Ok(ResourceAccess {
        workspace_id: workspace.map(WorkspaceId::new),
        grants: Grants::from_stored(user_status.as_deref(), membership, project_role.as_deref()),
    })
}

/// Human authorization through a borrowed, already-held transaction connection.
/// The mutex adapts AuthorizationStore's shared-reference port to SQLx's mutable
/// connection executor; it never acquires a pool connection. Borrowing prevents
/// this adapter from outliving the transaction whose locks protect its facts.
/// Machine authorization is not needed here and retains the port's deny default.
pub(crate) struct TransactionAuthorizationStore<'connection> {
    connection: tokio::sync::Mutex<&'connection mut PgConnection>,
}

impl<'connection> TransactionAuthorizationStore<'connection> {
    pub(crate) fn new(connection: &'connection mut PgConnection) -> Self {
        Self {
            connection: tokio::sync::Mutex::new(connection),
        }
    }
}

impl AuthorizationStore for TransactionAuthorizationStore<'_> {
    fn access<'a>(
        &'a self,
        user: UserId,
        resource: Resource,
    ) -> AuthorizationFuture<'a, Result<ResourceAccess, AuthzError>> {
        Box::pin(async move {
            let mut connection = self.connection.lock().await;
            read_access(&mut **connection, user, resource).await
        })
    }
}

impl AuthorizationStore for PostgresAuthorizationStore {
    fn access<'a>(
        &'a self,
        user: UserId,
        resource: Resource,
    ) -> AuthorizationFuture<'a, Result<ResourceAccess, AuthzError>> {
        Box::pin(read_access(&self.pool, user, resource))
    }
    fn service_access<'a>(
        &'a self,
        service: deepref_domain::ServicePrincipal,
        resource: Resource,
    ) -> AuthorizationFuture<'a, Result<ServiceAccess, AuthzError>> {
        Box::pin(async move {
            let (sql, id) = match resource {
                Resource::Workspace(id) => (
                    "SELECT w.id AS workspace_id, k.workspace_id AS key_workspace_id, k.scopes,
                            u.status AS user_status, m.role, m.status AS membership_status
                     FROM api_keys k
                     JOIN users u ON u.id = k.created_by_user_id
                     JOIN workspaces w ON w.id = $2
                     LEFT JOIN workspace_memberships m ON m.workspace_id = w.id AND m.user_id = u.id
                     WHERE k.id = $1 AND k.revoked_at IS NULL AND (k.expires_at IS NULL OR k.expires_at > now())",
                    id.as_uuid()),
                Resource::Project(id) => (
                    "SELECT p.workspace_id, k.workspace_id AS key_workspace_id, k.scopes,
                            u.status AS user_status, m.role, m.status AS membership_status
                     FROM api_keys k
                     JOIN users u ON u.id = k.created_by_user_id
                     JOIN projects p ON p.id = $2
                     LEFT JOIN workspace_memberships m ON m.workspace_id = p.workspace_id AND m.user_id = u.id
                     WHERE k.id = $1 AND k.revoked_at IS NULL AND (k.expires_at IS NULL OR k.expires_at > now())",
                    id.as_uuid()),
            };
            let read = |error: sqlx::Error| AuthzError::Store(error.to_string());
            let row = sqlx::query(sql)
                .bind(service.key_id.as_uuid())
                .bind(id)
                .fetch_optional(&self.pool)
                .await
                .map_err(read)?;
            let Some(row) = row else {
                return Ok(ServiceAccess::unresolved());
            };
            let key_workspace: Uuid = row.try_get("key_workspace_id").map_err(read)?;
            let workspace: Option<Uuid> = row.try_get("workspace_id").map_err(read)?;
            let status: String = row.try_get("user_status").map_err(read)?;
            let role: Option<String> = row.try_get("role").map_err(read)?;
            let membership_status: Option<String> =
                row.try_get("membership_status").map_err(read)?;
            let scopes: Vec<String> = row.try_get("scopes").map_err(read)?;
            Ok(ServiceAccess {
                key_workspace_id: Some(WorkspaceId::new(key_workspace)),
                resource: ResourceAccess {
                    workspace_id: workspace.map(WorkspaceId::new),
                    grants: Grants::from_stored(
                        Some(&status),
                        role.as_deref().zip(membership_status.as_deref()),
                        None,
                    ),
                },
                scopes: scopes
                    .iter()
                    .filter_map(|scope| Permission::parse(scope))
                    .collect(),
            })
        })
    }
}

/// The role strings that carry `project:view`, derived from the domain
/// mapping so a list filter can never disagree with [`Authorizer::require`].
///
/// [`Authorizer::require`]: deepref_application::authorization::Authorizer::require
#[derive(Debug, Clone)]
pub struct ProjectViewRoles {
    workspace: Vec<String>,
    project: Vec<String>,
}

impl ProjectViewRoles {
    pub fn current() -> Self {
        let workspace = [
            WorkspaceRole::Owner,
            WorkspaceRole::Admin,
            WorkspaceRole::Member,
        ]
        .into_iter()
        .filter(|role| workspace_role_grants(*role, Permission::ProjectView))
        .map(|role| role.as_str().to_owned())
        .collect();
        let project = [
            ProjectRole::ProjectAdmin,
            ProjectRole::Member,
            ProjectRole::Viewer,
        ]
        .into_iter()
        .filter(|role| project_role_grants(*role, Permission::ProjectView))
        .map(|role| role.as_str().to_owned())
        .collect();
        Self { workspace, project }
    }

    pub fn workspace_roles(&self) -> &[String] {
        &self.workspace
    }

    pub fn project_roles(&self) -> &[String] {
        &self.project
    }
}

/// A SQL boolean expression (a `&'static str`), true when the user
/// (`$user`, a `uuid` parameter) may view the project whose id
/// `project_id_expr` names. `$workspace_roles` and `$project_roles` are
/// `text[]` parameters bound from [`ProjectViewRoles`]. All arguments are
/// string literals: the expression is assembled at compile time, never from
/// request data. It is the single list-side mirror of `project:view`: an
/// active user, and either an active membership with a role that carries it
/// or a project grant that carries it.
#[macro_export]
macro_rules! viewable_project_predicate {
    ($project_id_expr:literal, $user:literal, $workspace_roles:literal, $project_roles:literal) => {
        concat!(
            "(EXISTS (SELECT 1 FROM users vu \
                JOIN projects vp ON vp.id = ",
            $project_id_expr,
            " JOIN workspace_memberships vm \
                  ON vm.workspace_id = vp.workspace_id AND vm.user_id = vu.id \
                WHERE vu.id = ",
            $user,
            " AND vu.status = 'active' \
                  AND vm.status = 'active' AND vm.role = ANY(",
            $workspace_roles,
            ")) OR EXISTS (SELECT 1 FROM users vu \
                JOIN project_access va ON va.user_id = vu.id AND va.project_id = ",
            $project_id_expr,
            " WHERE vu.id = ",
            $user,
            " AND vu.status = 'active' \
                  AND va.role = ANY(",
            $project_roles,
            ")))"
        )
    };
}

/// Projects the user may view, the unit a user-scoped list (notifications,
/// the project list) filters through.
pub async fn viewable_project_ids(
    pool: &PgPool,
    user: UserId,
) -> Result<Vec<ProjectId>, sqlx::Error> {
    let roles = ProjectViewRoles::current();
    let rows = sqlx::query(concat!(
        "SELECT p.id FROM projects p WHERE ",
        viewable_project_predicate!("p.id", "$1", "$2", "$3")
    ))
    .bind(user.as_uuid())
    .bind(roles.workspace_roles())
    .bind(roles.project_roles())
    .fetch_all(pool)
    .await?;
    Ok(rows
        .iter()
        .map(|row| ProjectId::new(row.get("id")))
        .collect())
}

/// The workspace a user with exactly one eligible workspace should create a
/// project in, or `None` when there is none or more than one. Only workspaces
/// where the user holds an active membership that carries `project:create`
/// count.
pub async fn sole_project_creation_workspace(
    pool: &PgPool,
    user: UserId,
) -> Result<Option<WorkspaceId>, sqlx::Error> {
    let roles: Vec<String> = [
        WorkspaceRole::Owner,
        WorkspaceRole::Admin,
        WorkspaceRole::Member,
    ]
    .into_iter()
    .filter(|role| workspace_role_grants(*role, Permission::ProjectCreate))
    .map(|role| role.as_str().to_owned())
    .collect();
    let rows: Vec<Uuid> = sqlx::query_scalar(
        "SELECT m.workspace_id FROM workspace_memberships m \
         JOIN users u ON u.id = m.user_id \
         JOIN workspaces w ON w.id = m.workspace_id \
         WHERE m.user_id = $1 AND u.status = 'active' AND m.status = 'active' \
           AND m.role = ANY($2) AND w.kind <> 'migration' \
         LIMIT 2",
    )
    .bind(user.as_uuid())
    .bind(&roles)
    .fetch_all(pool)
    .await?;
    Ok(match rows.as_slice() {
        [only] => Some(WorkspaceId::new(*only)),
        _ => None,
    })
}

/// What a denial was about, as a closed vocabulary. Nothing a caller typed
/// reaches the audit: a permission is one of the fixed names, a resource type
/// one of three, and an identifier only exists here as a parsed [`Uuid`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DeniedFor {
    Permission(Permission),
    /// A route with no policy row: denied, never allowed by default.
    RouteUnlisted,
    /// A route reserved for a platform operator, which no user is.
    PlatformOperator,
}

impl DeniedFor {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Permission(permission) => permission.as_str(),
            Self::RouteUnlisted => "route_unlisted",
            Self::PlatformOperator => "platform_operator",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DeniedResource {
    Project(Option<ProjectId>),
    Workspace(Option<WorkspaceId>),
    None,
}

impl DeniedResource {
    const fn type_name(self) -> &'static str {
        match self {
            Self::Project(_) => "project",
            Self::Workspace(_) => "workspace",
            Self::None => "none",
        }
    }
}

/// One denied request, reduced to what may be stored.
#[derive(Debug, Clone, Copy)]
pub struct Denial<'a> {
    pub user: UserId,
    pub denied_for: DeniedFor,
    pub resource: DeniedResource,
    pub request_id: Option<&'a str>,
}

/// Records `authorization.denied` into `admin_audit_events`, bounded two ways.
///
/// The audit table is append-only, so a count cannot be bumped in place. The
/// writer instead *samples*: one row per (user, permission, resource type,
/// one-minute window). A process-local set skips the round trip for keys it
/// already wrote this window, and a unique index (migration 0100) makes the
/// bound hold across processes. A flood of denials therefore stores at most
/// permissions x resource types rows per user per minute.
pub struct DenialAuditor {
    pool: PgPool,
    seen: Mutex<SeenWindow>,
}

#[derive(Default)]
struct SeenWindow {
    window: i64,
    keys: HashSet<(Uuid, &'static str, &'static str)>,
}

/// Process-local cap. The set holds at most 15 permissions x 3 resource types
/// per user in the current window; past this many entries it is simply
/// cleared, which only costs extra no-op inserts.
const MAX_SEEN_KEYS: usize = 8192;

impl DenialAuditor {
    pub fn new(pool: PgPool) -> Self {
        Self {
            pool,
            seen: Mutex::new(SeenWindow::default()),
        }
    }

    fn window_now() -> i64 {
        let seconds = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |elapsed| elapsed.as_secs());
        i64::try_from(seconds / 60).unwrap_or(0)
    }

    fn key(user: UserId, denial: &Denial<'_>) -> (Uuid, &'static str, &'static str) {
        (
            user.as_uuid(),
            denial.denied_for.as_str(),
            denial.resource.type_name(),
        )
    }

    /// Forgets a claim whose insert failed, so the next denial retries it.
    fn release(&self, user: UserId, denial: &Denial<'_>, window: i64) {
        if let Ok(mut seen) = self.seen.lock()
            && seen.window == window
        {
            seen.keys.remove(&Self::key(user, denial));
        }
    }

    /// Whether this process has not yet written the key in `window`; if so,
    /// claims it. The claim stands only once the insert succeeds (a unique
    /// conflict counts: the row exists).
    fn claim(&self, user: UserId, denial: &Denial<'_>, window: i64) -> bool {
        let Ok(mut seen) = self.seen.lock() else {
            return true;
        };
        if seen.window != window || seen.keys.len() >= MAX_SEEN_KEYS {
            seen.window = window;
            seen.keys.clear();
        }
        seen.keys.insert(Self::key(user, denial))
    }

    /// Returns whether a row was written. A failed write is the caller's to
    /// log; a denial is never turned into an error by its own audit.
    pub async fn record(&self, denial: Denial<'_>) -> Result<bool, sqlx::Error> {
        let window = Self::window_now();
        if !self.claim(denial.user, &denial, window) {
            return Ok(false);
        }
        let (project_id, workspace_id) = match denial.resource {
            DeniedResource::Project(id) => (id.map(ProjectId::as_uuid), None),
            DeniedResource::Workspace(id) => (None, id.map(WorkspaceId::as_uuid)),
            DeniedResource::None => (None, None),
        };
        let metadata = serde_json::json!({
            "permission": denial.denied_for.as_str(),
            "resource_type": denial.resource.type_name(),
            "window": window.to_string(),
        });
        let written = match sqlx::query(
            "INSERT INTO admin_audit_events \
               (id, event_type, principal_kind, principal_user_id, workspace_id, project_id, \
                request_id, metadata) \
             VALUES ($1, 'authorization.denied', 'user', $2, $3, $4, $5, $6) \
             ON CONFLICT DO NOTHING",
        )
        .bind(Uuid::new_v4())
        .bind(denial.user.as_uuid())
        .bind(workspace_id)
        .bind(project_id)
        .bind(denial.request_id.filter(|id| id.len() <= 200))
        .bind(metadata)
        .execute(&self.pool)
        .await
        {
            Ok(result) => result.rows_affected(),
            Err(error) => {
                self.release(denial.user, &denial, window);
                return Err(error);
            }
        };
        Ok(written == 1)
    }
}

/// The permission the equivalent direct route requires for an assistant tool
/// or plan action, so the assistant can never do what its user could not:
///
/// | tool | route it mirrors | permission |
/// | --- | --- | --- |
/// | `trigger_workflow` | `POST .../automations/runs` | `automation:manage` |
/// | `screen_reports` (plan action) | `POST .../reports/{id}/screening` | `screening:perform` |
/// | `propose_*` | `POST .../ai/*` suggestions | `ai:use` |
/// | read tools, overview, report listing | the project's read routes | `project:view` |
pub fn assistant_tool_permission(tool_name: &str) -> Permission {
    use deepref_ai::{
        AgentToolName, TOOL_LIST_REPORTS, TOOL_PROJECT_OVERVIEW, TOOL_SCREEN_REPORTS,
    };
    match tool_name {
        "trigger_workflow" => Permission::AutomationManage,
        TOOL_SCREEN_REPORTS => Permission::ScreeningPerform,
        TOOL_PROJECT_OVERVIEW | TOOL_LIST_REPORTS => Permission::ProjectView,
        other => match AgentToolName::parse(other) {
            Some(name) if name.is_read() => Permission::ProjectView,
            _ => Permission::AiUse,
        },
    }
}

/// The one boundary that decides *and* audits: the policy decision point plus
/// the bounded, sampled denial audit the HTTP middleware uses. Dispatch-level
/// checks (assistant tools, plan actions, worker turns) go through it so a
/// denial is never silent.
#[derive(Clone)]
pub struct AuditedAuthorizer {
    authorizer: std::sync::Arc<dyn deepref_application::authorization::Authorizer>,
    denials: std::sync::Arc<DenialAuditor>,
}

impl AuditedAuthorizer {
    pub fn new(
        authorizer: std::sync::Arc<dyn deepref_application::authorization::Authorizer>,
        denials: std::sync::Arc<DenialAuditor>,
    ) -> Self {
        Self {
            authorizer,
            denials,
        }
    }

    /// The default wiring over PostgreSQL.
    pub fn postgres(pool: &PgPool) -> Self {
        Self::new(
            std::sync::Arc::new(
                deepref_application::authorization::PolicyDecisionPoint::new(
                    PostgresAuthorizationStore::new(pool.clone()),
                ),
            ),
            std::sync::Arc::new(DenialAuditor::new(pool.clone())),
        )
    }

    pub async fn require(
        &self,
        principal: &deepref_domain::Principal,
        permission: Permission,
        resource: Resource,
        request_id: Option<&str>,
    ) -> Result<(), AuthzError> {
        match self
            .authorizer
            .require(principal, permission, resource)
            .await
        {
            Err(AuthzError::Denied(denied)) => {
                if let Some(user) = principal.user_id() {
                    let resource = match resource {
                        Resource::Workspace(id) => DeniedResource::Workspace(Some(id)),
                        Resource::Project(id) => DeniedResource::Project(Some(id)),
                    };
                    let denial = Denial {
                        user,
                        denied_for: DeniedFor::Permission(permission),
                        resource,
                        request_id,
                    };
                    if let Err(error) = self.denials.record(denial).await {
                        tracing::warn!(%error, "authorization denial could not be audited");
                    }
                }
                Err(AuthzError::Denied(denied))
            }
            other => other,
        }
    }
}
