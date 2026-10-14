use std::collections::HashMap;

/// One startup snapshot: every component sees the same environment values.
#[derive(Clone, Default)]
pub struct ConfigSource {
    values: HashMap<String, String>,
}

impl ConfigSource {
    pub fn from_env() -> Self {
        Self {
            values: std::env::vars().collect(),
        }
    }

    pub fn from_map(values: HashMap<String, String>) -> Self {
        Self { values }
    }

    pub fn get(&self, name: &str) -> Option<String> {
        self.values.get(name).cloned()
    }
}
