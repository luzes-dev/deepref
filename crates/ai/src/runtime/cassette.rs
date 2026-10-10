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
    AgentLoopConfig, AiError, ChatMessage, PriceBook, UsageLedger,
    runtime::{
        agent::RigTurnOutcome,
        agent::run_assistant_runner,
        builder::{assistant_runner, configure_assistant_builder, history_to_messages},
        context::{AssistantToolHost, DeepRefAgentContext},
        hooks::DeepRefHooks,
        tools::{PlanCollector, ToolHostScope},
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
    let plans = PlanCollector::new();
    let scope = ToolHostScope::new(replay.context.clone(), replay.host.clone(), plans);
    let hooks = DeepRefHooks::new(
        scope.clone(),
        replay.ledger.clone(),
        replay.prices,
        replay.config,
    );
    // The replay agent must be configured exactly like the recording agent:
    // both go through the shared builder, and check_replayable below fails
    // closed on any drift.
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
    let history = history_to_messages(&replay.history);
    let runner = assistant_runner(
        &agent,
        replay.user_message.clone(),
        history,
        &scope,
        replay.config,
    );
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

/// Builds the replay agent over the host bus through the shared builder, so
/// its configuration cannot drift from the live agent's.
/// [`AgentReplayExt::check_replayable`] fails closed on any residual drift.
fn build_replay_agent(
    dispatcher: Dispatcher,
    registrar: Registrar,
    model_key: rig_core::effect::HandlerKey,
    context: &DeepRefAgentContext,
    scope: &ToolHostScope,
    hooks: DeepRefHooks,
    config: AgentLoopConfig,
) -> rig_agent::agent::Agent {
    let builder = AgentBuilder::over_bus(
        dispatcher,
        registrar,
        crate::runtime::agent::DEEPREF_ASSISTANT_OWNER,
        model_key,
    );
    configure_assistant_builder(builder, context, scope, hooks, config).build()
}
