use std::collections::{BTreeMap, BTreeSet, VecDeque};

use deepref_ai::AiTaskKind;
use serde::{Deserialize, Serialize};

use crate::{ReviewDefinitionKey, ReviewError, ReviewHash};

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CompiledReviewIdentity {
    pub definition_id: String,
    pub definition_version: u32,
    pub declared_assets_hash: ReviewHash,
    pub workflow_hash: ReviewHash,
    pub prompt_bundle_hash: ReviewHash,
    pub schema_bundle_hash: ReviewHash,
    pub policy_hash: ReviewHash,
    pub parser_bundle_hash: ReviewHash,
}

#[derive(Debug, Clone)]
pub struct CompiledReviewDefinition {
    key: ReviewDefinitionKey,
    identity: CompiledReviewIdentity,
    workflow: ReviewWorkflow,
    system_prompt: &'static str,
}

impl CompiledReviewDefinition {
    pub const fn key(&self) -> ReviewDefinitionKey {
        self.key
    }

    pub const fn identity(&self) -> &CompiledReviewIdentity {
        &self.identity
    }

    pub fn node_version(&self, node_id: &str) -> Option<u32> {
        self.workflow
            .nodes
            .iter()
            .find(|node| node.id == node_id)
            .map(|node| node.version)
    }

    pub fn system_prompt(&self) -> &str {
        self.system_prompt.trim()
    }

    pub fn transition(
        &self,
        node_id: &str,
        signal: ReviewTransitionSignal,
    ) -> Result<&str, ReviewError> {
        let node = self
            .workflow
            .nodes
            .iter()
            .find(|node| node.id == node_id)
            .ok_or_else(|| ReviewError::InvalidWorkflow(format!("unknown node {node_id}")))?;
        let predicate = signal.predicate();
        let mut matches = node
            .transitions
            .iter()
            .filter(|transition| transition.predicate == predicate);
        let target = matches.next().ok_or_else(|| {
            ReviewError::InvalidWorkflow(format!(
                "node {node_id} has no transition for {}",
                signal.as_str()
            ))
        })?;
        if matches.next().is_some() {
            return Err(ReviewError::InvalidWorkflow(format!(
                "node {node_id} has duplicate transitions for {}",
                signal.as_str()
            )));
        }
        Ok(&target.to)
    }

    pub fn repair_budget(&self, node_id: &str) -> Result<u8, ReviewError> {
        let node = self
            .workflow
            .nodes
            .iter()
            .find(|node| node.id == node_id)
            .ok_or_else(|| ReviewError::InvalidWorkflow(format!("unknown node {node_id}")))?;
        match node.operation {
            ReviewNodeKind::SemanticRepair { max_cycles } => Ok(max_cycles),
            _ => Err(ReviewError::InvalidWorkflow(format!(
                "node {node_id} is not semantic repair"
            ))),
        }
    }

