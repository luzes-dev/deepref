//! Sign-in, sign-out and the current-session endpoint.
//!
//! The browser flow: `/auth/login` seals a one-time `state` and PKCE verifier in
//! a short-lived cookie and redirects to the provider; `/auth/callback` checks
//! `state` against that cookie, exchanges the code, provisions the DeepRef
//! user, sets the session cookie and returns the browser to a same-origin
//! path. These routes never read `x-actor-*` headers.

use axum::{
    Json,
    extract::{Query, State},
    http::{HeaderMap, HeaderValue, StatusCode, header},
    response::{IntoResponse, Redirect, Response},
};
use deepref_app::account_deletion::{self, AccountDeletionError};
use deepref_app::auth_rejections::RejectionReason;
use deepref_app::identity::{AuditRequest, NewBrowserSession, SignIn, record_logout, sign_in};
use deepref_application::identity::{
    IdentityError, LoginState, OpaqueSecret, ReturnPath, SessionStatus,
};
use deepref_domain::UserId;
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};
use uuid::Uuid;

use crate::{
    auth::{
        AuthRuntime, CurrentSession, LOGIN_COOKIE_MAX_AGE_SECS, SESSION_COOKIE_MAX_AGE_SECS,
        constant_time_eq, read_cookie,
    },
    error::{ApiError, ApiErrorBody, AuthFailure},
    state::AppState,
};

#[derive(Debug, Serialize, ToSchema)]
pub struct CurrentUserResponse {
    pub id: Uuid,
    pub email: String,
    pub display_name: Option<String>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct MeResponse {
    /// False when the deployment does not require sign-in; `user` is then null.
    pub auth_enabled: bool,
    pub user: Option<CurrentUserResponse>,
}

#[derive(Debug, Deserialize, IntoParams)]
pub struct LoginQuery {
    /// A path on this application to return to after sign-in, such as `/projects`.
    return_to: Option<String>,
}

#[derive(Debug, Deserialize, IntoParams)]
pub struct CallbackQuery {
    code: Option<String>,
    state: Option<String>,
    error: Option<String>,
}

fn auth_runtime(state: &AppState) -> Result<&AuthRuntime, ApiError> {
    state
        .auth
        .as_ref()
        .ok_or_else(|| ApiError::NotFound("authentication is not enabled".to_owned()))
}

fn no_store(mut response: Response) -> Response {
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

fn correlation_id() -> Option<String> {
    crate::error::REQUEST_CORRELATION_ID
        .try_with(|id| id.to_string())
        .ok()
}

#[utoipa::path(get, path = "/me", operation_id = "getCurrentUser", tag = "auth",
    responses(
        (status = 200, body = MeResponse),
        (status = 401, body = ApiErrorBody, description = "No valid session"),
    ))]
pub async fn get_me(
    State(state): State<AppState>,
    session: Option<CurrentSession>,
) -> Result<Response, ApiError> {
    if state.auth.is_none() {
        return Ok(no_store(
            Json(MeResponse {
                auth_enabled: false,
                user: None,
            })
            .into_response(),
        ));
    }
    let CurrentSession(session) = session.ok_or(ApiError::Auth(AuthFailure::Unauthenticated))?;
    Ok(no_store(
        Json(MeResponse {
            auth_enabled: true,
            user: Some(CurrentUserResponse {
                id: session.user.id.as_uuid(),
                email: session.user.email,
                display_name: session.user.display_name,
            }),
        })
        .into_response(),
    ))
}

#[utoipa::path(get, path = "/auth/login", operation_id = "startLogin", tag = "auth",
    params(LoginQuery),
    responses(
        (status = 303, description = "Redirect to the sign-in provider"),
        (status = 400, body = ApiErrorBody, description = "return_to is not a path on this application"),
        (status = 404, body = ApiErrorBody, description = "Authentication is not enabled"),
    ))]
