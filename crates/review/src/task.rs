use deepref_ai::{
    AiContext, AiError, AiTask, AiTaskKind, AuthorityTier, GroundedBlock, ModelProfile,
    ProposalDraft,
};
use serde_json::Value;

use crate::{CompiledReviewDefinition, ReviewError};

/// An existing typed AI task bound to one compiled review definition.
///
/// The task retains semantic validation and proposal construction. The
/// definition supplies the checked-in prompt asset and verifies that callers
/// cannot pair a task with an unrelated methodology.
pub struct DefinedAiTask<T> {
    definition: CompiledReviewDefinition,
    task: T,
    workflow_node: Option<WorkflowNodeContext>,
}

struct WorkflowNodeContext {
    id: String,
    semantic_context: Option<Value>,
}

impl<T: AiTask> DefinedAiTask<T> {
    pub fn bind(definition: CompiledReviewDefinition, task: T) -> Result<Self, ReviewError> {
        if !definition.accepts_task(task.kind()) {
            return Err(ReviewError::InvalidDefinition(format!(
                "definition {} cannot execute task {}",
                definition.key(),
                task.kind().as_str()
            )));
        }
        Ok(Self {
            definition,
            task,
            workflow_node: None,
        })
    }

    #[doc(hidden)]
    pub fn bind_for_node(
        definition: CompiledReviewDefinition,
        task: T,
        node_id: &str,
        semantic_context: Option<Value>,
    ) -> Result<Self, ReviewError> {
        if definition.node_version(node_id).is_none() {
            return Err(ReviewError::InvalidWorkflow(format!(
                "unknown execution node {node_id}"
            )));
        }
        let mut bound = Self::bind(definition, task)?;
        bound.workflow_node = Some(WorkflowNodeContext {
            id: node_id.to_owned(),
            semantic_context,
        });
        Ok(bound)
    }

    pub const fn definition(&self) -> &CompiledReviewDefinition {
        &self.definition
    }
}

impl<T: AiTask> AiTask for DefinedAiTask<T> {
    type Input = T::Input;
    type Output = T::Output;

    const KIND: AiTaskKind = T::KIND;
    const PROMPT_VERSION: &'static str = T::PROMPT_VERSION;
    const SCHEMA_VERSION: &'static str = T::SCHEMA_VERSION;

    fn kind(&self) -> AiTaskKind {
        self.task.kind()
    }

    fn prompt_version(&self) -> &str {
        self.task.prompt_version()
    }

    fn model_profile(&self) -> ModelProfile {
        self.task.model_profile()
    }

    fn build_context(&self, input: &Self::Input) -> Result<AiContext, AiError> {
        let mut context = self.task.build_context(input)?;
        context.system_prompt = self.definition.system_prompt().to_owned();
        if let Some(node) = &self.workflow_node {
            context
                .system_prompt
                .push_str("\n\nActive compiled workflow node: ");
            context.system_prompt.push_str(&node.id);
            context
                .system_prompt
                .push_str(". Follow only that node's role from the checked-in prompt bundle.");
            if let Some(semantic_context) = &node.semantic_context {
                let source = serde_json::from_str::<Value>(&context.user_prompt)
                    .unwrap_or_else(|_| Value::String(context.user_prompt.clone()));
                context.user_prompt = serde_json::to_string(&serde_json::json!({
                    "source": source,
                    "node_context": semantic_context,
                }))
                .map_err(|_| AiError::InputSerialization("review node context".to_owned()))?;
            }
        }
        Ok(context)
    }

    fn semantic_validate(&self, output: &Self::Output) -> Result<(), AiError> {
        self.task.semantic_validate(output)
    }

    fn semantic_validate_with_evidence(
        &self,
        output: &Self::Output,
        evidence: &[GroundedBlock],
    ) -> Result<(), AiError> {
        self.task.semantic_validate_with_evidence(output, evidence)
    }

    fn authority(&self) -> AuthorityTier {
        self.task.authority()
    }

    fn proposal(&self, output: &Self::Output) -> Option<ProposalDraft> {
        self.task.proposal(output)
    }
}

#[cfg(test)]
mod tests {
    use deepref_ai::{AiContext, AiTask, AuthorityTier, ModelProfile};
    use schemars::JsonSchema;
    use serde::{Deserialize, Serialize};

    use super::*;
    use crate::{ReviewCatalog, ReviewDefinitionKey};

    #[derive(Debug, Serialize)]
    struct Input;

    #[derive(Debug, Serialize, Deserialize, JsonSchema)]
    struct Output;

    struct FixtureTask;

    impl AiTask for FixtureTask {
        type Input = Input;
        type Output = Output;

        const KIND: AiTaskKind = AiTaskKind::DuplicateCandidateDetection;
        const PROMPT_VERSION: &'static str = "fixture.v1";
        const SCHEMA_VERSION: &'static str = "fixture.v1";

        fn model_profile(&self) -> ModelProfile {
            ModelProfile::FastClassifier
        }

        fn build_context(&self, _input: &Input) -> Result<AiContext, AiError> {
            Ok(AiContext {
                project_id: None,
                system_prompt: "inline prompt must be replaced".to_owned(),
                user_prompt: "{}".to_owned(),
                retrieval: None,
                protocol_hash: None,
                document_hash: None,
            })
        }

        fn semantic_validate(&self, _output: &Output) -> Result<(), AiError> {
            Ok(())
        }

        fn authority(&self) -> AuthorityTier {
            AuthorityTier::WorkflowSuggestion
        }
    }

    #[test]
    fn checked_in_prompt_replaces_task_inline_prompt() {
        let definition = ReviewCatalog
            .compile(ReviewDefinitionKey::DuplicateDetection)
            .expect("definition should compile");
        let expected = definition.system_prompt().to_owned();
        let task = DefinedAiTask::bind(definition, FixtureTask).expect("task should bind");
        let context = task.build_context(&Input).expect("context should build");
        assert_eq!(context.system_prompt, expected);
        assert!(!context.system_prompt.contains("inline prompt"));
    }

    #[test]
    fn mismatched_task_is_rejected() {
        let definition = ReviewCatalog
            .compile(ReviewDefinitionKey::DataExtraction)
            .expect("definition should compile");
        assert!(DefinedAiTask::bind(definition, FixtureTask).is_err());
    }
}
