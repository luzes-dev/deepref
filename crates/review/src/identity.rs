//! Decomposable semantic identity for calibration evidence.
//!
//! The aggregate semantic-bundle hash answers "same or different". Calibration
//! admission also needs to say *what* changed, so every compiled manifest keeps
//! the named components the aggregate was built from. The snapshot is stored
//! beside each calibration bundle and compared component by component when the
//! aggregate differs.
//!
//! Runtime provenance (toolchain, target, deployment build id) is recorded in the
//! manifest for audit and is never a component: changing the compiler or the
//! deployment does not, by itself, change screening semantics. The source-tree
//! hash is also recorded for audit. It is the implementation component of the
//! definitions that have no narrow boundary yet, as described below.
//!
//! # Scheme 2 recipe
//!
//! The aggregate `semantic_bundle_hash` is the SHA-256 of the canonical JSON of a
//! [`SemanticIdentity`], whose components are:
//!
//! | component | value |
//! |---|---|
//! | `definition`, `prompt`, `schema`, `policy`, `parser` | the compiled definition: id, version, declared assets and workflow graph, and the checked-in bundles under `review-definitions/` |
//! | `protocol` | the protocol criteria hash of the subject's task |
//! | `models` | the resolved model routes: profile, provider, model, version and parameters |
//! | `implementation` | screening: `DEEPREF_SEMANTIC_IMPLEMENTATION_SHA`, the narrow boundary of screening code. Other definitions: the broad source-tree hash, until their own boundary is declared |
//! | `dependencies` | `DEEPREF_SEMANTIC_DEPENDENCY_SHA`, the closure of the semantic third-party crates from `Cargo.lock`, with the manifest lines that declare them |
//!
//! The boundary table, its exclusions and the dependency roots are in
//! `crates/review/build_support/fingerprint.rs`, which `build.rs` runs. The
//! `implementation` and `dependencies` values are fixed when the crate is built,
//! so a manifest records the code that produced it.
//!
//! The `Implementation` value for the other definitions is the broad hash on
//! purpose. Their task code (`dedupe.rs`, `classification.rs`,
//! `review_assistance.rs`, appraisal application code) is not in the screening
//! boundary yet. A narrow value there would silently stop catching changes to
//! that code.

use std::collections::{BTreeMap, BTreeSet};

use deepref_domain::ScreeningStage;
use serde::{Deserialize, Serialize};

use crate::{ReviewDefinitionKey, ReviewError, ReviewHash};

/// The identity recipe version. A snapshot is only comparable with snapshots
/// built by the same recipe; hashes from another scheme are never treated as
/// equivalent, even when some component values happen to match.
pub const SEMANTIC_IDENTITY_SCHEME: u32 = 2;

/// The semantic dependency closure that `build.rs` computed, as an audit
/// listing: roots, excluded third-party crates, every closure package with its
/// source and checksum, and the manifest lines that declare a root.
#[doc(hidden)]
pub const SEMANTIC_DEPENDENCIES: &str =
    include_str!(concat!(env!("OUT_DIR"), "/semantic_dependencies.txt"));

/// The narrow screening implementation fingerprint computed by `build.rs`.
pub(crate) fn implementation_fingerprint() -> Result<ReviewHash, ReviewError> {
    ReviewHash::parse(env!("DEEPREF_SEMANTIC_IMPLEMENTATION_SHA"))
}

/// The semantic third-party dependency fingerprint computed by `build.rs`.
pub(crate) fn dependency_fingerprint() -> Result<ReviewHash, ReviewError> {
    ReviewHash::parse(env!("DEEPREF_SEMANTIC_DEPENDENCY_SHA"))
}

/// One named input to the aggregate semantic identity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IdentityComponent {
    /// Definition id, version, declared asset manifest and workflow graph.
    Definition,
    /// Checked-in prompt bundle.
    Prompt,
    /// Checked-in output schema bundle.
    Schema,
    /// Checked-in automation and admission policy.
    Policy,
    /// Checked-in declarative parser bundle.
    Parser,
    /// Protocol criteria the task judges.
    Protocol,
    /// Resolved model routes: profile, provider, model, version and parameters.
    Models,
    /// Secret-safe identity of the provider endpoint each route is sent to.
    ProviderEndpoint,
    /// Canonical model requests rendered from deterministic fixtures.
    GoldenRender,
    /// Interpreted outcomes of deterministic model-response fixtures.
    GoldenParse,
    /// Source of the semantic implementation: the narrow screening boundary, or
    /// the broad source-tree hash for definitions without a declared boundary.
    Implementation,
    /// The semantic third-party dependency closure and its declared features.
    Dependencies,
}

