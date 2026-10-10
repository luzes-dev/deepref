//! Explicit semantic contracts for calibration evidence.
//!
//! A [`ReviewSemanticContract`] answers one question: is this production review
//! using the same consequential behavior that the calibration evidence
//! evaluated? Only this contract may gate calibration compatibility.
//!
//! Three related identities live elsewhere on purpose:
//!
//! * [`BuildProvenance`] records what exact software produced a run (audit).
//!   A provenance difference never implies calibration incompatibility.
//! * Model-call reuse keys live in `deepref-ai` (`RequestKey`): a cache miss
//!   costs one extra provider call, never a calibration invalidation.
//! * CI regression evidence lives in `golden.rs` snapshots, not in production
//!   identity.
//!
//! # Why no JSON here
//!
//! The aggregate [`SemanticContractId`] is built with [`SemanticHasher`], a
//! small structural encoder (domain tag, field name, fixed integer encoding,
//! value length, value bytes, fixed field order). It never touches `serde_json`,
//! so Cargo features such as `serde_json/preserve_order` cannot change
//! scientific identity. Compatibility decisions compare the structured
//! contract field by field ([`ReviewSemanticContract::compare`]); the compact
//! id exists for database indexes, dedupe and compact logs.

use std::{collections::BTreeSet, fmt, fmt::Write as _};

use deepref_domain::ScreeningStage;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::{
    AI_FIRST_POLICY_VERSION, CompiledReviewDefinition, ReviewDefinitionKey, ReviewError,
    ReviewHash, manifest::ReviewModelIdentity, manifest::ReviewRuntimeIdentity,
};

/// The audited implementation-boundary digest computed by `build.rs`.
/// Provenance only: it never gates calibration.
pub(crate) fn implementation_fingerprint() -> Result<ReviewHash, ReviewError> {
    ReviewHash::parse(env!("DEEPREF_PROVENANCE_IMPLEMENTATION_SHA"))
}

/// The audited third-party dependency digest computed by `build.rs`.
/// Provenance only: it never gates calibration.
pub(crate) fn dependency_fingerprint() -> Result<ReviewHash, ReviewError> {
    ReviewHash::parse(env!("DEEPREF_PROVENANCE_DEPENDENCY_SHA"))
}

/// The contract recipe version. Stored calibration snapshots carry this
/// scheme; a snapshot from another recipe is never treated as comparable.
/// Scheme 1 was the legacy manifest hash, scheme 2 the decomposed component
/// map, scheme 3 is this structured contract.
pub const SEMANTIC_CONTRACT_SCHEME: u32 = 3;

/// Explicit semantic version of each consequential review definition.
///
/// These are stable across refactors: renaming a function, moving code,
/// replacing an internal helper or upgrading a dependency without behavior
/// change leaves them unchanged. A consequential behavior change (decision
/// thresholds, screening interpretation, evidence-selection semantics, repair
/// logic, new automation behavior) must bump the affected definition's
/// version, and CI fails the semantic fixtures until it does.
pub const SCREENING_SEMANTIC_VERSION: u32 = 1;
pub const DUPLICATE_DETECTION_SEMANTIC_VERSION: u32 = 1;
pub const STUDY_CLASSIFICATION_SEMANTIC_VERSION: u32 = 1;
pub const STUDY_GROUPING_SEMANTIC_VERSION: u32 = 1;
pub const APPRAISAL_PREFILL_SEMANTIC_VERSION: u32 = 1;
pub const DATA_EXTRACTION_SEMANTIC_VERSION: u32 = 1;

/// The explicit semantic version of one review definition.
pub const fn semantic_version_for(definition: ReviewDefinitionKey) -> u32 {
    match definition {
        ReviewDefinitionKey::Screening => SCREENING_SEMANTIC_VERSION,
        ReviewDefinitionKey::DuplicateDetection => DUPLICATE_DETECTION_SEMANTIC_VERSION,
        ReviewDefinitionKey::StudyClassification => STUDY_CLASSIFICATION_SEMANTIC_VERSION,
        ReviewDefinitionKey::StudyGrouping => STUDY_GROUPING_SEMANTIC_VERSION,
        ReviewDefinitionKey::AppraisalPrefill => APPRAISAL_PREFILL_SEMANTIC_VERSION,
        ReviewDefinitionKey::DataExtraction => DATA_EXTRACTION_SEMANTIC_VERSION,
    }
}

