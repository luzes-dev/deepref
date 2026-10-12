#![allow(
    clippy::expect_used,
    clippy::panic,
    clippy::unwrap_used,
    clippy::string_slice
)]
use std::collections::HashMap;

use deepref_config::{ConfigError, Environment, ListenConfig, RuntimeConfig};

#[test]
fn local_has_disposable_defaults() {
    let values = HashMap::from([("APP_ENV".to_owned(), "local".to_owned())]);
    let config = RuntimeConfig::from_map("test", &values).unwrap();
    assert!(config.database.url.contains("127.0.0.1"));
    assert!(!config.database.acquire_timeout.is_zero());
}

#[test]
fn hosted_database_is_fail_fast() {
    let values = HashMap::from([("APP_ENV".to_owned(), "production".to_owned())]);
    assert_eq!(
        RuntimeConfig::from_map("test", &values)
            .unwrap_err()
            .to_string(),
        "DATABASE_URL is required when APP_ENV=Production"
    );
}

#[test]
fn direct_and_file_secrets_are_exclusive() {
    let values = HashMap::from([
        ("APP_ENV".to_owned(), "local".to_owned()),
        ("DATABASE_URL".to_owned(), "postgres://direct".to_owned()),
        ("DATABASE_URL_FILE".to_owned(), "/unused".to_owned()),
    ]);
    assert!(RuntimeConfig::from_map("test", &values).is_err());
}

#[test]
fn unset_app_env_is_production_and_fails_fast() {
    let values = HashMap::new();
    let error = RuntimeConfig::from_map("test", &values).unwrap_err();
    assert!(
        matches!(
            error,
            ConfigError::Missing("DATABASE_URL", Environment::Production)
        ),
        "unexpected result: {error}"
    );
}

fn listen(pairs: &[(&str, &str)]) -> Result<ListenConfig, ConfigError> {
    let values = pairs
        .iter()
        .map(|(name, value)| ((*name).to_owned(), (*value).to_owned()))
        .collect::<HashMap<_, _>>();
    ListenConfig::from_map(&values)
}

#[test]
fn api_binds_loopback_by_default() {
    let config = listen(&[]).unwrap();
    assert_eq!(config.bind_addr, "127.0.0.1:8080".parse().unwrap());
}

#[test]
fn network_bind_without_opt_in_refuses_to_start() {
    for addr in ["0.0.0.0:8080", "[::]:8080", "192.168.1.20:8080"] {
        let error = listen(&[("API_BIND_ADDR", addr)]).unwrap_err();
        assert!(
            error.to_string().contains("DEEPREF_ALLOW_NETWORK_BIND"),
            "{addr}: unexpected error {error}"
        );
    }
}

#[test]
fn network_bind_with_explicit_opt_in_is_allowed() {
    let config = listen(&[
        ("API_BIND_ADDR", "0.0.0.0:8080"),
        ("DEEPREF_ALLOW_NETWORK_BIND", "1"),
    ])
    .unwrap();
    assert_eq!(config.bind_addr, "0.0.0.0:8080".parse().unwrap());
    assert!(config.allow_network_bind);
    assert!(listen(&[("API_BIND_ADDR", "[::1]:9000")]).is_ok());
    assert!(
        listen(&[
            ("API_BIND_ADDR", "0.0.0.0:8080"),
            ("DEEPREF_ALLOW_NETWORK_BIND", "maybe"),
        ])
        .is_err()
    );
}
