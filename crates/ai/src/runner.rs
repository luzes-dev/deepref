use chrono::{DateTime, Utc};
use deepref_domain::ProjectId;
use schemars::{JsonSchema, schema_for};
use serde::{Serialize, de::DeserializeOwned};
use serde_json::{Value, json};
use tracing::Instrument;
use uuid::Uuid;

use crate::{
    AiContext, AiError, AiFuture, AiGateway, AiProposal, AiRunRecord, AiRunStatus, AiTaskKind,
    AuthorityTier, CompletionRequest, GroundedBlock, ModelProfile, ResolvedModel, ReuseKeyInput,
    SafeErrorMetadata, TokenUsage, compute_reuse_hash, hash_json,
};

pub trait ModelRouter: Send + Sync {
    fn resolve<'a>(&'a self, profile: ModelProfile) -> AiFuture<'a, ResolvedModel>;
}

pub trait EvidenceRetriever: Send + Sync {
    fn retrieve<'a>(&'a self, request: crate::RetrievalRequest)
    -> AiFuture<'a, Vec<GroundedBlock>>;
}
pub trait AiRunStore: Send + Sync {
    fn find_reusable<'a>(
        &'a self,
        project_id: Option<ProjectId>,
        reuse_hash: &'a str,
    ) -> AiFuture<'a, Option<AiRunRecord>>;
    fn save_run<'a>(&'a self, run: AiRunRecord) -> AiFuture<'a, ()>;
}
pub trait ProposalStore: Send + Sync {
    fn find_for_run<'a>(&'a self, run_id: Uuid) -> AiFuture<'a, Option<AiProposal>>;
    fn create<'a>(&'a self, proposal: AiProposal) -> AiFuture<'a, AiProposal>;
}
pub trait Clock: Send + Sync {
    fn now(&self) -> DateTime<Utc>;
}
#[derive(Debug, Default, Clone, Copy)]
pub struct SystemClock;
impl Clock for SystemClock {
    fn now(&self) -> DateTime<Utc> {
        Utc::now()
    }
}
pub trait IdProvider: Send + Sync {
    fn next_id(&self) -> Uuid;
}
#[derive(Debug, Default, Clone, Copy)]
pub struct UuidProvider;
impl IdProvider for UuidProvider {
    fn next_id(&self) -> Uuid {
        Uuid::new_v4()
    }
}

