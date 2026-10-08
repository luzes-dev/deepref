use std::fmt;

use crate::ConfigError;

/// OpenCode Go subscription endpoint. Chat models are served at
/// `/chat/completions`; the model list is at `/models`.
pub const DEFAULT_OPENCODE_BASE_URL: &str = "https://opencode.ai/zen/go/v1";
/// Legacy Z.AI endpoint, used only with `DEEPREF_AI_PROVIDER=zai`.
pub const DEFAULT_ZAI_BASE_URL: &str = "https://api.z.ai/api/paas/v4";
pub const DEFAULT_AI_MODEL: &str = "glm-5.3-flash";

const PRICES_VARIABLE: &str = "DEEPREF_AI_MODEL_PRICES";
const ZAI_SELECTED_NOTICE: &str = "DEEPREF_AI_PROVIDER=zai selects Z.AI, which is retired. Use OpenCode Go (the default) instead; ZAI_API_KEY and ZAI_BASE_URL are deprecated.";
const ZAI_IGNORED_NOTICE: &str = "ZAI_API_KEY and ZAI_BASE_URL are deprecated and ignored. Set OPENCODE_API_KEY for OpenCode Go, or DEEPREF_AI_PROVIDER=zai to keep the legacy Z.AI provider.";

/// Loads `.env.local` and then `.env` from the working directory (or any
/// ancestor). Variables already present in the process environment always win,
/// and because `.env.local` is read first it takes precedence over `.env`.
/// Missing files are not an error.
pub fn load_dotenv_files() {
    let _ = dotenvy::from_filename(".env.local");
    let _ = dotenvy::from_filename(".env");
}

/// The OpenAI-compatible service that answers AI calls.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AiProviderKind {
    /// The OpenCode Go subscription. The default.
    OpenCodeGo,
    /// Z.AI, retired because its credits ran out. Only for emergencies.
    Zai,
}

impl AiProviderKind {
    /// Stable identifier stored with routes, usage rows and runs.
    pub const fn id(self) -> &'static str {
        match self {
            Self::OpenCodeGo => "opencode-go",
            Self::Zai => "zai",
        }
    }

    /// Name shown to people, for example in the AI status.
    pub const fn label(self) -> &'static str {
        match self {
            Self::OpenCodeGo => "OpenCode Go",
            Self::Zai => "Z.AI",
        }
    }

    /// The environment variable that holds this provider's API key.
    pub const fn api_key_variable(self) -> &'static str {
        match self {
            Self::OpenCodeGo => "OPENCODE_API_KEY",
            Self::Zai => "ZAI_API_KEY",
        }
    }
}

/// A price for one provider and model, in US dollars per million tokens as
/// written in `DEEPREF_AI_MODEL_PRICES` (for example
/// `opencode-go/glm-5.3=1.40:4.40`), stored as micro-dollars.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelPriceOverride {
    pub provider: String,
    pub model: String,
    pub input_micros_per_million_tokens: i64,
    pub output_micros_per_million_tokens: i64,
}

/// AI provider settings. API keys are never printed: `Debug` redacts them and
/// there is intentionally no `Display`.
#[derive(Clone, PartialEq, Eq)]
pub struct AiProviderConfig {
    api_key: Option<String>,
    pub provider: AiProviderKind,
    pub base_url: String,
    pub default_model: String,
    pub price_overrides: Vec<ModelPriceOverride>,
    deprecations: Vec<&'static str>,
}

impl fmt::Debug for AiProviderConfig {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AiProviderConfig")
            .field("provider", &self.provider)
            .field("api_key", &self.api_key.as_ref().map(|_| "<redacted>"))
            .field("base_url", &self.base_url)
            .field("default_model", &self.default_model)
            .field("price_overrides", &self.price_overrides)
            .finish()
    }
}

impl AiProviderConfig {
    pub fn from_env() -> Result<Self, ConfigError> {
        Self::from_source(&|name| std::env::var(name).ok())
    }

