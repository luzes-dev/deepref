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

use chrono::Datelike;
use rig_core::test_utils::{MockCompletionModel, MockStreamEvent};
use serde_json::Value;
use uuid::Uuid;

use super::*;
use crate::{
    Actor, ActorKind, AgentLoopConfig, AgentTool, AgentToolName, AiError, AiFuture, ChatMessage,
    ModelParameters, ModelProfile, PriceBook, ResolvedModel, UsageLedger, UsageObservation,
    UsageReservation, UsageReservationRequest, UsageTopUpRequest, ZeroLiabilityProof,
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
    pub(super) entries: Arc<Mutex<Vec<(UsageReservationRequest, UsageObservation)>>>,
    pub(super) pending: Arc<Mutex<HashMap<Uuid, UsageReservationRequest>>>,
    pub(super) exhausted: Arc<AtomicBool>,
}

impl UsageLedger for FakeLedger {
    fn reserve<'a>(&'a self, request: UsageReservationRequest) -> AiFuture<'a, UsageReservation> {
        let exhausted = self.exhausted.load(Ordering::SeqCst);
        Box::pin(async move {
            if exhausted {
                return Err(AiError::BudgetExceeded);
            }
            let reserved_cost_micros = request.reserved_cost_micros()?;
            let reservation = UsageReservation {
                id: Uuid::new_v4(),
                workspace_id: request
                    .attribution
                    .workspace_id
                    .unwrap_or_else(|| deepref_domain::WorkspaceId::new(Uuid::new_v4())),
                period_start: chrono::Utc::now()
                    .date_naive()
                    .with_day(1)
                    .unwrap_or(chrono::NaiveDate::MIN),
                reserved_cost_micros,
                price: request.price.clone(),
            };
            self.pending
                .lock()
                .expect("pending lock")
                .insert(reservation.id, request);
            Ok(reservation)
        })
    }

    fn top_up<'a>(
        &'a self,
        reservation: &'a UsageReservation,
        request: UsageTopUpRequest,
    ) -> AiFuture<'a, UsageReservation> {
        Box::pin(async move {
            let additional = request.reserved_cost_micros(&reservation.price)?;
            Ok(UsageReservation {
                id: reservation.id,
                workspace_id: reservation.workspace_id,
                period_start: reservation.period_start,
                reserved_cost_micros: reservation
                    .reserved_cost_micros
                    .checked_add(additional)
                    .ok_or(AiError::InvalidUsageBound)?,
                price: reservation.price.clone(),
            })
        })
    }

    fn observe<'a>(
        &'a self,
        _reservation: &'a UsageReservation,
        _observation: UsageObservation,
    ) -> AiFuture<'a, ()> {
        Box::pin(async { Ok(()) })
    }

    fn settle<'a>(
        &'a self,
        reservation: &'a UsageReservation,
        observation: UsageObservation,
        _current_price_version: &'a str,
    ) -> AiFuture<'a, ()> {
        Box::pin(async move {
            let request = self
                .pending
                .lock()
                .expect("pending lock")
                .remove(&reservation.id)
                .ok_or_else(|| AiError::Persistence("missing fake reservation".to_owned()))?;
            self.entries
                .lock()
                .expect("entries lock")
                .push((request, observation));
            Ok(())
        })
    }

    fn unresolved<'a>(
        &'a self,
        _reservation: &'a UsageReservation,
        _observation: Option<UsageObservation>,
        _reason: &'a str,
    ) -> AiFuture<'a, ()> {
        Box::pin(async { Ok(()) })
    }

    fn void_zero_liability<'a>(
        &'a self,
        _reservation: &'a UsageReservation,
        _proof: ZeroLiabilityProof,
    ) -> AiFuture<'a, ()> {
        Box::pin(async { Ok(()) })
    }
}

pub(super) fn test_context() -> DeepRefAgentContext {
    DeepRefAgentContext {
        project_id: ProjectId::new(Uuid::from_u128(7)),
        conversation_id: Uuid::from_u128(8),
        actor: Actor::new(ActorKind::User, "tester").expect("actor"),
        route: ResolvedModel {
            profile: ModelProfile::Reasoning,
            provider: "opencode-go".to_owned(),
            model: "glm-5.3-flash".to_owned(),
            model_version: "glm-5.3-flash".to_owned(),
            parameters: ModelParameters {
                max_tokens: Some(2_048),
                ..ModelParameters::default()
            },
            route_id: None,
        },
        job_id: None,
        attempt_fence: None,
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
            admission: None,
            admission_limits: Default::default(),
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

/// Deterministic lease seam for stream cancellation and renewal failures.
#[derive(Clone, Default)]
pub(super) struct TestAdmission {
    pub(super) released: Arc<std::sync::atomic::AtomicUsize>,
    pub(super) fail_renewal: bool,
    pub(super) hang_renewal: bool,
    pub(super) interval: Option<std::time::Duration>,
}

impl crate::AiConcurrencyAdmission for TestAdmission {
    fn acquire<'a>(
        &'a self,
        scope: crate::AiConcurrencyScope,
    ) -> crate::AdmissionFuture<'a, crate::SlotLease> {
        Box::pin(async move {
            let keys = scope.keys();
            let grant = |index: usize| crate::SlotGrant {
                scope_kind: keys[index].0,
                scope_key: keys[index].1.clone(),
                slot_index: 0,
                holder_id: Uuid::nil(),
                generation: 1,
                lease_expires_at: chrono::Utc::now() + chrono::Duration::seconds(60),
            };
            Ok(crate::SlotLease {
                workspace: grant(0),
                route: grant(1),
                scope,
            })
        })
    }

    fn renew<'a>(&'a self, _lease: &'a crate::SlotLease) -> crate::AdmissionFuture<'a, ()> {
        Box::pin(async move {
            if self.hang_renewal {
                return std::future::pending().await;
            }
            if self.fail_renewal {
                return Err(crate::AdmissionError::Persistence(
                    "database unavailable".to_owned(),
                ));
            }
            Ok(())
        })
    }

    fn release<'a>(&'a self, _lease: crate::SlotLease) -> crate::AdmissionFuture<'a, ()> {
        self.released.fetch_add(1, Ordering::SeqCst);
        Box::pin(async { Ok(()) })
    }

    fn renew_interval(&self) -> std::time::Duration {
        self.interval
            .unwrap_or(std::time::Duration::from_millis(20))
    }
}