pub async fn login(
    State(state): State<AppState>,
    Query(query): Query<LoginQuery>,
) -> Result<Response, ApiError> {
    let auth = auth_runtime(&state)?;
    let return_to = match query.return_to.as_deref() {
        None | Some("") => ReturnPath::root(),
        Some(raw) => {
            ReturnPath::parse(raw).map_err(|error| ApiError::BadRequest(error.to_string()))?
        }
    };
    let started = auth
        .provider()
        .begin_login(&auth.callback_url())
        .map_err(|error| {
            tracing::error!(%error, "could not start sign-in");
            ApiError::Auth(AuthFailure::ProviderUnavailable)
        })?;
    let sealed = auth
        .provider()
        .seal_login_state(&LoginState {
            state: started.state,
            code_verifier: started.code_verifier,
            return_to,
            expires_at_unix: chrono::Utc::now().timestamp() + LOGIN_COOKIE_MAX_AGE_SECS,
        })
        .map_err(|error| ApiError::Internal(anyhow::anyhow!(error)))?;
    let cookie = auth
        .cookies()
        .set(
            auth.cookies().login_name(),
            sealed.expose(),
            LOGIN_COOKIE_MAX_AGE_SECS,
        )
        .ok_or_else(|| ApiError::Internal(anyhow::anyhow!("login cookie is not a valid header")))?;
    let mut response = Redirect::to(&started.authorization_url).into_response();
    response.headers_mut().append(header::SET_COOKIE, cookie);
    Ok(no_store(response))
}

/// Why a callback was refused, as the sign-in page shows it. Codes carry no
/// provider detail.
#[derive(Debug, Clone, Copy)]
enum CallbackError {
    InvalidState,
    AccessDenied,
    InvalidCode,
    ProviderUnavailable,
    ProvisioningFailed,
    AccountDeactivated,
}

impl CallbackError {
    const fn code(self) -> &'static str {
        match self {
            Self::InvalidState => "invalid_state",
            Self::AccessDenied => "access_denied",
            Self::InvalidCode => "invalid_code",
            Self::ProviderUnavailable => "provider_unavailable",
            Self::ProvisioningFailed => "provisioning_failed",
            Self::AccountDeactivated => "account_deactivated",
        }
    }

    /// The closed audit vocabulary for the same refusal (11 Part B).
    const fn rejection_reason(self) -> RejectionReason {
        match self {
            Self::InvalidState => RejectionReason::CallbackInvalidState,
            Self::AccessDenied => RejectionReason::CallbackAccessDenied,
            Self::InvalidCode => RejectionReason::CallbackInvalidCode,
            Self::ProviderUnavailable => RejectionReason::CallbackProviderUnavailable,
            Self::ProvisioningFailed => RejectionReason::CallbackProvisioningFailed,
            Self::AccountDeactivated => RejectionReason::CallbackAccountDeactivated,
        }
    }
}

async fn callback_failure(state: &AppState, auth: &AuthRuntime, error: CallbackError) -> Response {
    tracing::warn!(reason = error.code(), "sign-in callback refused");
    state
        .record_auth_rejection(error.rejection_reason(), None)
        .await;
    let mut response =
        Redirect::to(&format!("{}/signin?error={}", auth.origin(), error.code())).into_response();
    if let Some(cookie) = auth.cookies().clear(auth.cookies().login_name()) {
        response.headers_mut().append(header::SET_COOKIE, cookie);
    }
    no_store(response)
}

#[utoipa::path(get, path = "/auth/callback", operation_id = "completeLogin", tag = "auth",
    params(CallbackQuery),
    responses(
        (status = 303, description = "Redirect to the requested page, or to /signin with an error code"),
        (status = 404, body = ApiErrorBody, description = "Authentication is not enabled"),
    ))]
