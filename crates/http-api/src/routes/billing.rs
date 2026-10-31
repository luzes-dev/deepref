//! Human-only hosted billing management. Links are bearer capabilities.
use crate::{
    auth::CurrentSession,
    error::{ApiError, ErrorResponse},
    state::AppState,
};
use axum::{
    Json,
    body::Bytes,
    extract::{Path, State},
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse, Response},
};
use deepref_app::billing::{BillingService, BillingStoreError};
use deepref_application::billing::{BillingContact, Offering, WebhookHeaders};
use deepref_domain::{UserId, WorkspaceId};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use uuid::Uuid;

fn actor(session: &crate::auth::SessionContext) -> Result<UserId, ApiError> {
    session
        .principal
        .user_id()
        .ok_or_else(|| ApiError::NotFound("resource not found".into()))
}
fn service(state: &AppState) -> Result<&BillingService, ApiError> {
    state
        .billing
        .as_ref()
        .ok_or_else(|| ApiError::Configuration("billing is unconfigured".into()))
}
fn error(e: BillingStoreError) -> ApiError {
    match e {
        BillingStoreError::Denied => ApiError::NotFound("resource not found".into()),
        BillingStoreError::Conflict => ApiError::Conflict {
            code: "BILLING_RECONCILIATION_REQUIRED".into(),
            message: "billing reconciliation required".into(),
            details: serde_json::json!({}),
        },
        BillingStoreError::Provider(deepref_application::billing::BillingError::Refused) => {
            ApiError::BadRequest("billing request refused".into())
        }
        _ => ApiError::Configuration("billing is unavailable".into()),
    }
}
fn protected<T: Serialize>(body: T) -> Response {
    (
        [
            (header::CACHE_CONTROL, "no-store"),
            (header::PRAGMA, "no-cache"),
        ],
        Json(body),
    )
        .into_response()
}

#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct BillingContactRequest {
    email: String,
    name: String,
}
#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct CheckoutRequest {
    /// Closed application offering, configured server-side. No provider product ID.
    offering: BillingOffering,
    /// Only /settings/billing or /settings.
    return_path: String,
}
#[derive(Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub(crate) enum BillingOffering {
    Primary,
    Secondary,
}
#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct PortalRequest {
    return_path: String,
}
#[derive(Serialize, ToSchema)]
pub(crate) struct HostedBillingLink {
    url: String,
}

#[utoipa::path(post,path="/workspaces/{workspace_id}/billing/customer",operation_id="createBillingCustomer",tag="billing",params(("workspace_id"=Uuid,Path)),request_body=BillingContactRequest,responses((status=204),(status=404,body=ErrorResponse),(status=409,body=ErrorResponse)))]
pub(crate) async fn provision(
    State(state): State<AppState>,
    Path(workspace): Path<Uuid>,
    CurrentSession(session): CurrentSession,
    Json(input): Json<BillingContactRequest>,
) -> Result<Response, ApiError> {
    let user = actor(&session)?;
    service(&state)?
        .provision(
            user,
            WorkspaceId::new(workspace),
            BillingContact::parse(input.email, input.name)
                .map_err(|_| ApiError::BadRequest("invalid billing contact".into()))?,
        )
        .await
        .map_err(error)?;
    Ok((
        StatusCode::NO_CONTENT,
        [(header::CACHE_CONTROL, "no-store")],
    )
        .into_response())
}
#[utoipa::path(post,path="/workspaces/{workspace_id}/billing/checkout",operation_id="createBillingCheckout",tag="billing",params(("workspace_id"=Uuid,Path)),request_body=CheckoutRequest,responses((status=200,body=HostedBillingLink),(status=404,body=ErrorResponse),(status=409,body=ErrorResponse)))]
pub(crate) async fn checkout(
    State(state): State<AppState>,
    Path(workspace): Path<Uuid>,
    CurrentSession(session): CurrentSession,
    Json(input): Json<CheckoutRequest>,
) -> Result<Response, ApiError> {
    let user = actor(&session)?;
    let offering = match input.offering {
        BillingOffering::Primary => Offering::Primary,
        BillingOffering::Secondary => Offering::Secondary,
    };
    let link = service(&state)?
        .checkout(
            user,
            WorkspaceId::new(workspace),
            offering,
            &input.return_path,
        )
        .await
        .map_err(error)?;
    Ok(protected(HostedBillingLink {
        url: link.expose().into(),
    }))
}
#[utoipa::path(post,path="/workspaces/{workspace_id}/billing/portal",operation_id="createBillingPortal",tag="billing",params(("workspace_id"=Uuid,Path)),request_body=PortalRequest,responses((status=200,body=HostedBillingLink),(status=404,body=ErrorResponse),(status=409,body=ErrorResponse)))]
pub(crate) async fn portal(
    State(state): State<AppState>,
    Path(workspace): Path<Uuid>,
    CurrentSession(session): CurrentSession,
    Json(input): Json<PortalRequest>,
) -> Result<Response, ApiError> {
    let user = actor(&session)?;
    let link = service(&state)?
        .portal(user, WorkspaceId::new(workspace), &input.return_path)
        .await
        .map_err(error)?;
    Ok(protected(HostedBillingLink {
        url: link.expose().into(),
    }))
}
#[utoipa::path(post,path="/hooks/billing/dodo",operation_id="receiveDodoWebhook",tag="billing",request_body(content=String,content_type="application/json"),responses((status=204),(status=400,body=ErrorResponse)))]
pub(crate) async fn webhook(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<StatusCode, ApiError> {
    let header_value = |name: &str| -> Result<&str, ApiError> {
        let mut values = headers.get_all(name).iter();
        let value = values
            .next()
            .and_then(|v| v.to_str().ok())
            .ok_or_else(|| ApiError::BadRequest("invalid billing webhook".into()))?;
        if values.next().is_some() {
            return Err(ApiError::BadRequest("invalid billing webhook".into()));
        }
        Ok(value)
    };
    let headers = WebhookHeaders {
        id: header_value("webhook-id")?,
        timestamp: header_value("webhook-timestamp")?,
        signatures: header_value("webhook-signature")?,
    };
    deepref_app::billing::receive(service(&state)?, &body, headers)
        .await
        .map_err(|e| match e {
            BillingStoreError::Provider(_) => {
                ApiError::BadRequest("invalid billing webhook".into())
            }
            _ => ApiError::Configuration("billing receipt unavailable".into()),
        })?;
    Ok(StatusCode::NO_CONTENT)
}
