use std::str::FromStr;

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// A service that answers bibliographic metadata for a DOI. The names are the
/// deployment chain's vocabulary and are stored as provenance. `lowercase`
/// matches `as_str`; `snake_case` would spell `OpenAlex` as `open_alex`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ProviderKind {
    OpenAlex,
    Crossref,
    DataCite,
}

impl ProviderKind {
    pub const ALL: [Self; 3] = [Self::OpenAlex, Self::Crossref, Self::DataCite];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::OpenAlex => "openalex",
            Self::Crossref => "crossref",
            Self::DataCite => "datacite",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("unknown metadata provider: {0}")]
pub struct InvalidProviderKind(pub String);

impl FromStr for ProviderKind {
    type Err = InvalidProviderKind;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::ALL
            .into_iter()
            .find(|kind| kind.as_str() == value)
            .ok_or_else(|| InvalidProviderKind(value.to_owned()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_provider_round_trips_through_its_configured_name() {
        for kind in ProviderKind::ALL {
            assert_eq!(kind.as_str().parse::<ProviderKind>(), Ok(kind));
        }
    }

    #[test]
    fn serde_uses_the_configured_names() {
        for kind in ProviderKind::ALL {
            let json = serde_json::to_string(&kind).expect("provider should serialize");
            assert_eq!(json, format!("\"{}\"", kind.as_str()));
            assert_eq!(
                serde_json::from_str::<ProviderKind>(&json).expect("provider should parse"),
                kind
            );
        }
    }

    #[test]
    fn names_are_exact_and_other_spellings_are_rejected() {
        for invalid in [
            "",
            " crossref",
            "Crossref",
            "open_alex",
            "data_cite",
            "pubmed",
        ] {
            assert_eq!(
                invalid.parse::<ProviderKind>(),
                Err(InvalidProviderKind(invalid.to_owned())),
                "{invalid:?} should not name a provider"
            );
        }
    }
}
