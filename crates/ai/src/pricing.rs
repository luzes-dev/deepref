//! Explicit per-route prices and the provider facts needed to bound them.
//!
//! Missing routes are unsupported. Adding a price alone never makes a route
//! billable: it also needs a reviewed capability entry below.

use std::collections::BTreeMap;

use crate::AiError;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ModelPrice {
    pub input_micros_per_million_tokens: i64,
    pub output_micros_per_million_tokens: i64,
    /// No currently supported route bills reasoning separately. A non-`None`
    /// value therefore makes that route unsupported until its billing contract
    /// is reviewed and added to [`RouteCapabilities`].
    pub reasoning_micros_per_million_tokens: Option<i64>,
}

/// Facts that make a provider route safe to meter. The table intentionally
/// repeats entries per provider/model so adding a model requires an explicit
/// review of every billable component.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RouteCapabilities {
    /// DeepRef sends this value as `max_tokens` on every call. This is a
    /// stricter local ceiling than the provider's documented maximum.
    pub max_output_tokens: u64,
    /// Reasoning tokens are part of the provider's output-token meter and are
    /// bounded by the same enforced `max_tokens` value.
    pub reasoning_included_in_output: bool,
    /// Prompt, declared function tools and returned tool-call arguments are
    /// charged through input/output token counts. Hosted search, retrieval,
    /// image, audio, and other separately billed tools are not enabled.
    pub billable_components: BillableComponents,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BillableComponents {
    TextTokensAndFunctionTools,
}

const OUTPUT_CAP: u64 = 2_048;
const TOKEN_ONLY: RouteCapabilities = RouteCapabilities {
    max_output_tokens: OUTPUT_CAP,
    reasoning_included_in_output: true,
    billable_components: BillableComponents::TextTokensAndFunctionTools,
};

/// Explicit capability allowlist for the current chat-completions adapters.
/// Inputs are bounded from the exact serialized request bytes; tokenizers are
/// not assumed. Provider usage caches read tokens at a lower advertised rate,
/// so charging all prompt tokens at the configured uncached input rate is a
/// conservative upper bound. No hosted or separately billed tools are sent.
const ROUTE_CAPABILITIES: &[((&str, &str), RouteCapabilities)] = &[
    (("opencode-go", "glm-5.3-flash"), TOKEN_ONLY),
    (("opencode-go", "glm-5.3"), TOKEN_ONLY),
    (("opencode-go", "kimi-k2.6"), TOKEN_ONLY),
    (("opencode-go", "deepseek-v4-flash"), TOKEN_ONLY),
    (("zai", "glm-5.3"), TOKEN_ONLY),
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct RoutePrice {
    price: ModelPrice,
    capabilities: RouteCapabilities,
}

/// Prices and reviewed billing semantics keyed by provider and model.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PriceBook {
    routes: BTreeMap<(String, String), RoutePrice>,
    version: String,
}

impl Default for PriceBook {
    fn default() -> Self {
        Self::new(
            [
                ("opencode-go", "glm-5.3-flash", 150_000, 500_000),
                ("opencode-go", "glm-5.3", 1_400_000, 4_400_000),
                ("opencode-go", "kimi-k2.6", 950_000, 4_000_000),
                ("opencode-go", "deepseek-v4-flash", 140_000, 280_000),
                ("zai", "glm-5.3", 1_400_000, 4_400_000),
            ]
            .into_iter()
            .map(|(provider, model, input, output)| {
                (
                    provider.to_owned(),
                    model.to_owned(),
                    ModelPrice {
                        input_micros_per_million_tokens: input,
                        output_micros_per_million_tokens: output,
                        reasoning_micros_per_million_tokens: None,
                    },
                )
            }),
        )
    }
}

impl PriceBook {
    pub fn new(overrides: impl IntoIterator<Item = (String, String, ModelPrice)>) -> Self {
        let mut routes = BTreeMap::new();
        for ((provider, model), capabilities) in ROUTE_CAPABILITIES {
            let defaults = default_price(provider, model);
            routes.insert(
                ((*provider).to_owned(), (*model).to_owned()),
                RoutePrice {
                    price: defaults,
                    capabilities: *capabilities,
                },
            );
        }
        for (provider, model, price) in overrides {
            if let Some(route) = routes.get_mut(&(provider, model)) {
                route.price = price;
            }
        }
        let version = version_for(&routes);
        Self { routes, version }
    }

    pub fn price(&self, provider: &str, model: &str) -> Result<ModelPrice, AiError> {
        let route = self.route(provider, model)?;
        if route.price.reasoning_micros_per_million_tokens.is_some() {
            return Err(unsupported(provider, model));
        }
        if route.price.input_micros_per_million_tokens < 0
            || route.price.output_micros_per_million_tokens < 0
        {
            return Err(AiError::InvalidUsageBound);
        }
        Ok(route.price)
    }

    pub fn capabilities(&self, provider: &str, model: &str) -> Result<RouteCapabilities, AiError> {
        Ok(self.route(provider, model)?.capabilities)
    }

    fn route(&self, provider: &str, model: &str) -> Result<RoutePrice, AiError> {
        self.routes
            .get(&(provider.to_owned(), model.to_owned()))
            .copied()
            .ok_or_else(|| unsupported(provider, model))
    }

    /// Stable value identity for configured prices and route capabilities.
    pub fn version(&self) -> String {
        self.version.clone()
    }

    /// Estimated cost in micro-dollars of one call on a supported route.
    pub fn estimate_cost_micros(
        &self,
        provider: &str,
        model: &str,
        input_tokens: u64,
        output_tokens: u64,
    ) -> Result<i64, AiError> {
        let price = self.price(provider, model)?;
        cost_micros(price, input_tokens, output_tokens)
    }
}