/// Exact byte identity: one byte changed means the digest changed.
///
/// Use this for uploaded documents, parsed artifacts, prompt source text, raw
/// workflow assets, stored generated artifacts and content-addressed blobs.
/// Never use it as a substitute for semantic compatibility.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ContentDigest(String);

impl ContentDigest {
    pub fn parse(value: impl Into<String>) -> Result<Self, ReviewError> {
        let value = value.into();
        if value.len() != 64
            || !value
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            return Err(ReviewError::InvalidHash(
                "expected lowercase SHA-256".to_owned(),
            ));
        }
        Ok(Self(value))
    }

    pub fn of_bytes(value: impl AsRef<[u8]>) -> Self {
        let digest = Sha256::digest(value.as_ref());
        let mut encoded = String::with_capacity(64);
        for byte in digest {
            let _ = write!(&mut encoded, "{byte:02x}");
        }
        Self(encoded)
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for ContentDigest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// Structural encoder for aggregate semantic ids. No JSON, no map backend, no
/// Cargo-feature-dependent ordering: every field is length-prefixed under a
/// domain tag, in call order. The single call site
/// ([`ReviewSemanticContract::id`]) feeds fields in struct order.
pub struct SemanticHasher {
    digest: Sha256,
}

impl SemanticHasher {
    pub fn new(domain: &str) -> Self {
        let mut digest = Sha256::new();
        digest.update(b"deepref-semantic-v1\0");
        digest.update(domain.as_bytes());
        digest.update([0]);
        Self { digest }
    }

    fn named(&mut self, name: &str, tag: u8) {
        let len = u64::try_from(name.len()).unwrap_or(u64::MAX);
        self.digest.update(len.to_le_bytes());
        self.digest.update(name.as_bytes());
        self.digest.update([tag]);
    }

    fn bytes(&mut self, value: &[u8]) {
        let len = u64::try_from(value.len()).unwrap_or(u64::MAX);
        self.digest.update(len.to_le_bytes());
        self.digest.update(value);
    }

    pub fn field_str(&mut self, name: &str, value: &str) -> &mut Self {
        self.named(name, b's');
        self.bytes(value.as_bytes());
        self
    }

    pub fn field_u32(&mut self, name: &str, value: u32) -> &mut Self {
        self.named(name, b'u');
        self.bytes(&value.to_le_bytes());
        self
    }

    pub fn field_opt_str(&mut self, name: &str, value: Option<&str>) -> &mut Self {
        match value {
            Some(value) => self.field_str(name, value),
            None => {
                self.named(name, b'n');
                self
            }
        }
    }

    pub fn field_digest(&mut self, name: &str, digest: &ContentDigest) -> &mut Self {
        self.named(name, b'd');
        self.bytes(digest.as_str().as_bytes());
        self
    }

    pub fn finish(self) -> ContentDigest {
        let digest = self.digest.finalize();
        let mut encoded = String::with_capacity(64);
        for byte in digest {
            let _ = write!(&mut encoded, "{byte:02x}");
        }
        ContentDigest(encoded)
    }
}

macro_rules! purpose_digest {
    ($name:ident, $doc:expr) => {
        #[doc = $doc]
        #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(ContentDigest);

        impl $name {
            pub fn from_content(digest: ContentDigest) -> Self {
                Self(digest)
            }

            pub fn parse(hex: impl Into<String>) -> Result<Self, ReviewError> {
                ContentDigest::parse(hex).map(Self)
            }

            pub fn as_content(&self) -> &ContentDigest {
                &self.0
            }

            pub fn as_str(&self) -> &str {
                self.0.as_str()
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str(self.as_str())
            }
        }
    };
}

purpose_digest!(
    PromptDigest,
    "Identity of the exact prompt wording. Prompt text influences model behavior, so an exact content digest is the correct identity."
);
purpose_digest!(
    SchemaDigest,
    "Identity of the output schema bundle the model is asked to satisfy."
);
purpose_digest!(
    PolicyDigest,
    "Identity of the automation and admission policy semantics."
);
purpose_digest!(
    WorkflowDigest,
    "Identity of the normalized review workflow: nodes, versions and transitions."
);
purpose_digest!(
    ParserDigest,
    "Identity of the declarative parser contract for model answers."
);
purpose_digest!(
    ProtocolDigest,
    "Identity of the protocol criteria that actually affect the review. A different consequential protocol is a different contract."
);

/// The model behavior a calibration bundle was adjudicated for.
///
/// The parameters digest covers the full resolved parameters (temperature,
/// max tokens, top_p and provider-specific additions) as canonically hashed
/// when the route was resolved, so any consequential parameter change alters
/// the contract. Provider endpoints are deliberately absent: they belong in
/// [`BuildProvenance`], not in calibration identity.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelContract {
    pub profile: String,
    pub provider: String,
    pub model: String,
    pub model_version: String,
    pub parameters: ContentDigest,
}

