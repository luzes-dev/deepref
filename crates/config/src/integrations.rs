use std::fmt;

use deepref_domain::ProviderKind;

/// The chain used when `DEEPREF_METADATA_PROVIDERS` is unset: OpenAlex answers first,
/// Crossref falls back, and DataCite catches the datasets and software the other two
/// do not hold. The default moves in stages (handoff section 5.1), and this constant is
/// the only place it is written.
const DEFAULT_METADATA_PROVIDERS: &[ProviderKind] = &[
    ProviderKind::OpenAlex,
    ProviderKind::Crossref,
    ProviderKind::DataCite,
];

/// Providers with an adapter in this build. A chain may name only these, so a provider
/// added to the domain enum stays out of every deployment until its adapter lands.
const PROVIDERS_WITH_ADAPTERS: &[ProviderKind] = &[
    ProviderKind::OpenAlex,
    ProviderKind::Crossref,
    ProviderKind::DataCite,
];

/// Which OpenAlex records metadata lookups may answer from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpenAlexCorpus {
    /// Crossref, PubMed, DataCite and repository records.
    Core,
    /// Core plus dataset and repository records that have no core match.
    All,
}

/// How Deepref identifies itself to the third-party providers it calls: the
/// polite-pool contact address and Crossref's abstract enrichment. It also
/// holds the metadata chain, which doubles as the deployment allowlist
/// (ADR 0007).
#[derive(Clone)]
pub struct IntegrationsConfig {
    pub abstract_enrichment: bool,
    /// Deployment-wide contact e-mail reported to Crossref, NCBI, Unpaywall,
    /// Europe PMC and OpenAlex. Empty when the operator has not set it.
    pub contact_email: String,
    /// The first provider that answers a DOI wins, so order is preference.
    pub metadata_providers: Vec<ProviderKind>,
    /// Optional OpenAlex key. A missing key is valid; keyless calls get a
    /// smaller rate budget. Never printed: `Debug` redacts it.
    pub openalex_api_key: Option<String>,
    pub openalex_corpus: OpenAlexCorpus,
}

impl Default for IntegrationsConfig {
    fn default() -> Self {
        Self {
            abstract_enrichment: true,
            contact_email: String::new(),
            metadata_providers: DEFAULT_METADATA_PROVIDERS.to_vec(),
            openalex_api_key: None,
            openalex_corpus: OpenAlexCorpus::Core,
        }
    }
}

impl fmt::Debug for IntegrationsConfig {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("IntegrationsConfig")
            .field("abstract_enrichment", &self.abstract_enrichment)
            .field("contact_email", &self.contact_email)
            .field("metadata_providers", &self.metadata_providers)
            .field(
                "openalex_api_key",
                &self.openalex_api_key.as_ref().map(|_| "<redacted>"),
            )
            .field("openalex_corpus", &self.openalex_corpus)
            .finish()
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
        let openalex_corpus = match crate::optional(get, "DEEPREF_OPENALEX_CORPUS").as_deref() {
            None | Some("core") => OpenAlexCorpus::Core,
            Some("all") => OpenAlexCorpus::All,
            Some(_) => {
                return Err(crate::ConfigError::Invalid {
                    name: "DEEPREF_OPENALEX_CORPUS",
                    reason: "must be core (default) or all".to_owned(),
                });
            }
        };
        Ok(Self {
            abstract_enrichment: !get("DEEPREF_ABSTRACT_ENRICHMENT")
                .is_some_and(|value| matches!(value.trim(), "off" | "0" | "false")),
            contact_email,
            metadata_providers: metadata_providers(get)?,
            openalex_api_key: crate::optional(get, "DEEPREF_OPENALEX_API_KEY"),
            openalex_corpus,
        })
    }

    /// The chain as one comma-separated string, for example `openalex,crossref`.
    pub fn metadata_chain_label(&self) -> String {
        join_names(&self.metadata_providers, ",")
    }

    /// The deployment chain filtered to providers with an adapter in this build. This is
    /// the allowlist a request may name, in deployment order.
    pub fn enabled_metadata_providers(&self) -> Vec<ProviderKind> {
        self.metadata_providers
            .iter()
            .copied()
            .filter(|kind| PROVIDERS_WITH_ADAPTERS.contains(kind))
            .collect()
    }
}

