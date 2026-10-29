//! The route policy table. It is the coverage matrix: one row per HTTP
//! operation, rendered to `docs/authorization-matrix.md` by the
//! `authz-matrix` binary and compared with the router and the OpenAPI
//! document by `cargo xtask authz-matrix --check`.
//!
//! Adding a route without a row denies it at run time (the PEP fails closed)
//! and fails the check in CI. Adding a `Require` row makes the table-driven
//! foreign-workspace test in `tests/authz_foreign_workspace.rs` cover it with
//! no further edit.

use deepref_domain::authorization::Permission;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Verb {
    Get,
    Post,
    Put,
    Patch,
    Delete,
}

impl Verb {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Get => "GET",
            Self::Post => "POST",
            Self::Put => "PUT",
            Self::Patch => "PATCH",
            Self::Delete => "DELETE",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value.to_ascii_uppercase().as_str() {
            "GET" => Some(Self::Get),
            "POST" => Some(Self::Post),
            "PUT" => Some(Self::Put),
            "PATCH" => Some(Self::Patch),
            "DELETE" => Some(Self::Delete),
            _ => None,
        }
    }
}

/// Where the resource id comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResourceFrom {
    /// A path parameter holding the id of a project.
    PathParam(&'static str),
    /// A query-string parameter holding the id of a project.
    Query(&'static str),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Rule {
    /// No session is read. Health probes, sign-in redirects and
    /// token-authenticated machine deliveries; each row says why.
    Open,
    /// A session is required but no tenant resource is named: the handler
    /// scopes its answer to the caller (their own profile, their inbox).
    Authenticated,
    /// The PEP requires `permission` on a project before the handler runs.
    Require(Permission, ResourceFrom),
    /// The resource only appears in the request body, so the handler calls
    /// the authorizer with this permission once it knows it. The PEP still
    /// requires a session.
    Handler(Permission),
    /// As `Require`, but only when the named query parameter is present. The
    /// request without it names no tenant resource and the handler returns no
    /// project data. A present value that is not a UUID is a denial.
    RequireIfPresent(Permission, ResourceFrom),
    /// Reserved for a platform operator. No user principal holds it.
    PlatformOperator,
}

#[derive(Debug, Clone, Copy)]
pub struct RouteRule {
    pub method: Verb,
    pub path: &'static str,
    pub rule: Rule,
    /// The test that proves a foreign-workspace principal is denied (or why
    /// none applies).
    pub foreign_test: &'static str,
    pub reason: &'static str,
}

/// Which test covers every `Require` row.
pub const FOREIGN_PROJECT_TEST: &str =
    "authz_foreign_workspace::foreign_project_is_indistinguishable_from_a_missing_one";
const NOT_APPLICABLE: &str = "n/a";

const fn open(method: Verb, path: &'static str, reason: &'static str) -> RouteRule {
    RouteRule {
        method,
        path,
        rule: Rule::Open,
        foreign_test: NOT_APPLICABLE,
        reason,
    }
}

const fn authenticated(method: Verb, path: &'static str, reason: &'static str) -> RouteRule {
    RouteRule {
        method,
        path,
        rule: Rule::Authenticated,
        foreign_test: NOT_APPLICABLE,
        reason,
    }
}

const fn tested(rule: RouteRule, test: &'static str) -> RouteRule {
    RouteRule {
        foreign_test: test,
        ..rule
    }
}

const fn platform_operator(method: Verb, path: &'static str, reason: &'static str) -> RouteRule {
    RouteRule {
        method,
        path,
        rule: Rule::PlatformOperator,
        foreign_test: "authz_user_scoped::settings_update_is_reserved_for_operators",
        reason,
    }
}

const fn project_query(
    method: Verb,
    path: &'static str,
    permission: Permission,
    reason: &'static str,
) -> RouteRule {
    RouteRule {
        method,
        path,
        rule: Rule::RequireIfPresent(permission, ResourceFrom::Query("project_id")),
        foreign_test: "authz_foreign_workspace::query_project_routes_are_indistinguishable_from_a_missing_one",
        reason,
    }
}

const fn handler(
    method: Verb,
    path: &'static str,
    permission: Permission,
    reason: &'static str,
) -> RouteRule {
    RouteRule {
        method,
        path,
        rule: Rule::Handler(permission),
        foreign_test: "authz_user_scoped::create_project_in_a_foreign_workspace_is_indistinguishable_from_a_missing_one",
        reason,
    }
}

const fn project(
    method: Verb,
    path: &'static str,
    permission: Permission,
    reason: &'static str,
) -> RouteRule {
    RouteRule {
        method,
        path,
        rule: Rule::Require(permission, ResourceFrom::PathParam("project_id")),
        foreign_test: FOREIGN_PROJECT_TEST,
        reason,
    }
}

/// Explicit machine-safe routes. Every one must also have a mandatory central
/// project-view policy. New reads stay human-only until reviewed here.
pub fn machine_safe(row: &RouteRule) -> bool {
    row.method == Verb::Get
        && matches!(
            row.rule,
            Rule::Require(
                Permission::ProjectView,
                ResourceFrom::PathParam("project_id")
            )
        )
        && matches!(
            row.path,
            "/projects/{project_id}" | "/projects/{project_id}/reports"
        )
}

/// Explicit product-command activity coverage, independent of permission.
/// This first tracer covers study creation only; other routes are not yet
/// inactivity evidence. Never derive activity from method or actor UUID alone.
pub fn human_command_attempt(row: &RouteRule) -> bool {
    matches!(
        (row.method, row.path),
        (Verb::Post, "/projects/{project_id}/studies")
    )
}

pub static ROUTES: &[RouteRule] = &[
    tested(
        handler(
            Verb::Post,
            "/workspaces/{workspace_id}/billing/customer",
            Permission::WorkspaceBilling,
            "human billing provisioning; transaction PDP verifies workspace",
        ),
        "billing::human_management_is_tenant_scoped",
    ),
    tested(
        handler(
            Verb::Post,
            "/workspaces/{workspace_id}/billing/checkout",
            Permission::WorkspaceBilling,
            "human hosted checkout; pre/post provider authority checks",
        ),
        "billing::human_management_is_tenant_scoped",
    ),
    tested(
        handler(
            Verb::Post,
            "/workspaces/{workspace_id}/billing/portal",
            Permission::WorkspaceBilling,
            "human hosted portal; pre/post provider authority checks",
        ),
        "billing::human_management_is_tenant_scoped",
    ),
    open(
        Verb::Post,
        "/hooks/billing/dodo",
        "raw StandardWebhook verification and business/mode-scoped durable PostgreSQL receipt",
    ),
    tested(
        handler(
            Verb::Get,
            "/workspaces/{workspace_id}/api-keys",
            Permission::ApiKeyManage,
            "human key management; handler authorizes the named workspace",
        ),
        "api_keys::lifecycle_is_human_only_and_tenant_scoped",
    ),
    tested(
        handler(
            Verb::Post,
            "/workspaces/{workspace_id}/api-keys",
            Permission::ApiKeyManage,
            "human key creation; full secret returned once",
        ),
        "api_keys::lifecycle_is_human_only_and_tenant_scoped",
    ),
    tested(
        handler(
            Verb::Delete,
            "/workspaces/{workspace_id}/api-keys/{key_id}",
            Permission::ApiKeyManage,
            "human revocation; handler authorizes the workspace and key pair",
        ),
        "api_keys::lifecycle_is_human_only_and_tenant_scoped",
    ),
    tested(
        handler(
            Verb::Post,
            "/workspaces/{workspace_id}/api-keys/{key_id}/rotate",
            Permission::ApiKeyManage,
            "human rotation; old key revoked atomically with replacement",
        ),
        "api_keys::lifecycle_is_human_only_and_tenant_scoped",
    ),
    project(
        Verb::Get,
        "/projects/{project_id}/health/dependencies",
        Permission::ProjectView,
        "project-scoped job backlog and failures",
    ),
    project(
        Verb::Get,
        "/projects/{project_id}/scientific-assignments",
        Permission::ProjectView,
        "active methodology assignments for project viewers",
    ),
    tested(
        handler(
            Verb::Post,
            "/projects/{project_id}/scientific-assignments",
            Permission::WorkspaceManage,
            "methodology assignments are managed by workspace managers",
        ),
        "authz_user_scoped::scientific_assignment_management_requires_workspace_manage",
    ),
    tested(
        handler(
            Verb::Delete,
            "/projects/{project_id}/scientific-assignments/{user_id}/{role}",
            Permission::WorkspaceManage,
            "methodology assignment revocation requires workspace managers",
        ),
        "authz_user_scoped::scientific_assignment_management_requires_workspace_manage",
    ),
    project_query(
        Verb::Get,
        "/ai/status",
        Permission::ProjectView,
        "provider status for everyone; the optional project_id adds that project's budget, so it needs project:view",
    ),
    open(
        Verb::Get,
        "/auth/callback",
        "completes sign-in before a session exists",
    ),
    open(
        Verb::Get,
        "/auth/login",
        "starts sign-in before a session exists",
    ),
    open(
        Verb::Post,
        "/auth/logout",
        "revokes by cookie digest and works while the identity provider is down",
    ),
    authenticated(
        Verb::Get,
        "/automations/catalog",
        "static block catalog, no tenant data",
    ),
    authenticated(
        Verb::Get,
        "/automations/templates",
        "static workflow templates, no tenant data",
    ),
    open(Verb::Get, "/health", "liveness probe, no data"),
    open(
        Verb::Get,
        "/health/dependencies",
        "deployment-wide dependency status: no tenant data and no project filter",
    ),
    open(Verb::Get, "/health/live", "liveness probe, no data"),
    open(
        Verb::Get,
        "/health/ready",
        "readiness probe, no tenant data",
    ),
    open(
        Verb::Post,
        "/hooks/email/{token}",
        "machine delivery authenticated by its private address token",
    ),
    open(
        Verb::Post,
        "/hooks/workflows/{token}",
        "machine delivery authenticated by its private address token and HMAC signature",
    ),
    authenticated(Verb::Get, "/me", "the caller reads their own profile"),
    tested(
        authenticated(
            Verb::Get,
            "/me/workspaces",
            "human current membership discovery for account settings; actions recheck central PDP",
        ),
        "account_settings::only_current_memberships_and_pdp_capabilities_are_listed",
    ),
    tested(
        authenticated(
            Verb::Delete,
            "/me/deletion-request",
            "the caller cancels their own pending account deletion",
        ),
        "authz_user_scoped::deletion_request_affects_only_its_caller",
    ),
    tested(
        authenticated(
            Verb::Post,
            "/me/deletion-request",
            "the caller schedules deletion of their own account (11, BUSINESS_DECISIONS)",
        ),
        "authz_user_scoped::deletion_request_affects_only_its_caller",
    ),
    tested(
        authenticated(
            Verb::Post,
            "/me/export",
            "the caller requests an account data export (11)",
        ),
        "account_export::test_export_request",
    ),
    tested(
        authenticated(
            Verb::Get,
            "/me/export",
            "the caller inspects the status of their account data export (11)",
        ),
        "account_export::test_export_status",
    ),
    tested(
        authenticated(
            Verb::Get,
            "/me/export/{id}/download",
            "the caller downloads their completed account data export (11)",
        ),
        "account_export::test_export_download",
    ),
    tested(
        authenticated(
            Verb::Delete,
            "/me/export/{id}",
            "the caller cancels or discards their account data export (11)",
        ),
        "account_export::test_export_cancel",
    ),
    tested(
        authenticated(
            Verb::Get,
            "/notifications",
            "user-scoped inbox, filtered to projects the caller can still view",
        ),
        "authz_user_scoped::notifications_include_only_viewable_projects",
    ),
    tested(
        authenticated(
            Verb::Post,
            "/notifications/mark-read",
            "marks only notifications of projects the caller can still view",
        ),
        "authz_user_scoped::notifications_include_only_viewable_projects",
    ),
    tested(
        authenticated(
            Verb::Get,
            "/notifications/unread-count",
            "user-scoped count over projects the caller can still view",
        ),
        "authz_user_scoped::notifications_include_only_viewable_projects",
    ),
    open(Verb::Get, "/openapi.json", "public API description"),
    tested(
        authenticated(
            Verb::Get,
            "/projects",
            "lists only projects the caller holds project:view on",
        ),
        "authz_user_scoped::project_list_includes_only_viewable_projects",
    ),
    handler(
        Verb::Post,
        "/projects",
        Permission::ProjectCreate,
        "the target workspace is named in the body; the handler requires project:create on it",
    ),
    project(
        Verb::Delete,
        "/projects/{project_id}",
        Permission::ProjectDelete,
        "project deletion",
    ),
    project(
        Verb::Get,
        "/projects/{project_id}",
        Permission::ProjectView,
        "project read",
    ),
    project(
        Verb::Patch,
        "/projects/{project_id}",
        Permission::ProjectEdit,
        "project update",
    ),
    project(
        Verb::Get,
        "/projects/{project_id}/acquisitions",
        Permission::ProjectView,
        "project-scoped read",
    ),
    project(
        Verb::Post,
        "/projects/{project_id}/acquisitions",
        Permission::ProjectEdit,
        "project-scoped mutation",
    ),
    project(
        Verb::Get,
        "/projects/{project_id}/acquisitions/{acquisition_id}",
        Permission::ProjectView,
        "project-scoped read",
    ),
    project(
        Verb::Post,
        "/projects/{project_id}/acquisitions/{acquisition_id}/cancel",
        Permission::ProjectEdit,
        "project-scoped mutation",
    ),
    project(
        Verb::Get,
        "/projects/{project_id}/acquisitions/{acquisition_id}/items",
        Permission::ProjectView,
        "project-scoped read",
    ),
    project(
        Verb::Post,
        "/projects/{project_id}/acquisitions/{acquisition_id}/refresh",
        Permission::ProjectEdit,
        "project-scoped mutation",
    ),
    project(
        Verb::Get,
        "/projects/{project_id}/ai/activity",
        Permission::ProjectView,
        "project-scoped read",
    ),
    project(
        Verb::Post,
        "/projects/{project_id}/ai/activity/batches/{batch_id}/undo",
        Permission::ProjectEdit,
        "AI configuration or activity change",
    ),
    project(
        Verb::Get,
        "/projects/{project_id}/ai/activity/overview",
        Permission::ProjectView,
        "project-scoped read",
    ),
    project(
        Verb::Post,
        "/projects/{project_id}/ai/activity/{activity_id}/undo",
        Permission::ProjectEdit,
        "AI configuration or activity change",
    ),
    project(
        Verb::Get,
        "/projects/{project_id}/ai/autonomy",
        Permission::ProjectView,
        "project-scoped read",
    ),
    project(
        Verb::Put,
        "/projects/{project_id}/ai/autonomy",
        Permission::ProjectEdit,
        "AI configuration or activity change",
    ),
    project(
        Verb::Get,
        "/projects/{project_id}/ai/budget",
        Permission::ProjectView,
        "project-scoped read",
    ),
    project(
        Verb::Put,
        "/projects/{project_id}/ai/budget",
        Permission::ProjectEdit,
        "AI configuration or activity change",
    ),
    project(
        Verb::Get,
        "/projects/{project_id}/ai/first",
        Permission::ProjectView,
        "project-scoped read",
    ),
    project(
        Verb::Post,
        "/projects/{project_id}/ai/first",
        Permission::ProjectEdit,
        "AI configuration or activity change",
    ),
    project(
        Verb::Post,
        "/projects/{project_id}/ai/first/{cohort_id}/close",
        Permission::ProjectEdit,
        "AI configuration or activity change",
    ),
    project(
        Verb::Post,
        "/projects/{project_id}/ai/first/{cohort_id}/draw",
        Permission::ProjectEdit,
        "AI configuration or activity change",
    ),
    project(
        Verb::Post,
        "/projects/{project_id}/ai/first/{cohort_id}/evaluate",
        Permission::ProjectEdit,
        "AI configuration or activity change",
    ),
    project(
        Verb::Post,
        "/projects/{project_id}/ai/first/{cohort_id}/finalize",
        Permission::ProjectEdit,
        "AI configuration or activity change",
    ),
    project(
        Verb::Post,
        "/projects/{project_id}/ai/first/{cohort_id}/recover",
        Permission::ProjectEdit,
        "AI configuration or activity change",
    ),
    project(
        Verb::Get,
        "/projects/{project_id}/ai/proposals",
        Permission::ProjectView,
        "project-scoped read",
    ),
    project(
        Verb::Get,
        "/projects/{project_id}/ai/proposals/{proposal_id}",
        Permission::ProjectView,
        "project-scoped read",
    ),
    project(
        Verb::Post,
        "/projects/{project_id}/ai/proposals/{proposal_id}/decision",
        Permission::ProjectEdit,
        "applies an AI proposal",
    ),
    project(
        Verb::Get,
        "/projects/{project_id}/ai/reviewer-agreement",
        Permission::ProjectView,
        "project-scoped read",
    ),
    project(
        Verb::Get,
        "/projects/{project_id}/ai/reviewer-decisions",
        Permission::ProjectView,
        "project-scoped read",
    ),
    project(
        Verb::Post,
        "/projects/{project_id}/ai/reviewer-decisions/{decision_id}/resolve",
        Permission::ScreeningResolve,
        "resolves a reviewer conflict",
    ),
    project(
        Verb::Get,
        "/projects/{project_id}/appraisal-definitions",
        Permission::ProjectView,
        "project-scoped read",
    ),
    project(
        Verb::Get,
        "/projects/{project_id}/appraisal-definitions/{definition_id}/{version}",
        Permission::ProjectView,
        "project-scoped read",
    ),
    project(
        Verb::Post,
        "/projects/{project_id}/appraisal-definitions/{definition_id}/{version}/judgment-suggestion",
        Permission::AiUse,
        "AI suggestion",
    ),
    project(
        Verb::Post,
        "/projects/{project_id}/assistant/chat",
        Permission::AiUse,
        "assistant mutation (streams the reply)",
    ),
    project(
        Verb::Get,
        "/projects/{project_id}/assistant/conversations",
        Permission::ProjectView,
        "project-scoped read",
    ),
    project(
        Verb::Post,
        "/projects/{project_id}/assistant/conversations",
        Permission::AiUse,
        "assistant mutation",
    ),
    project(
        Verb::Delete,
        "/projects/{project_id}/assistant/conversations/{conversation_id}",
        Permission::AiUse,
        "assistant mutation",
    ),
    project(
        Verb::Get,
        "/projects/{project_id}/assistant/conversations/{conversation_id}/messages",
        Permission::ProjectView,
        "project-scoped read",
    ),
    project(
        Verb::Get,
        "/projects/{project_id}/assistant/plans/{plan_id}",
        Permission::ProjectView,
        "project-scoped read",
    ),
    project(
        Verb::Post,
        "/projects/{project_id}/assistant/plans/{plan_id}/confirm",
        Permission::AiUse,
        "assistant mutation",
    ),
    project(
        Verb::Post,
        "/projects/{project_id}/assistant/plans/{plan_id}/reject",
        Permission::AiUse,
        "assistant mutation",
    ),
    project(
        Verb::Get,
        "/projects/{project_id}/assistant/runs/{run_id}",
        Permission::ProjectView,
        "project-scoped read",
    ),
    project(
        Verb::Get,
        "/projects/{project_id}/assistant/runs/{run_id}/events",
        Permission::ProjectView,
        "SSE stream; re-checked while open",
    ),
    project(
        Verb::Get,
        "/projects/{project_id}/assistant/tools",
        Permission::ProjectView,
        "project-scoped read",
    ),
    project(
        Verb::Post,
        "/projects/{project_id}/assistant/tools/execute",
        Permission::AiUse,
        "assistant mutation",
    ),
    project(
        Verb::Post,
        "/projects/{project_id}/automations/block-details",
        Permission::ProjectView,
        "read-only block lookup",
    ),
    project(
        Verb::Get,
        "/projects/{project_id}/automations/definitions",
        Permission::ProjectView,
        "project-scoped read",
    ),
    project(
        Verb::Put,
        "/projects/{project_id}/automations/definitions/{recipe}",
        Permission::AutomationManage,
        "automation management",
    ),
    project(
        Verb::Post,
        "/projects/{project_id}/automations/recipes/{recipe}/runs",
        Permission::AutomationManage,
        "automation management",
    ),
    project(
        Verb::Get,
        "/projects/{project_id}/automations/runs",
        Permission::ProjectView,
        "project-scoped read",
    ),
    project(
        Verb::Post,
        "/projects/{project_id}/automations/runs",
        Permission::AutomationManage,
        "automation management",
    ),
    project(
        Verb::Get,
        "/projects/{project_id}/automations/runs/{run_id}",
        Permission::ProjectView,
        "project-scoped read",
    ),
    project(
        Verb::Get,
        "/projects/{project_id}/automations/workflow-files/{file_id}",
        Permission::ProjectView,
        "workflow file download",
    ),
    project(
        Verb::Get,
        "/projects/{project_id}/automations/workflow-runs",
        Permission::ProjectView,
        "project-scoped read",
    ),
    project(
        Verb::Get,
        "/projects/{project_id}/automations/workflow-runs/{run_id}",
        Permission::ProjectView,
        "project-scoped read",
    ),
    project(
        Verb::Post,
        "/projects/{project_id}/automations/workflow-runs/{run_id}/cancel",
        Permission::AutomationManage,
        "automation management",
    ),
    project(
        Verb::Get,
        "/projects/{project_id}/automations/workflows",
        Permission::ProjectView,
        "project-scoped read",
    ),
    project(
        Verb::Post,
        "/projects/{project_id}/automations/workflows",
        Permission::AutomationManage,
        "automation management",
    ),
    project(
        Verb::Post,
        "/projects/{project_id}/automations/workflows/from-template",
        Permission::AutomationManage,
        "automation management",
    ),
    project(
        Verb::Delete,
        "/projects/{project_id}/automations/workflows/{workflow_id}",
        Permission::AutomationManage,
        "automation management",
    ),
    project(
        Verb::Get,
        "/projects/{project_id}/automations/workflows/{workflow_id}",
        Permission::ProjectView,
        "project-scoped read",
    ),
    project(
        Verb::Put,
        "/projects/{project_id}/automations/workflows/{workflow_id}",
        Permission::AutomationManage,
        "automation management",
    ),
    project(
        Verb::Post,
        "/projects/{project_id}/automations/workflows/{workflow_id}/disable",
        Permission::AutomationManage,
        "automation management",
    ),
    project(
        Verb::Post,
        "/projects/{project_id}/automations/workflows/{workflow_id}/enable",
        Permission::AutomationManage,
        "automation management",
    ),
    project(
        Verb::Get,
        "/projects/{project_id}/automations/workflows/{workflow_id}/endpoints",
        Permission::ProjectView,
        "project-scoped read",
    ),
    project(
        Verb::Put,
        "/projects/{project_id}/automations/workflows/{workflow_id}/endpoints",
        Permission::AutomationManage,
        "automation management",
    ),
    project(
        Verb::Post,
        "/projects/{project_id}/automations/workflows/{workflow_id}/endpoints/email/rotate",
        Permission::AutomationManage,
        "automation management",
    ),
    project(
        Verb::Post,
        "/projects/{project_id}/automations/workflows/{workflow_id}/endpoints/webhook-secret/rotate",
        Permission::AutomationManage,
        "automation management",
    ),
    project(
        Verb::Post,
        "/projects/{project_id}/automations/workflows/{workflow_id}/endpoints/webhook/rotate",
        Permission::AutomationManage,
        "automation management",
    ),
    project(
        Verb::Post,
        "/projects/{project_id}/automations/workflows/{workflow_id}/publish",
        Permission::AutomationManage,
        "automation management",
    ),
    project(
        Verb::Get,
        "/projects/{project_id}/automations/workflows/{workflow_id}/runs",
        Permission::ProjectView,
        "project-scoped read",
    ),
    project(
        Verb::Post,
        "/projects/{project_id}/automations/workflows/{workflow_id}/runs",
        Permission::AutomationManage,
        "automation management",
    ),
    project(
        Verb::Post,
        "/projects/{project_id}/automations/workflows/{workflow_id}/test-runs",
        Permission::AutomationManage,
        "automation management",
    ),
    project(
        Verb::Post,
        "/projects/{project_id}/automations/workflows/{workflow_id}/validate",
        Permission::AutomationManage,
        "automation management",
    ),
    project(
        Verb::Get,
        "/projects/{project_id}/automations/workflows/{workflow_id}/versions",
        Permission::ProjectView,
        "project-scoped read",
    ),
    project(
        Verb::Get,
        "/projects/{project_id}/automations/workflows/{workflow_id}/versions/{version}",
        Permission::ProjectView,
        "project-scoped read",
    ),
    project(
        Verb::Get,
        "/projects/{project_id}/deduplication/proposals",
        Permission::ProjectView,
        "project-scoped read",
    ),
    project(
        Verb::Post,
        "/projects/{project_id}/deduplication/proposals/{proposal_id}/decision",
        Permission::ProjectEdit,
        "project-scoped mutation",
    ),
    project(
        Verb::Post,
        "/projects/{project_id}/deduplication/run",
        Permission::ProjectEdit,
        "project-scoped mutation",
    ),
    project(
        Verb::Get,
        "/projects/{project_id}/exports/{export_kind}",
        Permission::ProjectView,
        "generated export download",
    ),
    project(
        Verb::Get,
        "/projects/{project_id}/extraction/fields",
        Permission::ProjectView,
        "project-scoped read",
    ),
    project(
        Verb::Post,
        "/projects/{project_id}/extraction/fields",
        Permission::ProjectEdit,
        "project-scoped mutation",
    ),
    project(
        Verb::Patch,
        "/projects/{project_id}/extraction/fields/{field_id}",
        Permission::ProjectEdit,
        "project-scoped mutation",
    ),
    project(
        Verb::Get,
        "/projects/{project_id}/graph",
        Permission::ProjectView,
        "project-scoped read",
    ),
    project(
        Verb::Post,
        "/projects/{project_id}/imports",
        Permission::ProjectEdit,
        "project-scoped mutation",
    ),
    project(
        Verb::Post,
        "/projects/{project_id}/metrics/recompute",
        Permission::ProjectEdit,
        "project-scoped mutation",
    ),
    project(
        Verb::Get,
        "/projects/{project_id}/prisma",
        Permission::ProjectView,
        "project-scoped read",
    ),
    project(
        Verb::Get,
        "/projects/{project_id}/projection",
        Permission::ProjectView,
        "project-scoped read",
    ),
    project(
        Verb::Get,
        "/projects/{project_id}/protocol",
        Permission::ProjectView,
        "project-scoped read",
    ),
    project(
        Verb::Get,
        "/projects/{project_id}/recommendations",
        Permission::ProjectView,
        "project-scoped read",
    ),
    project(
        Verb::Post,
        "/projects/{project_id}/records/{record_id}/ai/deduplication",
        Permission::AiUse,
        "AI suggestion",
    ),
    project(
        Verb::Post,
        "/projects/{project_id}/records/{record_id}/resolution",
        Permission::ProjectEdit,
        "project-scoped mutation",
    ),
    project(
        Verb::Get,
        "/projects/{project_id}/reports",
        Permission::ProjectView,
        "project-scoped read",
    ),
    project(
        Verb::Get,
        "/projects/{project_id}/reports/{report_id}",
        Permission::ProjectView,
        "project-scoped read",
    ),
    project(
        Verb::Post,
        "/projects/{project_id}/reports/{report_id}/ai/appraisal-prefill",
        Permission::AiUse,
        "AI suggestion",
    ),
    project(
        Verb::Post,
        "/projects/{project_id}/reports/{report_id}/ai/screening",
        Permission::AiUse,
        "AI suggestion",
    ),
    project(
        Verb::Post,
        "/projects/{project_id}/reports/{report_id}/ai/study-grouping",
        Permission::AiUse,
        "AI suggestion",
    ),
    project(
        Verb::Get,
        "/projects/{project_id}/reports/{report_id}/appraisals",
        Permission::ProjectView,
        "project-scoped read",
    ),
    project(
        Verb::Post,
        "/projects/{project_id}/reports/{report_id}/appraisals",
        Permission::AppraisalPerform,
        "appraisal completion",
    ),
    project(
        Verb::Get,
        "/projects/{project_id}/reports/{report_id}/documents",
        Permission::ProjectView,
        "project-scoped read",
    ),
    project(
        Verb::Post,
        "/projects/{project_id}/reports/{report_id}/documents",
        Permission::ProjectEdit,
        "project-scoped mutation",
    ),
    project(
        Verb::Post,
        "/projects/{project_id}/reports/{report_id}/documents/external",
        Permission::ProjectEdit,
        "project-scoped mutation",
    ),
    project(
        Verb::Post,
        "/projects/{project_id}/reports/{report_id}/documents/open-access",
        Permission::ProjectEdit,
        "project-scoped mutation",
    ),
    project(
        Verb::Delete,
        "/projects/{project_id}/reports/{report_id}/documents/{document_id}",
        Permission::ProjectEdit,
        "project-scoped mutation",
    ),
    project(
        Verb::Get,
        "/projects/{project_id}/reports/{report_id}/documents/{document_id}",
        Permission::ProjectView,
        "project-scoped read",
    ),
    project(
        Verb::Get,
        "/projects/{project_id}/reports/{report_id}/documents/{document_id}/blocks",
        Permission::ProjectView,
        "project-scoped read",
    ),
    project(
        Verb::Get,
        "/projects/{project_id}/reports/{report_id}/documents/{document_id}/content",
        Permission::ProjectView,
        "full-text download",
    ),
    project(
        Verb::Post,
        "/projects/{project_id}/reports/{report_id}/documents/{document_id}/identity/acknowledge",
        Permission::ProjectEdit,
        "project-scoped mutation",
    ),
    project(
        Verb::Get,
        "/projects/{project_id}/reports/{report_id}/documents/{document_id}/pages",
        Permission::ProjectView,
        "project-scoped read",
    ),
    project(
        Verb::Get,
        "/projects/{project_id}/reports/{report_id}/documents/{document_id}/references",
        Permission::ProjectView,
        "project-scoped read",
    ),
    project(
        Verb::Post,
        "/projects/{project_id}/reports/{report_id}/documents/{document_id}/reparse",
        Permission::ProjectEdit,
        "project-scoped mutation",
    ),
    project(
        Verb::Get,
        "/projects/{project_id}/reports/{report_id}/documents/{document_id}/sections",
        Permission::ProjectView,
        "project-scoped read",
    ),
    project(
        Verb::Post,
        "/projects/{project_id}/reports/{report_id}/screening",
        Permission::ScreeningPerform,
        "screening decision",
    ),
    project(
        Verb::Get,
        "/projects/{project_id}/reports/{report_id}/screening/history",
        Permission::ProjectView,
        "project-scoped read",
    ),
    project(
        Verb::Post,
        "/projects/{project_id}/reports/{report_id}/screening/undo",
        Permission::ScreeningPerform,
        "screening decision",
    ),
    project(
        Verb::Get,
        "/projects/{project_id}/reports/{report_id}/study",
        Permission::ProjectView,
        "project-scoped read",
    ),
    project(
        Verb::Put,
        "/projects/{project_id}/reports/{report_id}/study",
        Permission::ProjectEdit,
        "project-scoped mutation",
    ),
    project(
        Verb::Get,
        "/projects/{project_id}/review-runs/{run_id}",
        Permission::ProjectView,
        "project-scoped read",
    ),
    project(
        Verb::Post,
        "/projects/{project_id}/review-runs/{run_id}/retry",
        Permission::AiUse,
        "retries an AI review run",
    ),
    project(
        Verb::Get,
        "/projects/{project_id}/review/protocol",
        Permission::ProjectView,
        "project-scoped read",
    ),
    project(
        Verb::Put,
        "/projects/{project_id}/review/protocol",
        Permission::ProjectEdit,
        "project-scoped mutation",
    ),
    project(
        Verb::Post,
        "/projects/{project_id}/review/protocol/publish",
        Permission::ProjectEdit,
        "project-scoped mutation",
    ),
    project(
        Verb::Get,
        "/projects/{project_id}/review/protocol/versions",
        Permission::ProjectView,
        "project-scoped read",
    ),
    project(
        Verb::Get,
        "/projects/{project_id}/screening",
        Permission::ProjectView,
        "project-scoped read",
    ),
    project(
        Verb::Get,
        "/projects/{project_id}/screening/full-text",
        Permission::ProjectView,
        "project-scoped read",
    ),
    project(
        Verb::Get,
        "/projects/{project_id}/screening/full-text/missing",
        Permission::ProjectView,
        "project-scoped read",
    ),
    project(
        Verb::Get,
        "/projects/{project_id}/screening/full-text/reasons",
        Permission::ProjectView,
        "project-scoped read",
    ),
    project(
        Verb::Get,
        "/projects/{project_id}/screening/next",
        Permission::ProjectView,
        "project-scoped read",
    ),
    project(
        Verb::Get,
        "/projects/{project_id}/screening/title-abstract",
        Permission::ProjectView,
        "project-scoped read",
    ),
    project(
        Verb::Get,
        "/projects/{project_id}/studies",
        Permission::ProjectView,
        "project-scoped read",
    ),
    project(
        Verb::Post,
        "/projects/{project_id}/studies",
        Permission::ProjectEdit,
        "project-scoped mutation",
    ),
    project(
        Verb::Get,
        "/projects/{project_id}/studies/ungrouped-reports",
        Permission::ProjectView,
        "project-scoped read",
    ),
    project(
        Verb::Get,
        "/projects/{project_id}/studies/{study_id}",
        Permission::ProjectView,
        "project-scoped read",
    ),
    project(
        Verb::Put,
        "/projects/{project_id}/studies/{study_id}",
        Permission::ProjectEdit,
        "project-scoped mutation",
    ),
    project(
        Verb::Post,
        "/projects/{project_id}/studies/{study_id}/ai/extraction",
        Permission::AiUse,
        "AI suggestion",
    ),
    project(
        Verb::Put,
        "/projects/{project_id}/studies/{study_id}/classification",
        Permission::ProjectEdit,
        "project-scoped mutation",
    ),
    project(
        Verb::Get,
        "/projects/{project_id}/studies/{study_id}/extraction",
        Permission::ProjectView,
        "project-scoped read",
    ),
    project(
        Verb::Delete,
        "/projects/{project_id}/studies/{study_id}/extraction/values/{field_id}",
        Permission::ExtractionPerform,
        "extraction value change",
    ),
    project(
        Verb::Put,
        "/projects/{project_id}/studies/{study_id}/extraction/values/{field_id}",
        Permission::ExtractionPerform,
        "extraction value change",
    ),
    project(
        Verb::Post,
        "/projects/{project_id}/studies/{study_id}/extraction/values/{value_id}/confirm",
        Permission::ExtractionPerform,
        "extraction value change",
    ),
    project(
        Verb::Get,
        "/projects/{project_id}/studies/{study_id}/history",
        Permission::ProjectView,
        "project-scoped read",
    ),
    authenticated(
        Verb::Get,
        "/settings",
        "deployment-wide defaults every user may read",
    ),
    platform_operator(
        Verb::Patch,
        "/settings",
        "deployment-wide settings are an operator concern; no user role may change them",
    ),
];

/// The row for a request, by method and the router's matched path template.
/// `HEAD` is answered by the router from the `GET` handler, so it takes the
/// `GET` row.
pub fn lookup(method: &str, matched_path: &str) -> Option<&'static RouteRule> {
    let method = if method.eq_ignore_ascii_case("HEAD") {
        "GET"
    } else {
        method
    };
    let verb = Verb::parse(method)?;
    ROUTES
        .iter()
        .find(|row| row.method == verb && row.path == matched_path)
}