fn default_price(provider: &str, model: &str) -> ModelPrice {
    let (input, output) = match (provider, model) {
        ("opencode-go", "glm-5.3-flash") => (150_000, 500_000),
        ("opencode-go", "glm-5.3") | ("zai", "glm-5.3") => (1_400_000, 4_400_000),
        ("opencode-go", "kimi-k2.6") => (950_000, 4_000_000),
        ("opencode-go", "deepseek-v4-flash") => (140_000, 280_000),
        _ => (0, 0),
    };
    ModelPrice {
        input_micros_per_million_tokens: input,
        output_micros_per_million_tokens: output,
        reasoning_micros_per_million_tokens: None,
    }
}

fn unsupported(provider: &str, model: &str) -> AiError {
    AiError::UnsupportedUsageRoute {
        provider: provider.to_owned(),
        model: model.to_owned(),
    }
}

fn cost_micros(price: ModelPrice, input_tokens: u64, output_tokens: u64) -> Result<i64, AiError> {
    let total = u128::from(input_tokens)
        .checked_mul(price.input_micros_per_million_tokens as u128)
        .and_then(|input| {
            u128::from(output_tokens)
                .checked_mul(price.output_micros_per_million_tokens as u128)
                .and_then(|output| input.checked_add(output))
        })
        .ok_or(AiError::InvalidUsageBound)?;
    i64::try_from(total.div_ceil(1_000_000)).map_err(|_| AiError::InvalidUsageBound)
}

fn version_for(routes: &BTreeMap<(String, String), RoutePrice>) -> String {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    let mut feed = |bytes: &[u8]| {
        for byte in bytes {
            hash ^= u64::from(*byte);
            hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        }
    };
    for ((provider, model), route) in routes {
        feed(provider.as_bytes());
        feed(&[0]);
        feed(model.as_bytes());
        feed(&[0]);
        feed(
            route
                .price
                .input_micros_per_million_tokens
                .to_string()
                .as_bytes(),
        );
        feed(&[0]);
        feed(
            route
                .price
                .output_micros_per_million_tokens
                .to_string()
                .as_bytes(),
        );
        feed(&[0]);
        feed(
            route
                .price
                .reasoning_micros_per_million_tokens
                .unwrap_or(-1)
                .to_string()
                .as_bytes(),
        );
        feed(&[0]);
        feed(route.capabilities.max_output_tokens.to_string().as_bytes());
        feed(&[u8::from(route.capabilities.reasoning_included_in_output)]);
        feed(&[route.capabilities.billable_components as u8]);
        feed(&[0xff]);
    }
    format!("price-v2-{hash:016x}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_explicit_provider_model_routes_have_prices_and_capabilities() {
        let book = PriceBook::default();
        assert_eq!(
            book.price("opencode-go", "glm-5.3-flash")
                .expect("listed route has a price")
                .input_micros_per_million_tokens,
            150_000
        );
        let capability = book
            .capabilities("opencode-go", "glm-5.3-flash")
            .expect("listed route has capabilities");
        assert_eq!(capability.max_output_tokens, OUTPUT_CAP);
        assert!(capability.reasoning_included_in_output);
        assert_eq!(
            capability.billable_components,
            BillableComponents::TextTokensAndFunctionTools
        );
        assert!(matches!(
            book.price("unknown-provider", "unknown-model"),
            Err(AiError::UnsupportedUsageRoute { .. })
        ));
        assert!(matches!(
            book.capabilities("zai", "glm-5.3-flash"),
            Err(AiError::UnsupportedUsageRoute { .. })
        ));
    }

    #[test]
    fn route_overrides_change_prices_without_expanding_the_capability_allowlist() {
        let book = PriceBook::new([
            (
                "opencode-go".to_owned(),
                "glm-5.3-flash".to_owned(),
                ModelPrice {
                    input_micros_per_million_tokens: 2_000_000,
                    output_micros_per_million_tokens: 3_000_000,
                    reasoning_micros_per_million_tokens: None,
                },
            ),
            (
                "unknown".to_owned(),
                "model".to_owned(),
                ModelPrice {
                    input_micros_per_million_tokens: 1,
                    output_micros_per_million_tokens: 1,
                    reasoning_micros_per_million_tokens: None,
                },
            ),
        ]);
        assert_eq!(
            book.estimate_cost_micros("opencode-go", "glm-5.3-flash", 1_000_000, 1)
                .expect("listed route estimate"),
            2_000_003
        );
        assert!(matches!(
            book.price("unknown", "model"),
            Err(AiError::UnsupportedUsageRoute { .. })
        ));
    }

    #[test]
    fn separate_reasoning_rate_is_refused_until_a_route_declares_its_billing() {
        let book = PriceBook::new([(
            "opencode-go".to_owned(),
            "glm-5.3".to_owned(),
            ModelPrice {
                input_micros_per_million_tokens: 1,
                output_micros_per_million_tokens: 1,
                reasoning_micros_per_million_tokens: Some(1),
            },
        )]);
        assert!(matches!(
            book.price("opencode-go", "glm-5.3"),
            Err(AiError::UnsupportedUsageRoute { .. })
        ));
    }

    #[test]
    fn optional_provider_usage_is_required_before_actual_cost_is_calculated() {
        let observation = crate::UsageObservation {
            input_tokens: None,
            output_tokens: Some(1),
            reasoning_tokens: None,
            commercial_units: None,
            provider_request_id: None,
            provider_response_id: None,
        };
        assert!(!observation.has_token_usage());
    }
}
