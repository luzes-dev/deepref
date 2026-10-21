#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("APP_ENV is required: set APP_ENV=development or APP_ENV=production")]
    AppEnvMissing,
    #[error("{0} is required when APP_ENV={1:?}")]
    Missing(&'static str, crate::Environment),
    #[error("{section} is partially configured; also set {}", missing.join(", "))]
    PartialSection {
        section: &'static str,
        missing: Vec<&'static str>,
    },
    #[error("{name} is invalid: {reason}")]
    Invalid { name: &'static str, reason: String },
    #[error("{0} and {1} are mutually exclusive")]
    MutuallyExclusive(&'static str, &'static str),
    #[error("failed to read {name} from {path}: {source}")]
    SecretFile {
        name: &'static str,
        path: String,
        #[source]
        source: std::io::Error,
    },
}
