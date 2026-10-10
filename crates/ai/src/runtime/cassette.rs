//! rig-cassette recording and replay for DeepRef assistant turns.
//!
//! One effect recorder per assistant run
//! ([`EffectLogRecorder::keeping_stream_events`]) folds every served dispatch
//! — model calls and tool executions — into an [`EffectLog`]. The turn driver
//! stamps it with the agent's program identity and associates run metadata,
//! producing an [`AssistantCassette`]:
//!
//! ```text
//! assistant run id, conversation id, project id
//! DeepRef version, Rig versions, prompt version, resolved model
//! semantic contract id, build provenance
//! run-spec hash, stamped effect log
//! ```
//!
//! Cassette provides effect recording, replay, offline agent tests and
//! debugging. It does NOT provide durable job scheduling, database
//! persistence, transactional domain writes or exactly-once arbitrary side
//! effects — those stay with the PostgreSQL job queue and the plan
//! confirmation boundary.
//!
//! ## Replay model
//!
//! A recorded run replays **without a live provider**: model dispatches are
//! answered by payload-checking replayers registered for the log's
//! completion keys, while tool dispatches execute live against a (test or
//! production) [`AssistantToolHost`]. Reads re-execute safely, proposals
//! recreate `PlanAction` data without applying mutations, and any divergence
//! — a tampered log, a changed tool catalog, a different hook stack — fails
//! closed through [`AgentReplayExt::check_replayable`] or a replayer payload
//! refusal.
//!
//! Live tool execution during replay is deliberate: host-side plan collection
//! and read determinism are part of the asserted semantics, and replaying
//! tool *outcomes* without bodies would silently skip the policy and
//! validation paths the fixtures must cover.

use std::sync::Arc;

use rig_agent::{
    agent::AgentBuilder,
    bus::{Bus, BusDriver, Dispatcher, Registrar},
};
use rig_cassette::agent::AgentReplayExt;
use rig_cassette::effect_log::{EffectLog, EffectLogRecorder, EffectLogReplayer, RequestCheck};
use rig_core::{effect::EffectFamily, serve::ErasedHandler};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use crate::{
    AgentLoopConfig, AiError, ChatMessage, PriceBook, UsageLedger, assistant_system_prompt,
    runtime::{
        agent::RigTurnOutcome,
        agent::run_assistant_runner,
        context::{AssistantToolHost, DeepRefAgentContext},
        hooks::DeepRefHooks,
        tools::{PlanCollector, ToolHostScope, deepref_dynamic_tools},
    },
};

/// One recorded assistant turn: run association plus the stamped effect log.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AssistantCassette {
    pub run_id: Uuid,
    pub conversation_id: Uuid,
    pub project_id: deepref_domain::ProjectId,
    pub deepref_version: String,
    pub rig_version: String,
    pub prompt_version: String,
    pub route: crate::ResolvedModel,
    /// Stable hash of the agent's protocol-facing run specification.
    pub run_spec_hash: u64,
    pub semantic_contract_id: Option<String>,
    pub build_provenance: Option<Value>,
    pub effect_log: EffectLog,
}

impl AssistantCassette {
    /// Stamps a recorder's log with the agent's program identity and
    /// associates run metadata. Called by the turn driver on success only:
    /// failed runs carry no cassette.
    pub(crate) fn assemble(
        agent: &rig_agent::agent::Agent,
        recorder: &EffectLogRecorder,
        context: &DeepRefAgentContext,
        run_id: Uuid,
        semantic_contract_id: Option<String>,
        build_provenance: Option<Value>,
    ) -> Self {
        let stamped = agent.stamp(recorder.log());
        Self {
            run_id,
            conversation_id: context.conversation_id,
            project_id: context.project_id,
            deepref_version: env!("CARGO_PKG_VERSION").to_owned(),
            rig_version: env!("DEEPREF_RIG_VERSIONS").to_owned(),
            prompt_version: crate::ASSISTANT_PROMPT_VERSION.to_owned(),
            route: context.route.clone(),
            run_spec_hash: agent.run_spec_hash(),
            semantic_contract_id,
            build_provenance,
            effect_log: stamped,
        }
    }

    /// Refuses logs whose identity or required handlers differ from `agent`.
    /// Fails closed: any mismatch is an error, never a best-effort replay.
    pub fn check_compatible(&self, agent: &rig_agent::agent::Agent) -> Result<(), AiError> {
        agent
            .check_replayable(&self.effect_log)
            .map_err(|report| AiError::Cassette(report.to_string()))
    }
}

/// Inputs for replaying a recorded turn. Mirrors the live
/// [`RigTurn`][crate::runtime::RigTurn] minus model factory and recorder: no
/// provider is touched, and nothing is recorded.
pub struct ReplayTurn {
    pub context: DeepRefAgentContext,
    pub history: Vec<ChatMessage>,
    pub user_message: String,
    pub host: Arc<dyn AssistantToolHost>,
    pub ledger: Arc<dyn UsageLedger>,
    pub prices: PriceBook,
    pub config: AgentLoopConfig,
}

