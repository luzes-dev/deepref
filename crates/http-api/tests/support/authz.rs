#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]
// Each test binary uses a different part of this module.
#![allow(dead_code)]
//! Two (or more) signed-in people against the real router and PostgreSQL, for
//! the authorization tests. The identity provider is a fake.

use std::sync::Arc;

use axum::{
    Router,
    body::Body,
    http::{Method, Request, StatusCode, header},
};
use deepref_app::identity::AuditRequest;
use deepref_domain::{UserId, WorkspaceId};
use sqlx::PgPool;
use tower::ServiceExt;
use uuid::Uuid;

use super::{
    LOGIN_COOKIE, ORIGIN, SESSION_COOKIE, auth_app_with, auth_runtime, fake_identity::FakeIdentity,
};

/// The correlation id every harness request carries, so two responses to the
/// same denial are byte-identical.
pub const CORRELATION: &str = "00000000-0000-4000-8000-000000000042";

pub struct Harness {
    pub app: Router,
    pub pool: PgPool,
    pub fake: Arc<FakeIdentity>,
}

pub struct Person {
    pub cookie: String,
    pub user: UserId,
    pub workspace: WorkspaceId,
}

pub async fn database() -> Option<PgPool> {
    deepref_app::test_isolation::isolated_pool_from_env("DATABASE_URL", 8).await
}

impl Harness {
    pub async fn new() -> Option<Self> {
        let pool = database().await?;
        Some(Self::on(pool))
    }

    pub fn on(pool: PgPool) -> Self {
        let fake = Arc::new(FakeIdentity::default());
        let app = auth_app_with(
            pool.clone(),
            auth_runtime(Arc::clone(&fake), std::time::Duration::from_secs(5)),
        );
        Self { app, pool, fake }
    }

    /// A harness whose router is built from a state `tweak` adjusts (an
    /// injected authorizer, a short stream interval).
    pub fn with_state(
        pool: PgPool,
        tweak: impl FnOnce(deepref_http_api::state::AppState) -> deepref_http_api::state::AppState,
    ) -> Self {
        let fake = Arc::new(FakeIdentity::default());
        let runtime = super::runtime_with(&[
            ("DEEPREF_AUTH_ENABLED", "true"),
            ("DEEPREF_PUBLIC_ORIGIN", ORIGIN),
        ]);
        let config = deepref_http_api::config::ApiConfig {
            documents: deepref_config::DocumentConfig::from_source(&|name| {
                (name == "APP_ENV").then(|| "development".into())
            })
            .unwrap(),
            runtime,
            bind_addr: "127.0.0.1:8080".parse().unwrap(),
            cors_allow_any: false,
            cors_origins: vec![ORIGIN.parse().unwrap()],
        };
        let state = deepref_http_api::state::AppState::core(pool.clone()).with_auth(auth_runtime(
            Arc::clone(&fake),
            std::time::Duration::from_secs(5),
        ));
        let app = deepref_http_api::routes::router(tweak(state), &config);
        Self { app, pool, fake }
    }

    /// As [`Harness::on`], with a router built from `state`.
    pub fn with_app(pool: PgPool, fake: Arc<FakeIdentity>, app: Router) -> Self {
        Self { app, pool, fake }
    }

    /// Runs the browser sign-in flow. A first sign-in creates the user and a
    /// personal workspace they own.
    pub async fn sign_in(&self, external: &str, email: &str) -> Person {
        let login = self
            .app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/auth/login?return_to=/projects")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(login.status(), StatusCode::SEE_OTHER);
        let login_cookie = cookie_value(&login, LOGIN_COOKIE).expect("login cookie");
        let location = login.headers()[header::LOCATION].to_str().unwrap();
        let state = location.split("state=").nth(1).unwrap().to_owned();
        let code = self.fake.issue_code(external, email, Some("Test Person"));
        let callback = self
            .app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!("/auth/callback?code={code}&state={state}"))
                    .header(header::COOKIE, format!("{LOGIN_COOKIE}={login_cookie}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(callback.status(), StatusCode::SEE_OTHER);
        let cookie = cookie_value(&callback, SESSION_COOKIE).expect("session cookie");
        let user: Uuid = sqlx::query_scalar("SELECT id FROM users WHERE workos_user_id = $1")
            .bind(external)
            .fetch_one(&self.pool)
            .await
            .unwrap();
        let workspace: Uuid = sqlx::query_scalar(
            "SELECT workspace_id FROM workspace_memberships WHERE user_id = $1 AND role = 'owner'",
        )
        .bind(user)
        .fetch_one(&self.pool)
        .await
        .unwrap();
        Person {
            cookie,
            user: UserId::new(user),
            workspace: WorkspaceId::new(workspace),
        }
    }

    /// Sends one request as `person`. Mutating methods carry a `{}` JSON body
    /// unless `body` is given.
    pub async fn send(
        &self,
        person: &Person,
        method: Method,
        path: &str,
        body: Option<&str>,
    ) -> (StatusCode, Vec<u8>) {
        let has_body = !matches!(method, Method::GET | Method::DELETE);
        let mut request = Request::builder()
            .method(method)
            .uri(path)
            .header(
                header::COOKIE,
                format!("{SESSION_COOKIE}={}", person.cookie),
            )
            .header(header::ORIGIN, ORIGIN)
            .header("x-correlation-id", CORRELATION);
        let payload = if has_body || body.is_some() {
            request = request.header(header::CONTENT_TYPE, "application/json");
            Body::from(body.unwrap_or("{}").to_owned())
        } else {
            Body::empty()
        };
        let response = self
            .app
            .clone()
            .oneshot(request.body(payload).unwrap())
            .await
            .unwrap();
        let status = response.status();
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        (status, bytes.to_vec())
    }

    pub async fn get(&self, person: &Person, path: &str) -> (StatusCode, Vec<u8>) {
        self.send(person, Method::GET, path, None).await
    }

    pub async fn json(
        &self,
        person: &Person,
        method: Method,
        path: &str,
        body: &str,
    ) -> (StatusCode, serde_json::Value) {
        let (status, bytes) = self.send(person, method, path, Some(body)).await;
        (
            status,
            serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null),
        )
    }

    /// Creates a project in the person's own workspace through the API.
    pub async fn create_project(&self, person: &Person, name: &str) -> Uuid {
        let (status, body) = self
            .json(
                person,
                Method::POST,
                "/projects",
                &format!(r#"{{"name":"{name}"}}"#),
            )
            .await;
        assert_eq!(status, StatusCode::CREATED, "{body}");
        Uuid::parse_str(body["id"].as_str().unwrap()).unwrap()
    }

    /// Adds `member` to `owner`'s workspace with `role`.
    pub async fn add_member(
        &self,
        owner: &Person,
        member: &Person,
        role: deepref_domain::WorkspaceRole,
    ) {
        deepref_app::workspaces::add_workspace_member(
            &self.pool,
            owner.user,
            owner.workspace,
            member.user,
            role,
            AuditRequest::default(),
        )
        .await
        .unwrap();
    }
}

pub fn cookie_value(response: &axum::http::Response<Body>, name: &str) -> Option<String> {
    response
        .headers()
        .get_all(header::SET_COOKIE)
        .iter()
        .find_map(|cookie| {
            let (pair, _) = cookie.to_str().ok()?.split_once(';')?;
            let (key, value) = pair.split_once('=')?;
            (key == name && !value.is_empty()).then(|| value.to_owned())
        })
}