impl IdentityComponent {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Definition => "definition",
            Self::Prompt => "prompt",
            Self::Schema => "schema",
            Self::Policy => "policy",
            Self::Parser => "parser",
            Self::Protocol => "protocol",
            Self::Models => "models",
            Self::ProviderEndpoint => "provider_endpoint",
            Self::GoldenRender => "golden_render",
            Self::GoldenParse => "golden_parse",
            Self::Implementation => "implementation",
            Self::Dependencies => "dependencies",
        }
    }
}

/// The immutable identity snapshot of one compiled review for one subject kind.
///
/// `stage` is set for screening and absent for every other definition, so a
/// title/abstract identity can never equal a full-text identity.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SemanticIdentity {
    pub scheme: u32,
    pub definition: ReviewDefinitionKey,
    pub stage: Option<ScreeningStage>,
    pub components: BTreeMap<IdentityComponent, ReviewHash>,
}

/// How a stored identity relates to the identity of the review being admitted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IdentityComparison {
    Same,
    /// Built by another recipe, or with another component set: nothing about
    /// which component changed can be inferred.
    IncompatibleScheme {
        stored: u32,
        current: u32,
    },
    DefinitionMismatch {
        stored: ReviewDefinitionKey,
        current: ReviewDefinitionKey,
    },
    StageMismatch {
        stored: Option<ScreeningStage>,
        current: Option<ScreeningStage>,
    },
    /// Same recipe, same subject kind; these components changed.
    Stale(BTreeSet<IdentityComponent>),
}

impl SemanticIdentity {
    pub fn new(definition: ReviewDefinitionKey, stage: Option<ScreeningStage>) -> Self {
        Self {
            scheme: SEMANTIC_IDENTITY_SCHEME,
            definition,
            stage,
            components: BTreeMap::new(),
        }
    }

    pub fn with(mut self, component: IdentityComponent, hash: ReviewHash) -> Self {
        self.components.insert(component, hash);
        self
    }

    /// The aggregate semantic-bundle hash. Field order is fixed by the struct
    /// and components are ordered by the map, so the encoding is canonical.
    pub fn aggregate_hash(&self) -> Result<ReviewHash, ReviewError> {
        ReviewHash::digest_json(self)
    }

    /// The components the current recipe requires for this definition.
    pub fn required_components(definition: ReviewDefinitionKey) -> BTreeSet<IdentityComponent> {
        use IdentityComponent as C;
        let mut required = BTreeSet::from([
            C::Definition,
            C::Prompt,
            C::Schema,
            C::Policy,
            C::Parser,
            C::Protocol,
            C::Models,
            C::Implementation,
        ]);
        if definition == ReviewDefinitionKey::Screening {
            required.extend(SCREENING_ONLY_COMPONENTS);
        }
        required.extend(SCHEME_COMPONENTS);
        required
    }

    /// Structural validity: current scheme, stage present exactly for screening,
    /// and exactly the component set the recipe defines.
    pub fn validate(&self) -> Result<(), ReviewError> {
        if self.scheme != SEMANTIC_IDENTITY_SCHEME {
            return Err(ReviewError::InvalidDefinition(format!(
                "semantic identity scheme {} is not the current scheme {SEMANTIC_IDENTITY_SCHEME}",
                self.scheme
            )));
        }
        if (self.definition == ReviewDefinitionKey::Screening) != self.stage.is_some() {
            return Err(ReviewError::InvalidDefinition(
                "screening identities need a stage and other identities must not have one"
                    .to_owned(),
            ));
        }
        let present = self.components.keys().copied().collect::<BTreeSet<_>>();
        if present != Self::required_components(self.definition) {
            return Err(ReviewError::InvalidDefinition(
                "semantic identity components do not match the current recipe".to_owned(),
            ));
        }
        Ok(())
    }

