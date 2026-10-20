#![allow(
    clippy::expect_used,
    clippy::panic,
    clippy::unwrap_used,
    clippy::string_slice
)]
mod support;

#[test]
fn local_runtime_has_bounded_pools_and_timeouts() {
    let runtime = support::local_runtime();
    assert!(runtime.database.pool_min <= runtime.database.pool_max);
    assert!(!runtime.database.acquire_timeout.is_zero());
    assert!(!runtime.telemetry.shutdown_deadline.is_zero());
}

fn api_config(values: &[(&str, &str)]) -> anyhow::Result<deepref_http_api::config::ApiConfig> {
    let map: std::collections::HashMap<String, String> = values
        .iter()
        .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
        .collect();
    deepref_http_api::config::ApiConfig::from_source(&|name| map.get(name).cloned())
}

const PRODUCTION_BASE: &[(&str, &str)] = &[
    ("APP_ENV", "production"),
    ("DATABASE_URL", "postgres://u:p@db.internal:5432/deepref"),
    ("DEEPREF_PUBLIC_ORIGIN", "https://app.example.com"),
    ("DOCUMENT_STORAGE_BACKEND", "local"),
    ("DOCUMENT_STORAGE_ROOT", "/tmp/deepref-docs"),
    ("WORKOS_CLIENT_ID", "client_01"),
    ("WORKOS_API_KEY", "sk_test_key"),
    (
        "WORKOS_COOKIE_PASSWORD",
        "a-cookie-password-of-at-least-32-characters",
    ),
    ("WORKOS_WEBHOOK_SECRET", "whsec"),
];

#[test]
fn production_refuses_to_start_with_authorization_bypassed() {
    let error = api_config(PRODUCTION_BASE).expect_err("auth is off");
    assert!(
        error.to_string().contains("DEEPREF_AUTH_ENABLED"),
        "{error}"
    );
    let mut enabled = PRODUCTION_BASE.to_vec();
    enabled.push(("DEEPREF_AUTH_ENABLED", "true"));
    if let Err(error) = api_config(&enabled) {
        assert!(
            !error
                .to_string()
                .contains("DEEPREF_AUTH_ENABLED must be on"),
            "auth on must not trip the bypass guard: {error}"
        );
    }
}

#[test]
fn development_may_run_without_authentication() {
    let error = api_config(&[("APP_ENV", "development")]).err();
    assert!(
        error.is_none_or(|error| !error
            .to_string()
            .contains("DEEPREF_AUTH_ENABLED must be on")),
        "development keeps the local-dev bypass"
    );
}
