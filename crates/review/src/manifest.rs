use deepref_ai::ModelProfile;
use deepref_domain::{ProjectId, ProtocolVersionId};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{
    CompiledReviewDefinition, ReviewDefinitionKey, ReviewError, ReviewHash, ReviewOrigin,
    ReviewSubject,
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReviewModelIdentity {
    pub profile: ModelProfile,
    pub provider: String,
    pub model: String,
    pub model_version: String,
    pub parameters_hash: ReviewHash,
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReviewRuntimeIdentity {
    pub build_sha: ReviewHash,
    pub rust_version: String,
    pub target: String,
}

impl ReviewRuntimeIdentity {
    fn validate(&self) -> Result<(), ReviewError> {
        if self.rust_version.trim().is_empty() || self.target.trim().is_empty() {
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
    pub semantic_bundle_hash: ReviewHash,
    pub manifest_hash: ReviewHash,
}

#[derive(Serialize)]
struct SemanticBundle<'a> {
    definition_id: &'a str,
    definition_version: u32,
    declared_assets_hash: &'a ReviewHash,
    workflow_hash: &'a ReviewHash,
    prompt_bundle_hash: &'a ReviewHash,
    schema_bundle_hash: &'a ReviewHash,
    policy_hash: &'a ReviewHash,
    parser_bundle_hash: &'a ReviewHash,
    protocol_hash: &'a ReviewHash,
    resolved_models: &'a [ReviewModelIdentity],
    runtime: &'a ReviewRuntimeIdentity,
}

#[derive(Serialize)]
struct ManifestWithoutOwnHash<'a> {
    project_id: ProjectId,
    definition: ReviewDefinitionKey,
    definition_id: &'a str,
    definition_version: u32,
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
        let semantic_bundle_hash = ReviewHash::digest_json(&SemanticBundle {
            definition_id: &identity.definition_id,
            definition_version: identity.definition_version,
            declared_assets_hash: &identity.declared_assets_hash,
            workflow_hash: &identity.workflow_hash,
            prompt_bundle_hash: &identity.prompt_bundle_hash,
            schema_bundle_hash: &identity.schema_bundle_hash,
            policy_hash: &identity.policy_hash,
            parser_bundle_hash: &identity.parser_bundle_hash,
            protocol_hash: &input.protocol_hash,
            resolved_models: &input.resolved_models,
            runtime: &input.runtime,
        })?;
        let manifest_hash = ReviewHash::digest_json(&ManifestWithoutOwnHash {
            project_id: input.project_id,
            definition: definition.key(),
            definition_id: &identity.definition_id,
            definition_version: identity.definition_version,
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
            semantic_bundle_hash: &semantic_bundle_hash,
        })?;

        Ok(Self {
            project_id: input.project_id,
            definition: definition.key(),
            definition_id: identity.definition_id.clone(),
            definition_version: identity.definition_version,
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
            semantic_bundle_hash,
            manifest_hash,
        })
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
    use super::*;
    use crate::{ReviewCatalog, ReviewDefinitionKey};
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
                }],
                runtime: ReviewRuntimeIdentity {
                    build_sha: hash("build"),
                    rust_version: "1.91".to_owned(),
                    target: "test".to_owned(),
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
    }

    #[test]
    fn protocol_model_and_runtime_changes_invalidate_semantics_and_node_reuse() {
        let definition = ReviewCatalog
            .compile(ReviewDefinitionKey::Screening)
            .expect("definition should compile");
        let original = manifest(&definition);
        let original_fingerprint = fingerprint_node(&definition, &original, "prepare", &[])
            .expect("fingerprint should build");

        let mut variants = Vec::new();
        let mut protocol = original.clone();
        protocol.protocol_hash = hash("changed-protocol");
        variants.push(("protocol", rebuild(&definition, protocol)));

        let mut model = original.clone();
        model.resolved_models[0].model_version = "v2".to_owned();
        variants.push(("resolved model", rebuild(&definition, model)));

        let mut runtime = original.clone();
        runtime.runtime.build_sha = hash("changed-build");
        variants.push(("runtime build", rebuild(&definition, runtime)));

        for (identity, changed) in variants {
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
