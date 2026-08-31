use std::collections::{BTreeMap, BTreeSet, VecDeque};

use deepref_ai::AiTaskKind;
use serde::{Deserialize, Serialize};

use crate::{
    ReviewDefinitionKey, ReviewError, ReviewHash,
    worker::{ReviewExecutionPlan, ReviewNode, ScreeningReviewPlan, StandardReviewPlan},
};

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

    fn transition(
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

    fn repair_budget(&self, node_id: &str) -> Result<u8, ReviewError> {
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

    pub fn final_proposal_type(&self) -> &'static str {
        self.workflow.final_proposal_type.as_str()
    }

    pub(crate) fn accepts_task(&self, task: AiTaskKind) -> bool {
        self.workflow.semantic_handler.accepts_task(task)
    }

    pub(crate) fn execution_plan(&self) -> Result<ReviewExecutionPlan, ReviewError> {
        match self.key {
            ReviewDefinitionKey::Screening => self
                .screening_plan()
                .map(Box::new)
                .map(ReviewExecutionPlan::Screening),
            ReviewDefinitionKey::DuplicateDetection
            | ReviewDefinitionKey::StudyClassification
            | ReviewDefinitionKey::StudyGrouping
            | ReviewDefinitionKey::AppraisalPrefill
            | ReviewDefinitionKey::DataExtraction => {
                self.standard_plan().map(ReviewExecutionPlan::Standard)
            }
        }
    }

    fn standard_plan(&self) -> Result<StandardReviewPlan, ReviewError> {
        let prepare = self.expect_entrypoint(ReviewNodeKind::Prepare)?;
        let generate = self.follow(prepare, ReviewTransitionPredicate::Always, |operation| {
            matches!(operation, ReviewNodeKind::Generate { .. })
        })?;
        let validate = self.follow(generate, ReviewTransitionPredicate::Always, |operation| {
            matches!(operation, ReviewNodeKind::Validate)
        })?;
        let assemble = self.follow(validate, ReviewTransitionPredicate::Valid, |operation| {
            matches!(operation, ReviewNodeKind::Assemble)
        })?;
        let finalize = self.follow(assemble, ReviewTransitionPredicate::Always, |operation| {
            matches!(operation, ReviewNodeKind::Finalize)
        })?;
        self.expect_target(validate, ReviewTransitionPredicate::Invalid, finalize)?;
        self.expect_exact_predicates(prepare, &[ReviewTransitionPredicate::Always])?;
        self.expect_exact_predicates(generate, &[ReviewTransitionPredicate::Always])?;
        self.expect_exact_predicates(
            validate,
            &[
                ReviewTransitionPredicate::Valid,
                ReviewTransitionPredicate::Invalid,
            ],
        )?;
        self.expect_exact_predicates(assemble, &[ReviewTransitionPredicate::Always])?;
        self.expect_exact_predicates(finalize, &[])?;
        Ok(StandardReviewPlan {
            prepare: review_node(prepare),
            generate: review_node(generate),
            validate: review_node(validate),
            assemble: review_node(assemble),
            finalize: review_node(finalize),
        })
    }

    fn screening_plan(&self) -> Result<ScreeningReviewPlan, ReviewError> {
        let prepare = self.expect_entrypoint(ReviewNodeKind::Prepare)?;
        let primary = self.follow(prepare, ReviewTransitionPredicate::Always, |operation| {
            matches!(operation, ReviewNodeKind::PrimaryScreen)
        })?;
        let validate_primary =
            self.follow(primary, ReviewTransitionPredicate::Always, |operation| {
                matches!(operation, ReviewNodeKind::Validate)
            })?;
        let derive = self.follow(
            validate_primary,
            ReviewTransitionPredicate::Valid,
            |operation| matches!(operation, ReviewNodeKind::Derive),
        )?;
        let independent = self.follow(
            derive,
            ReviewTransitionPredicate::NeedsIndependentScreen,
            |operation| matches!(operation, ReviewNodeKind::IndependentScreen),
        )?;
        let validate_independent = self.follow(
            independent,
            ReviewTransitionPredicate::Always,
            |operation| matches!(operation, ReviewNodeKind::Validate),
        )?;
        let reconcile = self.follow(
            validate_independent,
            ReviewTransitionPredicate::Valid,
            |operation| matches!(operation, ReviewNodeKind::Reconcile),
        )?;
        let assemble = self.follow(
            reconcile,
            ReviewTransitionPredicate::Agreement,
            |operation| matches!(operation, ReviewNodeKind::Assemble),
        )?;
        self.expect_target(derive, ReviewTransitionPredicate::PrimaryAccepted, assemble)?;
        let audit = self.follow(assemble, ReviewTransitionPredicate::Always, |operation| {
            matches!(operation, ReviewNodeKind::CandidateAudit)
        })?;
        let repair = self.follow(
            audit,
            ReviewTransitionPredicate::AuditRepairable,
            |operation| matches!(operation, ReviewNodeKind::SemanticRepair { .. }),
        )?;
        let validate_repair = self.follow(
            repair,
            ReviewTransitionPredicate::RepairReady,
            |operation| matches!(operation, ReviewNodeKind::Validate),
        )?;
        self.expect_target(validate_repair, ReviewTransitionPredicate::Valid, assemble)?;
        self.expect_target(
            validate_repair,
            ReviewTransitionPredicate::Invalid,
            assemble,
        )?;
        let finalize =
            self.node_for_operation(|operation| matches!(operation, ReviewNodeKind::Finalize))?;
        for (node, predicate) in [
            (validate_primary, ReviewTransitionPredicate::Invalid),
            (validate_independent, ReviewTransitionPredicate::Invalid),
            (reconcile, ReviewTransitionPredicate::Disagreement),
            (audit, ReviewTransitionPredicate::AuditPassed),
            (audit, ReviewTransitionPredicate::Invalid),
            (repair, ReviewTransitionPredicate::RepairExhausted),
        ] {
            self.expect_target(node, predicate, finalize)?;
        }
        self.expect_exact_predicates(prepare, &[ReviewTransitionPredicate::Always])?;
        self.expect_exact_predicates(primary, &[ReviewTransitionPredicate::Always])?;
        self.expect_exact_predicates(
            validate_primary,
            &[
                ReviewTransitionPredicate::Valid,
                ReviewTransitionPredicate::Invalid,
            ],
        )?;
        self.expect_exact_predicates(
            derive,
            &[
                ReviewTransitionPredicate::NeedsIndependentScreen,
                ReviewTransitionPredicate::PrimaryAccepted,
            ],
        )?;
        self.expect_exact_predicates(independent, &[ReviewTransitionPredicate::Always])?;
        self.expect_exact_predicates(
            validate_independent,
            &[
                ReviewTransitionPredicate::Valid,
                ReviewTransitionPredicate::Invalid,
            ],
        )?;
        self.expect_exact_predicates(
            reconcile,
            &[
                ReviewTransitionPredicate::Agreement,
                ReviewTransitionPredicate::Disagreement,
            ],
        )?;
        self.expect_exact_predicates(assemble, &[ReviewTransitionPredicate::Always])?;
        self.expect_exact_predicates(
            audit,
            &[
                ReviewTransitionPredicate::AuditPassed,
                ReviewTransitionPredicate::AuditRepairable,
                ReviewTransitionPredicate::Invalid,
            ],
        )?;
        self.expect_exact_predicates(
            repair,
            &[
                ReviewTransitionPredicate::RepairReady,
                ReviewTransitionPredicate::RepairExhausted,
            ],
        )?;
        self.expect_exact_predicates(
            validate_repair,
            &[
                ReviewTransitionPredicate::Valid,
                ReviewTransitionPredicate::Invalid,
            ],
        )?;
        self.expect_exact_predicates(finalize, &[])?;
        Ok(ScreeningReviewPlan {
            prepare: review_node(prepare),
            primary_screen: review_node(primary),
            validate_primary: review_node(validate_primary),
            derive_primary: review_node(derive),
            independent_screen: review_node(independent),
            validate_independent: review_node(validate_independent),
            reconcile: review_node(reconcile),
            assemble: review_node(assemble),
            candidate_audit: review_node(audit),
            semantic_repair: review_node(repair),
            validate_repair: review_node(validate_repair),
            finalize: review_node(finalize),
            repair_budget: self.repair_budget(&repair.id)?,
        })
    }

    fn expect_entrypoint(
        &self,
        expected: ReviewNodeKind,
    ) -> Result<&ReviewWorkflowNode, ReviewError> {
        let node = self.node(&self.workflow.entrypoint)?;
        if node.operation == expected {
            Ok(node)
        } else {
            Err(ReviewError::InvalidWorkflow(
                "entrypoint operation is invalid".to_owned(),
            ))
        }
    }

    fn node(&self, node_id: &str) -> Result<&ReviewWorkflowNode, ReviewError> {
        self.workflow
            .nodes
            .iter()
            .find(|node| node.id == node_id)
            .ok_or_else(|| ReviewError::InvalidWorkflow(format!("unknown node {node_id}")))
    }

    fn node_for_operation(
        &self,
        predicate: impl Fn(ReviewNodeKind) -> bool,
    ) -> Result<&ReviewWorkflowNode, ReviewError> {
        let mut matching = self
            .workflow
            .nodes
            .iter()
            .filter(|node| predicate(node.operation));
        let node = matching.next().ok_or_else(|| {
            ReviewError::InvalidWorkflow("required workflow operation is missing".to_owned())
        })?;
        if matching.next().is_some() {
            return Err(ReviewError::InvalidWorkflow(
                "workflow operation must be unique".to_owned(),
            ));
        }
        Ok(node)
    }

    fn follow(
        &self,
        node: &ReviewWorkflowNode,
        predicate: ReviewTransitionPredicate,
        expected: impl Fn(ReviewNodeKind) -> bool,
    ) -> Result<&ReviewWorkflowNode, ReviewError> {
        let target = self.transition(&node.id, ReviewTransitionSignal::from(predicate))?;
        let target = self.node(target)?;
        if expected(target.operation) {
            Ok(target)
        } else {
            Err(ReviewError::InvalidWorkflow(format!(
                "node {} has an unsafe transition target",
                node.id
            )))
        }
    }

    fn expect_target(
        &self,
        node: &ReviewWorkflowNode,
        predicate: ReviewTransitionPredicate,
        expected: &ReviewWorkflowNode,
    ) -> Result<(), ReviewError> {
        let target = self.transition(&node.id, ReviewTransitionSignal::from(predicate))?;
        if target == expected.id {
            Ok(())
        } else {
            Err(ReviewError::InvalidWorkflow(format!(
                "node {} has an unsafe transition target",
                node.id
            )))
        }
    }

    fn expect_exact_predicates(
        &self,
        node: &ReviewWorkflowNode,
        expected: &[ReviewTransitionPredicate],
    ) -> Result<(), ReviewError> {
        let actual = node
            .transitions
            .iter()
            .map(|transition| transition.predicate)
            .collect::<BTreeSet<_>>();
        let expected = expected.iter().copied().collect::<BTreeSet<_>>();
        if actual == expected {
            Ok(())
        } else {
            Err(ReviewError::InvalidWorkflow(format!(
                "node {} has unsafe transition predicates",
                node.id
            )))
        }
    }
}