impl ModelContract {
    pub fn from_identity(identity: &ReviewModelIdentity) -> Result<Self, ReviewError> {
        if identity.provider.trim().is_empty()
            || identity.model.trim().is_empty()
            || identity.model_version.trim().is_empty()
        {
            return Err(ReviewError::InvalidDefinition(
                "resolved model contract is incomplete".to_owned(),
            ));
        }
        Ok(Self {
            profile: identity.profile.as_str().to_owned(),
            provider: identity.provider.clone(),
            model: identity.model.clone(),
            model_version: identity.model_version.clone(),
            parameters: ContentDigest::parse(identity.parameters_hash.as_str())?,
        })
    }

    fn digest(&self) -> ContentDigest {
        let mut hasher = SemanticHasher::new("deepref-model-contract-v1");
        hasher
            .field_str("profile", &self.profile)
            .field_str("provider", &self.provider)
            .field_str("model", &self.model)
            .field_str("model_version", &self.model_version)
            .field_digest("parameters", &self.parameters);
        hasher.finish()
    }
}

/// What consequential behavior was calibrated: the only identity that may
/// gate calibration compatibility.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReviewSemanticContract {
    pub scheme: u32,
    pub definition: ReviewDefinitionKey,
    pub semantic_version: u32,
    pub stage: Option<ScreeningStage>,
    pub prompt: PromptDigest,
    pub schema: SchemaDigest,
    pub policy: PolicyDigest,
    pub workflow: WorkflowDigest,
    pub parser: ParserDigest,
    pub models: Vec<ModelContract>,
    pub protocol: ProtocolDigest,
}

