#![allow(
    clippy::expect_used,
    clippy::panic,
    clippy::unwrap_used,
    clippy::string_slice
)]
use std::collections::HashMap;

use deepref_config::{
    ApiSettings, ConfigError, DeletionDigestConfig, DocumentConfig, DocumentStorageBackend,
    Environment, ExposeSecret, ListenConfig, PolarServer, RuntimeConfig,
};

fn map(pairs: &[(&str, &str)]) -> HashMap<String, String> {
    pairs
        .iter()
        .map(|(name, value)| ((*name).to_owned(), (*value).to_owned()))
        .collect()
}

fn runtime(pairs: &[(&str, &str)]) -> Result<RuntimeConfig, ConfigError> {
    RuntimeConfig::from_map("test", &map(pairs))
}

#[test]
fn deletion_digest_key_is_required_in_production_and_redacted() {
    let production = DeletionDigestConfig::from_source(&|name| match name {
        "APP_ENV" => Some("production".into()),
        _ => None,
    });
    assert!(matches!(
        production,
        Err(ConfigError::Missing(
            "DEEPREF_DELETION_DIGEST_KEY",
            Environment::Production
        ))
    ));

    let secret = "production-deletion-digest-key-with-more-than-32-bytes";
    let configured = DeletionDigestConfig::from_source(&|name| match name {
        "APP_ENV" => Some("production".into()),
        "DEEPREF_DELETION_DIGEST_KEY" => Some(secret.into()),
        _ => None,
    })
    .unwrap();
    assert_eq!(configured.expose_secret(), secret);
    assert!(!format!("{configured:?}").contains(secret));
}

#[test]
fn deletion_digest_development_default_is_development_only() {
    let development = DeletionDigestConfig::from_source(&|name| {
        (name == "APP_ENV").then(|| "development".into())
    })
    .unwrap();
    assert_eq!(
        development.expose_secret(),
        "deepref-development-only-deletion-digest-key-not-for-production"
    );

    let short_key = DeletionDigestConfig::from_source(&|name| match name {
        "APP_ENV" => Some("development".into()),
        "DEEPREF_DELETION_DIGEST_KEY" => Some("too-short".into()),
        _ => None,
    });
    assert!(short_key.is_err());
}

/// The smallest valid production configuration.
const PRODUCTION: [(&str, &str); 3] = [
    ("APP_ENV", "production"),
    ("DATABASE_URL", "postgres://app:pw@db.internal/deepref"),
    ("DEEPREF_PUBLIC_ORIGIN", "https://app.deepref.example"),
];

fn production_with(extra: &[(&str, &str)]) -> Result<RuntimeConfig, ConfigError> {
    let mut pairs = PRODUCTION.to_vec();
    pairs.extend_from_slice(extra);
    runtime(&pairs)
}

const WORKOS: [(&str, &str); 4] = [
    ("WORKOS_CLIENT_ID", "client_01"),
    ("WORKOS_API_KEY", "sk_live_workos-api-secret"),
    (
        "WORKOS_COOKIE_PASSWORD",
        "workos-cookie-secret-0123456789abcdef",
    ),
    ("WORKOS_WEBHOOK_SECRET", "workos-webhook-secret"),
];

const POLAR: [(&str, &str); 3] = [
    ("POLAR_ACCESS_TOKEN", "polar-access-secret"),
    ("POLAR_WEBHOOK_SECRET", "polar-webhook-secret"),
    ("POLAR_SERVER", "production"),
];

#[test]
fn missing_app_env_is_an_error_and_never_development() {
    let error = runtime(&[]).unwrap_err();
    assert!(matches!(error, ConfigError::AppEnvMissing), "{error}");
    assert!(matches!(
        runtime(&[("APP_ENV", "  ")]).unwrap_err(),
        ConfigError::AppEnvMissing
    ));
    // Even a fully valid production-shaped config does not resolve without it.
    let without_env: Vec<_> = PRODUCTION
        .iter()
        .copied()
        .filter(|(name, _)| *name != "APP_ENV")
        .collect();
    assert!(matches!(
        runtime(&without_env).unwrap_err(),
        ConfigError::AppEnvMissing
    ));
}