pub async fn callback(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<CallbackQuery>,
) -> Result<Response, ApiError> {
    let auth = auth_runtime(&state)?;
    let fail = |error| callback_failure(&state, auth, error);

    let login_state = read_cookie(&headers, auth.cookies().login_name())
        .and_then(|sealed| auth.provider().open_login_state(sealed))
        .filter(|login| login.expires_at_unix > chrono::Utc::now().timestamp());
    let Some(login_state) = login_state else {
        return Ok(fail(CallbackError::InvalidState).await);
    };
    let state_matches = query.state.as_deref().is_some_and(|returned| {
        constant_time_eq(returned.as_bytes(), login_state.state.as_bytes())
    });
    if !state_matches {
        return Ok(fail(CallbackError::InvalidState).await);
    }
    if query.error.is_some() {
        return Ok(fail(CallbackError::AccessDenied).await);
    }
    let Some(code) = query.code.as_deref().filter(|code| !code.is_empty()) else {
        return Ok(fail(CallbackError::InvalidCode).await);
    };

    let login = match auth
        .provider()
        .complete_login(code, &login_state.code_verifier)
        .await
    {
        Ok(login) => login,
        Err(IdentityError::Rejected) => return Ok(fail(CallbackError::InvalidCode).await),
        Err(IdentityError::Unavailable) => {
            return Ok(fail(CallbackError::ProviderUnavailable).await);
        }
        Err(IdentityError::Internal(_)) => return Ok(fail(CallbackError::ProvisioningFailed).await),
    };
    let cookie_token = Uuid::new_v4().to_string();
    let Some(session_cookie) = auth.cookies().set(
        auth.cookies().session_name(),
        &cookie_token,
        SESSION_COOKIE_MAX_AGE_SECS,
    ) else {
        return Ok(fail(CallbackError::ProvisioningFailed).await);
    };
    let cookie_token_hash = auth.session_tokens().token_hash(&cookie_token);
    let session_ref = auth.session_refs().session_ref(&login.session.session_id);
    let request_id = correlation_id();
    let outcome = sign_in(
        &state.pool,
        &login.session,
        &login.profile,
        NewBrowserSession {
            token_hash: &cookie_token_hash,
            sealed_provider_session: &login.sealed_session,
            session_ref: &session_ref,
            expires_at: chrono::Utc::now() + chrono::Duration::seconds(SESSION_COOKIE_MAX_AGE_SECS),
        },
        AuditRequest {
            request_id: request_id.as_deref(),
            session_ref: Some(&session_ref),
        },
    )
    .await;
    match outcome {
        Ok(SignIn::Active { .. }) => {}
        Ok(SignIn::Deactivated) => return Ok(fail(CallbackError::AccountDeactivated).await),
        Err(error) => {
            tracing::error!(%error, "could not provision the signed-in user");
            return Ok(fail(CallbackError::ProvisioningFailed).await);
        }
    }
    let mut response = Redirect::to(&format!(
        "{}{}",
        auth.origin(),
        login_state.return_to.as_str()
    ))
    .into_response();
    response
        .headers_mut()
        .append(header::SET_COOKIE, session_cookie);
    if let Some(cookie) = auth.cookies().clear(auth.cookies().login_name()) {
        response.headers_mut().append(header::SET_COOKIE, cookie);
    }
    Ok(no_store(response))
}

#[utoipa::path(post, path = "/auth/logout", operation_id = "logout", tag = "auth",
    responses(
        (status = 204, description = "Signed out; the session cookie is cleared"),
        (status = 403, body = ApiErrorBody, description = "Cross-origin request"),
    ))]
pub async fn logout(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    let auth = auth_runtime(&state)?;
    // The local row is revoked from the cookie alone. Whether the provider can
    // currently validate the session (an outage, a key rotation) must not leave
    // a signed-out session usable or unaudited.
    let cookie_token = read_cookie(&headers, auth.cookies().session_name())
        .filter(|value| Uuid::parse_str(value).is_ok());
    if let Some(cookie_token) = cookie_token {
        let request_id = correlation_id();
        let revoked = record_logout(
            &state.pool,
            &auth.session_tokens().token_hash(cookie_token),
            AuditRequest {
                request_id: request_id.as_deref(),
                session_ref: None,
            },
        )
        .await?;
        if let Some(revoked) = revoked {
            revoke_at_provider(auth, &revoked.sealed_provider_session).await;
        }
    }
    let mut response = StatusCode::NO_CONTENT.into_response();
    if let Some(cookie) = auth.cookies().clear(auth.cookies().session_name()) {
        response.headers_mut().append(header::SET_COOKIE, cookie);
    }
    Ok(no_store(response))
}