    pub(crate) fn accepts_task(&self, task: AiTaskKind) -> bool {
        match self.key {
            ReviewDefinitionKey::Screening => matches!(
                task,
                AiTaskKind::TitleAbstractScreening | AiTaskKind::FullTextScreening
            ),
            ReviewDefinitionKey::DuplicateDetection => {
                task == AiTaskKind::DuplicateCandidateDetection
            }
            ReviewDefinitionKey::StudyClassification => {
                task == AiTaskKind::StudyDesignClassification
            }
            ReviewDefinitionKey::StudyGrouping => task == AiTaskKind::StudyGrouping,
            ReviewDefinitionKey::AppraisalPrefill => task == AiTaskKind::AppraisalPrefill,
            ReviewDefinitionKey::DataExtraction => task == AiTaskKind::DataExtraction,
        }
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub struct ReviewCatalog;

impl ReviewCatalog {
    pub fn compile(
        &self,
        key: ReviewDefinitionKey,
    ) -> Result<CompiledReviewDefinition, ReviewError> {
        compile_definition(definition_source(key))
    }
}

#[derive(Debug, Clone, Copy)]
struct ReviewAsset {
    id: &'static str,
    version: u32,
    content: &'static str,
}

#[derive(Debug, Clone, Copy)]
struct DefinitionSource {
    key: ReviewDefinitionKey,
    id: &'static str,
    version: u32,
    workflow: ReviewAsset,
    prompt: ReviewAsset,
    schema: ReviewAsset,
    policy: ReviewAsset,
    parser: ReviewAsset,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ReviewWorkflow {
    id: String,
    version: u32,
    entrypoint: String,
    nodes: Vec<ReviewWorkflowNode>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ReviewWorkflowNode {
    id: String,
    version: u32,
    operation: ReviewNodeKind,
    #[serde(default)]
    transitions: Vec<ReviewTransition>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum ReviewNodeKind {
    Prepare,
    Generate { task: AiTaskKind },
    PrimaryScreen,
    Validate,
    Derive,
    IndependentScreen,
    Reconcile,
    Assemble,
    CandidateAudit,
    SemanticRepair { max_cycles: u8 },
    Finalize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ReviewTransition {
    predicate: ReviewTransitionPredicate,
    to: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum ReviewTransitionPredicate {
    Always,
    Valid,
    Invalid,
    NeedsIndependentScreen,
    PrimaryAccepted,
    Agreement,
    Disagreement,
    AuditPassed,
    AuditRepairable,
    RepairReady,
    RepairExhausted,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReviewTransitionSignal {
    Always,
    Valid,
    Invalid,
    NeedsIndependentScreen,
    PrimaryAccepted,
    Agreement,
    Disagreement,
    AuditPassed,
    AuditRepairable,
    RepairReady,
    RepairExhausted,
}

impl ReviewTransitionSignal {
    const fn predicate(self) -> ReviewTransitionPredicate {
        match self {
            Self::Always => ReviewTransitionPredicate::Always,
            Self::Valid => ReviewTransitionPredicate::Valid,
            Self::Invalid => ReviewTransitionPredicate::Invalid,
            Self::NeedsIndependentScreen => ReviewTransitionPredicate::NeedsIndependentScreen,
            Self::PrimaryAccepted => ReviewTransitionPredicate::PrimaryAccepted,
            Self::Agreement => ReviewTransitionPredicate::Agreement,
            Self::Disagreement => ReviewTransitionPredicate::Disagreement,
            Self::AuditPassed => ReviewTransitionPredicate::AuditPassed,
            Self::AuditRepairable => ReviewTransitionPredicate::AuditRepairable,
            Self::RepairReady => ReviewTransitionPredicate::RepairReady,
            Self::RepairExhausted => ReviewTransitionPredicate::RepairExhausted,
        }
    }

    const fn as_str(self) -> &'static str {
        match self {
            Self::Always => "always",
            Self::Valid => "valid",
            Self::Invalid => "invalid",
            Self::NeedsIndependentScreen => "needs_independent_screen",
            Self::PrimaryAccepted => "primary_accepted",
            Self::Agreement => "agreement",
            Self::Disagreement => "disagreement",
            Self::AuditPassed => "audit_passed",
            Self::AuditRepairable => "audit_repairable",
            Self::RepairReady => "repair_ready",
            Self::RepairExhausted => "repair_exhausted",
        }
    }
}

#[derive(Serialize)]
struct AssetIdentity<'a> {
    id: &'a str,
    version: u32,
    content_hash: ReviewHash,
}

#[derive(Serialize)]
struct DeclaredIdentity<'a> {
    definition_id: &'a str,
    definition_version: u32,
    key: ReviewDefinitionKey,
    workflow: AssetIdentity<'a>,
    prompt: AssetIdentity<'a>,
    schema: AssetIdentity<'a>,
    policy: AssetIdentity<'a>,
    parser: AssetIdentity<'a>,
}

fn asset_identity(asset: ReviewAsset) -> Result<AssetIdentity<'static>, ReviewError> {
    if asset.id.trim().is_empty() || asset.version == 0 || asset.content.trim().is_empty() {
        return Err(ReviewError::InvalidDefinition(
            "asset identity and content must be complete".to_owned(),
        ));
    }
    Ok(AssetIdentity {
        id: asset.id,
        version: asset.version,
        content_hash: ReviewHash::digest_bytes(asset.content.as_bytes()),
    })
}

fn compile_definition(source: DefinitionSource) -> Result<CompiledReviewDefinition, ReviewError> {
    if source.id.trim().is_empty() || source.version == 0 {
        return Err(ReviewError::InvalidDefinition(
            "definition identity must be complete".to_owned(),
        ));
    }
    let workflow: ReviewWorkflow = serde_json::from_str(source.workflow.content)
        .map_err(|error| ReviewError::InvalidWorkflow(error.to_string()))?;
    let _: serde_json::Value = serde_json::from_str(source.schema.content)
        .map_err(|error| ReviewError::InvalidDefinition(format!("schema asset: {error}")))?;
    let _: serde_json::Value = serde_json::from_str(source.parser.content)
        .map_err(|error| ReviewError::InvalidDefinition(format!("parser asset: {error}")))?;
    validate_workflow(source.key, source.id, source.version, &workflow)?;

    let workflow_asset = asset_identity(source.workflow)?;
    let prompt_asset = asset_identity(source.prompt)?;
    let schema_asset = asset_identity(source.schema)?;
    let policy_asset = asset_identity(source.policy)?;
    let parser_asset = asset_identity(source.parser)?;
    let declared_assets_hash = ReviewHash::digest_json(&DeclaredIdentity {
        definition_id: source.id,
        definition_version: source.version,
        key: source.key,
        workflow: workflow_asset,
        prompt: prompt_asset,
        schema: schema_asset,
        policy: policy_asset,
        parser: parser_asset,
    })?;

    Ok(CompiledReviewDefinition {
        key: source.key,
        identity: CompiledReviewIdentity {
            definition_id: source.id.to_owned(),
            definition_version: source.version,
            declared_assets_hash,
            workflow_hash: ReviewHash::digest_bytes(source.workflow.content.as_bytes()),
            prompt_bundle_hash: ReviewHash::digest_bytes(source.prompt.content.as_bytes()),
            schema_bundle_hash: ReviewHash::digest_bytes(source.schema.content.as_bytes()),
            policy_hash: ReviewHash::digest_bytes(source.policy.content.as_bytes()),
            parser_bundle_hash: ReviewHash::digest_bytes(source.parser.content.as_bytes()),
        },
        workflow,
        system_prompt: source.prompt.content,
    })
}

fn validate_workflow(
    key: ReviewDefinitionKey,
    definition_id: &str,
    definition_version: u32,
    workflow: &ReviewWorkflow,
) -> Result<(), ReviewError> {
    if workflow.id != definition_id || workflow.version != definition_version {
        return Err(ReviewError::InvalidWorkflow(
            "workflow identity does not match its definition".to_owned(),
        ));
    }
    if workflow.nodes.is_empty() {
        return Err(ReviewError::InvalidWorkflow(
            "workflow requires nodes".to_owned(),
        ));
    }

    let mut nodes = BTreeMap::new();
    for node in &workflow.nodes {
        if node.id.trim().is_empty() || node.version == 0 {
            return Err(ReviewError::InvalidWorkflow(
                "node identity must be complete".to_owned(),
            ));
        }
        if nodes.insert(node.id.as_str(), node).is_some() {
            return Err(ReviewError::InvalidWorkflow(format!(
                "duplicate node {}",
                node.id
            )));
        }
        if let ReviewNodeKind::SemanticRepair { max_cycles } = node.operation
            && !(1..=2).contains(&max_cycles)
        {
            return Err(ReviewError::InvalidWorkflow(
                "semantic repair budget must be between one and two cycles".to_owned(),
            ));
        }
    }

    let Some(entrypoint) = nodes.get(workflow.entrypoint.as_str()) else {
        return Err(ReviewError::InvalidWorkflow(
            "entrypoint does not name a node".to_owned(),
        ));
    };
    if !matches!(entrypoint.operation, ReviewNodeKind::Prepare) {
        return Err(ReviewError::InvalidWorkflow(
            "entrypoint must be a prepare node".to_owned(),
        ));
    }

    let final_nodes = workflow
        .nodes
        .iter()
        .filter(|node| matches!(node.operation, ReviewNodeKind::Finalize))
        .collect::<Vec<_>>();
    if final_nodes.len() != 1 || !final_nodes[0].transitions.is_empty() {
        return Err(ReviewError::InvalidWorkflow(
            "workflow requires exactly one terminal finalize node".to_owned(),
        ));
    }

    for node in &workflow.nodes {
        let mut predicates = BTreeSet::new();
        for transition in &node.transitions {
            if !predicates.insert(transition.predicate) {
                return Err(ReviewError::InvalidWorkflow(format!(
                    "node {} has duplicate transition predicates",
                    node.id
                )));
            }
            if !nodes.contains_key(transition.to.as_str()) {
                return Err(ReviewError::InvalidWorkflow(format!(
                    "node {} targets unknown node {}",
                    node.id, transition.to
                )));
            }
            if transition.to == workflow.entrypoint {
                return Err(ReviewError::InvalidWorkflow(
                    "transitions cannot re-enter the prepare node".to_owned(),
                ));
            }
        }
    }

    validate_task_binding(key, &workflow.nodes)?;

    let mut reachable = BTreeSet::new();
    let mut queue = VecDeque::from([workflow.entrypoint.as_str()]);
    while let Some(node_id) = queue.pop_front() {
        if !reachable.insert(node_id) {
            continue;
        }
        let node = nodes[node_id];
        queue.extend(
            node.transitions
                .iter()
                .map(|transition| transition.to.as_str()),
        );
    }
    if reachable.len() != workflow.nodes.len() {
        return Err(ReviewError::InvalidWorkflow(
            "workflow contains unreachable nodes".to_owned(),
        ));
    }
    Ok(())
}

fn validate_task_binding(
    key: ReviewDefinitionKey,
    nodes: &[ReviewWorkflowNode],
) -> Result<(), ReviewError> {
    let expected = match key {
        ReviewDefinitionKey::Screening => {
            let has_primary = nodes
                .iter()
                .any(|node| matches!(node.operation, ReviewNodeKind::PrimaryScreen));
            let has_independent = nodes
                .iter()
                .any(|node| matches!(node.operation, ReviewNodeKind::IndependentScreen));
            let has_audit = nodes
                .iter()
                .any(|node| matches!(node.operation, ReviewNodeKind::CandidateAudit));
            if !has_primary || !has_independent || !has_audit {
                return Err(ReviewError::InvalidWorkflow(
                    "screening requires primary, independent, and audit nodes".to_owned(),
                ));
            }
            return Ok(());
        }
        ReviewDefinitionKey::DuplicateDetection => AiTaskKind::DuplicateCandidateDetection,
        ReviewDefinitionKey::StudyClassification => AiTaskKind::StudyDesignClassification,
        ReviewDefinitionKey::StudyGrouping => AiTaskKind::StudyGrouping,
        ReviewDefinitionKey::AppraisalPrefill => AiTaskKind::AppraisalPrefill,
        ReviewDefinitionKey::DataExtraction => AiTaskKind::DataExtraction,
    };
    let tasks = nodes
        .iter()
        .filter_map(|node| match node.operation {
            ReviewNodeKind::Generate { task } => Some(task),
            ReviewNodeKind::Prepare
            | ReviewNodeKind::PrimaryScreen
            | ReviewNodeKind::Validate
            | ReviewNodeKind::Derive
            | ReviewNodeKind::IndependentScreen
            | ReviewNodeKind::Reconcile
            | ReviewNodeKind::Assemble
            | ReviewNodeKind::CandidateAudit
            | ReviewNodeKind::SemanticRepair { .. }
            | ReviewNodeKind::Finalize => None,
        })
        .collect::<Vec<_>>();
    if tasks != [expected] {
        return Err(ReviewError::InvalidWorkflow(
            "definition must bind exactly one matching AI task".to_owned(),
        ));
    }
    Ok(())
}

const SHARED_POLICY: ReviewAsset = ReviewAsset {
    id: "deepref/review-policy",
    version: 1,
    content: include_str!("../../../review-definitions/shared/v1/policy.md"),
};

const SHARED_PARSER: ReviewAsset = ReviewAsset {
    id: "deepref/parser-contract",
    version: 1,
    content: include_str!("../../../review-definitions/shared/v1/parser.json"),
};

fn definition_source(key: ReviewDefinitionKey) -> DefinitionSource {
    match key {
        ReviewDefinitionKey::Screening => source(
            key,
            "deepref.screening",
            include_str!("../../../review-definitions/screening/v1/workflow.json"),
            include_str!("../../../review-definitions/screening/v1/prompt.txt"),
            include_str!("../../../review-definitions/screening/v1/schema.json"),
        ),
        ReviewDefinitionKey::DuplicateDetection => source(
            key,
            "deepref.duplicate-detection",
            include_str!("../../../review-definitions/duplicate-detection/v1/workflow.json"),
            include_str!("../../../review-definitions/duplicate-detection/v1/prompt.txt"),
            include_str!("../../../review-definitions/duplicate-detection/v1/schema.json"),
        ),
        ReviewDefinitionKey::StudyClassification => source(
            key,
            "deepref.study-classification",
            include_str!("../../../review-definitions/study-classification/v1/workflow.json"),
            include_str!("../../../review-definitions/study-classification/v1/prompt.txt"),
            include_str!("../../../review-definitions/study-classification/v1/schema.json"),
        ),
        ReviewDefinitionKey::StudyGrouping => source(
            key,
            "deepref.study-grouping",
            include_str!("../../../review-definitions/study-grouping/v1/workflow.json"),
            include_str!("../../../review-definitions/study-grouping/v1/prompt.txt"),
            include_str!("../../../review-definitions/study-grouping/v1/schema.json"),
        ),
        ReviewDefinitionKey::AppraisalPrefill => source(
            key,
            "deepref.appraisal-prefill",
            include_str!("../../../review-definitions/appraisal-prefill/v1/workflow.json"),
            include_str!("../../../review-definitions/appraisal-prefill/v1/prompt.txt"),
            include_str!("../../../review-definitions/appraisal-prefill/v1/schema.json"),
        ),
        ReviewDefinitionKey::DataExtraction => source(
            key,
            "deepref.data-extraction",
            include_str!("../../../review-definitions/data-extraction/v1/workflow.json"),
            include_str!("../../../review-definitions/data-extraction/v1/prompt.txt"),
            include_str!("../../../review-definitions/data-extraction/v1/schema.json"),
        ),
    }
}

fn source(
    key: ReviewDefinitionKey,
    id: &'static str,
    workflow: &'static str,
    prompt: &'static str,
    schema: &'static str,
) -> DefinitionSource {
    DefinitionSource {
        key,
        id,
        version: 1,
        workflow: ReviewAsset {
            id,
            version: 1,
            content: workflow,
        },
        prompt: ReviewAsset {
            id,
            version: 1,
            content: prompt,
        },
        schema: ReviewAsset {
            id,
            version: 1,
            content: schema,
        },
        policy: SHARED_POLICY,
        parser: SHARED_PARSER,
    }
}

#[cfg(test)]
mod tests {
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
        }
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
        let original_manifest = ReviewRunManifest::build(&original, input.clone())
            .expect("original manifest should build");
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
            let changed_fingerprint =
                fingerprint_node(&changed, &changed_manifest, "generate", &[])
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
}
