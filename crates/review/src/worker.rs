//! Narrow worker-facing interface for compiled review execution.
//!
//! The checked-in graph, transition predicates, asset catalog, and hashing
//! implementation remain private. Workers receive a typed execution plan and
//! opaque node handles; persistence receives only stable attempt identities.

use crate::{
    CompiledReviewDefinition, ReviewCatalog, ReviewDefinitionKey, ReviewError,
    manifest::fingerprint_node,
};

pub use crate::execution::{ExecutedReviewTask, PreparedReviewTask};
pub use crate::hash::ReviewHash;
pub use crate::manifest::{
    AcceptedArtifactInput, ReviewManifestInput, ReviewModelIdentity, ReviewRunManifest,
    ReviewRuntimeIdentity,
};

#[derive(Debug, Clone)]
pub struct CompiledReview {
    definition: CompiledReviewDefinition,
    plan: ReviewExecutionPlan,
}

impl CompiledReview {
    pub fn compile(key: ReviewDefinitionKey) -> Result<Self, ReviewError> {
        let definition = ReviewCatalog.compile(key)?;
        let plan = definition.execution_plan()?;
        Ok(Self { definition, plan })
    }

    pub const fn key(&self) -> ReviewDefinitionKey {
        self.definition.key()
    }

    pub const fn plan(&self) -> &ReviewExecutionPlan {
        &self.plan
    }

    pub fn build_manifest(
        &self,
        input: ReviewManifestInput,
    ) -> Result<ReviewRunManifest, ReviewError> {
        ReviewRunManifest::build(&self.definition, input)
    }

    pub fn attempt_identity(
        &self,
        manifest: &ReviewRunManifest,
        node: &ReviewNode,
        predecessors: &[AcceptedArtifactInput],
    ) -> Result<ReviewAttemptIdentity, ReviewError> {
        let input_fingerprint =
            fingerprint_node(&self.definition, manifest, node.id(), predecessors)?;
        Ok(ReviewAttemptIdentity {
            node_id: node.id.clone(),
            node_version: node.version,
            input_fingerprint,
        })
    }

    pub fn final_proposal_type(&self) -> &'static str {
        self.definition.final_proposal_type()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReviewNode {
    pub(crate) id: String,
    pub(crate) version: u32,
}

impl ReviewNode {
    pub fn id(&self) -> &str {
        &self.id
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReviewAttemptIdentity {
    pub node_id: String,
    pub node_version: u32,
    pub input_fingerprint: ReviewHash,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReviewExecutionPlan {
    Standard(StandardReviewPlan),
    Screening(Box<ScreeningReviewPlan>),
}

impl ReviewExecutionPlan {
    pub const fn prepare(&self) -> &ReviewNode {
        match self {
            Self::Standard(plan) => &plan.prepare,
            Self::Screening(plan) => &plan.prepare,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StandardReviewPlan {
    pub prepare: ReviewNode,
    pub generate: ReviewNode,
    pub validate: ReviewNode,
    pub assemble: ReviewNode,
    pub finalize: ReviewNode,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScreeningReviewPlan {
    pub prepare: ReviewNode,
    pub primary_screen: ReviewNode,
    pub validate_primary: ReviewNode,
    pub derive_primary: ReviewNode,
    pub independent_screen: ReviewNode,
    pub validate_independent: ReviewNode,
    pub reconcile: ReviewNode,
    pub assemble: ReviewNode,
    pub candidate_audit: ReviewNode,
    pub semantic_repair: ReviewNode,
    pub validate_repair: ReviewNode,
    pub finalize: ReviewNode,
    pub repair_budget: u8,
}