#[test]
fn local_and_staging_are_rejected_with_their_replacement() {
    let local = runtime(&[("APP_ENV", "local")]).unwrap_err().to_string();
    assert!(
        local.contains("APP_ENV") && local.contains("use development"),
        "{local}"
    );
    let staging = runtime(&[("APP_ENV", "staging")]).unwrap_err().to_string();
    assert!(
        staging.contains("APP_ENV") && staging.contains("use production"),
        "{staging}"
    );
}

#[test]
fn development_has_disposable_defaults() {
    let config = runtime(&[("APP_ENV", "development")]).unwrap();
    assert_eq!(config.environment, Environment::Development);
    assert!(config.database.url().contains("127.0.0.1"));
    assert!(!config.database.acquire_timeout.is_zero());
    assert_eq!(config.public_origin.as_str(), "http://127.0.0.1:5173");
    assert!(!config.auth_enabled);
    assert!(config.workos.is_none() && config.polar.is_none());
    assert_eq!(
        runtime(&[("APP_ENV", "dev")]).unwrap().environment,
        Environment::Development
    );
}

#[test]
fn production_requires_explicit_hosted_settings() {
    let config = production_with(&[]).unwrap();
    assert_eq!(config.environment, Environment::Production);
    assert_eq!(config.public_origin.as_str(), "https://app.deepref.example");
    assert_eq!(
        config.database.url(),
        "postgres://app:pw@db.internal/deepref"
    );

    let missing_database: Vec<_> = PRODUCTION
        .iter()
        .copied()
        .filter(|(name, _)| *name != "DATABASE_URL")
        .collect();
    assert_eq!(
        runtime(&missing_database).unwrap_err().to_string(),
        "DATABASE_URL is required when APP_ENV=Production"
    );
}

#[test]
fn production_refuses_development_only_settings() {
    let missing_origin: Vec<_> = PRODUCTION
        .iter()
        .copied()
        .filter(|(name, _)| *name != "DEEPREF_PUBLIC_ORIGIN")
        .collect();
    assert!(matches!(
        runtime(&missing_origin).unwrap_err(),
        ConfigError::Missing("DEEPREF_PUBLIC_ORIGIN", Environment::Production)
    ));

    for origin in [
        "http://app.deepref.example",
        "https://app.deepref.example/app",
        "app.deepref.example",
    ] {
        let mut pairs: Vec<_> = PRODUCTION
            .iter()
            .copied()
            .filter(|(name, _)| *name != "DEEPREF_PUBLIC_ORIGIN")
            .collect();
        pairs.push(("DEEPREF_PUBLIC_ORIGIN", origin));
        assert!(runtime(&pairs).is_err(), "{origin} should be rejected");
    }

    let cors_any = ApiSettings::from_source(&|name| match name {
        "APP_ENV" => Some("production".into()),
        "API_CORS_ALLOW_ANY" => Some("true".into()),
        "API_CORS_ORIGINS" => Some("https://app.deepref.example".into()),
        _ => None,
    });
    assert!(
        cors_any
            .unwrap_err()
            .to_string()
            .contains("APP_ENV=development")
    );

    let local_storage = DocumentConfig::from_source(&|name| match name {
        "APP_ENV" => Some("production".into()),
        "DOCUMENT_STORAGE_BACKEND" => Some("local".into()),
        _ => None,
    });
    assert!(local_storage.is_err());
    let implicit_storage =
        DocumentConfig::from_source(&|name| (name == "APP_ENV").then(|| "production".into()));
    assert!(implicit_storage.is_err());
}

#[test]
fn development_allows_what_production_refuses() {
    let storage =
        DocumentConfig::from_source(&|name| (name == "APP_ENV").then(|| "development".into()))
            .unwrap();
    assert!(matches!(
        storage.backend,
        DocumentStorageBackend::Local { .. }
    ));
    let api = ApiSettings::from_source(&|name| match name {
        "APP_ENV" => Some("development".into()),
        "API_CORS_ALLOW_ANY" => Some("true".into()),
        _ => None,
    })
    .unwrap();
    assert!(api.cors_allow_any);
    let http = runtime(&[
        ("APP_ENV", "development"),
        ("DEEPREF_PUBLIC_ORIGIN", "http://localhost:3000"),
    ])
    .unwrap();
    assert_eq!(http.public_origin.as_str(), "http://localhost:3000");
}