fn metadata_providers(
    get: &dyn Fn(&str) -> Option<String>,
) -> Result<Vec<ProviderKind>, crate::ConfigError> {
    match get("DEEPREF_METADATA_PROVIDERS") {
        Some(value) => parse_metadata_chain(&value, PROVIDERS_WITH_ADAPTERS),
        None => Ok(DEFAULT_METADATA_PROVIDERS.to_vec()),
    }
}

/// Reads a comma-separated chain. Entries are trimmed, a repeated provider
/// keeps its first position, and a chain with no entries is refused. The
/// adapter set is a parameter so the ordering rules can be tested with any set.
fn parse_metadata_chain(
    value: &str,
    adapters: &[ProviderKind],
) -> Result<Vec<ProviderKind>, crate::ConfigError> {
    const NAME: &str = "DEEPREF_METADATA_PROVIDERS";
    let mut chain: Vec<ProviderKind> = Vec::new();
    for entry in value
        .split(',')
        .map(str::trim)
        .filter(|entry| !entry.is_empty())
    {
        let kind = entry
            .parse::<ProviderKind>()
            .map_err(|_| crate::ConfigError::Invalid {
                name: NAME,
                reason: format!(
                    "unknown metadata provider {entry:?}; known providers are {}",
                    join_names(&ProviderKind::ALL, ", ")
                ),
            })?;
        if !adapters.contains(&kind) {
            return Err(crate::ConfigError::Invalid {
                name: NAME,
                reason: format!(
                    "{} has no adapter yet; available providers are {}",
                    kind.as_str(),
                    join_names(adapters, ", ")
                ),
            });
        }
        if !chain.contains(&kind) {
            chain.push(kind);
        }
    }
    if chain.is_empty() {
        return Err(crate::ConfigError::Invalid {
            name: NAME,
            reason: "must name at least one provider, for example crossref".to_owned(),
        });
    }
    Ok(chain)
}

