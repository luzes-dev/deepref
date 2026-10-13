#[derive(Debug, Clone, Default)]
pub struct WorkflowConfig {
    pub niche_nodes_enabled: bool,
}

impl WorkflowConfig {
    pub fn from_env() -> Self {
        Self::from_source(&|name| std::env::var(name).ok())
    }

    pub fn from_source(get: &dyn Fn(&str) -> Option<String>) -> Self {
        Self {
            niche_nodes_enabled: get("DEEPREF_WORKFLOW_NICHE_NODES")
                .is_some_and(|value| matches!(value.to_ascii_lowercase().as_str(), "1" | "true" | "yes")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn niche_nodes_require_explicit_opt_in() {
        assert!(!WorkflowConfig::from_source(&|_| None).niche_nodes_enabled);
        for value in ["1", "true", "TRUE", "yes"] {
            assert!(WorkflowConfig::from_source(&|_| Some(value.into())).niche_nodes_enabled);
        }
        for value in ["0", "false", "", "invalid"] {
            assert!(!WorkflowConfig::from_source(&|_| Some(value.into())).niche_nodes_enabled);
        }
    }
}