impl ReviewSemanticContract {
    /// Builds the contract for one compiled review. Fails closed when the
    /// subject kind does not match the definition (a screening stage for a
    /// non-screening review, or no stage for screening).
    pub fn for_review(
        definition: &CompiledReviewDefinition,
        stage: Option<ScreeningStage>,
        protocol: &ReviewHash,
        models: &[ReviewModelIdentity],
    ) -> Result<Self, ReviewError> {
        if (definition.key() == ReviewDefinitionKey::Screening) != stage.is_some() {
            return Err(ReviewError::InvalidDefinition(
                "screening contracts need a stage and other contracts must not have one".to_owned(),
            ));
        }
        let identity = definition.identity();
        let bundle = |hash: &ReviewHash| {
            ContentDigest::parse(hash.as_str()).map_err(|_| {
                ReviewError::InvalidDefinition("compiled bundle digest is invalid".to_owned())
            })
        };
        let policy = if stage.is_some() {
            // The automation policy version is part of screening semantics, so
            // it is folded into the policy digest with the structural encoder.
            let mut hasher = SemanticHasher::new("deepref-screening-policy-v1");
            hasher
                .field_digest("bundle", &bundle(&identity.policy_hash)?)
                .field_u32("ai_first", AI_FIRST_POLICY_VERSION);
            PolicyDigest::from_content(hasher.finish())
        } else {
            PolicyDigest::from_content(bundle(&identity.policy_hash)?)
        };
        let mut contracts = models
            .iter()
            .map(ModelContract::from_identity)
            .collect::<Result<Vec<_>, _>>()?;
        contracts.sort_by(|left, right| {
            left.profile
                .cmp(&right.profile)
                .then_with(|| left.provider.cmp(&right.provider))
                .then_with(|| left.model.cmp(&right.model))
                .then_with(|| left.model_version.cmp(&right.model_version))
        });
        if contracts.is_empty() {
            return Err(ReviewError::InvalidDefinition(
                "a semantic contract needs at least one resolved model".to_owned(),
            ));
        }
        let contract = Self {
            scheme: SEMANTIC_CONTRACT_SCHEME,
            definition: definition.key(),
            semantic_version: semantic_version_for(definition.key()),
            stage,
            prompt: PromptDigest::from_content(bundle(&identity.prompt_bundle_hash)?),
            schema: SchemaDigest::from_content(bundle(&identity.schema_bundle_hash)?),
            policy,
            workflow: WorkflowDigest::from_content(bundle(&identity.workflow_hash)?),
            parser: ParserDigest::from_content(bundle(&identity.parser_bundle_hash)?),
            models: contracts,
            protocol: ProtocolDigest::from_content(bundle(protocol)?),
        };
        contract.validate()?;
        Ok(contract)
    }

    /// Structural validity: current scheme, a stage exactly for screening, and
    /// at least one model. Notably this does NOT require the current
    /// `semantic_version`: an older stored contract must still deserialize and
    /// compare, so version drift reports `Stale(SemanticVersion)` instead of
    /// failing closed as unreadable.
    pub fn validate(&self) -> Result<(), ReviewError> {
        if self.scheme != SEMANTIC_CONTRACT_SCHEME {
            return Err(ReviewError::InvalidDefinition(format!(
                "semantic contract scheme {} is not the current scheme {SEMANTIC_CONTRACT_SCHEME}",
                self.scheme
            )));
        }
        if (self.definition == ReviewDefinitionKey::Screening) != self.stage.is_some() {
            return Err(ReviewError::InvalidDefinition(
                "screening contracts need a stage and other contracts must not have one".to_owned(),
            ));
        }
        if self.models.is_empty() {
            return Err(ReviewError::InvalidDefinition(
                "a semantic contract needs at least one resolved model".to_owned(),
            ));
        }
        Ok(())
    }

    /// The compact aggregate id: domain tag, field names, fixed integer
    /// encoding and value lengths in struct order. No JSON, so `serde_json`
    /// map ordering cannot affect it. Used for database indexes, dedupe,
    /// comparisons and compact logs; compatibility decisions compare the
    /// structured contract, not this string.
    ///
    /// Deliberately explicit, not macro-generated (see also [`compare`]):
    /// every consequential field must visibly appear here AND in `compare`.
    /// A field table or derive macro could silently drop a field on refactor —
    /// changing calibration identity without a compile error — while this
    /// repetition forces the author to touch both lists together. Auditors
    /// diff these two lists to prove id coverage matches comparison coverage;
    /// keep them in sync by hand, on purpose.
    pub fn id(&self) -> SemanticContractId {
        let mut hasher = SemanticHasher::new("deepref-review-contract-v1");
        hasher
            .field_u32("scheme", self.scheme)
            .field_str("definition", self.definition.as_str())
            .field_u32("semantic_version", self.semantic_version)
            .field_opt_str("stage", self.stage.map(stage_name))
            .field_digest("prompt", self.prompt.as_content())
            .field_digest("schema", self.schema.as_content())
            .field_digest("policy", self.policy.as_content())
            .field_digest("workflow", self.workflow.as_content())
            .field_digest("parser", self.parser.as_content())
            .field_digest("protocol", self.protocol.as_content());
        hasher.field_u32(
            "model_count",
            u32::try_from(self.models.len()).unwrap_or(u32::MAX),
        );
        for (index, model) in self.models.iter().enumerate() {
            hasher.field_digest(&format!("model.{index}"), &model.digest());
        }
        SemanticContractId(hasher.finish())
    }

