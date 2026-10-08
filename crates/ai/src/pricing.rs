//! Per-model price table used to estimate the cost of every model call.
//!
//! Prices are micro-dollars (1e-6 USD) per one million tokens. Providers
//! return token counts but not cost, so cost is always an estimate.
//!
//! The OpenCode Go subscription is a flat monthly fee with dollar caps per
//! model, not a per-token bill. The per-project budget is therefore a soft
//! guard in USD-equivalent: each call is priced at the published per-token
//! rate for its model, so the budget tracks how much subscription a project
//! has used. Configuration can override any provider and model.

use std::collections::BTreeMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ModelPrice {
    pub input_micros_per_million_tokens: i64,
    pub output_micros_per_million_tokens: i64,
}

/// glm-5.3-flash: US$0.15 / 1M input tokens, US$0.50 / 1M output tokens. This
/// is OpenCode's published price (https://opencode.ai/docs/zen) and matches
/// Z.AI's. Cached-input discounts (US$0.03 / 1M) are ignored, so cost is a
/// slight over-estimate, which is the safe direction for a spending cap.
pub const GLM_5_3_FLASH_PRICE: ModelPrice = ModelPrice {
    input_micros_per_million_tokens: 150_000,
    output_micros_per_million_tokens: 500_000,
};

/// glm-5.3: US$1.40 / 1M input, US$4.40 / 1M output (OpenCode's published price).
pub const GLM_5_3_PRICE: ModelPrice = ModelPrice {
    input_micros_per_million_tokens: 1_400_000,
    output_micros_per_million_tokens: 4_400_000,
};

/// kimi-k2.6: US$0.95 / 1M input, US$4.00 / 1M output (OpenCode's published price).
pub const KIMI_K2_6_PRICE: ModelPrice = ModelPrice {
    input_micros_per_million_tokens: 950_000,
    output_micros_per_million_tokens: 4_000_000,
};

/// deepseek-v4-flash: US$0.14 / 1M input, US$0.28 / 1M output (OpenCode's published price).
pub const DEEPSEEK_V4_FLASH_PRICE: ModelPrice = ModelPrice {
    input_micros_per_million_tokens: 140_000,
    output_micros_per_million_tokens: 280_000,
};

/// Used for any model that is not in the table. Deliberately pessimistic so an
/// unknown model can only exhaust the budget sooner, never later.
pub const UNKNOWN_MODEL_PRICE: ModelPrice = ModelPrice {
    input_micros_per_million_tokens: 3_000_000,
    output_micros_per_million_tokens: 15_000_000,
};

/// Built-in prices by model id. The ids are the same on OpenCode Go and Z.AI.
const PRICES_BY_MODEL: [(&str, ModelPrice); 4] = [
    ("glm-5.3-flash", GLM_5_3_FLASH_PRICE),
    ("glm-5.3", GLM_5_3_PRICE),
    ("kimi-k2.6", KIMI_K2_6_PRICE),
    ("deepseek-v4-flash", DEEPSEEK_V4_FLASH_PRICE),
];

pub fn price_for_model(model: &str) -> ModelPrice {
    let model = model.trim().to_ascii_lowercase();
    if model.starts_with("glm-5.3-flash-") {
        return GLM_5_3_FLASH_PRICE;
    }
    PRICES_BY_MODEL
        .iter()
        .find(|(name, _)| *name == model)
        .map_or(UNKNOWN_MODEL_PRICE, |(_, price)| *price)
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

/// Prices by provider and model. Configured overrides win; otherwise the
/// built-in table prices the model under any provider.
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
            .unwrap_or_else(|| price_for_model(model))
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
    fn glm_flash_cost_matches_published_prices() {
        // 1M in + 100K out = $0.15 + $0.05 = $0.20 (per the pricing page).
        assert_eq!(
            estimate_cost_micros("glm-5.3-flash", 1_000_000, 100_000),
            200_000
        );
        assert_eq!(estimate_cost_micros("glm-5.3-flash", 1, 0), 1);
        assert_eq!(estimate_cost_micros("glm-5.3-flash", 0, 0), 0);
        assert_eq!(
            estimate_cost_micros("glm-5.3-flash-preview", 1_000_000, 0),
            150_000
        );
    }

    #[test]
    fn opencode_models_are_priced_at_their_published_rates() {
        assert_eq!(price_for_model("glm-5.3"), GLM_5_3_PRICE);
        assert_eq!(price_for_model("GLM-5.3-Flash"), GLM_5_3_FLASH_PRICE);
        assert_eq!(price_for_model("kimi-k2.6"), KIMI_K2_6_PRICE);
        assert_eq!(
            price_for_model("deepseek-v4-flash"),
            DEEPSEEK_V4_FLASH_PRICE
        );
    }

    #[test]
    fn unknown_models_are_priced_pessimistically() {
        assert!(
            estimate_cost_micros("mystery", 1_000, 1_000)
                > estimate_cost_micros("glm-5.3-flash", 1_000, 1_000)
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
            },
        )]);
        // The override applies to its provider only.
        assert_eq!(
            book.estimate_cost_micros("opencode-go", "glm-5.3-flash", 1_000_000, 0),
            2_000_000
        );
        assert_eq!(
            book.estimate_cost_micros("zai", "glm-5.3-flash", 1_000_000, 0),
            150_000
        );
        assert_eq!(
            book.price("opencode-go", "glm-5.3"),
            GLM_5_3_PRICE,
            "models without an override keep the built-in price"
        );
    }
}