/// Best effort: the local revocation already ended the session.
async fn revoke_at_provider(auth: &AuthRuntime, sealed: &OpaqueSecret) {
    let session_id = match auth.provider().inspect_session(sealed).await {
        Ok(SessionStatus::Active(session) | SessionStatus::Expired(session)) => session.session_id,
        Ok(SessionStatus::Invalid) => return,
        Err(error) => {
            tracing::warn!(%error, "could not look up the provider session at sign-out");
            return;
        }
    };
    if let Err(error) = auth.provider().revoke_session(&session_id).await {
        tracing::warn!(%error, "could not revoke the provider session at sign-out");
    }
}

/// The account whose deletion the caller manages: always their own. Without
/// authentication (local development) there is no account to delete.
fn account_owner(state: &AppState, session: Option<&CurrentSession>) -> Result<UserId, ApiError> {
    if state.auth.is_none() {
        return Err(ApiError::NotFound(
            "authentication is not enabled".to_owned(),
        ));
    }
    let CurrentSession(context) = session.ok_or(ApiError::Auth(AuthFailure::Unauthenticated))?;
    context
        .principal
        .user_id()
        .ok_or_else(|| ApiError::NotFound("resource not found".to_owned()))
}

fn deletion_error(error: AccountDeletionError) -> ApiError {
    match error {
        AccountDeletionError::Database(error) => ApiError::Database(error),
        AccountDeletionError::UnknownUser | AccountDeletionError::Inactive => {
            ApiError::NotFound("resource not found".to_owned())
        }
        AccountDeletionError::Workspace(error) => ApiError::Internal(anyhow::anyhow!(error)),
        AccountDeletionError::DeletionDigest => {
            ApiError::Internal(anyhow::anyhow!("account deletion digest failed"))
        }
        // The completion has been claimed (or already ran): the deletion can
        // no longer be cancelled, and pretending otherwise would be a lie.
        AccountDeletionError::CompletionInFlight | AccountDeletionError::AlreadyCompleted => {
            ApiError::Conflict {
                code: "DELETION_IRREVERSIBLE".to_owned(),
                message: "the account deletion can no longer be cancelled".to_owned(),
                details: serde_json::json!({}),
            }
        }
    }
}

#[utoipa::path(post, path = "/me/deletion-request", operation_id = "requestAccountDeletion",
    tag = "auth",
    responses(
        (status = 202, description = "Deletion requested; completion runs 30 days later"),
        (status = 401, body = ApiErrorBody, description = "No valid session"),
        (status = 404, body = ApiErrorBody, description = "Authentication is not enabled"),
    ))]
pub async fn request_account_deletion(
    State(state): State<AppState>,
    session: Option<CurrentSession>,
) -> Result<StatusCode, ApiError> {
    let user = account_owner(&state, session.as_ref())?;
    let request_id = correlation_id();
    account_deletion::request_account_deletion(
        &state.pool,
        user,
        AuditRequest {
            request_id: request_id.as_deref(),
            session_ref: None,
        },
    )
    .await
    .map_err(deletion_error)?;
    Ok(StatusCode::ACCEPTED)
}

#[utoipa::path(delete, path = "/me/deletion-request", operation_id = "cancelAccountDeletion",
    tag = "auth",
    responses(
        (status = 204, description = "Any pending deletion request is cancelled"),
        (status = 401, body = ApiErrorBody, description = "No valid session"),
        (status = 404, body = ApiErrorBody, description = "Authentication is not enabled"),
        (status = 409, body = ApiErrorBody, description = "The deletion is already completing or complete and can no longer be cancelled"),
    ))]
pub async fn cancel_account_deletion(
    State(state): State<AppState>,
    session: Option<CurrentSession>,
) -> Result<StatusCode, ApiError> {
    let user = account_owner(&state, session.as_ref())?;
    let request_id = correlation_id();
    account_deletion::cancel_account_deletion(
        &state.pool,
        user,
        AuditRequest {
            request_id: request_id.as_deref(),
            session_ref: None,
        },
    )
    .await
    .map_err(deletion_error)?;
    Ok(StatusCode::NO_CONTENT)
}
