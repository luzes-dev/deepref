#![allow(
    clippy::expect_used,
    clippy::panic,
    clippy::unwrap_used,
    clippy::string_slice
)]
use std::collections::HashMap;

use deepref_config::RuntimeConfig;

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
