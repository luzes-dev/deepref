//! WorkOS AuthKit behind the [`IdentityProvider`] port.
//!
//! Built on the official `workos` crate: PKCE and `state` generation, the code
//! exchange, JWKS-verified access tokens, refresh-token rotation and AES-GCM
//! sealed sessions all come from it. Its DTOs stop in this module. The sealed
//! session is the browser cookie, so access and refresh tokens are encrypted
//! at rest in the cookie and never reach browser storage.
//!
//! The sealing key is derived from `WORKOS_COOKIE_PASSWORD` per purpose, so the
//! session cookie, the login-state cookie and the audit session hash use
//! unrelated keys, and the password may be any string of 32 or more characters.

use deepref_application::identity::{
    IdentityError, IdentityFuture, IdentityProvider, LoginStart, LoginState, OpaqueSecret,
    ProviderLogin, ProviderProfile, ProviderSession, RefreshOutcome, SessionStatus, derive_key,
};
use deepref_config::WorkOsConfig;
use deepref_domain::ExternalUserId;
use std::{fmt::Write as _, time::Duration};
use workos::{
    AuthKitAuthorizationUrlParams, AuthKitPkceCodeExchangeParams, Client, RevokeSession,
    SessionData, SessionRefreshOptions, SessionState,
    resources::user_management::{ListSessionsParams, RevokeSessionParams},
};

const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);
const SESSION_SEAL_PURPOSE: &str = "deepref/workos-session-seal/v1";
const LOGIN_STATE_SEAL_PURPOSE: &str = "deepref/workos-login-state-seal/v1";
const SESSION_LIST_LIMIT: i64 = 100;

pub struct WorkOsIdentityProvider {
    client: Client,
    session_key: String,
    login_state_key: String,
}

impl WorkOsIdentityProvider {
    pub fn new(config: &WorkOsConfig) -> Self {
        Self::build(config, None)
    }

    /// Points the client at another API host. Tests use it to stand in for
    /// WorkOS; production code calls [`Self::new`].
    pub fn with_base_url(config: &WorkOsConfig, base_url: &str) -> Self {
        Self::build(config, Some(base_url))
    }

    fn build(config: &WorkOsConfig, base_url: Option<&str>) -> Self {
        let mut builder = Client::builder()
            .api_key(config.api_key())
            .client_id(config.client_id.as_str())
            .timeout(REQUEST_TIMEOUT)
            .max_retries(1);
        if let Some(base_url) = base_url {
            builder = builder.base_url(base_url);
        }
        Self {
            client: builder.build(),
            session_key: key_hex(config.cookie_password(), SESSION_SEAL_PURPOSE),
            login_state_key: key_hex(config.cookie_password(), LOGIN_STATE_SEAL_PURPOSE),
        }
    }

    async fn read_session(&self, sealed: &str) -> SessionState {
        self.client
            .session(sealed, self.session_key.as_str())
            .authenticate()
            .await
    }
}

fn key_hex(password: &str, purpose: &str) -> String {
    derive_key(password, purpose)
        .iter()
        .fold(String::with_capacity(64), |mut out, byte| {
            let _ = write!(out, "{byte:02x}");
            out
        })
}

/// Maps a provider failure to the port's error without carrying its message,
/// which can name tokens, codes or internal request detail.
fn classify(error: &workos::Error) -> IdentityError {
    match error.status() {
        Some(400..=499) if error.status() != Some(429) => IdentityError::Rejected,
        _ => IdentityError::Unavailable,
    }
}

fn provider_session(state: &SessionState) -> Option<ProviderSession> {
    if state.session_id.is_empty() {
        return None;
    }
    let user = state.user.as_ref()?;
    Some(ProviderSession {
        session_id: state.session_id.clone(),
        external_user_id: ExternalUserId::parse(user.id.as_str()).ok()?,
    })
}

impl IdentityProvider for WorkOsIdentityProvider {
    fn begin_login(&self, redirect_uri: &str) -> Result<LoginStart, IdentityError> {
        let started = self
            .client
            .authkit()
            .pkce_authorization_url(AuthKitAuthorizationUrlParams {
                redirect_uri: redirect_uri.to_owned(),
                provider: Some("authkit".to_owned()),
                ..Default::default()
            })
            .map_err(|error| {
                tracing::error!(%error, "could not build the WorkOS authorization URL");
                IdentityError::Internal("could not build the authorization URL".to_owned())
            })?;
        Ok(LoginStart {
            authorization_url: started.url,
            state: started.state,
            code_verifier: OpaqueSecret::new(started.code_verifier),
        })
    }

