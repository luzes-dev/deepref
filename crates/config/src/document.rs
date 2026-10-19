use std::path::PathBuf;

use secrecy::SecretString;

use crate::{APP_ENV, ConfigError, Environment};

pub const DEFAULT_MAX_DOCUMENT_BYTES: usize = 25 * 1024 * 1024;
pub const MAX_DOCUMENT_BYTES: usize = 100 * 1024 * 1024;

#[derive(Debug, Clone)]
pub struct DocumentConfig {
    pub backend: DocumentStorageBackend,
    pub max_bytes: usize,
}

#[derive(Debug, Clone)]
pub enum DocumentStorageBackend {
    Local {
        root: PathBuf,
    },
    S3 {
        endpoint: String,
        bucket: String,
        region: String,
        access_key_id: String,
        secret_access_key: SecretString,
    },
}

impl DocumentConfig {
    pub fn from_source(get: &dyn Fn(&str) -> Option<String>) -> Result<Self, ConfigError> {
        let invalid = |name, reason: &str| ConfigError::Invalid {
            name,
            reason: reason.to_owned(),
        };
        let required = |name| {
            get(name)
                .filter(|value| !value.trim().is_empty())
                .ok_or_else(|| invalid(name, "required document storage setting is missing"))
        };
        let is_development = Environment::from_app_env(get(APP_ENV).as_deref())?.is_development();
        let backend = get("DOCUMENT_STORAGE_BACKEND")
            .map(|value| value.trim().to_ascii_lowercase())
            .or_else(|| is_development.then(|| "local".to_owned()))
            .ok_or_else(|| {
                invalid(
                    "DOCUMENT_STORAGE_BACKEND",
                    "required document storage setting is missing",
                )
            })?;
        let backend = match backend.as_str() {
            "local" if is_development => {
                let root = get("DOCUMENT_STORAGE_ROOT")
                    .unwrap_or_else(|| "/tmp/deepref-documents".to_owned());
                if root.is_empty() {
                    return Err(invalid("DOCUMENT_STORAGE_ROOT", "must not be empty"));
                }
                DocumentStorageBackend::Local { root: root.into() }
            }
            "local" => {
                return Err(invalid(
                    "DOCUMENT_STORAGE_BACKEND",
                    "local is allowed only when APP_ENV=development",
                ));
            }
            "s3" => {
                let endpoint = required("DOCUMENT_STORAGE_ENDPOINT")?;
                let bucket = required("DOCUMENT_STORAGE_BUCKET")?;
                let region = required("DOCUMENT_STORAGE_REGION")?;
                let access_key_id = required("DOCUMENT_STORAGE_ACCESS_KEY_ID")?;
                let secret_access_key = required("DOCUMENT_STORAGE_SECRET_ACCESS_KEY")?;
                let url = url::Url::parse(&endpoint)
                    .map_err(|_| invalid("DOCUMENT_STORAGE_ENDPOINT", "must be a valid URL"))?;
                if url.scheme() != "https"
                    || url.host_str().is_none()
                    || !url.username().is_empty()
                    || url.password().is_some()
                {
                    return Err(invalid(
                        "DOCUMENT_STORAGE_ENDPOINT",
                        "must be HTTPS with a host and no credentials",
                    ));
                }
                DocumentStorageBackend::S3 {
                    endpoint,
                    bucket,
                    region,
                    access_key_id,
                    secret_access_key: SecretString::from(secret_access_key),
                }
            }
            _ => return Err(invalid("DOCUMENT_STORAGE_BACKEND", "must be local or s3")),
        };
        let max_bytes =
            get("DOCUMENT_MAX_BYTES").map_or(Ok(DEFAULT_MAX_DOCUMENT_BYTES), |value| {
                value.trim().parse::<usize>().map_err(|_| {
                    invalid(
                        "DOCUMENT_MAX_BYTES",
                        "must be a positive integer up to 104857600",
                    )
                })
            })?;
        if max_bytes == 0 || max_bytes > MAX_DOCUMENT_BYTES {
            return Err(invalid(
                "DOCUMENT_MAX_BYTES",
                "must be a positive integer up to 104857600",
            ));
        }
        Ok(Self { backend, max_bytes })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn storage_requires_explicit_hosted_backend_and_bounded_limits() {
        assert!(DocumentConfig::from_source(&|_| None).is_err());
        assert!(
            DocumentConfig::from_source(&|name| (name == "APP_ENV").then(|| "production".into()))
                .is_err()
        );
        let development = |name: &str| (name == "APP_ENV").then(|| "development".into());
        let config = DocumentConfig::from_source(&development).unwrap();
        assert_eq!(config.max_bytes, DEFAULT_MAX_DOCUMENT_BYTES);
        assert!(matches!(
            config.backend,
            DocumentStorageBackend::Local { .. }
        ));
        for value in ["0", "104857601", "bad"] {
            assert!(
                DocumentConfig::from_source(&|name| match name {
                    "DOCUMENT_MAX_BYTES" => Some(value.into()),
                    _ => development(name),
                })
                .is_err()
            );
        }
    }
}
