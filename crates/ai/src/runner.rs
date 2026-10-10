use chrono::{DateTime, Utc};
use deepref_domain::ProjectId;
use schemars::{JsonSchema, schema_for};
use serde::{Serialize, de::DeserializeOwned};
use serde_json::{Value, json};
use tracing::Instrument;
use uuid::Uuid;

use crate::{
    AiContext, AiError, AiFuture, AiGateway, AiProposal, AiRunRecord, AiRunStatus, AiTaskKind,
    AuthorityTier, CompletionRequest, GatewayCompletion, GroundedBlock, ModelProfile,
    ResolvedModel, ReuseKeyInput, SafeErrorMetadata, TokenUsage, compute_reuse_hash, hash_json,
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
    /// Deterministic repair before schema validation. Implementations may only
    /// apply unambiguous rewrites and must never invent a judgment.
    fn normalize_output(&self, _raw: &mut Value, _evidence: &[GroundedBlock]) {}
    /// Corrective re-asks after a schema, semantic or malformed-output failure.
    fn repair_attempts(&self) -> u8 {
        0
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
        let schema = structured_output_schema::<T>()?;
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
            provider_served_model: None,
            provider_system_fingerprint: None,
            output: None,
            status: AiRunStatus::Running,
            error: None,
            parent_automation_run_id: execution.parent_automation_run_id,
            completed_at: None,
            created_at: self.clock.now(),
        };
        run.validate()?;
        self.store.save_run(run.clone()).await?;
        let system_prompt = context.system_prompt;
        let base_user_prompt = context.user_prompt;
        let mut user_prompt = base_user_prompt.clone();
        let mut repairs_left = task.repair_attempts();
        // Every provider call is counted on the run, so a repaired run carries
        // the usage of both calls. The metered gateway records each call in the
        // usage ledger and checks the budget before it is made.
        let (raw, output) = loop {
            let completion = match self
                .gateway
                .complete(CompletionRequest {
                    project_id,
                    route: route.clone(),
                    system_prompt: system_prompt.clone(),
                    user_prompt: user_prompt.clone(),
                    evidence: evidence.clone(),
                    schema: schema.clone(),
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
            record_completion(&mut run, &completion);
            let attempt = interpret_structured_response(task, &completion.output_json, &evidence);
            match attempt {
                Ok(accepted) => break accepted,
                Err(error) => match repair_feedback(&error) {
                    Some(feedback) if repairs_left > 0 => {
                        repairs_left -= 1;
                        tracing::info!(
                            ai.task_kind = task.kind().as_str(),
                            reason = %feedback,
                            "repairing AI output after a validation failure"
                        );
                        user_prompt =
                            format!("{base_user_prompt}\n\n{}", repair_message(&feedback));
                    }
                    _ => return Err(self.persist_failure(run, error).await),
                },
            }
        };
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

/// The JSON Schema that a task's output must satisfy, as the provider receives it.
pub fn structured_output_schema<T: AiTask>() -> Result<Value, AiError> {
    serde_json::to_value(schema_for!(T::Output))
        .map_err(|_| AiError::InputSerialization("output schema".to_owned()))
}

/// Interprets one provider answer for a task: parses the JSON, applies the
/// task's deterministic normalization, then validates it against the output
/// schema and the task's semantic rules.
///
/// Returns the JSON as the provider sent it, which the run records, with the
/// validated output. The gateway has already removed any code-fence envelope,
/// so `output_json` is the JSON text. The repair loop stays in the runner.
pub fn interpret_structured_response<T: AiTask>(
    task: &T,
    output_json: &str,
    evidence: &[GroundedBlock],
) -> Result<(Value, T::Output), AiError> {
    let raw = serde_json::from_str::<Value>(output_json)
        .map_err(|_| AiError::MalformedOutput(String::new()))?;
    let output = validate_output(task, raw.clone(), evidence)?;
    Ok((raw, output))
}

fn validate_output<T: AiTask>(
    task: &T,
    mut raw: Value,
    evidence: &[GroundedBlock],
) -> Result<T::Output, AiError> {
    task.normalize_output(&mut raw, evidence);
    let schema = structured_output_schema::<T>()?;
    jsonschema::validator_for(&schema)
        .map_err(|_| AiError::SchemaValidation(String::new()))?
        .validate(&raw)
        .map_err(|error| {
            tracing::warn!(reason = %error, "AI output rejected by JSON Schema validation");
            AiError::SchemaValidation(schema_reason(
                &raw,
                error.instance_path().to_string().as_str(),
            ))
        })?;
    let output =
        serde_json::from_value(raw).map_err(|_| AiError::SchemaValidation(String::new()))?;
    task.semantic_validate_with_evidence(&output, evidence)
        .map_err(|error| {
            // Validation messages are static strings that never echo model
            // output; keep them in the log so rejected runs are diagnosable.
            let reason = match error {
                AiError::SemanticValidation(reason) => reason,
                _ => String::new(),
            };
            tracing::warn!(reason = %reason, "AI output rejected by semantic validation");
            AiError::SemanticValidation(reason)
        })?;
    Ok(output)
}

/// Describes a schema failure without echoing any value. When the failing
/// node sits inside a keyed judgment, the reason names that judgment by its
/// criterion id, which is the same id the model wrote. A list index would
/// point at a different entry once out-of-stage entries have been removed.
fn schema_reason(raw: &Value, pointer: &str) -> String {
    let mut criterion = None;
    let mut current = Some(raw);
    for segment in pointer.split('/').filter(|segment| !segment.is_empty()) {
        current = current.and_then(|value| match value {
            Value::Object(map) => map.get(segment),
            Value::Array(items) => segment
                .parse::<usize>()
                .ok()
                .and_then(|index| items.get(index)),
            _ => None,
        });
        if let Some(id) = current
            .and_then(|value| value.get("criterion_id"))
            .and_then(Value::as_str)
            .and_then(|text| Uuid::parse_str(text).ok())
        {
            criterion = Some(id);
        }
    }
    match criterion {
        Some(id) => format!("the judgment for criterion {id} does not match the output schema"),
        None => "the response does not match the output schema".to_owned(),
    }
}

fn record_completion(run: &mut AiRunRecord, completion: &GatewayCompletion) {
    run.usage.input_tokens = run
        .usage
        .input_tokens
        .saturating_add(completion.input_tokens);
    run.usage.output_tokens = run
        .usage
        .output_tokens
        .saturating_add(completion.output_tokens);
    if let Some(cost) = completion.cost_micros {
        run.cost_micros = Some(run.cost_micros.unwrap_or(0).saturating_add(cost));
    }
    // The run keeps the provider's report for its most recent call, which is the call whose
    // output it accepted. A repaired run therefore shows the repair's report.
    run.provider_served_model = completion.served_model.clone();
    run.provider_system_fingerprint = completion.system_fingerprint.clone();
}

/// The safe reason shown to the model when its answer is rejected. Provider
/// and budget failures are never repaired.
fn repair_feedback(error: &AiError) -> Option<String> {
    match error {
        AiError::MalformedOutput(_) => Some("the response was not one JSON object".to_owned()),
        AiError::SchemaValidation(reason) | AiError::SemanticValidation(reason) => {
            Some(if reason.is_empty() {
                "the response failed validation".to_owned()
            } else {
                reason.clone()
            })
        }
        _ => None,
    }
}

fn repair_message(feedback: &str) -> String {
    format!(
        "Validation rejected your previous answer: {feedback}. Return one corrected JSON object \
         that satisfies the output contract exactly. Do not repeat the rejected answer."
    )
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
        AiError::BudgetExceeded => "budget_exceeded",
        AiError::SubscriptionLimit => "subscription_limit",
    };
    let message = match code {
        "gateway" => "provider request failed",
        "budget_exceeded" => "monthly AI budget reached",
        "subscription_limit" => "AI subscription limit reached; try again later",
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
