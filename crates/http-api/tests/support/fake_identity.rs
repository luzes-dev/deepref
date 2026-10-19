#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]
//! A stand-in identity provider. No test reaches WorkOS.
//!
//! Sealed sessions are opaque tokens the fake issued, so a string it never
//! issued is a tampered cookie. Expiry and refresh-token rotation are modelled:
//! a refresh trades a token for a new one exactly once.
use std::{
    collections::HashMap,
    sync::{
        Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
};

use deepref_application::identity::{
    IdentityError, IdentityFuture, IdentityProvider, LoginStart, LoginState, OpaqueSecret,
    ProviderLogin, ProviderProfile, ProviderSession, RefreshOutcome, SessionStatus,
};
use deepref_domain::ExternalUserId;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Lifetime {
    Live,
    Expired,
    Rotated,
}

#[derive(Default)]
struct Inner {
    sealed: HashMap<String, (ProviderSession, Lifetime)>,
    codes: HashMap<String, (ProviderSession, ProviderProfile)>,
}

#[derive(Default)]
pub struct FakeIdentity {
    inner: Mutex<Inner>,
    counter: AtomicUsize,
    pub revoked: Mutex<Vec<String>>,
    pub refresh_calls: AtomicUsize,
    /// How long a refresh takes at the provider before it answers.
    pub refresh_delay: Mutex<std::time::Duration>,
    /// While set, `inspect_session` fails as it does in a provider outage.
    pub inspect_unavailable: AtomicBool,
    pub fail_exchange: Mutex<Option<IdentityError>>,
    pub extra_sessions: Mutex<Vec<String>>,
}

impl FakeIdentity {
    fn next(&self, prefix: &str) -> String {
        format!("{prefix}-{}", self.counter.fetch_add(1, Ordering::SeqCst))
    }

    fn session(user: &str, session_id: &str) -> ProviderSession {
        ProviderSession {
            session_id: session_id.to_owned(),
            external_user_id: ExternalUserId::parse(user).unwrap(),
        }
    }

    /// A one-time authorization code that exchanges for a session of `user`.
    pub fn issue_code(&self, user: &str, email: &str, name: Option<&str>) -> String {
        let code = self.next("code");
        let session = Self::session(user, &self.next("sess"));
        let profile = ProviderProfile {
            email: email.to_owned(),
            display_name: name.map(str::to_owned),
        };
        self.inner
            .lock()
            .unwrap()
            .codes
            .insert(code.clone(), (session, profile));
        code
    }

    /// A live sealed session, as if the person had already signed in.
    pub fn mint_session(&self, user: &str) -> String {
        self.mint(Self::session(user, &self.next("sess")), Lifetime::Live)
    }

    pub fn mint_expired_session(&self, user: &str) -> String {
        self.mint(Self::session(user, &self.next("sess")), Lifetime::Expired)
    }

    /// An expired session whose refresh token the provider already spent.
    pub fn mint_revoked_session(&self, user: &str) -> String {
        self.mint(Self::session(user, &self.next("sess")), Lifetime::Rotated)
    }

    fn mint(&self, session: ProviderSession, lifetime: Lifetime) -> String {
        let sealed = self.next("sealed");
        self.inner
            .lock()
            .unwrap()
            .sealed
            .insert(sealed.clone(), (session, lifetime));
        sealed
    }

    pub fn provider_session_id(&self, sealed: &str) -> String {
        self.inner.lock().unwrap().sealed[sealed]
            .0
            .session_id
            .clone()
    }
}

impl IdentityProvider for FakeIdentity {
    fn begin_login(&self, redirect_uri: &str) -> Result<LoginStart, IdentityError> {
        let state = self.next("state");
        Ok(LoginStart {
            authorization_url: format!(
                "https://auth.fake.example/authorize?redirect_uri={redirect_uri}&state={state}"
            ),
            state,
            code_verifier: OpaqueSecret::new(self.next("verifier")),
        })
    }

    fn complete_login<'a>(
        &'a self,
        code: &'a str,
        _: &'a OpaqueSecret,
    ) -> IdentityFuture<'a, ProviderLogin> {
        Box::pin(async move {
            if let Some(error) = self.fail_exchange.lock().unwrap().take() {
                return Err(error);
            }
            let (session, profile) = self
                .inner
                .lock()
                .unwrap()
                .codes
                .remove(code)
                .ok_or(IdentityError::Rejected)?;
            let sealed = self.mint(session.clone(), Lifetime::Live);
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
            if self.inspect_unavailable.load(Ordering::SeqCst) {
                return Err(IdentityError::Unavailable);
            }
            Ok(
                match self.inner.lock().unwrap().sealed.get(sealed.expose()) {
                    Some((session, Lifetime::Live)) => SessionStatus::Active(session.clone()),
                    Some((session, Lifetime::Expired | Lifetime::Rotated)) => {
                        SessionStatus::Expired(session.clone())
                    }
                    None => SessionStatus::Invalid,
                },
            )
        })
    }

    fn refresh_session<'a>(
        &'a self,
        sealed: &'a OpaqueSecret,
    ) -> IdentityFuture<'a, RefreshOutcome> {
        Box::pin(async move {
            self.refresh_calls.fetch_add(1, Ordering::SeqCst);
            let delay = *self.refresh_delay.lock().unwrap();
            if !delay.is_zero() {
                tokio::time::sleep(delay).await;
            }
            let session = {
                let mut inner = self.inner.lock().unwrap();
                match inner.sealed.get_mut(sealed.expose()) {
                    Some((session, lifetime @ Lifetime::Expired)) => {
                        *lifetime = Lifetime::Rotated;
                        session.clone()
                    }
                    _ => return Ok(RefreshOutcome::Rejected),
                }
            };
            let renewed = self.mint(session.clone(), Lifetime::Live);
            Ok(RefreshOutcome::Refreshed {
                sealed_session: OpaqueSecret::new(renewed),
                session,
            })
        })
    }

    fn revoke_session<'a>(&'a self, session_id: &'a str) -> IdentityFuture<'a, ()> {
        Box::pin(async move {
            self.revoked.lock().unwrap().push(session_id.to_owned());
            Ok(())
        })
    }

    fn list_session_ids<'a>(&'a self, _: &'a ExternalUserId) -> IdentityFuture<'a, Vec<String>> {
        Box::pin(async move { Ok(self.extra_sessions.lock().unwrap().clone()) })
    }

    fn seal_login_state(&self, state: &LoginState) -> Result<OpaqueSecret, IdentityError> {
        let json = serde_json::to_vec(state).unwrap();
        Ok(OpaqueSecret::new(
            json.iter().map(|b| format!("{b:02x}")).collect::<String>(),
        ))
    }

    fn open_login_state(&self, sealed: &str) -> Option<LoginState> {
        let bytes: Option<Vec<u8>> = (0..sealed.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(sealed.get(i..i + 2)?, 16).ok())
            .collect();
        serde_json::from_slice(&bytes?).ok()
    }
}
