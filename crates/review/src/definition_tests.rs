use super::*;
use crate::manifest::{
    ReviewManifestInput, ReviewModelIdentity, ReviewRunManifest, ReviewRuntimeIdentity,
    fingerprint_node,
};
use crate::{ReviewOrigin, ReviewSubject};
use deepref_ai::ModelProfile;
use deepref_domain::{ProjectId, RecordId, ReportId};
use uuid::Uuid;

fn valid_definition() -> DefinitionSource {
    definition_source(ReviewDefinitionKey::DuplicateDetection)
}

#[test]
fn all_checked_in_definitions_compile() {
    let catalog = ReviewCatalog;
    for key in ReviewDefinitionKey::ALL {
        let compiled = catalog.compile(key).expect("definition should compile");
        assert_eq!(compiled.key(), key);
        assert!(!compiled.system_prompt().is_empty());
        assert!(!compiled.final_proposal_type().is_empty());
    }
}

#[test]
fn rejects_mismatched_semantic_handler_and_final_proposal_type() {
    let source = valid_definition();
    let workflow: ReviewWorkflow =
        serde_json::from_str(source.workflow.content).expect("fixture workflow");

    let mut wrong_handler = workflow.clone();
    wrong_handler.semantic_handler = ReviewSemanticHandler::DataExtraction;
    assert!(validate_workflow(source.key, source.id, source.version, &wrong_handler).is_err());

    let mut wrong_proposal = workflow;
    wrong_proposal.final_proposal_type = ReviewProposalType::DataExtraction;
    assert!(validate_workflow(source.key, source.id, source.version, &wrong_proposal).is_err());
}

#[test]
fn rejects_unknown_predicates_and_duplicate_nodes() {
    let mut source = valid_definition();
    source.workflow.content = r#"{
          "id":"deepref.duplicate-detection","version":1,"entrypoint":"prepare",
          "nodes":[
            {"id":"prepare","version":1,"operation":{"kind":"prepare"},"transitions":[{"predicate":"invented","to":"prepare"}]},
            {"id":"prepare","version":1,"operation":{"kind":"finalize"}}
          ]
        }"#;
    assert!(matches!(
        compile_definition(source),
        Err(ReviewError::InvalidWorkflow(_))
    ));
}

#[test]
fn compiled_transitions_are_typed_and_duplicate_predicates_are_rejected() {
    let screening = ReviewCatalog
        .compile(ReviewDefinitionKey::Screening)
        .expect("screening definition compiles");
    assert_eq!(
        screening
            .transition("prepare", ReviewTransitionSignal::Always)
            .expect("prepare transition exists"),
        "primary_screen"
    );
    assert_eq!(
        screening
            .transition("semantic_repair", ReviewTransitionSignal::RepairReady)
            .expect("repair transition exists"),
        "validate_repair"
    );
    assert_eq!(
        screening
            .repair_budget("semantic_repair")
            .expect("repair budget exists"),
        2
    );

    let mut source = valid_definition();
    source.workflow.content = r#"{
          "id":"deepref.duplicate-detection","version":1,"entrypoint":"prepare",
          "nodes":[
            {"id":"prepare","version":1,"operation":{"kind":"prepare"},"transitions":[
              {"predicate":"always","to":"generate"},{"predicate":"always","to":"finalize"}
            ]},
            {"id":"generate","version":1,"operation":{"kind":"generate","task":"duplicate_candidate_detection"},"transitions":[{"predicate":"always","to":"validate"}]},
            {"id":"validate","version":1,"operation":{"kind":"validate"},"transitions":[{"predicate":"valid","to":"assemble"}]},
            {"id":"assemble","version":1,"operation":{"kind":"assemble"},"transitions":[{"predicate":"always","to":"finalize"}]},
            {"id":"finalize","version":1,"operation":{"kind":"finalize"}}
          ]
        }"#;
    assert!(matches!(
        compile_definition(source),
        Err(ReviewError::InvalidWorkflow(_))
    ));
}

#[test]
fn typed_plan_rejects_an_ai_transition_into_assembly() {
    let mut source = valid_definition();
    source.workflow.content = r#"{
          "id":"deepref.duplicate-detection","version":1,"entrypoint":"prepare",
          "nodes":[
            {"id":"prepare","version":1,"operation":{"kind":"prepare"},"transitions":[{"predicate":"always","to":"generate"}]},
            {"id":"generate","version":1,"operation":{"kind":"generate","task":"duplicate_candidate_detection"},"transitions":[{"predicate":"always","to":"assemble"}]},
            {"id":"assemble","version":1,"operation":{"kind":"validate"},"transitions":[{"predicate":"valid","to":"validate"}]},
            {"id":"validate","version":1,"operation":{"kind":"assemble"},"transitions":[{"predicate":"always","to":"finalize"}]},
            {"id":"finalize","version":1,"operation":{"kind":"finalize"}}
          ],
          "semantic_handler":"duplicate_analysis",
          "final_proposal_type":"dedupe_suggestion"
        }"#;
    let compiled = compile_definition(source).expect("base graph remains structurally valid");
    assert!(matches!(
        compiled.execution_plan(),
        Err(ReviewError::InvalidWorkflow(_))
    ));
}

