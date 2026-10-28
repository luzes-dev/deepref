//! Human settings discovery. Capability flags are advisory; writes recheck PDP.
use crate::{
    auth::CurrentSession,
    error::{ApiError, ErrorResponse},
    state::AppState,
};
use axum::{
    Json,
    extract::{Query, State},
    http::header,
    response::{IntoResponse, Response},
};
use deepref_app::authorization::PostgresAuthorizationStore;
use deepref_application::authorization::{Authorizer, AuthzError, PolicyDecisionPoint};
use deepref_domain::{
    Principal, WorkspaceId,
    authorization::{Permission, Resource},
};
use serde::{Deserialize, Serialize};
use sqlx::Row;
use utoipa::{IntoParams, ToSchema};
use uuid::Uuid;

const PAGE_SIZE: usize = 25;
#[derive(Deserialize, IntoParams)]
pub(crate) struct WorkspacePageQuery {
    /// Exclusive workspace UUID cursor. Ordering is stable by UUID, not name.
    after: Option<Uuid>,
}
#[derive(Serialize, ToSchema)]
pub(crate) struct SettingsWorkspace {
    id: Uuid,
    name: String,
    /// Advisory UI hint; every key operation independently reauthorizes.
    can_manage_keys: bool,
    /// Advisory UI hint; every billing operation independently reauthorizes.
    can_manage_billing: bool,
}
#[derive(Serialize, ToSchema)]
pub(crate) struct SettingsWorkspacePage {
    items: Vec<SettingsWorkspace>,
    /// Follow to continue. An empty page can still have a cursor after revocation.
    next_cursor: Option<Uuid>,
}

async fn permitted(
    authorizer: &impl Authorizer,
    principal: &Principal,
    permission: Permission,
    resource: Resource,
) -> Result<bool, ApiError> {
    match authorizer.require(principal, permission, resource).await {
        Ok(()) => Ok(true),
        Err(AuthzError::Denied(_)) => Ok(false),
        Err(error) => Err(ApiError::Internal(error.into())),
    }
}

#[utoipa::path(get, path="/me/workspaces", operation_id="listSettingsWorkspaces", tag="auth",params(WorkspacePageQuery),
    responses((status=200,body=SettingsWorkspacePage),(status=400,body=ErrorResponse),(status=401,body=ErrorResponse)))]
pub(crate) async fn list_workspaces(
    State(state): State<AppState>,
    CurrentSession(session): CurrentSession,
    Query(page): Query<WorkspacePageQuery>,
) -> Result<Response, ApiError> {
    let user = session
        .principal
        .user_id()
        .ok_or_else(|| ApiError::Forbidden("human session required".into()))?;
    // No transaction/pool connection is held while PDP reads. max_connections=1
    // works without nested acquisition. No provider or credential fields leave.
    let rows=sqlx::query("SELECT w.id,w.name FROM workspaces w JOIN workspace_memberships m ON m.workspace_id=w.id JOIN users u ON u.id=m.user_id WHERE m.user_id=$1 AND m.status='active' AND u.status='active' AND u.deleted_at IS NULL AND ($2::uuid IS NULL OR w.id>$2) ORDER BY w.id LIMIT 26")
        .bind(user.as_uuid()).bind(page.after).fetch_all(&state.pool).await?;
    let more = rows.len() > PAGE_SIZE;
    let authorizer = PolicyDecisionPoint::new(PostgresAuthorizationStore::new(state.pool.clone()));
    let mut items = Vec::with_capacity(PAGE_SIZE);
    let mut next_cursor = None;
    for row in rows.into_iter().take(PAGE_SIZE) {
        let id: Uuid = row.try_get("id")?;
        if more {
            next_cursor = Some(id);
        }
        let resource = Resource::Workspace(WorkspaceId::new(id));
        if !permitted(
            &authorizer,
            &session.principal,
            Permission::WorkspaceView,
            resource,
        )
        .await?
        {
            continue;
        }
        let can_manage_keys = permitted(
            &authorizer,
            &session.principal,
            Permission::ApiKeyManage,
            resource,
        )
        .await?;
        let can_manage_billing = permitted(
            &authorizer,
            &session.principal,
            Permission::WorkspaceBilling,
            resource,
        )
        .await?;
        items.push(SettingsWorkspace {
            id,
            name: row.try_get("name")?,
            can_manage_keys,
            can_manage_billing,
        });
    }
    Ok((
        [(header::CACHE_CONTROL, "no-store")],
        Json(SettingsWorkspacePage { items, next_cursor }),
    )
        .into_response())
}