fn review_node(node: &ReviewWorkflowNode) -> ReviewNode {
    ReviewNode {
        id: node.id.clone(),
        version: node.version,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum ReviewSemanticHandler {
    ScreeningAnalysis,
    DuplicateAnalysis,
    StudyDesignClassification,
    StudyGrouping,
    AppraisalPrefill,
    DataExtraction,
}

impl ReviewSemanticHandler {
    fn accepts_task(self, task: AiTaskKind) -> bool {
        match self {
            Self::ScreeningAnalysis => matches!(
                task,
                AiTaskKind::TitleAbstractScreening | AiTaskKind::FullTextScreening
            ),
            Self::DuplicateAnalysis => task == AiTaskKind::DuplicateCandidateDetection,
            Self::StudyDesignClassification => task == AiTaskKind::StudyDesignClassification,
            Self::StudyGrouping => task == AiTaskKind::StudyGrouping,
            Self::AppraisalPrefill => task == AiTaskKind::AppraisalPrefill,
            Self::DataExtraction => task == AiTaskKind::DataExtraction,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum ReviewProposalType {
    ScreeningSuggestion,
    DedupeSuggestion,
    StudyDesignClassificationSuggestion,
    StudyGroupingSuggestion,
    AppraisalPrefill,
    DataExtraction,
}

impl ReviewProposalType {
    const fn as_str(self) -> &'static str {
        match self {
            Self::ScreeningSuggestion => "screening_suggestion",
            Self::DedupeSuggestion => "dedupe_suggestion",
            Self::StudyDesignClassificationSuggestion => "study_design_classification_suggestion",
            Self::StudyGroupingSuggestion => "study_grouping_suggestion",
            Self::AppraisalPrefill => "appraisal_prefill",
            Self::DataExtraction => "data_extraction",
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
    semantic_handler: ReviewSemanticHandler,
    final_proposal_type: ReviewProposalType,
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

impl From<ReviewTransitionPredicate> for ReviewTransitionSignal {
    fn from(predicate: ReviewTransitionPredicate) -> Self {
        match predicate {
            ReviewTransitionPredicate::Always => Self::Always,
            ReviewTransitionPredicate::Valid => Self::Valid,
            ReviewTransitionPredicate::Invalid => Self::Invalid,
            ReviewTransitionPredicate::NeedsIndependentScreen => Self::NeedsIndependentScreen,
            ReviewTransitionPredicate::PrimaryAccepted => Self::PrimaryAccepted,
            ReviewTransitionPredicate::Agreement => Self::Agreement,
            ReviewTransitionPredicate::Disagreement => Self::Disagreement,
            ReviewTransitionPredicate::AuditPassed => Self::AuditPassed,
            ReviewTransitionPredicate::AuditRepairable => Self::AuditRepairable,
            ReviewTransitionPredicate::RepairReady => Self::RepairReady,
            ReviewTransitionPredicate::RepairExhausted => Self::RepairExhausted,
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
    let (expected_handler, expected_proposal_type) = expected_binding(key);
    if workflow.semantic_handler != expected_handler
        || workflow.final_proposal_type != expected_proposal_type
    {
        return Err(ReviewError::InvalidWorkflow(
            "semantic handler or final proposal type does not match the definition".to_owned(),
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
    let (handler, _) = expected_binding(key);
    match key {
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
            Ok(())
        }
        ReviewDefinitionKey::DuplicateDetection
        | ReviewDefinitionKey::StudyClassification
        | ReviewDefinitionKey::StudyGrouping
        | ReviewDefinitionKey::AppraisalPrefill
        | ReviewDefinitionKey::DataExtraction => {
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
            if tasks.len() != 1 || !handler.accepts_task(tasks[0]) {
                return Err(ReviewError::InvalidWorkflow(
                    "definition must bind exactly one matching AI task".to_owned(),
                ));
            }
            Ok(())
        }
    }
}

const fn expected_binding(key: ReviewDefinitionKey) -> (ReviewSemanticHandler, ReviewProposalType) {
    match key {
        ReviewDefinitionKey::Screening => (
            ReviewSemanticHandler::ScreeningAnalysis,
            ReviewProposalType::ScreeningSuggestion,
        ),
        ReviewDefinitionKey::DuplicateDetection => (
            ReviewSemanticHandler::DuplicateAnalysis,
            ReviewProposalType::DedupeSuggestion,
        ),
        ReviewDefinitionKey::StudyClassification => (
            ReviewSemanticHandler::StudyDesignClassification,
            ReviewProposalType::StudyDesignClassificationSuggestion,
        ),
        ReviewDefinitionKey::StudyGrouping => (
            ReviewSemanticHandler::StudyGrouping,
            ReviewProposalType::StudyGroupingSuggestion,
        ),
        ReviewDefinitionKey::AppraisalPrefill => (
            ReviewSemanticHandler::AppraisalPrefill,
            ReviewProposalType::AppraisalPrefill,
        ),
        ReviewDefinitionKey::DataExtraction => (
            ReviewSemanticHandler::DataExtraction,
            ReviewProposalType::DataExtraction,
        ),
    }
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
#[path = "definition_tests.rs"]
mod tests;