    pub fn from_source(get: &dyn Fn(&str) -> Option<String>) -> Result<Self, ConfigError> {
        let provider = match crate::optional(get, "DEEPREF_AI_PROVIDER").as_deref() {
            None | Some("opencode-go") => AiProviderKind::OpenCodeGo,
            Some("zai") => AiProviderKind::Zai,
            Some(_) => {
                return Err(ConfigError::Invalid {
                    name: "DEEPREF_AI_PROVIDER",
                    reason: "must be opencode-go (default) or zai".to_owned(),
                });
            }
        };
        let mut deprecations = Vec::new();
        let (url_variable, default_url) = match provider {
            AiProviderKind::OpenCodeGo => {
                if crate::optional(get, "ZAI_API_KEY").is_some()
                    || crate::optional(get, "ZAI_BASE_URL").is_some()
                {
                    deprecations.push(ZAI_IGNORED_NOTICE);
                }
                ("OPENCODE_BASE_URL", DEFAULT_OPENCODE_BASE_URL)
            }
            AiProviderKind::Zai => {
                deprecations.push(ZAI_SELECTED_NOTICE);
                ("ZAI_BASE_URL", DEFAULT_ZAI_BASE_URL)
            }
        };
        let base_url = crate::optional(get, url_variable)
            .unwrap_or_else(|| default_url.to_owned())
            .trim_end_matches('/')
            .to_owned();
        if !(base_url.starts_with("https://") || base_url.starts_with("http://")) {
            return Err(ConfigError::Invalid {
                name: url_variable,
                reason: "must be an http(s) URL".to_owned(),
            });
        }
        Ok(Self {
            api_key: crate::optional(get, provider.api_key_variable())
                .filter(|key| key != "replace-me"),
            provider,
            base_url,
            default_model: crate::optional(get, "DEEPREF_AI_DEFAULT_MODEL")
                .unwrap_or_else(|| DEFAULT_AI_MODEL.to_owned()),
            price_overrides: parse_price_overrides(
                crate::optional(get, PRICES_VARIABLE).as_deref(),
            )?,
            deprecations,
        })
    }

    pub const fn is_configured(&self) -> bool {
        self.api_key.is_some()
    }

    /// The only accessor for the secret; callers must hand it straight to the
    /// HTTP client and never log it.
    pub fn api_key(&self) -> Option<&str> {
        self.api_key.as_deref()
    }

    /// Plain-language notices about deprecated settings that were read. Log
    /// them once at startup.
    pub fn deprecations(&self) -> &[&'static str] {
        &self.deprecations
    }
}

fn invalid_price(reason: &str) -> ConfigError {
    ConfigError::Invalid {
        name: PRICES_VARIABLE,
        reason: reason.to_owned(),
    }
}

/// Parses `provider/model=input:output` entries separated by commas. Prices are
/// US dollars per million tokens.
fn parse_price_overrides(raw: Option<&str>) -> Result<Vec<ModelPriceOverride>, ConfigError> {
    let Some(raw) = raw else {
        return Ok(Vec::new());
    };
    raw.split(',')
        .map(str::trim)
        .filter(|entry| !entry.is_empty())
        .map(parse_price_override)
        .collect()
}

fn parse_price_override(entry: &str) -> Result<ModelPriceOverride, ConfigError> {
    const SHAPE: &str = "entries look like provider/model=input:output";
    let (route, prices) = entry.split_once('=').ok_or_else(|| invalid_price(SHAPE))?;
    let (provider, model) = route
        .split_once('/')
        .map(|(provider, model)| (provider.trim(), model.trim()))
        .filter(|(provider, model)| !provider.is_empty() && !model.is_empty())
        .ok_or_else(|| invalid_price(SHAPE))?;
    let (input, output) = prices.split_once(':').ok_or_else(|| invalid_price(SHAPE))?;
    Ok(ModelPriceOverride {
        provider: provider.to_owned(),
        model: model.to_owned(),
        input_micros_per_million_tokens: usd_to_micros(input)?,
        output_micros_per_million_tokens: usd_to_micros(output)?,
    })
}

