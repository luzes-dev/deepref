#[derive(Debug, Clone)]
pub struct CrossrefConfig {
    pub abstract_enrichment: bool,
}

impl Default for CrossrefConfig {
    fn default() -> Self {
        Self {
            abstract_enrichment: true,
        }
    }
}

impl CrossrefConfig {
    pub fn from_source(get: &dyn Fn(&str) -> Option<String>) -> Self {
        Self {
            abstract_enrichment: !get("DEEPREF_ABSTRACT_ENRICHMENT")
                .is_some_and(|value| matches!(value.trim(), "off" | "0" | "false")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enrichment_preserves_case_sensitive_opt_out() {
        assert!(CrossrefConfig::from_source(&|_| None).abstract_enrichment);
        for value in ["off", " 0 ", "false"] {
            assert!(!CrossrefConfig::from_source(&|_| Some(value.into())).abstract_enrichment);
        }
        for value in ["OFF", "true", ""] {
            assert!(CrossrefConfig::from_source(&|_| Some(value.into())).abstract_enrichment);
        }
    }
}
