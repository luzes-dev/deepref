/// How Deepref identifies itself to the third-party providers it calls: the
/// polite-pool contact address and Crossref's abstract enrichment.
#[derive(Debug, Clone)]
pub struct IntegrationsConfig {
    pub abstract_enrichment: bool,
    /// Deployment-wide contact e-mail reported to Crossref, NCBI, Unpaywall,
    /// Europe PMC and OpenAlex. Empty when the operator has not set it.
    pub contact_email: String,
}

impl Default for IntegrationsConfig {
    fn default() -> Self {
        Self {
            abstract_enrichment: true,
            contact_email: String::new(),
        }
    }
}

impl IntegrationsConfig {
    pub fn from_source(get: &dyn Fn(&str) -> Option<String>) -> Result<Self, crate::ConfigError> {
        let contact_email = crate::optional(get, "DEEPREF_CONTACT_EMAIL").unwrap_or_default();
        if !contact_email.is_empty() && !is_plausible_email(&contact_email) {
            return Err(crate::ConfigError::Invalid {
                name: "DEEPREF_CONTACT_EMAIL",
                reason: "must be an e-mail address such as research@example.org".to_owned(),
            });
        }
        Ok(Self {
            abstract_enrichment: !get("DEEPREF_ABSTRACT_ENRICHMENT")
                .is_some_and(|value| matches!(value.trim(), "off" | "0" | "false")),
            contact_email,
        })
    }
}

/// Shape check for the contact address: one `@`, no whitespace, and a dotted
/// domain whose top-level label has at least two characters. It is not an RFC
/// 5322 parser; it only catches values that would clearly fail the contact
/// checks of the providers that ask for one.
fn is_plausible_email(value: &str) -> bool {
    if value.len() > 254 || value.chars().any(char::is_whitespace) {
        return false;
    }
    let Some((local, domain)) = value.split_once('@') else {
        return false;
    };
    if local.is_empty() || domain.contains('@') {
        return false;
    }
    let labels: Vec<&str> = domain.split('.').collect();
    labels.len() >= 2
        && labels.iter().all(|label| !label.is_empty())
        && labels.last().is_some_and(|top| top.len() >= 2)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enrichment_preserves_case_sensitive_opt_out() {
        assert!(
            IntegrationsConfig::from_source(&|_| None)
                .unwrap()
                .abstract_enrichment
        );
        for value in ["off", " 0 ", "false"] {
            assert!(
                !IntegrationsConfig::from_source(&|name| {
                    (name == "DEEPREF_ABSTRACT_ENRICHMENT").then(|| value.to_owned())
                })
                .unwrap()
                .abstract_enrichment
            );
        }
        for value in ["OFF", "true", ""] {
            assert!(
                IntegrationsConfig::from_source(&|name| {
                    (name == "DEEPREF_ABSTRACT_ENRICHMENT").then(|| value.to_owned())
                })
                .unwrap()
                .abstract_enrichment
            );
        }
    }

    #[test]
    fn an_unset_contact_email_is_allowed_and_blank_values_are_empty() {
        let config = IntegrationsConfig::from_source(&|_| None).unwrap();
        assert_eq!(config.contact_email, "");
        let config = IntegrationsConfig::from_source(&|name| {
            (name == "DEEPREF_CONTACT_EMAIL").then(|| "   ".to_owned())
        })
        .unwrap();
        assert_eq!(config.contact_email, "");
    }

    #[test]
    fn the_contact_email_is_trimmed_and_a_malformed_one_refuses_to_start() {
        let config = IntegrationsConfig::from_source(&|name| {
            (name == "DEEPREF_CONTACT_EMAIL").then(|| "  padded@example.org  ".to_owned())
        })
        .unwrap();
        assert_eq!(config.contact_email, "padded@example.org");

        for value in [
            "not-an-email",
            "a@b",
            "a@b.",
            "@example.org",
            "a b@example.org",
            "a@@example.org",
            "a@.example.org",
            "a@example..org",
            "a@example.o",
        ] {
            let error = IntegrationsConfig::from_source(&|name| {
                (name == "DEEPREF_CONTACT_EMAIL").then(|| value.to_owned())
            })
            .unwrap_err();
            assert!(
                error.to_string().contains("DEEPREF_CONTACT_EMAIL"),
                "{value}: unexpected error {error}"
            );
        }
    }

    #[test]
    fn plausible_contact_shapes_are_accepted() {
        for value in [
            "research@example.org",
            "first.last+tag@univ.ac.uk",
            "  padded@example.org  ",
        ] {
            assert!(
                IntegrationsConfig::from_source(&|name| {
                    (name == "DEEPREF_CONTACT_EMAIL").then(|| value.to_owned())
                })
                .is_ok(),
                "{value} should be accepted"
            );
        }
    }
}
