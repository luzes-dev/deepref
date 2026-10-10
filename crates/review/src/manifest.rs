use deepref_ai::{ModelProfile, ProviderEndpoint};
use deepref_domain::{ProjectId, ProtocolVersionId};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{
    CompiledReviewDefinition, ReviewDefinitionKey, ReviewError, ReviewHash, ReviewOrigin,
    ReviewSubject,
    identity::{
        IdentityComponent, SemanticIdentity, dependency_fingerprint, implementation_fingerprint,
    },
    screening_golden_fingerprints,
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

/// Where and how a manifest was built, recorded for audit. `rust_version`,
/// `target` and `deployment_build_id` never enter a semantic identity. See the
/// scheme 2 recipe in `identity`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReviewRuntimeIdentity {
    /// Source-tree hash (`DEEPREF_SOURCE_TREE_SHA`). Audit only, except that it
    /// is the implementation component of definitions without a narrow boundary.
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
    pub workflow_hash: ReviewHash,
    pub prompt_bundle_hash: ReviewHash,
    pub schema_bundle_hash: ReviewHash,
    pub policy_hash: ReviewHash,
    pub parser_bundle_hash: ReviewHash,
    pub resolved_models: Vec<ReviewModelIdentity>,
    pub runtime: ReviewRuntimeIdentity,
    /// Named components of `semantic_bundle_hash`. Manifests persisted before
    /// identity scheme 2 have none and can never match current evidence.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub semantic_identity: Option<SemanticIdentity>,
    pub semantic_bundle_hash: ReviewHash,
    pub manifest_hash: ReviewHash,
}

impl ReviewRunManifest {
    /// The identity recipe this manifest was built with; 1 for legacy manifests.
    pub fn identity_scheme(&self) -> u32 {
        self.semantic_identity
            .as_ref()
            .map_or(1, |identity| identity.scheme)
    }
}

#[derive(Serialize)]
struct DefinitionComponent<'a> {
    definition_id: &'a str,
    definition_version: u32,
    definition_hash: &'a ReviewHash,
    workflow_hash: &'a ReviewHash,
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
    semantic_identity: &'a SemanticIdentity,
    semantic_bundle_hash: &'a ReviewHash,
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
        let semantic_identity = semantic_identity(definition, &input)?;
        semantic_identity.validate()?;
        let semantic_bundle_hash = semantic_identity.aggregate_hash()?;
        let manifest_hash = ReviewHash::digest_json(&ManifestWithoutOwnHash {
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
            semantic_identity: &semantic_identity,
            semantic_bundle_hash: &semantic_bundle_hash,
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
            semantic_identity: Some(semantic_identity),
            semantic_bundle_hash,
            manifest_hash,
        })
    }
}

/// Builds the scheme-2 identity of one compiled review for one subject.
///
/// Runtime provenance is deliberately absent: only `Implementation` speaks for
/// the code, and it is computed from the semantic boundary rather than from
/// the deployment.
fn semantic_identity(
    definition: &CompiledReviewDefinition,
    input: &ReviewManifestInput,
) -> Result<SemanticIdentity, ReviewError> {
    let identity = definition.identity();
    let stage = match &input.subject {
        ReviewSubject::Screening { stage, .. } => Some(*stage),
        _ => None,
    };
    let mut semantic = SemanticIdentity::new(definition.key(), stage)
        .with(
            IdentityComponent::Definition,
            ReviewHash::digest_json(&DefinitionComponent {
                definition_id: &identity.definition_id,
                definition_version: identity.definition_version,
                definition_hash: &identity.declared_assets_hash,
                workflow_hash: &identity.workflow_hash,
            })?,
        )
        .with(
            IdentityComponent::Prompt,
            identity.prompt_bundle_hash.clone(),
        )
        .with(
            IdentityComponent::Schema,
            identity.schema_bundle_hash.clone(),
        )
        .with(
            IdentityComponent::Policy,
            if stage.is_some() {
                ReviewHash::digest_json(&(
                    identity.policy_hash.clone(),
                    crate::AI_FIRST_POLICY_VERSION,
                ))?
            } else {
                identity.policy_hash.clone()
            },
        )
        .with(
            IdentityComponent::Parser,
            identity.parser_bundle_hash.clone(),
        )
        .with(IdentityComponent::Protocol, input.protocol_hash.clone())
        .with(
            IdentityComponent::Models,
            ReviewHash::digest_json(&routes_without_endpoints(&input.resolved_models))?,
        )
        .with(
            IdentityComponent::ProviderEndpoint,
            ReviewHash::digest_json(&provider_endpoints(&input.resolved_models))?,
        )
        .with(
            IdentityComponent::Implementation,
            implementation_component(definition.key(), input)?,
        )
        .with(IdentityComponent::Dependencies, dependency_fingerprint()?);
    if definition.key() == ReviewDefinitionKey::Screening {
        // Behavioural fingerprints of screening; a failure here fails the manifest.
        let (golden_render, golden_parse) = screening_golden_fingerprints()?;
        semantic = semantic
            .with(IdentityComponent::GoldenRender, golden_render)
            .with(IdentityComponent::GoldenParse, golden_parse);
    }
    Ok(semantic)
}