/// Replays a recorded turn without a live provider.
///
/// Model dispatches are answered by payload-checking replayers built from
/// the cassette; tool dispatches execute live through `host`, so plan
/// collection, validation and policy behave exactly like the recording when
/// the host is deterministic. Returns the replayed outcome for semantic
/// comparison with the recording.
pub async fn replay_rig_turn(
    replay: ReplayTurn,
    cassette: &AssistantCassette,
) -> Result<RigTurnOutcome, AiError> {
    use rig_core::completion::Message;
    use rig_core::tool::ToolContext;

    let plans = PlanCollector::new();
    let scope = ToolHostScope::new(replay.context.clone(), replay.host.clone(), plans);
    let hooks = DeepRefHooks::new(
        scope.clone(),
        replay.ledger.clone(),
        replay.prices,
        replay.config,
    );
    // The replay agent must be configured exactly like the recording agent;
    // check_replayable below fails closed on any drift.
    let model_key = completion_key(cassette)?;
    let (dispatcher, registrar, mut bus_driver) = Bus::channel();
    register_model_replayers(cassette, &mut bus_driver)?;
    let agent = build_replay_agent(
        dispatcher,
        registrar,
        model_key,
        &replay.context,
        &scope,
        hooks.clone(),
        replay.config,
    );
    cassette.check_compatible(&agent)?;
    let bus_handle = tokio::spawn(bus_driver);
    let history: Vec<rig_core::completion::Message> = replay
        .history
        .iter()
        .filter_map(|message| match message {
            ChatMessage::User(text) => Some(Message::user(text.clone())),
            ChatMessage::Assistant { content, .. } => Some(Message::assistant(content.clone())),
            ChatMessage::System(_) | ChatMessage::Tool { .. } => None,
        })
        .collect();
    let tool_scope: Arc<dyn std::any::Any + Send + Sync> = Arc::new(scope.clone());
    let runner = agent
        .prompt(Message::user(replay.user_message.clone()))
        .history(history)
        .max_turns(replay.config.max_steps)
        .tool_context(ToolContext::new().with_scope(tool_scope))
        .tool_concurrency(1)
        .max_invalid_tool_call_retries(replay.config.max_steps);
    let (progress_tx, progress_rx) = tokio::sync::mpsc::unbounded_channel();
    drop(progress_rx);
    let outcome = run_assistant_runner(
        scope,
        hooks,
        replay.user_message.clone(),
        runner,
        progress_tx,
    )
    .await;
    bus_handle.abort();
    outcome
}

/// The log's single completion key: the model dispatch address the replay
/// registers a replayer for. Anything else is a corrupt or multi-model log.
fn completion_key(cassette: &AssistantCassette) -> Result<rig_core::effect::HandlerKey, AiError> {
    let mut keys = cassette
        .effect_log
        .header
        .signature
        .iter()
        .filter(|(_, family)| **family == EffectFamily::Completion)
        .map(|(key, _)| key.clone());
    match (keys.next(), keys.next()) {
        (Some(key), None) => Ok(key),
        _ => Err(AiError::Cassette(
            "cassette has no single model key".to_owned(),
        )),
    }
}

/// Payload-checking replayers for the log's completion keys. Tool keys are
/// deliberately NOT registered: tools execute live during replay.
fn register_model_replayers(
    cassette: &AssistantCassette,
    bus_driver: &mut BusDriver,
) -> Result<(), AiError> {
    let replayers = EffectLogReplayer::for_log(&cassette.effect_log)
        .map_err(|report| AiError::Cassette(report.to_string()))?;
    for replayer in replayers {
        let key = replayer.key().clone();
        if cassette.effect_log.header.signature.get(&key) == Some(&EffectFamily::Completion) {
            bus_driver
                .register_erased(
                    key,
                    ErasedHandler::new(replayer.checking(RequestCheck::Payload)),
                )
                .map_err(|report| AiError::Cassette(report.to_string()))?;
        }
    }
    Ok(())
}

/// Builds the replay agent over the host bus. Configuration must mirror the
/// live agent exactly; [`AgentReplayExt::check_replayable`] fails closed on
/// drift.
fn build_replay_agent(
    dispatcher: Dispatcher,
    registrar: Registrar,
    model_key: rig_core::effect::HandlerKey,
    context: &DeepRefAgentContext,
    scope: &ToolHostScope,
    hooks: DeepRefHooks,
    config: AgentLoopConfig,
) -> rig_agent::agent::Agent {
    use serde_json::Value;
    let mut builder = AgentBuilder::over_bus(
        dispatcher,
        registrar,
        crate::runtime::agent::DEEPREF_ASSISTANT_OWNER,
        model_key,
    )
    .preamble(assistant_system_prompt(context.project_id))
    .max_tokens(u64::from(config.max_output_tokens_per_call))
    .dynamic_tools(deepref_dynamic_tools(scope))
    .add_hook(hooks);
    if let Some(temperature) = context.route.parameters.temperature {
        builder = builder.temperature(f64::from(temperature));
    }
    if let Some(top_p) = context.route.parameters.top_p {
        builder = builder.top_p(f64::from(top_p));
    }
    if !context.route.parameters.additional.is_empty() {
        builder = builder.additional_params(Value::Object(
            context
                .route
                .parameters
                .additional
                .clone()
                .into_iter()
                .collect(),
        ));
    }
    builder.build()
}
