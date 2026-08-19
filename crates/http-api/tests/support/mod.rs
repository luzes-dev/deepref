use std::collections::HashMap;

pub fn local_runtime() -> deepref_config::RuntimeConfig {
    deepref_config::RuntimeConfig::from_map(
        "deepref-api-test",
        &HashMap::from([("APP_ENV".to_owned(), "local".to_owned())]),
    )
    .unwrap()
}