    fn complete_login<'a>(
        &'a self,
        code: &'a str,
        code_verifier: &'a OpaqueSecret,
    ) -> IdentityFuture<'a, ProviderLogin> {
        Box::pin(async move {
            let response = self
                .client
                .authkit()
                .pkce_code_exchange(AuthKitPkceCodeExchangeParams {
                    code: code.to_owned(),
                    code_verifier: code_verifier.expose().to_owned(),
                })
                .await
                .map_err(|error| {
                    tracing::warn!(
                        status = ?error.status(),
                        code = ?error.code(),
                        "WorkOS code exchange failed"
                    );
                    classify(&error)
                })?;
            let profile = ProviderProfile {
                email: response.user.email.clone(),
                display_name: response
                    .user
                    .name
                    .clone()
                    .filter(|name| !name.trim().is_empty()),
            };
            let sealed = workos::seal_session(
                &SessionData {
                    access_token: response.access_token,
                    refresh_token: response.refresh_token,
                    user: Some(response.user),
                    impersonator: response.impersonator,
                },
                &self.session_key,
            )
            .map_err(|_| IdentityError::Internal("could not seal the session".to_owned()))?;
            // The session ID comes from the signature-checked access token, not
            // from the exchange response body.
            let state = self.read_session(&sealed).await;
            let session = state
                .authenticated
                .then(|| provider_session(&state))
                .flatten()
                .ok_or(IdentityError::Rejected)?;
            Ok(ProviderLogin {
                sealed_session: OpaqueSecret::new(sealed),
                session,
                profile,
            })
        })
    }

    fn inspect_session<'a>(
        &'a self,
        sealed: &'a OpaqueSecret,
    ) -> IdentityFuture<'a, SessionStatus> {
        Box::pin(async move {
            let state = self.read_session(sealed.expose()).await;
            let Some(session) = provider_session(&state) else {
                return Ok(SessionStatus::Invalid);
            };
            Ok(if state.authenticated {
                SessionStatus::Active(session)
            } else if state.needs_refresh {
                SessionStatus::Expired(session)
            } else {
                SessionStatus::Invalid
            })
        })
    }

    fn refresh_session<'a>(
        &'a self,
        sealed: &'a OpaqueSecret,
    ) -> IdentityFuture<'a, RefreshOutcome> {
        Box::pin(async move {
            let refreshed = self
                .client
                .session(sealed.expose(), self.session_key.as_str())
                .refresh(SessionRefreshOptions::default())
                .await
                .map_err(|_| IdentityError::Unavailable)?;
            if refreshed.authenticated {
                let state = self.read_session(&refreshed.sealed_session).await;
                return match provider_session(&state).filter(|_| state.authenticated) {
                    Some(session) => Ok(RefreshOutcome::Refreshed {
                        sealed_session: OpaqueSecret::new(refreshed.sealed_session),
                        session,
                    }),
                    None => Ok(RefreshOutcome::Rejected),
                };
            }
            match refreshed.reason.as_str() {
                "refresh_failed" => Err(IdentityError::Unavailable),
                _ => Ok(RefreshOutcome::Rejected),
            }
        })
    }

    fn revoke_session<'a>(&'a self, session_id: &'a str) -> IdentityFuture<'a, ()> {
        Box::pin(async move {
            self.client
                .user_management()
                .revoke_session(RevokeSessionParams::new(RevokeSession {
                    session_id: session_id.to_owned(),
                }))
                .await
                .map_err(|error| classify(&error))
        })
    }

    fn list_session_ids<'a>(&'a self, user: &'a ExternalUserId) -> IdentityFuture<'a, Vec<String>> {
        Box::pin(async move {
            let page = self
                .client
                .user_management()
                .list_sessions(
                    user.as_str(),
                    ListSessionsParams {
                        limit: Some(SESSION_LIST_LIMIT),
                        ..Default::default()
                    },
                )
                .await
                .map_err(|error| classify(&error))?;
            Ok(page.data.into_iter().map(|session| session.id).collect())
        })
    }

    fn seal_login_state(&self, state: &LoginState) -> Result<OpaqueSecret, IdentityError> {
        workos::seal(state, &self.login_state_key)
            .map(OpaqueSecret::new)
            .map_err(|_| IdentityError::Internal("could not seal the login state".to_owned()))
    }

    fn open_login_state(&self, sealed: &str) -> Option<LoginState> {
        workos::unseal(sealed, &self.login_state_key).ok()
    }
}
