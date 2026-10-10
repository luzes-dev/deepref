use deepref_ai::{ModelProfile, ProviderEndpoint};
use deepref_domain::{ProjectId, ProtocolVersionId, ScreeningStage};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{
    BuildProvenance, CompiledReviewDefinition, ReviewDefinitionKey, ReviewError, ReviewHash,
    ReviewOrigin, ReviewSemanticContract, ReviewSubject,
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReviewModelIdentity {
    pub profile: ModelProfile,
    pub provider: String,
    pub model: String,
    pub model_version: String,
    pub parameters_hash: ReviewHash,
    /// The normalized endpoint this route is sent to, as the scheduling process was configured.
    /// Absent when that process had no endpoint for the provider, and in manifests persisted
    /// before the endpoint was recorded; a configured worker refuses such an unpinned route.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub endpoint: Option<ProviderEndpoint>,
}

impl ReviewModelIdentity {
    fn validate(&self) -> Result<(), ReviewError> {
        if self.provider.trim().is_empty()
            || self.model.trim().is_empty()
            || self.model_version.trim().is_empty()
        {
            return Err(ReviewError::InvalidDefinition(
                "resolved model identity is incomplete".to_owned(),
            ));
        }
        Ok(())
    }
}

/// Where and how a manifest was built, recorded for audit. None of these
/// values enters the semantic contract; they feed [`BuildProvenance`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReviewRuntimeIdentity {
    /// Source-tree hash (`DEEPREF_SOURCE_TREE_SHA`). Audit only.
    pub build_sha: ReviewHash,
    pub rust_version: String,
    pub target: String,
    /// Deployment build id from `DEEPREF_BUILD_SHA`, when the build set one.
    /// Audit only. Absent in manifests persisted before it existed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deployment_build_id: Option<String>,
}