#[test]
fn injected_configuration_is_deterministic() {
    let values = map(&[
        ("APP_ENV", "production"),
        ("DATABASE_URL", "postgres://app:pw@db.internal/deepref"),
        ("DEEPREF_PUBLIC_ORIGIN", "https://app.deepref.example"),
        ("DEEPREF_AUTH_ENABLED", "1"),
        ("WORKOS_CLIENT_ID", "client_01"),
        ("WORKOS_API_KEY", "key"),
        ("WORKOS_COOKIE_PASSWORD", "0123456789abcdef0123456789abcdef"),
        ("WORKOS_WEBHOOK_SECRET", "whsec"),
    ]);
    let first = RuntimeConfig::from_map("test", &values).unwrap();
    let second = RuntimeConfig::from_map("test", &values).unwrap();
    assert_eq!(format!("{first:?}"), format!("{second:?}"));
    assert_eq!(first.environment, Environment::Production);
    assert!(first.auth_enabled);
}

#[test]
fn workos_is_all_or_nothing_and_required_with_production_auth() {
    let config = production_with(&WORKOS).unwrap();
    let workos = config.workos.as_ref().unwrap();
    assert_eq!(workos.client_id, "client_01");
    assert_eq!(workos.api_key(), "sk_live_workos-api-secret");
    assert_eq!(workos.webhook_secret(), "workos-webhook-secret");
    assert_eq!(workos.cookie_password().len(), 37);

    for (omitted, (name, _)) in WORKOS.iter().enumerate() {
        let partial: Vec<_> = WORKOS
            .iter()
            .enumerate()
            .filter(|(index, _)| *index != omitted)
            .map(|(_, pair)| *pair)
            .collect();
        let error = production_with(&partial).unwrap_err();
        assert!(
            matches!(
                error,
                ConfigError::PartialSection {
                    section: "WorkOS",
                    ..
                }
            ),
            "omitting {} gave {error}",
            name
        );
        assert!(error.to_string().contains(name), "{error}");
    }

    let short = production_with(&[
        ("WORKOS_CLIENT_ID", "c"),
        ("WORKOS_API_KEY", "k"),
        ("WORKOS_COOKIE_PASSWORD", "too-short"),
        ("WORKOS_WEBHOOK_SECRET", "w"),
    ])
    .unwrap_err();
    assert!(
        short.to_string().contains("WORKOS_COOKIE_PASSWORD"),
        "{short}"
    );

    let error = production_with(&[("DEEPREF_AUTH_ENABLED", "true")]).unwrap_err();
    assert!(
        error.to_string().contains("DEEPREF_AUTH_ENABLED"),
        "{error}"
    );
    let mut enabled = WORKOS.to_vec();
    enabled.push(("DEEPREF_AUTH_ENABLED", "true"));
    assert!(production_with(&enabled).unwrap().auth_enabled);
    assert!(
        runtime(&[("APP_ENV", "development"), ("DEEPREF_AUTH_ENABLED", "true")]).is_ok(),
        "the production-only requirement does not apply to development"
    );
    assert!(production_with(&[("DEEPREF_AUTH_ENABLED", "maybe")]).is_err());
}

