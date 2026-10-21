//! Generic token-cost calculations and a provider/model rate table.
//!
//! Prices are micro-dollars (1e-6 USD) per one million tokens. Providers
//! return token counts but not cost, so cost is always an estimate.
//!
//! Concrete provider rates are supplied by configuration; this module has no
//! provider-specific model prices. A conservative generic fallback is used
//! when configuration does not contain a route price.

use std::collections::BTreeMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ModelPrice {
    pub input_micros_per_million_tokens: i64,
    pub output_micros_per_million_tokens: i64,
    /// Separately billed reasoning tokens, if the provider publishes a rate.
    /// When absent, reasoning remains included in output-token pricing.
    pub reasoning_micros_per_million_tokens: Option<i64>,
}

/// Generic conservative fallback for an unconfigured route; provider-specific
/// prices belong in configuration and are passed to [`PriceBook::new`].
pub const UNKNOWN_MODEL_PRICE: ModelPrice = ModelPrice {
    input_micros_per_million_tokens: 3_000_000,
    output_micros_per_million_tokens: 15_000_000,
    reasoning_micros_per_million_tokens: None,
};

pub fn price_for_model(_model: &str) -> ModelPrice {
    UNKNOWN_MODEL_PRICE
}

/// Estimated cost in micro-dollars, rounded up to the next micro-dollar.
pub fn estimate_cost_micros(model: &str, input_tokens: u64, output_tokens: u64) -> i64 {
    cost_micros(price_for_model(model), input_tokens, output_tokens)
}

fn cost_micros(price: ModelPrice, input_tokens: u64, output_tokens: u64) -> i64 {
    let total = u128::from(input_tokens)
        * u128::from(price.input_micros_per_million_tokens.unsigned_abs())
        + u128::from(output_tokens)
            * u128::from(price.output_micros_per_million_tokens.unsigned_abs());
    i64::try_from(total.div_ceil(1_000_000)).unwrap_or(i64::MAX)
}

/// Prices by provider and model. Route rates are supplied by configuration;
/// missing routes use the same conservative fallback regardless of model ID.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PriceBook {
    overrides: BTreeMap<(String, String), ModelPrice>,
}

impl PriceBook {
    pub fn new(overrides: impl IntoIterator<Item = (String, String, ModelPrice)>) -> Self {
        Self {
            overrides: overrides
                .into_iter()
                .map(|(provider, model, price)| ((provider, model), price))
                .collect(),
        }
    }

    pub fn price(&self, provider: &str, model: &str) -> ModelPrice {
        self.overrides
            .get(&(provider.to_owned(), model.to_owned()))
            .copied()
            .unwrap_or(UNKNOWN_MODEL_PRICE)
    }

    /// Stable value identity for this fallback and configured rate table.
    /// The version is derived from rates, not from a provider's billing schema.
    pub fn version(&self) -> String {
        let mut hash = 0xcbf2_9ce4_8422_2325_u64;
        let mut feed = |bytes: &[u8]| {
            for byte in bytes {
                hash ^= u64::from(*byte);
                hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
            }
        };
        feed(b"conservative-fallback");
        feed(&[0]);
        feed(
            UNKNOWN_MODEL_PRICE
                .input_micros_per_million_tokens
                .to_string()
                .as_bytes(),
        );
        feed(&[0]);
        feed(
            UNKNOWN_MODEL_PRICE
                .output_micros_per_million_tokens
                .to_string()
                .as_bytes(),
        );
        feed(&[0]);
        feed(b"none");
        feed(&[0xff]);
        for ((provider, model), price) in &self.overrides {
            feed(provider.as_bytes());
            feed(&[0]);
            feed(model.as_bytes());
            feed(&[0]);
            feed(price.input_micros_per_million_tokens.to_string().as_bytes());
            feed(&[0]);
            feed(
                price
                    .output_micros_per_million_tokens
                    .to_string()
                    .as_bytes(),
            );
            feed(&[0]);
            feed(
                price
                    .reasoning_micros_per_million_tokens
                    .map_or_else(|| "none".to_owned(), |rate| rate.to_string())
                    .as_bytes(),
            );
            feed(&[0xfe]);
        }
        format!("price-v1-{hash:016x}")
    }

    /// Estimated cost in micro-dollars of one call on `provider`/`model`.
    pub fn estimate_cost_micros(
        &self,
        provider: &str,
        model: &str,
        input_tokens: u64,
        output_tokens: u64,
    ) -> i64 {
        cost_micros(self.price(provider, model), input_tokens, output_tokens)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn route_rates_are_injected_and_unconfigured_routes_use_the_generic_fallback() {
        let book = PriceBook::default();
        assert_eq!(price_for_model("any-model"), UNKNOWN_MODEL_PRICE);
        assert_eq!(book.price("provider-a", "model-a"), UNKNOWN_MODEL_PRICE);
        assert_eq!(
            estimate_cost_micros("any-model", 1_000_000, 100_000),
            4_500_000
        );
    }

    #[test]
    fn configured_prices_apply_per_provider_and_model() {
        let book = PriceBook::new([(
            "opencode-go".to_owned(),
            "glm-5.3-flash".to_owned(),
            ModelPrice {
                input_micros_per_million_tokens: 2_000_000,
                output_micros_per_million_tokens: 0,
                reasoning_micros_per_million_tokens: None,
            },
        )]);
        // The configured rate applies only to its provider/model identity.
        assert_eq!(
            book.estimate_cost_micros("opencode-go", "glm-5.3-flash", 1_000_000, 0),
            2_000_000
        );
        assert_eq!(
            book.estimate_cost_micros("zai", "glm-5.3-flash", 1_000_000, 0),
            UNKNOWN_MODEL_PRICE.input_micros_per_million_tokens
        );
        assert_eq!(
            book.price("opencode-go", "glm-5.3"),
            UNKNOWN_MODEL_PRICE,
            "models without an override use the generic fallback"
        );
    }
}
