#![allow(
    clippy::expect_used,
    clippy::panic,
    clippy::unwrap_used,
    clippy::string_slice
)]
// Each test binary uses a different part of this module.
#![allow(dead_code)]
use std::collections::HashMap;

pub mod fake_identity;

pub fn local_runtime() -> deepref_config::RuntimeConfig {
    runtime_with(&[])
}

/// A development runtime with extra environment variables.
pub fn runtime_with(extra: &[(&str, &str)]) -> deepref_config::RuntimeConfig {
    let mut values = HashMap::from([("APP_ENV".to_owned(), "development".to_owned())]);
    values.extend(
        extra
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned())),
    );
    deepref_config::RuntimeConfig::from_map("deepref-api-test", &values).unwrap()
}

pub const ORIGIN: &str = "https://app.example.com";
pub const SESSION_COOKIE: &str = "__Host-deepref_session";
pub const LOGIN_COOKIE: &str = "__Host-deepref_login";
pub const COOKIE_PASSWORD: &str = "a-cookie-password-of-at-least-32-characters";

/// The API router with authentication on and `fake` as the identity provider.
/// The https origin selects the production cookie names and attributes.
pub fn auth_app(
    pool: sqlx::PgPool,
    fake: std::sync::Arc<fake_identity::FakeIdentity>,
) -> axum::Router {
    let runtime = runtime_with(&[
        ("DEEPREF_AUTH_ENABLED", "true"),
        ("DEEPREF_PUBLIC_ORIGIN", ORIGIN),
    ]);
    let auth =
        deepref_http_api::auth::AuthRuntime::new(fake, &runtime.public_origin, COOKIE_PASSWORD);
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
    deepref_http_api::routes::router(
        deepref_http_api::state::AppState::core(pool).with_auth(auth),
        &config,
    )
}

/// A pool that never connects, for requests that must be decided before any query.
pub fn unused_pool() -> sqlx::PgPool {
    sqlx::postgres::PgPoolOptions::new()
        .connect_lazy("postgres://postgres:postgres@127.0.0.1:1/unused")
        .unwrap()
}