fn join_names(kinds: &[ProviderKind], separator: &str) -> String {
    kinds
        .iter()
        .map(|kind| kind.as_str())
        .collect::<Vec<_>>()
        .join(separator)
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

    fn load(pairs: &[(&str, &str)]) -> Result<IntegrationsConfig, crate::ConfigError> {
        IntegrationsConfig::from_source(&|name| {
            pairs
                .iter()
                .find(|(key, _)| *key == name)
                .map(|(_, value)| (*value).to_owned())
        })
    }

    fn load_error(pairs: &[(&str, &str)]) -> String {
        match load(pairs) {
            Ok(_) => unreachable!("the configuration should be rejected"),
            Err(error) => error.to_string(),
        }
    }

    #[test]
    fn the_metadata_chain_defaults_to_openalex_crossref_then_datacite_with_a_keyless_core_corpus() {
        let default_chain = [
            ProviderKind::OpenAlex,
            ProviderKind::Crossref,
            ProviderKind::DataCite,
        ];
        let default = IntegrationsConfig::default();
        assert_eq!(default.metadata_providers, default_chain);
        assert_eq!(default.openalex_corpus, OpenAlexCorpus::Core);

        let config = load(&[]).unwrap();
        assert_eq!(config.metadata_providers, default_chain);
        assert_eq!(config.openalex_api_key, None);
        assert_eq!(config.openalex_corpus, OpenAlexCorpus::Core);
        assert_eq!(config.metadata_chain_label(), "openalex,crossref,datacite");
    }

    #[test]
    fn crossref_alone_is_the_rollback_chain() {
        let config = load(&[("DEEPREF_METADATA_PROVIDERS", "crossref")]).unwrap();
        assert_eq!(config.metadata_providers, [ProviderKind::Crossref]);
        assert_eq!(
            config.enabled_metadata_providers(),
            [ProviderKind::Crossref]
        );
    }

    #[test]
    fn a_configured_chain_and_corpus_override_the_defaults() {
        let config = load(&[
            ("DEEPREF_METADATA_PROVIDERS", "crossref"),
            ("DEEPREF_OPENALEX_CORPUS", " all "),
        ])
        .unwrap();
        assert_eq!(config.metadata_providers, [ProviderKind::Crossref]);
        assert_eq!(config.openalex_corpus, OpenAlexCorpus::All);
        assert_eq!(
            load(&[("DEEPREF_OPENALEX_CORPUS", "core")])
                .unwrap()
                .openalex_corpus,
            OpenAlexCorpus::Core
        );
    }

    #[test]
    fn a_chain_is_trimmed_deduplicated_and_kept_in_configured_order() {
        let adapters = [
            ProviderKind::OpenAlex,
            ProviderKind::Crossref,
            ProviderKind::DataCite,
        ];
        assert_eq!(
            parse_metadata_chain(" datacite , crossref,datacite,openalex ", &adapters).unwrap(),
            [
                ProviderKind::DataCite,
                ProviderKind::Crossref,
                ProviderKind::OpenAlex
            ]
        );
        assert_eq!(
            load(&[("DEEPREF_METADATA_PROVIDERS", " crossref , crossref ")])
                .unwrap()
                .metadata_providers,
            [ProviderKind::Crossref]
        );
    }

    #[test]
    fn every_configured_provider_is_enabled_in_deployment_order() {
        let config = IntegrationsConfig {
            metadata_providers: vec![
                ProviderKind::DataCite,
                ProviderKind::OpenAlex,
                ProviderKind::Crossref,
            ],
            ..IntegrationsConfig::default()
        };
        assert_eq!(
            config.enabled_metadata_providers(),
            [
                ProviderKind::DataCite,
                ProviderKind::OpenAlex,
                ProviderKind::Crossref
            ]
        );
        assert_eq!(
            IntegrationsConfig::default().enabled_metadata_providers(),
            [
                ProviderKind::OpenAlex,
                ProviderKind::Crossref,
                ProviderKind::DataCite
            ]
        );
    }

    #[test]
    fn unknown_metadata_providers_refuse_to_start() {
        for value in ["pubmed", "crossref,pubmed", "Crossref", "open_alex"] {
            let error = load_error(&[("DEEPREF_METADATA_PROVIDERS", value)]);
            assert!(
                error.contains("DEEPREF_METADATA_PROVIDERS")
                    && error.contains("unknown metadata provider"),
                "{value}: unexpected error {error}"
            );
        }
    }

    #[test]
    fn an_empty_metadata_chain_refuses_to_start() {
        for value in ["", "   ", " , "] {
            let error = load_error(&[("DEEPREF_METADATA_PROVIDERS", value)]);
            assert!(
                error.contains("must name at least one provider"),
                "{value:?}: unexpected error {error}"
            );
        }
    }

    #[test]
    fn a_provider_without_an_adapter_refuses_to_start_and_names_the_available_ones() {
        // Every provider in the domain enum has an adapter now, so the refusal is checked
        // against an adapter set that leaves DataCite out.
        let adapters = [ProviderKind::OpenAlex, ProviderKind::Crossref];
        for (value, unavailable) in [("datacite", "datacite"), ("openalex,datacite", "datacite")] {
            let error = parse_metadata_chain(value, &adapters)
                .expect_err("a chain naming a provider without an adapter should be refused")
                .to_string();
            assert!(
                error.contains(&format!("{unavailable} has no adapter yet"))
                    && error.contains("available providers are openalex, crossref"),
                "{value}: unexpected error {error}"
            );
        }
    }

    #[test]
    fn openalex_is_accepted_and_keeps_its_configured_position() {
        assert_eq!(
            load(&[("DEEPREF_METADATA_PROVIDERS", "crossref,openalex")])
                .unwrap()
                .metadata_providers,
            [ProviderKind::Crossref, ProviderKind::OpenAlex]
        );
    }

    #[test]
    fn an_unknown_openalex_corpus_refuses_to_start() {
        for value in ["everything", "ALL", "all,core"] {
            let error = load_error(&[("DEEPREF_OPENALEX_CORPUS", value)]);
            assert!(
                error.contains("DEEPREF_OPENALEX_CORPUS"),
                "{value}: unexpected error {error}"
            );
        }
    }

    #[test]
    fn the_openalex_key_is_optional_trimmed_and_never_printed() {
        assert_eq!(
            load(&[("DEEPREF_OPENALEX_API_KEY", "   ")])
                .unwrap()
                .openalex_api_key,
            None
        );
        let config = load(&[("DEEPREF_OPENALEX_API_KEY", "  test-openalex-key  ")]).unwrap();
        assert_eq!(
            config.openalex_api_key.as_deref(),
            Some("test-openalex-key")
        );
        let debug = format!("{config:?}");
        assert!(!debug.contains("test-openalex-key"), "{debug}");
        assert!(debug.contains("<redacted>"), "{debug}");
    }
}