#[test]
fn rejects_identity_mismatch_and_unreachable_nodes() {
    let mut source = valid_definition();
    source.workflow.content = r#"{
          "id":"wrong","version":1,"entrypoint":"prepare",
          "nodes":[
            {"id":"prepare","version":1,"operation":{"kind":"prepare"},"transitions":[{"predicate":"always","to":"finalize"}]},
            {"id":"orphan","version":1,"operation":{"kind":"validate"}},
            {"id":"finalize","version":1,"operation":{"kind":"finalize"}}
          ]
        }"#;
    assert!(matches!(
        compile_definition(source),
        Err(ReviewError::InvalidWorkflow(_))
    ));
}

fn manifest_input() -> ReviewManifestInput {
    ReviewManifestInput {
        project_id: ProjectId::new(Uuid::from_u128(1)),
        subject: ReviewSubject::DuplicateDetection {
            record_id: RecordId::new(Uuid::from_u128(2)),
            candidate_report_id: ReportId::new(Uuid::from_u128(3)),
        },
        origin: ReviewOrigin::ReviewerRequested,
        protocol_version_id: None,
        protocol_hash: ReviewHash::digest_bytes(b"protocol"),
        source_manifest_hash: ReviewHash::digest_bytes(b"source-manifest"),
        source_content_hash: ReviewHash::digest_bytes(b"source-content"),
        resolved_models: vec![ReviewModelIdentity {
            profile: ModelProfile::FastClassifier,
            provider: "fixture".to_owned(),
            model: "classifier".to_owned(),
            model_version: "v1".to_owned(),
            parameters_hash: ReviewHash::digest_bytes(b"parameters"),
        }],
        runtime: ReviewRuntimeIdentity {
            build_sha: ReviewHash::digest_bytes(b"build"),
            rust_version: "1.91".to_owned(),
            target: "test".to_owned(),
        },
    }
}

fn changed_content(content: &'static str) -> &'static str {
    Box::leak(format!(" {content}").into_boxed_str())
}

#[test]
fn every_definition_asset_change_invalidates_semantics_and_node_reuse() {
    let source = valid_definition();
    let original = compile_definition(source).expect("definition should compile");
    let input = manifest_input();
    let original_manifest =
        ReviewRunManifest::build(&original, input.clone()).expect("original manifest should build");
    let original_fingerprint = fingerprint_node(&original, &original_manifest, "generate", &[])
        .expect("original fingerprint should build");

    for asset in ["workflow", "prompt", "schema", "policy", "parser"] {
        let mut changed_source = source;
        match asset {
            "workflow" => {
                changed_source.workflow.content = changed_content(source.workflow.content)
            }
            "prompt" => changed_source.prompt.content = changed_content(source.prompt.content),
            "schema" => changed_source.schema.content = changed_content(source.schema.content),
            "policy" => changed_source.policy.content = changed_content(source.policy.content),
            "parser" => changed_source.parser.content = changed_content(source.parser.content),
            _ => unreachable!(),
        }
        let changed = compile_definition(changed_source)
            .unwrap_or_else(|error| panic!("changed {asset} should compile: {error}"));
        let changed_manifest = ReviewRunManifest::build(&changed, input.clone())
            .unwrap_or_else(|error| panic!("changed {asset} manifest should build: {error}"));
        let changed_fingerprint = fingerprint_node(&changed, &changed_manifest, "generate", &[])
            .expect("changed fingerprint should build");
        assert_ne!(
            original.identity().declared_assets_hash,
            changed.identity().declared_assets_hash,
            "{asset} must invalidate compiled identity"
        );
        assert_ne!(
            original_manifest.semantic_bundle_hash, changed_manifest.semantic_bundle_hash,
            "{asset} must invalidate semantic bundle"
        );
        assert_ne!(
            original_fingerprint, changed_fingerprint,
            "{asset} must invalidate node reuse"
        );
    }
}

#[test]
fn rejects_missing_or_malformed_assets() {
    let mut missing = valid_definition();
    missing.prompt.content = "   ";
    assert!(matches!(
        compile_definition(missing),
        Err(ReviewError::InvalidDefinition(_))
    ));

    let mut malformed = valid_definition();
    malformed.schema.content = "not-json";
    assert!(matches!(
        compile_definition(malformed),
        Err(ReviewError::InvalidDefinition(_))
    ));
}
