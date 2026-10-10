//! Tests for manifest construction, node fingerprints, contract recording
//! and legacy-row compatibility.
//! Extracted from `manifest.rs`: the `#[cfg(test)] mod tests` declaration
//! there points here via `#[path]`, so `super::*` still resolves to the
//! manifest module and test paths are unchanged.
use std::collections::BTreeSet;

use super::*;
use crate::{CalibrationCompatibility, ReviewCatalog, ReviewDefinitionKey, SemanticChange};
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
fn source_changes_manifest_but_not_the_contract() {
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
    assert_eq!(contract_of(&first).id(), contract_of(&rebuilt).id());
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
fn protocol_and_model_changes_invalidate_the_contract_and_node_reuse() {
    let definition = ReviewCatalog
        .compile(ReviewDefinitionKey::Screening)
        .expect("definition should compile");
    let original = manifest(&definition);
    let original_fingerprint =
        fingerprint_node(&definition, &original, "prepare", &[]).expect("fingerprint should build");

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
            contract_of(&original).id(),
            contract_of(&changed).id(),
            "{identity} must invalidate the semantic contract"
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
    let restored: ReviewRunManifest = serde_json::from_str(&json).expect("manifest deserializes");
    assert_eq!(restored, routed);
}

#[test]
fn non_screening_definitions_carry_explicit_versions_and_ignore_build_provenance() {
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
    let contract = contract_of(&original);
    assert_eq!(
        contract.semantic_version,
        crate::DUPLICATE_DETECTION_SEMANTIC_VERSION
    );
    assert_eq!(contract.stage, None);

    // A source-tree change without a behavior change keeps the contract
    // but moves provenance: calibration stays valid, audit still sees it.
    let mut changed = original.clone();
    changed.runtime.build_sha = hash("changed-source-tree");
    let rebuilt = rebuild(&definition, changed);
    assert_eq!(
        contract.compare(&contract_of(&rebuilt)),
        CalibrationCompatibility::Compatible
    );
    assert_ne!(original.provenance, rebuilt.provenance);
}

#[test]
fn provenance_records_the_build_time_digests_for_audit() {
    let definition = ReviewCatalog
        .compile(ReviewDefinitionKey::Screening)
        .expect("definition should compile");
    let manifest = manifest(&definition);
    let provenance = &manifest.provenance;
    assert!(!provenance.deepref_version.is_empty());
    assert_eq!(
        provenance
            .source_tree_digest
            .as_ref()
            .map(|digest| digest.as_str()),
        Some(manifest.runtime.build_sha.as_str()),
        "the broad source-tree digest is preserved as provenance"
    );
    assert!(
        provenance.cargo_lock_digest.is_some(),
        "the dependency closure digest is preserved as provenance"
    );
    assert!(
        provenance.implementation_digest.is_some(),
        "screening keeps its narrow boundary digest as provenance"
    );
    assert!(provenance.rig_version.is_some());
    assert!(provenance.serde_json_version.is_some());
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

fn contract_of(manifest: &ReviewRunManifest) -> crate::ReviewSemanticContract {
    manifest
        .semantic_contract
        .clone()
        .expect("a semantic contract is recorded")
}

#[test]
fn the_manifest_records_a_valid_semantic_contract() {
    let definition = ReviewCatalog
        .compile(ReviewDefinitionKey::Screening)
        .expect("definition should compile");
    let manifest = manifest(&definition);
    let contract = contract_of(&manifest);
    contract.validate().expect("contract is valid");
    assert_eq!(contract.definition, ReviewDefinitionKey::Screening);
    assert_eq!(contract.stage, Some(ScreeningStage::TitleAbstract));
    assert_eq!(contract.semantic_version, crate::SCREENING_SEMANTIC_VERSION);
    assert_eq!(contract.models.len(), 1);
}

#[test]
fn identical_manifests_have_compatible_contracts_with_equal_ids() {
    let definition = ReviewCatalog
        .compile(ReviewDefinitionKey::Screening)
        .expect("definition should compile");
    let first = manifest(&definition);
    let second = rebuild(&definition, first.clone());
    let (stored, current) = (contract_of(&first), contract_of(&second));
    assert_eq!(
        stored.compare(&current),
        CalibrationCompatibility::Compatible
    );
    assert_eq!(stored.id(), current.id());
}

#[test]
fn protocol_and_model_changes_change_the_contract_id_with_named_changes() {
    let definition = ReviewCatalog
        .compile(ReviewDefinitionKey::Screening)
        .expect("definition should compile");
    let baseline = manifest(&definition);
    let original = contract_of(&baseline);
    let mut protocol = baseline.clone();
    protocol.protocol_hash = hash("changed-protocol");
    let mut model = baseline;
    model.resolved_models[0].model_version = "v2".to_owned();
    for (changed, expected) in [
        (
            rebuild(&definition, protocol),
            BTreeSet::from([SemanticChange::Protocol]),
        ),
        (
            rebuild(&definition, model),
            BTreeSet::from([SemanticChange::Model]),
        ),
    ] {
        let current = contract_of(&changed);
        assert_eq!(
            original.compare(&current),
            CalibrationCompatibility::Incompatible { changes: expected }
        );
        assert_ne!(original.id(), current.id());
    }
}

#[test]
fn build_provenance_never_changes_the_semantic_contract() {
    let definition = ReviewCatalog
        .compile(ReviewDefinitionKey::Screening)
        .expect("definition should compile");
    let original = manifest(&definition);
    let stored = contract_of(&original);
    let stored_id = stored.id();

    let mut build = original.clone();
    build.runtime.build_sha = hash("changed-source-tree");
    let mut rust = original.clone();
    rust.runtime.rust_version = "1.96".to_owned();
    let mut target = original.clone();
    target.runtime.target = "aarch64-macos".to_owned();
    let mut deployment = original.clone();
    deployment.runtime.deployment_build_id = Some("rev:tree".to_owned());
    let mut endpoint = original.clone();
    endpoint.resolved_models[0].endpoint = Some(
        ProviderEndpoint::from_configured_url("https://moved.example/v1").expect("endpoint parses"),
    );

    for (field, changed) in [
        ("build_sha", rebuild(&definition, build)),
        ("rust_version", rebuild(&definition, rust)),
        ("target", rebuild(&definition, target)),
        ("deployment_build_id", rebuild(&definition, deployment)),
        ("provider endpoint", rebuild(&definition, endpoint)),
    ] {
        let current = contract_of(&changed);
        assert_eq!(
            stored.compare(&current),
            CalibrationCompatibility::Compatible,
            "{field} is provenance and must not change calibration"
        );
        assert_eq!(
            stored_id,
            current.id(),
            "{field} must not change the contract id"
        );
        // ... but it is still recorded for audit.
        assert_ne!(
            original.provenance, changed.provenance,
            "{field} must be recorded in provenance"
        );
    }
}

#[test]
fn manifests_persisted_before_the_contract_still_deserialize() {
    let definition = ReviewCatalog
        .compile(ReviewDefinitionKey::Screening)
        .expect("definition should compile");
    let manifest = manifest(&definition);
    let mut json = serde_json::to_value(&manifest).expect("manifest serializes");
    let object = json.as_object_mut().expect("manifest is an object");
    for key in ["semantic_contract", "provenance"] {
        object.remove(key);
    }
    // Rows persisted under older recipes carry unknown identity keys;
    // serde ignores them, and the missing contract fails admission closed.
    object.insert(
        "semantic_identity".to_owned(),
        serde_json::json!({"scheme": 2}),
    );
    object.insert(
        "semantic_bundle_hash".to_owned(),
        serde_json::json!("a".repeat(64)),
    );
    let restored: ReviewRunManifest =
        serde_json::from_value(json).expect("legacy manifest deserializes");
    assert_eq!(restored.semantic_contract, None);
    assert!(restored.semantic_contract_id().is_err());
    assert_eq!(restored.manifest_hash, manifest.manifest_hash);
}