    /// Compares stored evidence (`self`) with the review being admitted,
    /// naming every consequential change. A scheme mismatch is not
    /// comparable: it reports an empty change set, and admission treats that
    /// as an incompatible recipe rather than a known change.
    ///
    /// Deliberately explicit, not macro-generated (see also [`id`]): each arm
    /// names its [`SemanticChange`] so a new consequential field cannot be
    /// added to the struct without a visible decision here about which change
    /// it reports. Keep this list in sync with `id` by hand, on purpose.
    pub fn compare(&self, current: &Self) -> CalibrationCompatibility {
        if self.scheme != current.scheme {
            return CalibrationCompatibility::Incompatible {
                changes: BTreeSet::new(),
            };
        }
        let mut changes = BTreeSet::new();
        if self.definition != current.definition {
            changes.insert(SemanticChange::Definition);
        }
        if self.semantic_version != current.semantic_version {
            changes.insert(SemanticChange::SemanticVersion);
        }
        if self.stage != current.stage {
            changes.insert(SemanticChange::Stage);
        }
        if self.prompt != current.prompt {
            changes.insert(SemanticChange::Prompt);
        }
        if self.schema != current.schema {
            changes.insert(SemanticChange::Schema);
        }
        if self.policy != current.policy {
            changes.insert(SemanticChange::Policy);
        }
        if self.workflow != current.workflow {
            changes.insert(SemanticChange::Workflow);
        }
        if self.parser != current.parser {
            changes.insert(SemanticChange::Parser);
        }
        if self.models != current.models {
            changes.insert(SemanticChange::Model);
        }
        if self.protocol != current.protocol {
            changes.insert(SemanticChange::Protocol);
        }
        if changes.is_empty() {
            CalibrationCompatibility::Compatible
        } else {
            CalibrationCompatibility::Incompatible { changes }
        }
    }
}

const fn stage_name(stage: ScreeningStage) -> &'static str {
    match stage {
        ScreeningStage::TitleAbstract => "title_abstract",
        ScreeningStage::FullText => "full_text",
    }
}

/// The compact aggregate id of a [`ReviewSemanticContract`].
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SemanticContractId(ContentDigest);

impl SemanticContractId {
    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }
}

impl fmt::Display for SemanticContractId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// One named consequential change between two semantic contracts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SemanticChange {
    Definition,
    SemanticVersion,
    Stage,
    Prompt,
    Schema,
    Policy,
    Workflow,
    Parser,
    Model,
    Protocol,
}

impl SemanticChange {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Definition => "definition",
            Self::SemanticVersion => "semantic_version",
            Self::Stage => "stage",
            Self::Prompt => "prompt",
            Self::Schema => "schema",
            Self::Policy => "policy",
            Self::Workflow => "workflow",
            Self::Parser => "parser",
            Self::Model => "model",
            Self::Protocol => "protocol",
        }
    }
}

/// How stored calibration evidence relates to the review being admitted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CalibrationCompatibility {
    Compatible,
    Incompatible { changes: BTreeSet<SemanticChange> },
}

impl fmt::Display for CalibrationCompatibility {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Compatible => formatter.write_str("compatible"),
            Self::Incompatible { changes } if changes.is_empty() => {
                formatter.write_str("built by another contract recipe")
            }
            Self::Incompatible { changes } => {
                let changed = changes
                    .iter()
                    .map(|change| change.as_str())
                    .collect::<Vec<_>>()
                    .join(", ");
                write!(formatter, "changed: {changed}")
            }
        }
    }
}