pub trait AiTask {
    type Input: Serialize + Send + Sync;
    type Output: Serialize + DeserializeOwned + JsonSchema + Send + Sync + 'static;
    const KIND: AiTaskKind;
    const PROMPT_VERSION: &'static str;
    const SCHEMA_VERSION: &'static str;
    fn kind(&self) -> AiTaskKind {
        Self::KIND
    }
    fn prompt_version(&self) -> &str {
        Self::PROMPT_VERSION
    }
    fn model_profile(&self) -> ModelProfile;
    fn build_context(&self, input: &Self::Input) -> Result<AiContext, AiError>;
    fn semantic_validate(&self, output: &Self::Output) -> Result<(), AiError>;
    fn semantic_validate_with_evidence(
        &self,
        output: &Self::Output,
        _evidence: &[GroundedBlock],
    ) -> Result<(), AiError> {
        self.semantic_validate(output)
    }
    fn authority(&self) -> AuthorityTier {
        AuthorityTier::ReadOnly
    }
    fn proposal(&self, _output: &Self::Output) -> Option<crate::ProposalDraft> {
        None
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct AiTaskResult<T> {
    pub output: T,
    pub run: AiRunRecord,
    pub proposal: Option<AiProposal>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ProposalPersistence {
    #[default]
    Persist,
    Skip,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct AiExecutionContext {
    pub parent_automation_run_id: Option<Uuid>,
    pub node_fingerprint: Option<String>,
    pub proposal_persistence: ProposalPersistence,
}

pub struct AiTaskRunner<'a, G: ?Sized, R, E, S, P, C, I> {
    gateway: &'a G,
    router: &'a R,
    retriever: &'a E,
    store: &'a S,
    proposals: &'a P,
    clock: &'a C,
    ids: &'a I,
}

impl<'a, G: ?Sized, R, E, S, P, C, I> AiTaskRunner<'a, G, R, E, S, P, C, I>
where
    G: AiGateway,
    R: ModelRouter,
    E: EvidenceRetriever,
    S: AiRunStore,
    P: ProposalStore,
    C: Clock,
    I: IdProvider,
{
    pub const fn new(
        gateway: &'a G,
        router: &'a R,
        retriever: &'a E,
        store: &'a S,
        proposals: &'a P,
        clock: &'a C,
        ids: &'a I,
    ) -> Self {
        Self {
            gateway,
            router,
            retriever,
            store,
            proposals,
            clock,
            ids,
        }
    }

    pub async fn run<T>(
        &self,
        task: &T,
        input: T::Input,
    ) -> Result<AiTaskResult<T::Output>, AiError>
    where
        T: AiTask,
    {
        self.run_with_context(task, input, AiExecutionContext::default())
            .await
    }

    pub async fn run_with_context<T>(
        &self,
        task: &T,
        input: T::Input,
        execution: AiExecutionContext,
    ) -> Result<AiTaskResult<T::Output>, AiError>
    where
        T: AiTask,
    {
        if execution
            .node_fingerprint
            .as_deref()
            .is_some_and(|fingerprint| !crate::is_sha256(fingerprint))
        {
            return Err(AiError::InvalidContext(
                "node fingerprint must be a SHA-256 digest".to_owned(),
            ));
        }
        let task_input_json = serde_json::to_value(&input)
            .map_err(|_| AiError::InputSerialization("task input".to_owned()))?;
        let input_json = json!({
            "task_input": task_input_json,
            "node_fingerprint": execution.node_fingerprint,
        });
        let input_hash = hash_json(&input_json)?;
        let mut context = task.build_context(&input)?;
        context.validate()?;
        let project_id = context
            .project_id
            .or_else(|| context.retrieval.as_ref().map(|request| request.project_id));
        let evidence = match context.retrieval.take() {
            Some(request) => {
                self.retriever
                    .retrieve(request)
                    .instrument(tracing::info_span!(
                        "retrieval.search",
                        ai.task_kind = task.kind().as_str()
                    ))
                    .await?
            }
            None => Vec::new(),
        };
        for block in &evidence {
            block.validate()?;
        }
        let evidence_refs = evidence
            .iter()
            .map(|block| block.evidence.clone())
            .collect::<Vec<_>>();
        let evidence_hash = if evidence_refs.is_empty() {
            None
        } else {
            Some(hash_json(&serde_json::to_value(&evidence_refs).map_err(
                |_| AiError::InputSerialization("evidence".to_owned()),
            )?)?)
        };
        let route = self.router.resolve(task.model_profile()).await?;
        route.validate()?;
        let schema = serde_json::to_value(schema_for!(T::Output))
            .map_err(|_| AiError::InputSerialization("output schema".to_owned()))?;
        let schema_hash = hash_json(&schema)?;
        let prompt_hash =
            hash_json(&json!({"system": context.system_prompt, "user": context.user_prompt}))?;
        let reuse_hash = compute_reuse_hash(&ReuseKeyInput {
            task_kind: task.kind().as_str().to_owned(),
            provider: route.provider.clone(),
            model: route.model.clone(),
            model_version: route.model_version.clone(),
            parameters: serde_json::to_value(&route.parameters)
                .map_err(|_| AiError::InputSerialization("route parameters".to_owned()))?,
            prompt_version: task.prompt_version().to_owned(),
            prompt_hash: prompt_hash.clone(),
            schema_version: T::SCHEMA_VERSION.to_owned(),
            schema_hash: schema_hash.clone(),
            input_hash: input_hash.clone(),
            protocol_hash: context.protocol_hash.clone(),
            document_hash: context.document_hash.clone(),
            evidence_hash: evidence_hash.clone(),
        })?;

        if let Some(run) = self.store.find_reusable(project_id, &reuse_hash).await? {
            run.validate()?;
            let raw = run
                .output
                .clone()
                .ok_or_else(|| AiError::Persistence("completed run has no output".to_owned()))?;
            let output = validate_output(task, raw, &evidence)?;
            let proposal = match execution.proposal_persistence {
                ProposalPersistence::Persist => {
                    self.ensure_proposal(task, &output, &run, project_id)
                        .await?
                }
                ProposalPersistence::Skip => None,
            };
            return Ok(AiTaskResult {
                output,
                run,
                proposal,
            });
        }

        let mut run = AiRunRecord {
            id: self.ids.next_id(),
            project_id,
            task_kind: task.kind(),
            route: route.clone(),
            prompt_version: task.prompt_version().to_owned(),
            prompt_hash,
            schema_version: T::SCHEMA_VERSION.to_owned(),
            schema_hash,
            input_hash,
            reuse_hash,
            protocol_hash: context.protocol_hash.clone(),
            document_hash: context.document_hash.clone(),
            evidence_hash,
            evidence_refs,
            usage: TokenUsage::default(),
            cost_micros: None,
            output: None,
            status: AiRunStatus::Running,
            error: None,
            parent_automation_run_id: execution.parent_automation_run_id,
            completed_at: None,
            created_at: self.clock.now(),
        };
        run.validate()?;
        self.store.save_run(run.clone()).await?;
        let completion = match self
            .gateway
            .complete(CompletionRequest {
                route,
                system_prompt: context.system_prompt,
                user_prompt: context.user_prompt,
                evidence: evidence.clone(),
                schema,
            })
            .instrument(tracing::info_span!(
                "model.complete",
                ai.task_kind = task.kind().as_str(),
                ai.prompt_version = task.prompt_version(),
                ai.schema_version = T::SCHEMA_VERSION
            ))
            .await
        {
            Ok(completion) => completion,
            Err(error) => return Err(self.persist_failure(run, error).await),
        };
        let raw: Value = match serde_json::from_str(&completion.output_json) {
            Ok(raw) => raw,
            Err(_) => {
                return Err(self
                    .persist_failure(run, AiError::MalformedOutput(String::new()))
                    .await);
            }
        };
        let output = match validate_output(task, raw.clone(), &evidence) {
            Ok(output) => output,
            Err(error) => return Err(self.persist_failure(run, error).await),
        };
        run.usage = TokenUsage {
            input_tokens: completion.input_tokens,
            output_tokens: completion.output_tokens,
        };
        run.cost_micros = completion.cost_micros;
        run.output = Some(raw);
        run.status = AiRunStatus::Completed;
        run.completed_at = Some(self.clock.now());
        run.validate()?;
        self.store.save_run(run.clone()).await?;
        let proposal = match execution.proposal_persistence {
            ProposalPersistence::Persist => {
                self.ensure_proposal(task, &output, &run, project_id)
                    .await?
            }
            ProposalPersistence::Skip => None,
        };
        Ok(AiTaskResult {
            output,
            run,
            proposal,
        })
    }

    async fn ensure_proposal<T: AiTask>(
        &self,
        task: &T,
        output: &T::Output,
        run: &AiRunRecord,
        project_id: Option<ProjectId>,
    ) -> Result<Option<AiProposal>, AiError> {
        if !task.authority().requires_proposal() {
            return Ok(None);
        }
        let mut draft = task.proposal(output).ok_or_else(|| {
            AiError::Proposal("consequential task did not produce a proposal".to_owned())
        })?;
        if Some(draft.project_id) != project_id
            || draft.authority != task.authority()
            || draft.entity_type.trim().is_empty()
            || draft.operation.trim().is_empty()
            || !draft.payload.is_object()
        {
            return Err(AiError::Proposal(
                "proposal does not match task authority or project context".to_owned(),
            ));
        }
        draft.authority = task.authority();
        let Some(payload) = draft.payload.as_object_mut() else {
            return Err(AiError::Proposal(
                "proposal payload must be an object".to_owned(),
            ));
        };
        payload.insert(
            "task_kind".to_owned(),
            Value::String(task.kind().as_str().to_owned()),
        );
        if let Some(existing) = self.proposals.find_for_run(run.id).await? {
            if same_proposal_content(&existing, &draft, run.id) {
                return Ok(Some(existing));
            }
            return Err(AiError::Proposal(
                "existing proposal diverges from validated task output".to_owned(),
            ));
        }
        self.proposals
            .create(AiProposal {
                id: self.ids.next_id(),
                draft,
                model_run_id: run.id,
                status: crate::ProposalStatus::Pending,
                resolved_at: None,
                resolved_by_actor_id: None,
            })
            .await
            .map(Some)
    }

    async fn persist_failure(&self, mut run: AiRunRecord, error: AiError) -> AiError {
        run.status = AiRunStatus::Failed;
        run.error = Some(safe_error_metadata(&error));
        run.completed_at = Some(self.clock.now());
        if run.validate().is_err() {
            return AiError::Persistence("failed AI run state is invalid".to_owned());
        }
        match self.store.save_run(run).await {
            Ok(()) => error,
            Err(_) => AiError::Persistence("failed to persist AI run".to_owned()),
        }
    }
}

fn same_proposal_content(
    existing: &AiProposal,
    expected: &crate::ProposalDraft,
    run_id: Uuid,
) -> bool {
    existing.model_run_id == run_id
        && existing.draft.project_id == expected.project_id
        && existing.draft.entity_type == expected.entity_type
        && existing.draft.entity_id == expected.entity_id
        && existing.draft.operation == expected.operation
        && existing.draft.payload == expected.payload
        && existing.draft.authority == expected.authority
}

fn validate_output<T: AiTask>(
    task: &T,
    raw: Value,
    evidence: &[GroundedBlock],
) -> Result<T::Output, AiError> {
    let schema = serde_json::to_value(schemars::schema_for!(T::Output))
        .map_err(|_| AiError::InputSerialization("output schema".to_owned()))?;
    jsonschema::validator_for(&schema)
        .map_err(|_| AiError::SchemaValidation(String::new()))?
        .validate(&raw)
        .map_err(|_| AiError::SchemaValidation(String::new()))?;
    let output =
        serde_json::from_value(raw).map_err(|_| AiError::SchemaValidation(String::new()))?;
    task.semantic_validate_with_evidence(&output, evidence)
        .map_err(|_| AiError::SemanticValidation(String::new()))?;
    Ok(output)
}

pub fn safe_error_metadata(error: &AiError) -> SafeErrorMetadata {
    let code = match error {
        AiError::InputSerialization(_) => "input_serialization",
        AiError::InvalidContext(_) => "invalid_context",
        AiError::Route(_) => "route",
        AiError::Gateway(_) => "gateway",
        AiError::MalformedOutput(_) => "malformed_output",
        AiError::SchemaValidation(_) => "schema_validation",
        AiError::SemanticValidation(_) => "semantic_validation",
        AiError::Persistence(_) => "persistence",
        AiError::Proposal(_) => "proposal",
        AiError::PromptRegistry(_) => "prompt_registry",
        AiError::InvalidEmbedding(_) => "invalid_embedding",
    };
    let message = match code {
        "gateway" => "provider request failed",
        "malformed_output" => "provider returned malformed structured output",
        "schema_validation" => "structured output failed schema validation",
        "semantic_validation" => "structured output failed semantic validation",
        "persistence" => "AI run persistence failed",
        "proposal" => "proposal persistence or validation failed",
        _ => "AI task failed",
    };
    SafeErrorMetadata {
        code: code.to_owned(),
        message: message.to_owned(),
    }
}