fn usd_to_micros(text: &str) -> Result<i64, ConfigError> {
    let value: f64 = text
        .trim()
        .parse()
        .map_err(|_| invalid_price("prices are decimal US dollars per million tokens"))?;
    if !value.is_finite() || !(0.0..=1_000_000.0).contains(&value) {
        return Err(invalid_price(
            "prices must be between 0 and 1000000 US dollars per million tokens",
        ));
    }
    Ok((value * 1_000_000.0).round() as i64)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn source(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> + use<> {
        let owned: Vec<(String, String)> = pairs
            .iter()
            .map(|(name, value)| ((*name).to_owned(), (*value).to_owned()))
            .collect();
        move |name| {
            owned
                .iter()
                .find(|(key, _)| key == name)
                .map(|(_, value)| value.clone())
        }
    }

    fn load(pairs: &[(&str, &str)]) -> AiProviderConfig {
        AiProviderConfig::from_source(&source(pairs)).unwrap_or_else(|_| unreachable!())
    }

    fn load_error(pairs: &[(&str, &str)]) -> String {
        match AiProviderConfig::from_source(&source(pairs)) {
            Ok(_) => unreachable!("the configuration should be rejected"),
            Err(error) => error.to_string(),
        }
    }

    #[test]
    fn opencode_go_is_the_default_provider() {
        let config = load(&[("OPENCODE_API_KEY", "super-secret")]);
        assert_eq!(config.provider, AiProviderKind::OpenCodeGo);
        assert_eq!(config.provider.id(), "opencode-go");
        assert_eq!(config.provider.label(), "OpenCode Go");
        assert_eq!(config.base_url, DEFAULT_OPENCODE_BASE_URL);
        assert_eq!(config.default_model, DEFAULT_AI_MODEL);
        assert!(config.is_configured());
        assert!(config.deprecations().is_empty());
        assert!(!format!("{config:?}").contains("super-secret"));
    }

    #[test]
    fn no_key_means_not_configured_and_placeholders_do_not_count() {
        assert!(!load(&[]).is_configured());
        assert!(!load(&[("OPENCODE_API_KEY", "replace-me")]).is_configured());
    }

    #[test]
    fn opencode_base_url_and_model_can_be_overridden() {
        let config = load(&[
            ("OPENCODE_API_KEY", "k"),
            ("OPENCODE_BASE_URL", " https://proxy.example/zen/go/v1/ "),
            ("DEEPREF_AI_DEFAULT_MODEL", "kimi-k2.6"),
        ]);
        assert_eq!(config.base_url, "https://proxy.example/zen/go/v1");
        assert_eq!(config.default_model, "kimi-k2.6");
        assert!(load_error(&[("OPENCODE_BASE_URL", "ftp://nope")]).contains("OPENCODE_BASE_URL"));
    }

    #[test]
    fn zai_variables_alone_are_ignored_with_a_notice() {
        let config = load(&[("ZAI_API_KEY", "legacy-secret")]);
        assert_eq!(config.provider, AiProviderKind::OpenCodeGo);
        assert!(!config.is_configured(), "Z.AI is never used by default");
        assert_eq!(config.deprecations(), [ZAI_IGNORED_NOTICE]);
    }

    #[test]
    fn legacy_zai_is_selected_explicitly_with_its_own_variables() {
        let config = load(&[
            ("DEEPREF_AI_PROVIDER", "zai"),
            ("ZAI_API_KEY", "legacy-secret"),
            ("ZAI_BASE_URL", "https://api.z.ai/api/paas/v4/"),
            ("OPENCODE_API_KEY", "ignored-for-zai"),
        ]);
        assert_eq!(config.provider, AiProviderKind::Zai);
        assert_eq!(config.provider.id(), "zai");
        assert_eq!(config.provider.label(), "Z.AI");
        assert_eq!(config.base_url, DEFAULT_ZAI_BASE_URL);
        assert!(config.is_configured());
        assert_eq!(config.deprecations(), [ZAI_SELECTED_NOTICE]);
        assert!(!format!("{config:?}").contains("legacy-secret"));
    }

    #[test]
    fn legacy_zai_without_its_key_is_not_configured() {
        let config = load(&[("DEEPREF_AI_PROVIDER", "zai"), ("OPENCODE_API_KEY", "k")]);
        assert_eq!(config.provider, AiProviderKind::Zai);
        assert!(!config.is_configured());
    }

    #[test]
    fn unknown_provider_is_rejected() {
        assert!(load_error(&[("DEEPREF_AI_PROVIDER", "openai")]).contains("DEEPREF_AI_PROVIDER"));
    }

    #[test]
    fn price_overrides_parse_per_provider_and_model() {
        let config = load(&[(
            "DEEPREF_AI_MODEL_PRICES",
            "opencode-go/glm-5.3=1.40:4.40, zai/glm-5.3-flash=0.15:0.5,",
        )]);
        assert_eq!(
            config.price_overrides,
            vec![
                ModelPriceOverride {
                    provider: "opencode-go".to_owned(),
                    model: "glm-5.3".to_owned(),
                    input_micros_per_million_tokens: 1_400_000,
                    output_micros_per_million_tokens: 4_400_000,
                },
                ModelPriceOverride {
                    provider: "zai".to_owned(),
                    model: "glm-5.3-flash".to_owned(),
                    input_micros_per_million_tokens: 150_000,
                    output_micros_per_million_tokens: 500_000,
                },
            ]
        );
    }

    #[test]
    fn malformed_price_overrides_are_rejected() {
        for raw in [
            "glm-5.3=1:2",
            "opencode-go/glm-5.3=1",
            "opencode-go/=1:2",
            "opencode-go/glm-5.3=-1:2",
            "opencode-go/glm-5.3=free:2",
        ] {
            assert!(
                load_error(&[("DEEPREF_AI_MODEL_PRICES", raw)]).contains("DEEPREF_AI_MODEL_PRICES"),
                "{raw} should be rejected"
            );
        }
    }
}
