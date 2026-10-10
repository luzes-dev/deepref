//! Shared fixtures for the runtime parity tests: the fake host and ledger,
//! scripted-model helpers and the live-turn runner.
//!
//! Both the live-path tests ([`super::tests_live`]) and the record/replay
//! tests ([`super::tests_cassette`]) script turns through these helpers, so
//! the two suites assert the same semantics from one script vocabulary.

use std::{
    collections::HashMap,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
};

use rig_core::test_utils::{MockCompletionModel, MockStreamEvent};
use serde_json::Value;
use uuid::Uuid;

use super::*;
use crate::{
    Actor, ActorKind, AgentLoopConfig, AgentTool, AgentToolName, AiError, AiFuture, BudgetSnapshot,
    ChatMessage, ModelParameters, ModelProfile, PriceBook, ResolvedModel, UsageEntry, UsageLedger,
    runtime::{AgentProgress, context::DeepRefAgentContext, model::StaticModelFactory},
};
use deepref_domain::ProjectId;

#[derive(Clone, Default)]
pub(super) struct FakeHost {
    pub(super) calls: Arc<Mutex<Vec<(ProjectId, String, Value)>>>,
    pub(super) canned: Arc<Mutex<HashMap<String, Value>>>,
    pub(super) forced_error: Arc<Mutex<Option<AiError>>>,
}

impl FakeHost {
    pub(super) fn with_read(self, tool: &str, output: Value) -> Self {
        self.canned
            .lock()
            .expect("canned lock")
            .insert(tool.to_owned(), output);
        self
    }

    pub(super) fn received_project(&self) -> Option<ProjectId> {
        self.calls
            .lock()
            .expect("calls lock")
            .first()
            .map(|call| call.0)
    }
}

impl AssistantToolHost for FakeHost {
    fn execute_read<'a>(
        &'a self,
        project_id: ProjectId,
        tool: &'a str,
        args: Value,
    ) -> AiFuture<'a, Value> {
        Box::pin(async move {
            self.calls.lock().expect("calls lock").push((
                project_id,
                tool.to_owned(),
                args.clone(),
            ));
            if let Some(error) = self.forced_error.lock().expect("error lock").clone() {
                return Err(error);
            }
            if let Some(name) = AgentToolName::parse(tool) {
                let parsed = AgentTool::from_name_and_args(name, args).map_err(|_| {
                    AiError::InvalidContext("the arguments are malformed".to_owned())
                })?;
                parsed
                    .validate()
                    .map_err(|_| AiError::InvalidContext("the arguments are invalid".to_owned()))?;
            }
            self.canned
                .lock()
                .expect("canned lock")
                .get(tool)
                .cloned()
                .ok_or_else(|| AiError::InvalidContext("unknown tool".to_owned()))
        })
    }
}

#[derive(Clone, Default)]
pub(super) struct FakeLedger {
    pub(super) entries: Arc<Mutex<Vec<UsageEntry>>>,
    pub(super) exhausted: Arc<AtomicBool>,
}

impl UsageLedger for FakeLedger {
    fn budget<'a>(&'a self, _project_id: ProjectId) -> AiFuture<'a, BudgetSnapshot> {
        let exhausted = self.exhausted.load(Ordering::SeqCst);
        Box::pin(async move {
            if exhausted {
                Ok(BudgetSnapshot {
                    budget_micros: 0,
                    spent_micros: 0,
                })
            } else {
                Ok(BudgetSnapshot {
                    budget_micros: 1_000_000,
                    spent_micros: 0,
                })
            }
        })
    }

    fn record<'a>(&'a self, entry: UsageEntry) -> AiFuture<'a, ()> {
        Box::pin(async move {
            self.entries.lock().expect("entries lock").push(entry);
            Ok(())
        })
    }
}

pub(super) fn test_context() -> DeepRefAgentContext {
    DeepRefAgentContext {
        project_id: ProjectId::new(Uuid::from_u128(7)),
        conversation_id: Uuid::from_u128(8),
        actor: Actor::new(ActorKind::User, "tester").expect("actor"),
        route: ResolvedModel {
            profile: ModelProfile::Reasoning,
            provider: "test-provider".to_owned(),
            model: "test-model".to_owned(),
            model_version: "v1".to_owned(),
            parameters: ModelParameters::default(),
            route_id: None,
        },
    }
}

pub(super) fn test_config() -> AgentLoopConfig {
    AgentLoopConfig::default()
}

#[allow(clippy::too_many_arguments)]
pub(super) async fn run_script(
    script: Vec<Vec<MockStreamEvent>>,
    host: FakeHost,
    ledger: FakeLedger,
    config: AgentLoopConfig,
    history: Vec<ChatMessage>,
    user_message: &str,
    progress: Option<&mut (dyn AgentProgress + Send)>,
) -> (Result<RigTurnOutcome, AiError>, MockCompletionModel) {
    let context = test_context();
    let model = MockCompletionModel::from_stream_turns(script);
    let factory = Arc::new(StaticModelFactory::new(model.clone().erase()));
    let outcome = run_rig_turn(
        RigTurn {
            factory: factory.clone(),
            run_id: Uuid::new_v4(),
            semantic_contract_id: None,
            build_provenance: None,
            context,
            history,
            user_message: user_message.to_owned(),
            host: Arc::new(host),
            ledger: Arc::new(ledger),
            prices: PriceBook::default(),
            config,
            recorder: None,
            progress,
        },
        |_| {},
    )
    .await;
    (outcome, model)
}

pub(super) async fn run_simple(
    script: Vec<Vec<MockStreamEvent>>,
    host: FakeHost,
) -> Result<RigTurnOutcome, AiError> {
    run_script(
        script,
        host,
        FakeLedger::default(),
        test_config(),
        Vec::new(),
        "do it",
        None,
    )
    .await
    .0
}

pub(super) fn text_turn(text: &str) -> Vec<MockStreamEvent> {
    vec![
        MockStreamEvent::text(text.to_owned()),
        MockStreamEvent::final_response_with_total_tokens(4),
    ]
}

pub(super) fn call_turn(id: &str, name: &str, args: Value) -> Vec<MockStreamEvent> {
    vec![
        MockStreamEvent::tool_call(id.to_owned(), name.to_owned(), args)
            .with_call_id(id.to_owned()),
        MockStreamEvent::final_response_with_total_tokens(4),
    ]
}

pub(super) fn error_turn(message: &str) -> Vec<MockStreamEvent> {
    vec![MockStreamEvent::error(message.to_owned())]
}

pub(super) fn stream_call(id: &str, name: &str, args: Value) -> Vec<MockStreamEvent> {
    vec![
        MockStreamEvent::tool_call(id.to_owned(), name.to_owned(), args)
            .with_call_id(id.to_owned()),
        MockStreamEvent::final_response_with_total_tokens(4),
    ]
}