impl ReviewRuntimeIdentity {
    fn validate(&self) -> Result<(), ReviewError> {
        if self.rust_version.trim().is_empty()
            || self.target.trim().is_empty()
            || self
                .deployment_build_id
                .as_deref()
                .is_some_and(|id| id.trim().is_empty())
        {
            return Err(ReviewError::InvalidDefinition(
                "runtime identity is incomplete".to_owned(),
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReviewManifestInput {
    pub project_id: ProjectId,
    pub subject: ReviewSubject,
    pub origin: ReviewOrigin,
    pub protocol_version_id: Option<ProtocolVersionId>,
    pub protocol_hash: ReviewHash,
    pub source_manifest_hash: ReviewHash,
    pub source_content_hash: ReviewHash,
    pub resolved_models: Vec<ReviewModelIdentity>,
    pub runtime: ReviewRuntimeIdentity,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReviewRunManifest {
    pub project_id: ProjectId,
    pub definition: ReviewDefinitionKey,
    pub definition_id: String,
    pub definition_version: u32,
    pub definition_hash: ReviewHash,
    pub subject: ReviewSubject,
    pub origin: ReviewOrigin,
    pub protocol_version_id: Option<ProtocolVersionId>,
    pub protocol_hash: ReviewHash,
    pub source_manifest_hash: ReviewHash,
    pub source_content_hash: ReviewHash,
    /// Legacy behavior-identity hashes, retained alongside
    /// [`semantic_contract`][Self::semantic_contract] — not dead.
    ///
    /// Readers that still need them:
    /// * [`fingerprint_node`], via `ReviewCatalogIdentity::from_manifest`
    ///   below: node reuse is gated on the stored bundle identity matching
    ///   the compiled definition, so a manifest from another behavior can
    ///   never sponsor node fingerprints.
    /// * [`manifest_hash`][Self::manifest_hash]: the tamper-evident manifest
    ///   digest covers these fields, so a stored row cannot silently swap
    ///   behavior without changing its hash.
    /// * Forensic audit: the stored manifest JSON reconstructs exactly which
    ///   behavior ran, independent of the current definition sources.
    /// * Legacy rows persisted before the contract migration carry no
    ///   `semantic_contract`; these hashes are their only behavior identity
    ///   (see the legacy-deserialization test).
    ///
    /// Calibration compatibility gates on `semantic_contract` only; nothing
    /// here implies calibration.
    pub workflow_hash: ReviewHash,
    pub prompt_bundle_hash: ReviewHash,
    pub schema_bundle_hash: ReviewHash,
    pub policy_hash: ReviewHash,
    pub parser_bundle_hash: ReviewHash,
    pub resolved_models: Vec<ReviewModelIdentity>,
    pub runtime: ReviewRuntimeIdentity,
    /// The structured semantic contract: the only identity that gates
    /// calibration compatibility. Absent in manifests persisted before the
    /// contract migration; those manifests can never admit new calibration.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub semantic_contract: Option<ReviewSemanticContract>,
    /// What exact software produced this run: audit only, never calibration.
    #[serde(default)]
    pub provenance: BuildProvenance,
    pub manifest_hash: ReviewHash,
}

impl ReviewRunManifest {
    /// The structured semantic contract id of this run: what
    /// `review_run_manifests.semantic_bundle_hash` and calibration bundles
    /// store. Absent in manifests persisted before the contract migration.
    pub fn semantic_contract_id(&self) -> Result<crate::SemanticContractId, ReviewError> {
        self.semantic_contract
            .as_ref()
            .map(|contract| contract.id())
            .ok_or_else(|| {
                ReviewError::InvalidDefinition(
                    "compiled review manifest has no semantic contract".to_owned(),
                )
            })
    }
}

#[derive(Serialize)]
struct ManifestWithoutOwnHash<'a> {
    project_id: ProjectId,
    definition: ReviewDefinitionKey,
    definition_id: &'a str,
    definition_version: u32,
    definition_hash: &'a ReviewHash,
    subject: &'a ReviewSubject,
    origin: ReviewOrigin,
    protocol_version_id: Option<ProtocolVersionId>,
    protocol_hash: &'a ReviewHash,
    source_manifest_hash: &'a ReviewHash,
    source_content_hash: &'a ReviewHash,
    workflow_hash: &'a ReviewHash,
    prompt_bundle_hash: &'a ReviewHash,
    schema_bundle_hash: &'a ReviewHash,
    policy_hash: &'a ReviewHash,
    parser_bundle_hash: &'a ReviewHash,
    resolved_models: &'a [ReviewModelIdentity],
    runtime: &'a ReviewRuntimeIdentity,
}

impl ReviewRunManifest {
    pub fn build(
        definition: &CompiledReviewDefinition,
        mut input: ReviewManifestInput,
    ) -> Result<Self, ReviewError> {
        if input.project_id.as_uuid().is_nil() {
            return Err(ReviewError::InvalidProjectId);
        }
        if definition.key() != input.subject.definition_key() {
            return Err(ReviewError::SubjectDefinitionMismatch {
                definition: definition.key(),
                subject: input.subject.definition_key(),
            });
        }
        input.runtime.validate()?;
        input
            .resolved_models
            .iter()
            .try_for_each(|model| model.validate())?;
        input.resolved_models.sort_by(|left, right| {
            left.profile
                .as_str()
                .cmp(right.profile.as_str())
                .then_with(|| left.provider.cmp(&right.provider))
                .then_with(|| left.model.cmp(&right.model))
                .then_with(|| left.model_version.cmp(&right.model_version))
        });
        if input
            .resolved_models
            .windows(2)
            .any(|models| models[0].profile == models[1].profile)
        {
            return Err(ReviewError::InvalidDefinition(
                "resolved model profiles must be unique".to_owned(),
            ));
        }

        let identity = definition.identity();
        let semantic_contract = ReviewSemanticContract::for_review(
            definition,
            subject_stage(&input.subject),
            &input.protocol_hash,
            &input.resolved_models,
        )?;
        let provenance =
            BuildProvenance::for_manifest(definition.key(), &input.runtime, &input.resolved_models);
        let manifest_hash = ReviewHash::digest_input(&ManifestWithoutOwnHash {
            project_id: input.project_id,
            definition: definition.key(),
            definition_id: &identity.definition_id,
            definition_version: identity.definition_version,
            definition_hash: &identity.declared_assets_hash,
            subject: &input.subject,
            origin: input.origin,
            protocol_version_id: input.protocol_version_id,
            protocol_hash: &input.protocol_hash,
            source_manifest_hash: &input.source_manifest_hash,
            source_content_hash: &input.source_content_hash,
            workflow_hash: &identity.workflow_hash,
            prompt_bundle_hash: &identity.prompt_bundle_hash,
            schema_bundle_hash: &identity.schema_bundle_hash,
            policy_hash: &identity.policy_hash,
            parser_bundle_hash: &identity.parser_bundle_hash,
            resolved_models: &input.resolved_models,
            runtime: &input.runtime,
        })?;

        Ok(Self {
            project_id: input.project_id,
            definition: definition.key(),
            definition_id: identity.definition_id.clone(),
            definition_version: identity.definition_version,
            definition_hash: identity.declared_assets_hash.clone(),
            subject: input.subject,
            origin: input.origin,
            protocol_version_id: input.protocol_version_id,
            protocol_hash: input.protocol_hash,
            source_manifest_hash: input.source_manifest_hash,
            source_content_hash: input.source_content_hash,
            workflow_hash: identity.workflow_hash.clone(),
            prompt_bundle_hash: identity.prompt_bundle_hash.clone(),
            schema_bundle_hash: identity.schema_bundle_hash.clone(),
            policy_hash: identity.policy_hash.clone(),
            parser_bundle_hash: identity.parser_bundle_hash.clone(),
            resolved_models: input.resolved_models,
            runtime: input.runtime,
            semantic_contract: Some(semantic_contract),
            provenance,
            manifest_hash,
        })
    }
}

/// The screening stage of a subject, if it is a screening subject.
fn subject_stage(subject: &ReviewSubject) -> Option<ScreeningStage> {
    match subject {
        ReviewSubject::Screening { stage, .. } => Some(*stage),
        _ => None,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AcceptedArtifactInput {
    pub artifact_id: Uuid,
    pub content_hash: ReviewHash,
}

#[derive(Serialize)]
struct FingerprintInput<'a> {
    manifest_hash: &'a ReviewHash,
    node_id: &'a str,
    node_version: u32,
    predecessor_artifacts: &'a [AcceptedArtifactInputForHash<'a>],
}

#[derive(Serialize)]
struct AcceptedArtifactInputForHash<'a> {
    artifact_id: Uuid,
    content_hash: &'a ReviewHash,
}

pub fn fingerprint_node(
    definition: &CompiledReviewDefinition,
    manifest: &ReviewRunManifest,
    node_id: &str,
    predecessor_artifacts: &[AcceptedArtifactInput],
) -> Result<ReviewHash, ReviewError> {
    if definition.key() != manifest.definition
        || definition.identity().declared_assets_hash
            != ReviewCatalogIdentity::from_manifest(manifest, definition)?
    {
        return Err(ReviewError::InvalidDefinition(
            "manifest does not belong to the compiled definition".to_owned(),
        ));
    }
    let node_version = definition
        .node_version(node_id)
        .ok_or_else(|| ReviewError::InvalidWorkflow(format!("unknown node {node_id}")))?;
    let mut predecessors = predecessor_artifacts.iter().collect::<Vec<_>>();
    predecessors.sort_by_key(|artifact| artifact.artifact_id);
    if predecessors
        .windows(2)
        .any(|artifacts| artifacts[0].artifact_id == artifacts[1].artifact_id)
    {
        return Err(ReviewError::InvalidWorkflow(
            "predecessor artifacts must be unique".to_owned(),
        ));
    }
    let predecessor_artifacts = predecessors
        .into_iter()
        .map(|artifact| AcceptedArtifactInputForHash {
            artifact_id: artifact.artifact_id,
            content_hash: &artifact.content_hash,
        })
        .collect::<Vec<_>>();
    ReviewHash::digest_input(&FingerprintInput {
        manifest_hash: &manifest.manifest_hash,
        node_id,
        node_version,
        predecessor_artifacts: &predecessor_artifacts,
    })
}

struct ReviewCatalogIdentity;

impl ReviewCatalogIdentity {
    fn from_manifest(
        manifest: &ReviewRunManifest,
        definition: &CompiledReviewDefinition,
    ) -> Result<ReviewHash, ReviewError> {
        let identity = definition.identity();
        if manifest.definition_id != identity.definition_id
            || manifest.definition_version != identity.definition_version
            || manifest.definition_hash != identity.declared_assets_hash
            || manifest.workflow_hash != identity.workflow_hash
            || manifest.prompt_bundle_hash != identity.prompt_bundle_hash
            || manifest.schema_bundle_hash != identity.schema_bundle_hash
            || manifest.policy_hash != identity.policy_hash
            || manifest.parser_bundle_hash != identity.parser_bundle_hash
        {
            return Err(ReviewError::InvalidDefinition(
                "manifest asset identity does not match definition".to_owned(),
            ));
        }
        Ok(identity.declared_assets_hash.clone())
    }
}

#[cfg(test)]
#[path = "manifest_tests.rs"]
mod tests;
