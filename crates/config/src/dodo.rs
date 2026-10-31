//! Explicit optional Dodo configuration. No prices or live defaults.
use crate::{ConfigError, PublicOrigin};
use deepref_domain::billing::Entitlements;
use secrecy::{ExposeSecret, SecretString};
use serde::Deserialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DodoMode {
    Test,
    Live,
}
impl DodoMode {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Test => "test_mode",
            Self::Live => "live_mode",
        }
    }
    pub const fn api_origin(self) -> &'static str {
        match self {
            Self::Test => "https://test.dodopayments.com",
            Self::Live => "https://live.dodopayments.com",
        }
    }
    pub const fn checkout_host(self) -> &'static str {
        match self {
            Self::Test => "test.checkout.dodopayments.com",
            Self::Live => "checkout.dodopayments.com",
        }
    }
    pub const fn portal_host(self) -> &'static str {
        match self {
            Self::Test => "test.customer.dodopayments.com",
            Self::Live => "customer.dodopayments.com",
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DodoOffering {
    /// Closed application offering: primary or secondary. Not a price/plan name.
    pub offering: String,
    pub product_id: String,
    pub entitlements: Entitlements,
}

#[derive(Debug, Clone)]
pub struct DodoConfig {
    pub mode: DodoMode,
    pub business_id: String,
    pub offerings: Vec<DodoOffering>,
    pub public_origin: PublicOrigin,
    token: SecretString,
    webhook_keys: Vec<SecretString>,
}

impl DodoConfig {
    pub(crate) fn from_source(
        get: &dyn Fn(&str) -> Option<String>,
        public_origin: PublicOrigin,
    ) -> Result<Option<Self>, ConfigError> {
        let Some([token, business_id, mode, key]) = crate::section(
            "Dodo",
            get,
            [
                "DODO_API_KEY",
                "DODO_BUSINESS_ID",
                "DODO_MODE",
                "DODO_WEBHOOK_KEY",
            ],
        )?
        else {
            return Ok(None);
        };
        let invalid = |name, reason: &str| ConfigError::Invalid {
            name,
            reason: reason.into(),
        };
        let mode = match mode.as_str() {
            "test_mode" => DodoMode::Test,
            "live_mode" if crate::flag(get, "DODO_LIVE_ENABLED")? => DodoMode::Live,
            _ => {
                return Err(invalid(
                    "DODO_MODE",
                    "explicit test_mode required; live_mode additionally requires DODO_LIVE_ENABLED",
                ));
            }
        };
        if business_id.len() > 128
            || !business_id
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'_')
        {
            return Err(invalid("DODO_BUSINESS_ID", "invalid business identifier"));
        }
        let offerings: Vec<DodoOffering> =
            serde_json::from_str(&get("DODO_OFFERINGS_JSON").unwrap_or_else(|| "[]".into()))
                .map_err(|_| invalid("DODO_OFFERINGS_JSON", "invalid reviewed offering map"))?;
        if offerings.len() > 2
            || offerings.iter().any(|o| {
                !matches!(o.offering.as_str(), "primary" | "secondary")
                    || o.product_id.is_empty()
                    || o.product_id.len() > 128
                    || !o
                        .product_id
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b == b'_')
            })
            || (offerings.len() == 2
                && (offerings[0].offering == offerings[1].offering
                    || offerings[0].product_id == offerings[1].product_id))
        {
            return Err(invalid(
                "DODO_OFFERINGS_JSON",
                "offerings and products must be closed, unique identifiers",
            ));
        }
        let mut webhook_keys = vec![SecretString::from(key)];
        if let Some(previous) = crate::optional(get, "DODO_WEBHOOK_KEY_PREVIOUS") {
            webhook_keys.push(SecretString::from(previous));
        }
        Ok(Some(Self {
            mode,
            business_id,
            offerings,
            public_origin,
            token: token.into(),
            webhook_keys,
        }))
    }
    pub fn token(&self) -> &str {
        self.token.expose_secret()
    }
    pub fn webhook_keys(&self) -> impl Iterator<Item = &str> {
        self.webhook_keys.iter().map(|k| k.expose_secret())
    }
}