/// The `Implementation` value of a definition.
///
/// Screening uses the narrow boundary computed at build time. Every other
/// definition keeps the broad source-tree hash that was recorded before the
/// identity was decomposed, because its task code is outside the narrow
/// boundary and a narrow value there would stop catching changes to that code.
/// For those definitions `build_sha` is the one runtime field that enters the
/// semantic identity. `rust_version`, `target` and the deployment id never do.
fn implementation_component(
    definition: ReviewDefinitionKey,
    input: &ReviewManifestInput,
) -> Result<ReviewHash, ReviewError> {
    match definition {
        ReviewDefinitionKey::Screening => implementation_fingerprint(),
        _ => Ok(input.runtime.build_sha.clone()),
    }
}

/// The resolved routes without their endpoints. The endpoint is its own component, so an
/// endpoint change is reported as `ProviderEndpoint` alone, and the `Models` hash stays what it
/// was before endpoints were recorded.
fn routes_without_endpoints(models: &[ReviewModelIdentity]) -> Vec<ReviewModelIdentity> {
    models
        .iter()
        .map(|model| ReviewModelIdentity {
            endpoint: None,
            ..model.clone()
        })
        .collect()
}

/// The endpoint each profile is sent to, in the sorted model order. `null` where the endpoint
/// is unknown.
fn provider_endpoints(
    models: &[ReviewModelIdentity],
) -> Vec<(ModelProfile, Option<&ProviderEndpoint>)> {
    models
        .iter()
        .map(|model| (model.profile, model.endpoint.as_ref()))
        .collect()
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
    ReviewHash::digest_json(&FingerprintInput {
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
mod tests {
    use std::collections::BTreeSet;

    use super::*;
    use crate::{IdentityComparison, ReviewCatalog, ReviewDefinitionKey};
    use deepref_domain::{Actor, ActorKind, ReportId, ScreeningStage};

    fn hash(value: &str) -> ReviewHash {
        ReviewHash::digest_bytes(value)
    }

    fn manifest(definition: &CompiledReviewDefinition) -> ReviewRunManifest {
        let project_id = ProjectId::new(Uuid::new_v4());
        ReviewRunManifest::build(
            definition,
            ReviewManifestInput {
                project_id,
                subject: ReviewSubject::Screening {
                    report_id: ReportId::new(Uuid::new_v4()),
                    stage: ScreeningStage::TitleAbstract,
                    protocol_version_id: ProtocolVersionId::new(Uuid::new_v4()),
                    expected_revision: 0,
                },
                origin: ReviewOrigin::ReviewerRequested,
                protocol_version_id: None,
                protocol_hash: hash("protocol"),
                source_manifest_hash: hash("manifest"),
                source_content_hash: hash("source"),
                resolved_models: vec![ReviewModelIdentity {
                    profile: ModelProfile::Reasoning,
                    provider: "fixture".to_owned(),
                    model: "reasoning".to_owned(),
                    model_version: "v1".to_owned(),
                    parameters_hash: hash("parameters"),
                    endpoint: None,
                }],
                runtime: ReviewRuntimeIdentity {
                    build_sha: hash("build"),
                    rust_version: "1.91".to_owned(),
                    target: "test".to_owned(),
                    deployment_build_id: None,
                },
            },
        )
        .expect("manifest should build")
    }

    fn rebuild(
        definition: &CompiledReviewDefinition,
        manifest: ReviewRunManifest,
    ) -> ReviewRunManifest {
        ReviewRunManifest::build(
            definition,
            ReviewManifestInput {
                project_id: manifest.project_id,
                subject: manifest.subject,
                origin: manifest.origin,
                protocol_version_id: manifest.protocol_version_id,
                protocol_hash: manifest.protocol_hash,
                source_manifest_hash: manifest.source_manifest_hash,
                source_content_hash: manifest.source_content_hash,
                resolved_models: manifest.resolved_models,
                runtime: manifest.runtime,
            },
        )
        .expect("manifest should rebuild")
    }

    #[test]
    fn source_changes_manifest_but_not_semantic_bundle() {
        let definition = ReviewCatalog
            .compile(ReviewDefinitionKey::Screening)
            .expect("definition should compile");
        let first = manifest(&definition);
        let mut second = first.clone();
        second.source_content_hash = hash("different-source");
        let rebuilt = ReviewRunManifest::build(
            &definition,
            ReviewManifestInput {
                project_id: second.project_id,
                subject: second.subject,
                origin: second.origin,
                protocol_version_id: second.protocol_version_id,
                protocol_hash: second.protocol_hash,
                source_manifest_hash: second.source_manifest_hash,
                source_content_hash: second.source_content_hash,
                resolved_models: second.resolved_models,
                runtime: second.runtime,
            },
        )
        .expect("manifest should rebuild");
        assert_eq!(first.semantic_bundle_hash, rebuilt.semantic_bundle_hash);
        assert_ne!(first.manifest_hash, rebuilt.manifest_hash);
    }

    #[test]
    fn fingerprints_are_order_independent_and_node_specific() {
        let definition = ReviewCatalog
            .compile(ReviewDefinitionKey::Screening)
            .expect("definition should compile");
        let manifest = manifest(&definition);
        let left = AcceptedArtifactInput {
            artifact_id: Uuid::new_v4(),
            content_hash: hash("left"),
        };
        let right = AcceptedArtifactInput {
            artifact_id: Uuid::new_v4(),
            content_hash: hash("right"),
        };
        let forward = fingerprint_node(
            &definition,
            &manifest,
            "validate_primary",
            &[left.clone(), right.clone()],
        )
        .expect("fingerprint should build");
        let reverse = fingerprint_node(&definition, &manifest, "validate_primary", &[right, left])
            .expect("fingerprint should build");
        let other_node = fingerprint_node(&definition, &manifest, "derive_primary", &[])
            .expect("fingerprint should build");
        assert_eq!(forward, reverse);
        assert_ne!(forward, other_node);

        let mut tampered = manifest;
        tampered.definition_hash = hash("different-definition");
        assert!(matches!(
            fingerprint_node(&definition, &tampered, "derive_primary", &[]),
            Err(ReviewError::InvalidDefinition(_))
        ));
    }

    #[test]
    fn protocol_and_model_changes_invalidate_semantics_and_node_reuse() {
        let definition = ReviewCatalog
            .compile(ReviewDefinitionKey::Screening)
            .expect("definition should compile");
        let original = manifest(&definition);
        let original_fingerprint = fingerprint_node(&definition, &original, "prepare", &[])
            .expect("fingerprint should build");

        let mut protocol = original.clone();
        protocol.protocol_hash = hash("changed-protocol");
        let mut model = original.clone();
        model.resolved_models[0].model_version = "v2".to_owned();

        for (identity, changed) in [
            ("protocol", rebuild(&definition, protocol)),
            ("resolved model", rebuild(&definition, model)),
        ] {
            let changed_fingerprint = fingerprint_node(&definition, &changed, "prepare", &[])
                .expect("changed fingerprint should build");
            assert_ne!(
                original.semantic_bundle_hash, changed.semantic_bundle_hash,
                "{identity} must invalidate the semantic bundle"
            );
            assert_ne!(
                original_fingerprint, changed_fingerprint,
                "{identity} must invalidate node reuse"
            );
        }
    }

    fn endpoint(raw: &str) -> ProviderEndpoint {
        ProviderEndpoint::from_configured_url(raw).unwrap_or_else(|_| unreachable!())
    }

    #[test]
    fn an_endpoint_change_is_reported_as_provider_endpoint_alone() {
        let definition = ReviewCatalog
            .compile(ReviewDefinitionKey::Screening)
            .expect("definition should compile");
        let original = manifest(&definition);
        let mut moved = original.clone();
        moved.resolved_models[0].endpoint = Some(endpoint("https://opencode.ai/zen/go/v1"));
        let moved = rebuild(&definition, moved);
        assert_ne!(original.semantic_bundle_hash, moved.semantic_bundle_hash);
        let mut other = moved.clone();
        other.resolved_models[0].endpoint = Some(endpoint("https://backup.example/zen/go/v1"));
        let other = rebuild(&definition, other);
        let stored = original.semantic_identity.as_ref().expect("identity");
        assert_eq!(
            stored.compare(moved.semantic_identity.as_ref().expect("identity")),
            IdentityComparison::Stale(BTreeSet::from([IdentityComponent::ProviderEndpoint]))
        );
        assert_eq!(
            moved
                .semantic_identity
                .as_ref()
                .expect("identity")
                .compare(other.semantic_identity.as_ref().expect("identity")),
            IdentityComparison::Stale(BTreeSet::from([IdentityComponent::ProviderEndpoint]))
        );
    }

    #[test]
    fn a_model_change_is_reported_as_models_alone() {
        let definition = ReviewCatalog
            .compile(ReviewDefinitionKey::Screening)
            .expect("definition should compile");
        let original = manifest(&definition);
        let mut changed = original.clone();
        changed.resolved_models[0].model = "reasoning-next".to_owned();
        let changed = rebuild(&definition, changed);
        assert_eq!(
            original
                .semantic_identity
                .as_ref()
                .expect("identity")
                .compare(changed.semantic_identity.as_ref().expect("identity")),
            IdentityComparison::Stale(BTreeSet::from([IdentityComponent::Models]))
        );
    }

    #[test]
    fn the_manifest_records_only_the_normalized_endpoint() {
        let definition = ReviewCatalog
            .compile(ReviewDefinitionKey::Screening)
            .expect("definition should compile");
        let mut routed = manifest(&definition);
        routed.resolved_models[0].endpoint = Some(endpoint(
            "https://alice:s3cr3t-pass@Proxy.Example:443/zen/go/v1/?api_key=q-leaked-token#frag",
        ));
        let routed = rebuild(&definition, routed);
        let json = serde_json::to_string(&routed).expect("manifest serializes");
        assert!(
            json.contains(r#""endpoint":"https://proxy.example/zen/go/v1""#),
            "{json}"
        );
        assert!(!json.contains("s3cr3t-pass"), "{json}");
        assert!(!json.contains("q-leaked-token"), "{json}");
        assert!(!json.contains("alice"), "{json}");
        let restored: ReviewRunManifest =
            serde_json::from_str(&json).expect("manifest deserializes");
        assert_eq!(restored, routed);
    }

    #[test]
    fn runtime_provenance_never_changes_the_screening_semantic_identity() {
        let definition = ReviewCatalog
            .compile(ReviewDefinitionKey::Screening)
            .expect("definition should compile");
        let original = manifest(&definition);
        let original_fingerprint = fingerprint_node(&definition, &original, "prepare", &[])
            .expect("fingerprint should build");

        let mut build = original.clone();
        build.runtime.build_sha = hash("changed-source-tree");
        let mut rust = original.clone();
        rust.runtime.rust_version = "1.96".to_owned();
        let mut target = original.clone();
        target.runtime.target = "aarch64-macos".to_owned();
        let mut deployment = original.clone();
        deployment.runtime.deployment_build_id = Some("rev:tree".to_owned());

        for (field, changed) in [
            ("build_sha", rebuild(&definition, build)),
            ("rust_version", rebuild(&definition, rust)),
            ("target", rebuild(&definition, target)),
            ("deployment_build_id", rebuild(&definition, deployment)),
        ] {
            assert_eq!(
                original.semantic_bundle_hash, changed.semantic_bundle_hash,
                "{field} is audit only and must not change calibration"
            );
            assert_eq!(
                original.semantic_identity, changed.semantic_identity,
                "{field} must not change any identity component"
            );
            // Documented, not desired: the runtime stays in the manifest hash, so
            // the manifest and its node fingerprints still move. The AI reuse key
            // includes the node fingerprint, so a changed runtime re-issues calls.
            // Excluding the runtime from the manifest hash is a separate change.
            assert_ne!(
                original.manifest_hash, changed.manifest_hash,
                "{field} is still recorded in the manifest hash"
            );
            assert_ne!(
                original_fingerprint,
                fingerprint_node(&definition, &changed, "prepare", &[])
                    .expect("changed fingerprint should build"),
                "{field} still moves node fingerprints"
            );
        }
    }

    #[test]
    fn identity_components_are_the_build_time_fingerprints() {
        let definition = ReviewCatalog
            .compile(ReviewDefinitionKey::Screening)
            .expect("definition should compile");
        let manifest = manifest(&definition);
        let identity = manifest
            .semantic_identity
            .as_ref()
            .expect("a scheme 2 identity is recorded");
        identity
            .validate()
            .expect("the identity matches the recipe");
        assert_eq!(
            identity.components.get(&IdentityComponent::Implementation),
            Some(&ReviewHash::parse(env!("DEEPREF_SEMANTIC_IMPLEMENTATION_SHA")).expect("hash")),
        );
        assert_eq!(
            identity.components.get(&IdentityComponent::Dependencies),
            Some(&ReviewHash::parse(env!("DEEPREF_SEMANTIC_DEPENDENCY_SHA")).expect("hash")),
        );
        assert_eq!(
            identity.components.get(&IdentityComponent::Implementation),
            Some(&crate::identity::implementation_fingerprint().expect("hash")),
        );
    }

    #[test]
    fn non_screening_definitions_keep_the_broad_source_hash_until_their_boundary_is_declared() {
        let definition = ReviewCatalog
            .compile(ReviewDefinitionKey::DuplicateDetection)
            .expect("definition should compile");
        let original = ReviewRunManifest::build(
            &definition,
            ReviewManifestInput {
                project_id: ProjectId::new(Uuid::new_v4()),
                subject: ReviewSubject::DuplicateDetection {
                    record_id: deepref_domain::RecordId::new(Uuid::new_v4()),
                    candidate_report_id: ReportId::new(Uuid::new_v4()),
                },
                origin: ReviewOrigin::ReviewerRequested,
                protocol_version_id: None,
                protocol_hash: hash("protocol"),
                source_manifest_hash: hash("manifest"),
                source_content_hash: hash("source"),
                resolved_models: vec![ReviewModelIdentity {
                    profile: ModelProfile::FastClassifier,
                    provider: "fixture".to_owned(),
                    model: "classifier".to_owned(),
                    model_version: "v1".to_owned(),
                    parameters_hash: hash("parameters"),
                    endpoint: None,
                }],
                runtime: ReviewRuntimeIdentity {
                    build_sha: hash("build"),
                    rust_version: "1.91".to_owned(),
                    target: "test".to_owned(),
                    deployment_build_id: None,
                },
            },
        )
        .expect("manifest should build");
        let identity = original
            .semantic_identity
            .as_ref()
            .expect("a scheme 2 identity is recorded");
        assert_eq!(
            identity.components.get(&IdentityComponent::Implementation),
            Some(&hash("build")),
            "the broad source hash stands in for definitions without a narrow boundary"
        );

        let mut changed = original.clone();
        changed.runtime.build_sha = hash("changed-source-tree");
        let rebuilt = rebuild(&definition, changed);
        assert_ne!(original.semantic_bundle_hash, rebuilt.semantic_bundle_hash);
    }

    #[test]
    fn runtime_identity_from_a_manifest_without_a_deployment_id_still_deserializes() {
        let legacy = serde_json::json!({
            "build_sha": hash("legacy-build"),
            "rust_version": "1.95",
            "target": "x86_64-linux",
        });
        let runtime: ReviewRuntimeIdentity =
            serde_json::from_value(legacy).expect("legacy runtime identity deserializes");
        assert_eq!(runtime.deployment_build_id, None);
        let serialized = serde_json::to_value(&runtime).expect("serializes");
        assert!(
            serialized.get("deployment_build_id").is_none(),
            "an absent deployment id is not written, so older hashes stay stable"
        );
    }

    #[test]
    fn schedule_command_cannot_pair_the_wrong_subject_and_definition() {
        let command = crate::ScheduleReviewRun {
            project_id: ProjectId::new(Uuid::new_v4()),
            definition: ReviewDefinitionKey::DataExtraction,
            subject: ReviewSubject::Screening {
                report_id: ReportId::new(Uuid::new_v4()),
                stage: ScreeningStage::FullText,
                protocol_version_id: ProtocolVersionId::new(Uuid::new_v4()),
                expected_revision: 1,
            },
            origin: ReviewOrigin::ReviewerRequested,
            actor: Actor::new(ActorKind::User, "reviewer").expect("actor should be valid"),
        };
        assert!(matches!(
            command.validate(),
            Err(ReviewError::SubjectDefinitionMismatch { .. })
        ));
    }
}