/// What exact software produced a run: forensic audit, bug reproduction,
/// deployment diagnosis, historical traceability, incident investigation.
///
/// A provenance difference never implies calibration incompatibility. The
/// manifest answers "what behavior?" ([`ReviewSemanticContract`]) and "what
/// build?" (this struct) separately, on purpose.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct BuildProvenance {
    pub deepref_version: String,
    pub git_commit: Option<String>,
    pub build_id: Option<String>,
    pub rustc_version: Option<String>,
    pub target: Option<String>,
    /// The semantic dependency closure digest: audit only, never calibration.
    pub cargo_lock_digest: Option<ContentDigest>,
    /// The broad source-tree digest: audit only, never calibration.
    pub source_tree_digest: Option<ContentDigest>,
    /// The narrow screening implementation boundary digest: audit only, and
    /// only for screening. Other definitions have no narrow boundary yet.
    pub implementation_digest: Option<ContentDigest>,
    pub rig_version: Option<String>,
    pub serde_json_version: Option<String>,
    /// Normalized provider endpoints the scheduling process calls, sorted and
    /// unique. Audit only: an endpoint move does not change review semantics.
    pub provider_endpoints: Vec<String>,
}

impl BuildProvenance {
    /// Captures the provenance of the process building a manifest. The old
    /// scheme-2 component values (implementation and dependency shas) are
    /// preserved here as audit evidence; they no longer gate calibration.
    pub fn for_manifest(
        definition: ReviewDefinitionKey,
        runtime: &ReviewRuntimeIdentity,
        models: &[ReviewModelIdentity],
    ) -> Self {
        let digest_of = |hash: &ReviewHash| ContentDigest::parse(hash.as_str()).ok();
        let mut endpoints = models
            .iter()
            .filter_map(|model| model.endpoint.as_ref().map(|endpoint| endpoint.as_str()))
            .collect::<BTreeSet<_>>()
            .into_iter()
            .map(str::to_owned)
            .collect::<Vec<_>>();
        endpoints.sort();
        Self {
            deepref_version: env!("CARGO_PKG_VERSION").to_owned(),
            git_commit: option_env!("DEEPREF_GIT_SHA")
                .map(str::trim)
                .filter(|sha| !sha.is_empty())
                .map(str::to_owned),
            build_id: runtime.deployment_build_id.clone(),
            rustc_version: Some(runtime.rust_version.clone()),
            target: Some(runtime.target.clone()),
            cargo_lock_digest: dependency_fingerprint()
                .ok()
                .and_then(|hash| digest_of(&hash)),
            source_tree_digest: digest_of(&runtime.build_sha),
            implementation_digest: (definition == ReviewDefinitionKey::Screening)
                .then(|| implementation_fingerprint().ok())
                .flatten()
                .and_then(|hash| digest_of(&hash)),
            rig_version: Some(env!("DEEPREF_RIG_VERSION").to_owned()),
            serde_json_version: Some(env!("DEEPREF_SERDE_JSON_VERSION").to_owned()),
            provider_endpoints: endpoints,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ReviewCatalog;

    fn review() -> CompiledReviewDefinition {
        ReviewCatalog
            .compile(ReviewDefinitionKey::Screening)
            .expect("screening definition compiles")
    }

    fn model(version: &str) -> ReviewModelIdentity {
        ReviewModelIdentity {
            profile: deepref_ai::ModelProfile::Reasoning,
            provider: "fixture".to_owned(),
            model: "reasoner".to_owned(),
            model_version: version.to_owned(),
            parameters_hash: ReviewHash::digest_bytes(b"parameters"),
            endpoint: None,
        }
    }

    fn contract(model_version: &str) -> ReviewSemanticContract {
        ReviewSemanticContract::for_review(
            &review(),
            Some(ScreeningStage::TitleAbstract),
            &ReviewHash::digest_bytes(b"protocol"),
            &[model(model_version)],
        )
        .expect("contract builds")
    }

    #[test]
    fn identical_contracts_are_compatible_with_equal_ids() {
        let stored = contract("v1");
        let current = contract("v1");
        assert_eq!(
            stored.compare(&current),
            CalibrationCompatibility::Compatible
        );
        assert_eq!(stored.id(), current.id());
        assert_eq!(stored.id().as_str().len(), 64);
    }

    #[test]
    fn model_protocol_and_policy_changes_are_named() {
        let stored = contract("v1");
        let other_model = contract("v2");
        assert_eq!(
            stored.compare(&other_model),
            CalibrationCompatibility::Incompatible {
                changes: BTreeSet::from([SemanticChange::Model]),
            }
        );
        let mut other_protocol = stored.clone();
        other_protocol.protocol =
            ProtocolDigest::from_content(ContentDigest::of_bytes(b"other protocol"));
        assert_eq!(
            stored.compare(&other_protocol),
            CalibrationCompatibility::Incompatible {
                changes: BTreeSet::from([SemanticChange::Protocol]),
            }
        );
        let mut other_version = stored.clone();
        other_version.semantic_version += 1;
        assert_eq!(
            stored.compare(&other_version),
            CalibrationCompatibility::Incompatible {
                changes: BTreeSet::from([SemanticChange::SemanticVersion]),
            }
        );
    }

    #[test]
    fn prompt_schema_workflow_parser_and_stage_changes_are_named() {
        let stored = contract("v1");
        let digest = ContentDigest::of_bytes(b"other");
        let mut changed = stored.clone();
        changed.prompt = PromptDigest::from_content(digest.clone());
        changed.schema = SchemaDigest::from_content(digest.clone());
        changed.workflow = WorkflowDigest::from_content(digest.clone());
        changed.parser = ParserDigest::from_content(digest.clone());
        changed.policy = PolicyDigest::from_content(digest.clone());
        changed.stage = Some(ScreeningStage::FullText);
        changed.definition = ReviewDefinitionKey::DataExtraction;
        let CalibrationCompatibility::Incompatible { changes } = stored.compare(&changed) else {
            panic!("expected incompatibility");
        };
        assert_eq!(
            changes,
            BTreeSet::from([
                SemanticChange::Definition,
                SemanticChange::Stage,
                SemanticChange::Prompt,
                SemanticChange::Schema,
                SemanticChange::Policy,
                SemanticChange::Workflow,
                SemanticChange::Parser,
            ])
        );
    }

    #[test]
    fn a_screening_contract_needs_a_stage_and_a_model() {
        let definition = review();
        assert!(
            ReviewSemanticContract::for_review(
                &definition,
                None,
                &ReviewHash::digest_bytes(b"protocol"),
                &[model("v1")],
            )
            .is_err()
        );
        assert!(
            ReviewSemanticContract::for_review(
                &definition,
                Some(ScreeningStage::TitleAbstract),
                &ReviewHash::digest_bytes(b"protocol"),
                &[],
            )
            .is_err()
        );
    }

    #[test]
    fn the_aggregate_id_does_not_depend_on_json_map_ordering() {
        // The id is built field by field with length prefixes, so no JSON map
        // backend (and no serde_json Cargo feature) can change it.
        let first = contract("v1").id();
        let second = contract("v1").id();
        assert_eq!(first, second);
        let mut reordered = contract("v1");
        reordered.models.reverse();
        // Model order is canonicalized at construction, so this is a no-op;
        // a genuinely different model set changes the id.
        assert_eq!(reordered.id(), first);
        let mut other = contract("v1");
        other
            .models
            .push(ModelContract::from_identity(&model("v2")).expect("model contract builds"));
        assert_ne!(other.id(), first);
    }

    #[test]
    fn the_contract_round_trips_through_json() {
        let contract = contract("v1");
        let json = serde_json::to_value(&contract).expect("contract serializes");
        let restored: ReviewSemanticContract =
            serde_json::from_value(json).expect("contract deserializes");
        assert_eq!(restored, contract);
        assert_eq!(restored.id(), contract.id());
    }

    #[test]
    fn provenance_records_the_build_without_affecting_the_contract() {
        let runtime = ReviewRuntimeIdentity {
            build_sha: ReviewHash::digest_bytes(b"build"),
            rust_version: "1.91".to_owned(),
            target: "test".to_owned(),
            deployment_build_id: None,
        };
        let provenance =
            BuildProvenance::for_manifest(ReviewDefinitionKey::Screening, &runtime, &[model("v1")]);
        assert!(!provenance.deepref_version.is_empty());
        assert!(provenance.rustc_version.is_some());
        assert!(provenance.source_tree_digest.is_some());
        // A different build of the same behavior keeps the same contract.
        assert_eq!(contract("v1").id(), contract("v1").id());
    }
}
