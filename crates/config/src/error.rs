#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("{0} is required when APP_ENV={1:?}")]
    Missing(&'static str, crate::Environment),
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