#[test]
fn polar_is_all_or_nothing_and_production_requires_the_production_server() {
    let config = production_with(&POLAR).unwrap();
    let polar = config.polar.as_ref().unwrap();
    assert_eq!(polar.server, PolarServer::Production);
    assert_eq!(polar.access_token(), "polar-access-secret");
    assert_eq!(polar.webhook_secret(), "polar-webhook-secret");

    for (omitted, (name, _)) in POLAR.iter().enumerate() {
        let partial: Vec<_> = POLAR
            .iter()
            .enumerate()
            .filter(|(index, _)| *index != omitted)
            .map(|(_, pair)| *pair)
            .collect();
        let error = production_with(&partial).unwrap_err();
        assert!(
            matches!(
                error,
                ConfigError::PartialSection {
                    section: "Polar",
                    ..
                }
            ),
            "omitting {} gave {error}",
            name
        );
    }

    let sandbox = [
        ("POLAR_ACCESS_TOKEN", "t"),
        ("POLAR_WEBHOOK_SECRET", "w"),
        ("POLAR_SERVER", "sandbox"),
    ];
    let error = production_with(&sandbox).unwrap_err();
    assert!(error.to_string().contains("POLAR_SERVER"), "{error}");
    let mut development = vec![("APP_ENV", "development")];
    development.extend_from_slice(&sandbox);
    let polar = runtime(&development).unwrap().polar.unwrap();
    assert_eq!(polar.server.as_str(), "sandbox");

    let mut bogus = vec![("APP_ENV", "development")];
    bogus.extend_from_slice(&[
        ("POLAR_ACCESS_TOKEN", "t"),
        ("POLAR_WEBHOOK_SECRET", "w"),
        ("POLAR_SERVER", "staging"),
    ]);
    assert!(runtime(&bogus).is_err());
}

#[test]
fn no_secret_appears_in_debug_output() {
    let mut pairs = PRODUCTION.to_vec();
    pairs.extend_from_slice(&WORKOS);
    pairs.extend_from_slice(&POLAR);
    pairs.extend_from_slice(&[
        ("DEEPREF_AUTH_ENABLED", "1"),
        ("OPENCODE_API_KEY", "ai-provider-secret"),
        ("SMTP_HOST", "mail.example"),
        ("SMTP_USERNAME", "mailer"),
        ("SMTP_PASSWORD", "smtp-password-secret"),
        ("DEEPREF_OPENALEX_API_KEY", "openalex-secret"),
    ]);
    let config = runtime(&pairs).unwrap();
    let storage = map(&[
        ("APP_ENV", "production"),
        ("DOCUMENT_STORAGE_BACKEND", "s3"),
        ("DOCUMENT_STORAGE_ENDPOINT", "https://s3.example.com"),
        ("DOCUMENT_STORAGE_BUCKET", "bucket"),
        ("DOCUMENT_STORAGE_REGION", "sa-east-1"),
        ("DOCUMENT_STORAGE_ACCESS_KEY_ID", "AKIAEXAMPLE"),
        ("DOCUMENT_STORAGE_SECRET_ACCESS_KEY", "s3-secret-key"),
    ]);
    let documents = DocumentConfig::from_source(&|name| storage.get(name).cloned()).unwrap();
    let rendered = [
        format!("{config:#?}"),
        format!("{config:?}"),
        format!("{documents:?}"),
    ];
    for secret in [
        "postgres://app:pw@db.internal/deepref",
        "pw@db",
        "sk_live_workos-api-secret",
        "workos-cookie-secret",
        "workos-webhook-secret",
        "polar-access-secret",
        "polar-webhook-secret",
        "ai-provider-secret",
        "smtp-password-secret",
        "openalex-secret",
        "s3-secret-key",
    ] {
        for output in &rendered {
            assert!(!output.contains(secret), "{secret} leaked into: {output}");
        }
    }
    // The accessors still hand the values to their one consumer.
    assert_eq!(config.ai.api_key(), Some("ai-provider-secret"));
    assert_eq!(
        config.smtp.credentials(),
        Some(("mailer", "smtp-password-secret"))
    );
    assert_eq!(
        config.integrations.openalex_api_key(),
        Some("openalex-secret")
    );
    let DocumentStorageBackend::S3 {
        secret_access_key, ..
    } = &documents.backend
    else {
        panic!("expected the s3 backend");
    };
    assert_eq!(secret_access_key.expose_secret(), "s3-secret-key");
}

#[test]
fn direct_and_file_secrets_are_exclusive() {
    let values = map(&[
        ("APP_ENV", "development"),
        ("DATABASE_URL", "postgres://direct"),
        ("DATABASE_URL_FILE", "/unused"),
    ]);
    assert!(RuntimeConfig::from_map("test", &values).is_err());
}

fn listen(pairs: &[(&str, &str)]) -> Result<ListenConfig, ConfigError> {
    ListenConfig::from_map(&map(pairs))
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
