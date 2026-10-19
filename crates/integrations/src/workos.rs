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
    AuthKitAuthorizationUrlParams, AuthKitPkceCodeExchangeParams, Client, JwksHelper,
    RevokeSession, SessionData, SessionState,
    resources::user_management::{
        AuthenticateWithRefreshTokenParams, ListSessionsParams, RevokeSessionParams,
    },
};

const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);
const SESSION_SEAL_PURPOSE: &str = "deepref/workos-session-seal/v1";
const LOGIN_STATE_SEAL_PURPOSE: &str = "deepref/workos-login-state-seal/v1";
const SESSION_LIST_LIMIT: i64 = 100;

pub struct WorkOsIdentityProvider {
    client: Client,
    /// Uncached view of the signing keys. The SDK reports a JWKS outage and a
    /// bad signature with the same `invalid_jwt` reason, so this tells them
    /// apart.
    jwks_probe: JwksHelper,
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
        let client = builder.build();
        Self {
            jwks_probe: client.jwks().with_ttl(Duration::ZERO),
            client,
            session_key: key_hex(config.cookie_password(), SESSION_SEAL_PURPOSE),
            login_state_key: key_hex(config.cookie_password(), LOGIN_STATE_SEAL_PURPOSE),
        }
    }

    async fn authenticate(&self, sealed: &str) -> SessionState {
        self.client
            .session(sealed, self.session_key.as_str())
            .authenticate()
            .await
    }

    /// Reads a sealed session. `Invalid` is decided only against signing keys
    /// this adapter fetched and checked itself: the SDK reports a failed key
    /// fetch and a bad signature with the same `invalid_jwt` reason, so that
    /// reason is never trusted on its own. Any key-fetch failure is
    /// `Unavailable`, so a WorkOS outage never reads as an invalid session.
    async fn read_session(&self, sealed: &str) -> Result<SessionState, IdentityError> {
        let state = self.authenticate(sealed).await;
        if state.reason != "invalid_jwt" {
            return Ok(state);
        }
        let keys = match self.jwks_probe.fetch().await {
            Ok(keys) => keys,
            Err(error) => {
                tracing::warn!(%error, "WorkOS signing keys are unavailable");
                return Err(IdentityError::Unavailable);
            }
        };
        let Ok(session) = workos::unseal_session(sealed, &self.session_key) else {
            return Ok(state);
        };
        let token = session.access_token.expose();
        let signed_by_provider = signature_claims(token, &keys).is_some_and(|claims| {
            session
                .user
                .as_ref()
                .is_none_or(|user| user.id == claims.sub)
        });
        if !signed_by_provider {
            // Verified against keys we hold: genuinely not a valid session.
            return Ok(state);
        }
        // The token verifies, so the SDK's failure was its own key fetch.
        // Give it one more read; if it still cannot decide, keep the session.
        let second = self.authenticate(sealed).await;
        if second.reason == "invalid_jwt" {
            tracing::warn!("WorkOS session verification failed on a validly signed token");
            return Err(IdentityError::Unavailable);
        }
        Ok(second)
    }
}

#[derive(serde::Deserialize)]
struct SignedClaims {
    sub: String,
}

/// The claims of `token` if its RS256 signature verifies against `keys`.
/// Expiry is not judged here: an expired but genuine token must still read as
/// one that needs a refresh.
fn signature_claims(token: &str, keys: &workos::JwkSet) -> Option<SignedClaims> {
    use jsonwebtoken::{Algorithm, DecodingKey, Validation};
    let header = jsonwebtoken::decode_header(token).ok()?;
    if header.alg != Algorithm::RS256 {
        return None;
    }
    let kid = header.kid.filter(|kid| !kid.is_empty())?;
    let mut matching = keys
        .keys
        .iter()
        .filter(|key| key.kid.as_deref() == Some(kid.as_str()));
    let key = matching.next()?;
    if matching.next().is_some()
        || key.kty.as_deref() != Some("RSA")
        || key.alg.as_deref().is_some_and(|alg| alg != "RS256")
        || key.use_.as_deref().is_some_and(|usage| usage != "sig")
    {
        return None;
    }
    let jwk = serde_json::from_value(serde_json::to_value(key).ok()?).ok()?;
    let decoding_key = DecodingKey::from_jwk(&jwk).ok()?;
    let mut validation = Validation::new(Algorithm::RS256);
    validation.set_required_spec_claims(&["exp", "sub"]);
    validation.validate_exp = false;
    validation.validate_nbf = true;
    validation.validate_aud = false;
    let claims = jsonwebtoken::decode::<SignedClaims>(token, &decoding_key, &validation)
        .ok()?
        .claims;
    (!claims.sub.is_empty()).then_some(claims)
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
            let state = self.read_session(&sealed).await?;
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
            let state = self.read_session(sealed.expose()).await?;
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
            let state = self.read_session(sealed.expose()).await?;
            if !state.needs_refresh {
                return Ok(RefreshOutcome::Rejected);
            }
            let Ok(cookie) = workos::unseal_session(sealed.expose(), &self.session_key) else {
                return Ok(RefreshOutcome::Rejected);
            };
            if cookie.refresh_token.expose().is_empty() {
                return Ok(RefreshOutcome::Rejected);
            }
            let mut params = AuthenticateWithRefreshTokenParams::new(cookie.refresh_token.expose());
            // The organization comes from the signature-checked claims.
            params.organization_id = Some(state.organization_id).filter(|id| !id.is_empty());
            let response = match self
                .client
                .user_management()
                .authenticate_with_refresh_token(params)
                .await
            {
                Ok(response) => response,
                // WorkOS answers a spent or revoked refresh token with
                // `invalid_grant`, documented as HTTP 400 and seen as 401. Only
                // that answer ends the session; anything else may be transient.
                Err(error)
                    if error.code() == Some("invalid_grant")
                        && matches!(error.status(), Some(400 | 401)) =>
                {
                    return Ok(RefreshOutcome::Rejected);
                }
                Err(error) => {
                    tracing::warn!(
                        status = ?error.status(),
                        code = ?error.code(),
                        "WorkOS session refresh failed"
                    );
                    return Err(IdentityError::Unavailable);
                }
            };
            let sealed_session = workos::seal_session(
                &SessionData {
                    access_token: response.access_token,
                    refresh_token: response.refresh_token,
                    user: Some(response.user),
                    impersonator: response.impersonator,
                },
                &self.session_key,
            )
            .map_err(|_| IdentityError::Internal("could not seal the session".to_owned()))?;
            let state = self.read_session(&sealed_session).await?;
            Ok(
                match provider_session(&state).filter(|_| state.authenticated) {
                    Some(session) => RefreshOutcome::Refreshed {
                        sealed_session: OpaqueSecret::new(sealed_session),
                        session,
                    },
                    None => RefreshOutcome::Rejected,
                },
            )
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