/// A way to reach data that is not an ordinary HTTP route, with the control
/// that protects it and the test that proves the control.
#[derive(Debug, Clone, Copy)]
pub struct NonHttpRow {
    pub kind: &'static str,
    pub name: &'static str,
    pub control: &'static str,
    pub test: &'static str,
}

pub static NON_HTTP: &[NonHttpRow] = &[
    NonHttpRow {
        kind: "Operator CLI",
        name: "accounts complete-deletions and erasure-status",
        control: "Trusted deployment operator with database/storage credentials. Thirty-day request eligibility is rechecked under locks. Physical fulfillment waits for actual storage and staging acknowledgements; unknown upload outcomes stay pending. No HTTP tenant principal is inferred.",
        test: "privacy_erasure_postgres::full_thirty_day_cancellation_precedes_any_object_retirement",
    },
    NonHttpRow {
        kind: "Operator CLI",
        name: "accounts activate-erasure --confirm-drained <token>",
        control: "Default-false physical proof gate. Exact operator attestation requires actual older-writer drain and untracked staging cleanup; an elapsed deadline is never cleanup evidence. Restrict shell/database access to trusted operators.",
        test: "deepref-server command::tests::parses_erasure_operator_commands",
    },
    NonHttpRow {
        kind: "Operator CLI",
        name: "projection status <project_uuid>",
        control: "Trusted deployment operator with database credentials; no tenant HTTP principal is inferred. Scoped UUID reads projection metadata only; no mutations or tenant authority inferred. Restrict shell/database access to operators.",
        test: "deepref-server command::tests::parses_projection_commands",
    },
    NonHttpRow {
        kind: "Operator CLI",
        name: "projection rebuild <project_uuid>",
        control: "Trusted deployment operator with database credentials; scoped UUID starts a fenced repair generation through the durable outbox. Restrict shell/database access to operators; ordinary user HTTP requests use central PDP/PEP.",
        test: "deepref-server command::tests::parses_projection_commands",
    },
    NonHttpRow {
        kind: "SSE stream",
        name: "Assistant run events (`GET /projects/{project_id}/assistant/runs/{run_id}/events`)",
        control: "PEP requires `project:view` on connect. A resume is a new request (`after_seq`), so it is authorized again. An open stream re-checks `project:view` every `stream_recheck` (20 s by default) and ends when denied. The run is loaded with its `project_id` and a run of another project is a 404.",
        test: "authz_sse::revoked_member_stream_closes_and_resume_is_refused",
    },
    NonHttpRow {
        kind: "SSE stream",
        name: "Assistant chat reply (`POST /projects/{project_id}/assistant/chat`, `text/event-stream`)",
        control: "PEP requires `ai:use` before the body is read. The stream is one bounded turn started by that request and cannot be resumed; a reconnect is a new authorized request. No other route produces `text/event-stream`.",
        test: "authz_foreign_workspace::foreign_project_is_indistinguishable_from_a_missing_one",
    },
    NonHttpRow {
        kind: "Download",
        name: "Article full-text content (`GET .../documents/{document_id}/content`)",
        control: "PEP requires `project:view`. The document is loaded by project, report and document id together, so a document id of another project is a 404. Bytes stream through the API; no object key leaves the server.",
        test: "authz_foreign_workspace::foreign_project_is_indistinguishable_from_a_missing_one",
    },
    NonHttpRow {
        kind: "Download",
        name: "Workflow file (`GET .../automations/workflow-files/{file_id}`)",
        control: "PEP requires `project:view`; the file is loaded within the project.",
        test: "authz_foreign_workspace::foreign_project_is_indistinguishable_from_a_missing_one",
    },
    NonHttpRow {
        kind: "Export",
        name: "Generated project exports (`GET .../exports/{export_kind}`)",
        control: "PEP requires `project:view`; the export is generated from the path's project only and is not stored.",
        test: "authz_foreign_workspace::foreign_project_is_indistinguishable_from_a_missing_one",
    },
    NonHttpRow {
        kind: "Object access",
        name: "Document object-store keys",
        control: "Keys are opaque, derived from a document id, and never returned by the API. The API issues no signed or pre-signed URL; every read goes through a project-scoped route above. `validate_object_key` rejects traversal.",
        test: "deepref-documents unit test `object_keys_are_opaque_and_reject_traversal`",
    },
    NonHttpRow {
        kind: "Machine delivery",
        name: "Workflow webhook and inbound e-mail (`POST /hooks/workflows/{token}`, `POST /hooks/email/{token}`)",
        control: "Open at the PEP by design. The private address token is the capability; it resolves to one workflow and therefore one project, and the run starts as that workflow. Signature verification and a rate limit apply.",
        test: "workflows hook tests in `tests/automations_postgres.rs`",
    },
    NonHttpRow {
        kind: "Machine principal",
        name: "Worker, system and API-key principals",
        control: "`Principal::System` is trusted and never reaches the store; no HTTP request can produce it (`x-actor-*` headers are ignored for authority and cannot name `system`). `Principal::Service` is authorized by the same central PDP: live credential expiry/revocation and creator status, active workspace membership/current role, explicit key scopes and resource tenant must intersect. The HTTP PEP also restricts services to the explicit machine-safe read allowlist; services cannot impersonate human scientific actors.",
        test: "api_keys::lifecycle_is_human_only_and_tenant_scoped (positive service reads); deepref-application `system_is_trusted_and_unknown_service_principals_hold_nothing` (unknown-service denial)",
    },
    NonHttpRow {
        kind: "Job payload",
        name: "Queue handlers that load a project, report or document from a job payload",
        control: "Trusted after enqueue authorization. Every enqueue site in `http-api` runs inside a route the PEP authorized for that `project_id`; the remaining enqueue sites are the system (outbox dispatch, reconciler, workflow engine). The handlers run as System.",
        test: "authz_foreign_workspace::foreign_project_is_indistinguishable_from_a_missing_one (the enqueueing routes are Require rows)",
    },
    NonHttpRow {
        kind: "Notification inbox",
        name: "Notifications and their unread count",
        control: "Authenticated and user-scoped: lists, counts and mark-read include only rows of projects the caller can view now.",
        test: "authz_user_scoped::notifications_include_only_viewable_projects",
    },
];