    /// Compares stored evidence (`self`) with the review being admitted.
    pub fn compare(&self, current: &Self) -> IdentityComparison {
        if self.scheme != current.scheme || self.components.keys().ne(current.components.keys()) {
            return IdentityComparison::IncompatibleScheme {
                stored: self.scheme,
                current: current.scheme,
            };
        }
        if self.definition != current.definition {
            return IdentityComparison::DefinitionMismatch {
                stored: self.definition,
                current: current.definition,
            };
        }
        if self.stage != current.stage {
            return IdentityComparison::StageMismatch {
                stored: self.stage,
                current: current.stage,
            };
        }
        let changed = self
            .components
            .iter()
            .filter(|(component, hash)| current.components.get(component) != Some(hash))
            .map(|(component, _)| *component)
            .collect::<BTreeSet<_>>();
        if changed.is_empty() {
            IdentityComparison::Same
        } else {
            IdentityComparison::Stale(changed)
        }
    }
}

/// Components only screening has: its golden fixtures are screening-specific.
const SCREENING_ONLY_COMPONENTS: [IdentityComponent; 0] = [];

/// Components every definition has under the current scheme in addition to the
/// declarative ones.
const SCHEME_COMPONENTS: [IdentityComponent; 1] = [IdentityComponent::Dependencies];

#[cfg(test)]
mod tests {
    use super::*;

    fn hash(value: &str) -> ReviewHash {
        ReviewHash::digest_bytes(value)
    }

    fn identity(stage: ScreeningStage) -> SemanticIdentity {
        SemanticIdentity::required_components(ReviewDefinitionKey::Screening)
            .into_iter()
            .fold(
                SemanticIdentity::new(ReviewDefinitionKey::Screening, Some(stage)),
                |identity, component| identity.with(component, hash(component.as_str())),
            )
    }

    #[test]
    fn identical_identities_compare_same_and_hash_equal() {
        let stored = identity(ScreeningStage::TitleAbstract);
        let current = identity(ScreeningStage::TitleAbstract);
        stored.validate().expect("identity is valid");
        assert_eq!(stored.compare(&current), IdentityComparison::Same);
        assert_eq!(
            stored.aggregate_hash().expect("hash"),
            current.aggregate_hash().expect("hash")
        );
    }

    #[test]
    fn stage_is_part_of_the_aggregate_and_compares_as_a_mismatch() {
        let title = identity(ScreeningStage::TitleAbstract);
        let full = identity(ScreeningStage::FullText);
        assert_ne!(
            title.aggregate_hash().expect("hash"),
            full.aggregate_hash().expect("hash")
        );
        assert!(matches!(
            title.compare(&full),
            IdentityComparison::StageMismatch { .. }
        ));
    }

    #[test]
    fn changed_components_are_named() {
        let stored = identity(ScreeningStage::FullText);
        let current = identity(ScreeningStage::FullText)
            .with(IdentityComponent::Models, hash("other model"))
            .with(IdentityComponent::Protocol, hash("other protocol"));
        assert_eq!(
            stored.compare(&current),
            IdentityComparison::Stale(BTreeSet::from([
                IdentityComponent::Protocol,
                IdentityComponent::Models,
            ]))
        );
    }

    #[test]
    fn a_different_component_set_is_incompatible_rather_than_stale() {
        let stored = identity(ScreeningStage::TitleAbstract);
        let mut current = identity(ScreeningStage::TitleAbstract);
        current.components.remove(&IdentityComponent::Policy);
        assert!(matches!(
            stored.compare(&current),
            IdentityComparison::IncompatibleScheme { .. }
        ));
        let mut other_scheme = identity(ScreeningStage::TitleAbstract);
        other_scheme.scheme = SEMANTIC_IDENTITY_SCHEME + 1;
        assert!(matches!(
            stored.compare(&other_scheme),
            IdentityComparison::IncompatibleScheme { .. }
        ));
    }

    #[test]
    fn validation_requires_stage_only_for_screening() {
        let mut missing_stage = identity(ScreeningStage::TitleAbstract);
        missing_stage.stage = None;
        assert!(missing_stage.validate().is_err());
        let extraction = SemanticIdentity::required_components(ReviewDefinitionKey::DataExtraction)
            .into_iter()
            .fold(
                SemanticIdentity::new(
                    ReviewDefinitionKey::DataExtraction,
                    Some(ScreeningStage::FullText),
                ),
                |identity, component| identity.with(component, hash(component.as_str())),
            );
        assert!(extraction.validate().is_err());
    }
}
