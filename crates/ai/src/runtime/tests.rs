//! Parity tests: the Rig runtime must enforce the same domain rules as the
//! custom loop (same policy, same project isolation, same plan semantics).

use std::{
    collections::HashMap,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
};

use rig_core::completion::Usage;
use rig_core::test_utils::{MockCompletionModel, MockStreamEvent};
use serde_json::{Value, json};
use uuid::Uuid;

use super::*;
use crate::{
    Actor, ActorKind, AgentLoopConfig, AgentTool, AgentToolName, AiError, AiFuture, BudgetSnapshot,
    ChatMessage, ChatToolCall, ModelParameters, ModelProfile, PriceBook, ResolvedModel, UsageEntry,
    UsageLedger, agent_tool_declarations,
    runtime::{AgentProgress, context::DeepRefAgentContext, model::StaticModelFactory},
};
use deepref_domain::ProjectId;

#[derive(Clone, Default)]
struct FakeHost {
    calls: Arc<Mutex<Vec<(ProjectId, String, Value)>>>,
    canned: Arc<Mutex<HashMap<String, Value>>>,
    forced_error: Arc<Mutex<Option<AiError>>>,
}

impl FakeHost {
    fn with_read(self, tool: &str, output: Value) -> Self {
        self.canned
            .lock()
            .expect("canned lock")
            .insert(tool.to_owned(), output);
        self
    }

    fn received_project(&self) -> Option<ProjectId> {
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
struct FakeLedger {
    entries: Arc<Mutex<Vec<UsageEntry>>>,
    exhausted: Arc<AtomicBool>,
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

fn test_context() -> DeepRefAgentContext {
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

fn test_config() -> AgentLoopConfig {
    AgentLoopConfig::default()
}

#[allow(clippy::too_many_arguments)]
async fn run_script(
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

async fn run_simple(
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

fn text_turn(text: &str) -> Vec<MockStreamEvent> {
    vec![
        MockStreamEvent::text(text.to_owned()),
        MockStreamEvent::final_response_with_total_tokens(4),
    ]
}

fn call_turn(id: &str, name: &str, args: Value) -> Vec<MockStreamEvent> {
    vec![
        MockStreamEvent::tool_call(id.to_owned(), name.to_owned(), args)
            .with_call_id(id.to_owned()),
        MockStreamEvent::final_response_with_total_tokens(4),
    ]
}

fn error_turn(message: &str) -> Vec<MockStreamEvent> {
    vec![MockStreamEvent::error(message.to_owned())]
}

#[tokio::test]
async fn plain_answer_needs_no_tools() {
    let outcome = run_simple(
        vec![text_turn("There are three reports.")],
        FakeHost::default(),
    )
    .await
    .expect("plain answer runs");
    assert_eq!(outcome.reply, "There are three reports.");
    assert!(outcome.actions.is_empty());
    assert!(outcome.trace.is_empty());
    assert!(!outcome.truncated);
    assert_eq!(outcome.steps, 1);
}

#[tokio::test]
async fn single_read_runs_and_lands_in_the_trace() {
    let host = FakeHost::default().with_read("get_project_overview", json!({"reports": 3}));
    let outcome = run_simple(
        vec![
            call_turn("c1", "get_project_overview", json!({})),
            text_turn("Three reports match."),
        ],
        host,
    )
    .await
    .expect("read turn runs");
    assert_eq!(outcome.reply, "Three reports match.");
    assert!(outcome.actions.is_empty());
    assert_eq!(outcome.trace.len(), 1);
    assert_eq!(outcome.trace[0].call.name, "get_project_overview");
    assert_eq!(outcome.trace[0].call.id, "c1");
    assert_eq!(outcome.trace[0].output, json!({"reports": 3}));
}

#[tokio::test]
async fn multiple_reads_in_one_turn_all_execute() {
    let host = FakeHost::default()
        .with_read("get_project_overview", json!({"reports": 3}))
        .with_read("get_report", json!({"title": "T"}));
    let report = Uuid::from_u128(1);
    let outcome = run_simple(
        vec![
            [
                MockStreamEvent::tool_call(
                    "c1".to_owned(),
                    "get_project_overview".to_owned(),
                    json!({}),
                )
                .with_call_id("c1".to_owned()),
                MockStreamEvent::tool_call(
                    "c2".to_owned(),
                    "get_report".to_owned(),
                    json!({"report_id": report}),
                )
                .with_call_id("c2".to_owned()),
                MockStreamEvent::final_response_with_total_tokens(4),
            ]
            .into_iter()
            .collect::<Vec<_>>(),
            text_turn("done"),
        ],
        host,
    )
    .await
    .expect("multi-call turn runs");
    assert_eq!(outcome.trace.len(), 2);
    assert_eq!(outcome.trace[0].call.id, "c1");
    assert_eq!(outcome.trace[1].call.id, "c2");
}

#[tokio::test]
async fn unknown_tool_gets_feedback_and_the_turn_continues() {
    let outcome = run_simple(
        vec![
            call_turn("c1", "drop_database", json!({})),
            text_turn("There is no such tool."),
        ],
        FakeHost::default(),
    )
    .await
    .expect("unknown tool recovers");
    assert_eq!(outcome.reply, "There is no such tool.");
    assert!(outcome.actions.is_empty());
    assert!(outcome.trace.is_empty());
}

#[tokio::test]
async fn invalid_tool_arguments_are_rejected_back_to_the_model() {
    let outcome = run_simple(
        vec![
            call_turn(
                "c1",
                "screen_reports",
                json!({"report_ids": ["nope"], "decision": "exclude"}),
            ),
            text_turn("Sorry."),
        ],
        FakeHost::default(),
    )
    .await
    .expect("invalid args recover");
    assert!(outcome.actions.is_empty());
    assert_eq!(outcome.trace.len(), 1);
    assert!(outcome.trace[0].output.get("error").is_some());
}

#[tokio::test]
async fn proposal_tool_creates_a_plan_action() {
    let ids = json!([Uuid::from_u128(1), Uuid::from_u128(2)]);
    let outcome = run_simple(
        vec![
            call_turn(
                "c1",
                "screen_reports",
                json!({
                    "report_ids": ids,
                    "decision": "exclude",
                    "summary": "Exclude 2 animal studies",
                    "rationale": "Protocol requires humans",
                }),
            ),
            text_turn("I prepared a plan."),
        ],
        FakeHost::default(),
    )
    .await
    .expect("proposal turn runs");
    assert_eq!(outcome.reply, "I prepared a plan.");
    assert_eq!(outcome.actions.len(), 1);
    assert!(outcome.actions[0].executable);
    assert_eq!(outcome.actions[0].tool, "screen_reports");
    assert_eq!(outcome.actions[0].affected_count, 2);
    assert!(outcome.actions[0].manual.is_none());
}

#[tokio::test]
async fn manual_only_tool_becomes_a_manual_step() {
    let outcome = run_simple(
        vec![
            call_turn(
                "c1",
                "request_protocol_publish",
                json!({"summary": "Publish it", "rationale": "done"}),
            ),
            text_turn("You must do that yourself."),
        ],
        FakeHost::default(),
    )
    .await
    .expect("manual turn runs");
    assert_eq!(outcome.actions.len(), 1);
    assert!(!outcome.actions[0].executable);
    assert_eq!(
        outcome.actions[0]
            .manual
            .as_ref()
            .map(|manual| manual.link_target.as_str()),
        Some("protocol")
    );
}

#[tokio::test]
async fn project_scope_is_host_enforced_not_model_supplied() {
    let other = Uuid::from_u128(999);
    let host = FakeHost::default().with_read("get_report", json!({"title": "T"}));
    let report = Uuid::from_u128(1);
    let outcome = run_simple(
        vec![
            call_turn(
                "c1",
                "get_report",
                json!({"project_id": other, "report_id": report}),
            ),
            text_turn("Got it."),
        ],
        host.clone(),
    )
    .await
    .expect("scoped read runs");
    assert_eq!(outcome.trace.len(), 1);
    assert_eq!(
        host.received_project(),
        Some(ProjectId::new(Uuid::from_u128(7))),
        "the host scope wins over model-supplied project ids"
    );
}

#[tokio::test]
async fn exhausted_budget_stops_before_any_model_call() {
    let ledger = FakeLedger::default();
    ledger.exhausted.store(true, Ordering::SeqCst);
    let (outcome, model) = run_script(
        vec![text_turn("unused")],
        FakeHost::default(),
        ledger,
        test_config(),
        Vec::new(),
        "do it",
        None,
    )
    .await;
    assert!(matches!(outcome, Err(AiError::BudgetExceeded)));
    assert!(
        model.requests().is_empty(),
        "no provider call may happen once the budget is exhausted"
    );
}

#[tokio::test]
async fn budget_exhaustion_mid_turn_aborts_the_turn() {
    let host = FakeHost::default();
    host.forced_error
        .lock()
        .expect("error lock")
        .replace(AiError::BudgetExceeded);
    let outcome = run_simple(
        vec![call_turn("c1", "get_project_overview", json!({}))],
        host,
    )
    .await;
    assert!(matches!(outcome, Err(AiError::BudgetExceeded)));
}

#[tokio::test]
async fn usage_is_recorded_per_turn_with_deepref_pricing() {
    let ledger = FakeLedger::default();
    let turn = vec![
        MockStreamEvent::text("Done.".to_owned()),
        MockStreamEvent::final_response(Usage::new().input_tokens(100).output_tokens(10)),
    ];
    let (outcome, _) = run_script(
        vec![turn],
        FakeHost::default(),
        ledger.clone(),
        test_config(),
        Vec::new(),
        "do it",
        None,
    )
    .await;
    let outcome = outcome.expect("usage turn runs");
    assert_eq!(outcome.input_tokens, 100);
    assert_eq!(outcome.output_tokens, 10);
    let entries = ledger.entries.lock().expect("entries lock").clone();
    assert_eq!(entries.len(), 1);
    let entry = &entries[0];
    assert_eq!(entry.purpose, "chat");
    assert_eq!(entry.provider, "test-provider");
    assert_eq!(entry.model, "test-model");
    assert_eq!(entry.input_tokens, 100);
    assert_eq!(entry.output_tokens, 10);
    assert_eq!(
        entry.cost_micros,
        PriceBook::default().estimate_cost_micros("test-provider", "test-model", 100, 10)
    );
}

#[tokio::test]
async fn step_cap_truncates_the_turn() {
    let host = FakeHost::default().with_read("get_project_overview", json!({"reports": 1}));
    let config = AgentLoopConfig {
        max_steps: 1,
        ..test_config()
    };
    let (outcome, _) = run_script(
        vec![
            call_turn("c1", "get_project_overview", json!({})),
            text_turn("done"),
        ],
        host,
        FakeLedger::default(),
        config,
        Vec::new(),
        "do it",
        None,
    )
    .await;
    let outcome = outcome.expect("truncated turn still settles");
    assert!(outcome.truncated);
    assert_eq!(
        outcome.reply,
        "I ran out of room to finish this in one go. Try a narrower question."
    );
}

#[tokio::test]
async fn token_cap_truncates_the_turn() {
    let host = FakeHost::default().with_read("get_project_overview", json!({"reports": 1}));
    let config = AgentLoopConfig {
        max_total_tokens: 5,
        ..test_config()
    };
    let big_call = vec![
        MockStreamEvent::tool_call(
            "c2".to_owned(),
            "get_project_overview".to_owned(),
            json!({}),
        )
        .with_call_id("c2".to_owned()),
        MockStreamEvent::final_response(Usage::new().input_tokens(100).output_tokens(10)),
    ];
    let (outcome, _) = run_script(
        vec![
            call_turn("c1", "get_project_overview", json!({})),
            big_call,
            text_turn("unreached"),
        ],
        host,
        FakeLedger::default(),
        config,
        Vec::new(),
        "do it",
        None,
    )
    .await;
    // Both calls fit the checks before them; the second spends past the cap,
    // so the third call is refused and the turn truncates with its work so far.
    let outcome = outcome.expect("token-capped turn still settles");
    assert!(outcome.truncated);
    assert_eq!(outcome.steps, 2);
}

#[tokio::test]
async fn false_queue_claim_gets_one_corrective_reprompt() {
    let (outcome, model) = run_script(
        vec![
            text_turn("I've queued a plan to propose the sample size. Confirm to apply it."),
            text_turn("I could not find a change to make, so nothing has been queued."),
        ],
        FakeHost::default(),
        FakeLedger::default(),
        test_config(),
        Vec::new(),
        "propose the sample size extraction",
        None,
    )
    .await;
    let outcome = outcome.expect("corrected turn runs");
    assert!(outcome.actions.is_empty());
    assert_eq!(
        outcome.reply,
        "I could not find a change to make, so nothing has been queued."
    );
    assert_eq!(
        model.requests().len(),
        2,
        "exactly one corrective re-prompt"
    );
}

#[tokio::test]
async fn claim_that_survives_the_reprompt_is_annotated() {
    let (outcome, _) = run_script(
        vec![
            text_turn("I've queued the exclusion; confirm to apply."),
            text_turn("The exclusion is queued in the plan."),
        ],
        FakeHost::default(),
        FakeLedger::default(),
        test_config(),
        Vec::new(),
        "exclude it",
        None,
    )
    .await;
    let outcome = outcome.expect("annotated turn runs");
    assert!(outcome.actions.is_empty());
    assert!(
        outcome
            .reply
            .starts_with("The exclusion is queued in the plan.")
    );
    assert!(outcome.reply.ends_with(
        "Nothing was queued: no change was added to a plan, so nothing is waiting for your confirmation."
    ));
}

#[tokio::test]
async fn history_reaches_the_model() {
    let history = vec![
        ChatMessage::User("how many are left?".to_owned()),
        ChatMessage::Assistant {
            content: "Fifty.".to_owned(),
            tool_calls: Vec::new(),
        },
    ];
    let (outcome, model) = run_script(
        vec![text_turn("Still fifty.")],
        FakeHost::default(),
        FakeLedger::default(),
        test_config(),
        history,
        "and now?",
        None,
    )
    .await;
    outcome.expect("history turn runs");
    let requests = model.requests();
    assert_eq!(requests.len(), 1);
    let rendered = format!("{:?}", requests[0].chat_history);
    assert!(rendered.contains("how many are left?"), "{rendered}");
    assert!(rendered.contains("Fifty."), "{rendered}");
    assert!(rendered.contains("and now?"), "{rendered}");
}

#[tokio::test]
async fn provider_failure_maps_to_a_gateway_error() {
    let outcome = run_simple(vec![error_turn("boom")], FakeHost::default()).await;
    assert!(matches!(outcome, Err(AiError::Gateway(_))));
}

#[tokio::test]
async fn tool_failure_is_reported_back_to_the_model() {
    let host = FakeHost::default();
    host.forced_error
        .lock()
        .expect("error lock")
        .replace(AiError::InvalidContext("nope".to_owned()));
    let outcome = run_simple(
        vec![
            call_turn("c1", "get_project_overview", json!({})),
            text_turn("It failed."),
        ],
        host,
    )
    .await
    .expect("failed tool continues");
    assert_eq!(outcome.reply, "It failed.");
    assert_eq!(outcome.trace.len(), 1);
    assert_eq!(outcome.trace[0].output, json!({"error": "nope"}));
}

#[derive(Default)]
struct ProgressLog {
    deltas: Vec<String>,
    discards: usize,
    started: Vec<String>,
}

struct ProgressRecorder {
    log: Arc<Mutex<ProgressLog>>,
}

impl AgentProgress for ProgressRecorder {
    fn text_delta(&mut self, delta: &str) {
        self.log
            .lock()
            .expect("log lock")
            .deltas
            .push(delta.to_owned());
    }
    fn text_discarded(&mut self) {
        self.log.lock().expect("log lock").discards += 1;
    }
    fn tool_started(&mut self, call: &ChatToolCall) {
        self.log
            .lock()
            .expect("log lock")
            .started
            .push(call.name.clone());
    }
}

#[tokio::test]
async fn progress_announces_each_tool_before_it_runs() {
    let log = Arc::new(Mutex::new(ProgressLog::default()));
    let mut progress = ProgressRecorder { log: log.clone() };
    let host = FakeHost::default().with_read("get_project_overview", json!({"reports": 2}));
    let (outcome, _) = run_script(
        vec![
            call_turn("c1", "get_project_overview", json!({})),
            text_turn("Two reports match."),
        ],
        host,
        FakeLedger::default(),
        test_config(),
        Vec::new(),
        "how many reports match?",
        Some(&mut progress),
    )
    .await;
    let outcome = outcome.expect("progress turn runs");
    assert_eq!(outcome.reply, "Two reports match.");
    let log = log.lock().expect("log lock");
    assert_eq!(log.started, vec!["get_project_overview"]);
    assert_eq!(log.deltas.join(""), "Two reports match.");
}

#[test]
fn tool_catalog_is_offered_without_project_scope_or_workflow_trigger() {
    let context = test_context();
    let scope = ToolHostScope::new(context, Arc::new(FakeHost::default()), PlanCollector::new());
    let tools = deepref_dynamic_tools(&scope);
    let names: Vec<String> = agent_tool_declarations()
        .iter()
        .map(|declaration| declaration.name.clone())
        .collect();
    assert_eq!(tools.len(), names.len());
    assert!(!names.iter().any(|name| name == "trigger_workflow"));
    for tool in &tools {
        let parameters =
            serde_json::to_string(&tool.definition().parameters).expect("parameters serialize");
        assert!(!parameters.contains("project_id"), "{parameters}");
    }
}

#[test]
fn model_factory_constructs_without_calling() {
    let factory = crate::runtime::OpenAiCompatModelFactory::new("https://proxy.example/v1", "key");
    let route = test_context().route;
    factory.model(&route).expect("model constructs");
    let empty = crate::runtime::OpenAiCompatModelFactory::new("", "");
    assert!(empty.model(&route).is_err());
}

fn stream_call(id: &str, name: &str, args: Value) -> Vec<MockStreamEvent> {
    vec![
        MockStreamEvent::tool_call(id.to_owned(), name.to_owned(), args)
            .with_call_id(id.to_owned()),
        MockStreamEvent::final_response_with_total_tokens(4),
    ]
}

/// Records a scripted turn with a cassette recorder, returning the outcome,
/// the cassette and the scripted model (for request assertions).
async fn record_script(
    script: Vec<Vec<MockStreamEvent>>,
    host: FakeHost,
    user_message: &str,
) -> (RigTurnOutcome, AssistantCassette, MockCompletionModel) {
    record_script_with_config(script, host, user_message, test_config()).await
}

async fn record_script_with_config(
    script: Vec<Vec<MockStreamEvent>>,
    host: FakeHost,
    user_message: &str,
    config: AgentLoopConfig,
) -> (RigTurnOutcome, AssistantCassette, MockCompletionModel) {
    let context = test_context();
    let model = MockCompletionModel::from_stream_turns(script);
    let factory = Arc::new(StaticModelFactory::new(model.clone().erase()));
    let recorder = rig_cassette::effect_log::EffectLogRecorder::keeping_stream_events();
    let outcome = run_rig_turn(
        RigTurn {
            factory: factory.clone(),
            run_id: Uuid::new_v4(),
            semantic_contract_id: Some("contract-id".to_owned()),
            build_provenance: None,
            context,
            history: Vec::new(),
            user_message: user_message.to_owned(),
            host: Arc::new(host),
            ledger: Arc::new(FakeLedger::default()),
            prices: PriceBook::default(),
            config,
            recorder: Some(recorder),
            progress: None,
        },
        |_| {},
    )
    .await
    .expect("recording run succeeds");
    let cassette = outcome
        .cassette
        .clone()
        .expect("a recorded turn carries a cassette");
    (outcome, cassette, model)
}

/// Replays a cassette with fresh host and ledger, returning the outcome
/// and the replay ledger for comparison.
async fn try_replay_with_config(
    cassette: &AssistantCassette,
    host: FakeHost,
    user_message: &str,
    config: AgentLoopConfig,
) -> (Result<RigTurnOutcome, AiError>, FakeLedger) {
    let context = test_context();
    let ledger = FakeLedger::default();
    let replayed = replay_rig_turn(
        ReplayTurn {
            context,
            history: Vec::new(),
            user_message: user_message.to_owned(),
            host: Arc::new(host),
            ledger: Arc::new(ledger.clone()),
            prices: PriceBook::default(),
            config,
        },
        cassette,
    )
    .await;
    (replayed, ledger)
}

async fn try_replay(
    cassette: &AssistantCassette,
    host: FakeHost,
    user_message: &str,
) -> (Result<RigTurnOutcome, AiError>, FakeLedger) {
    try_replay_with_config(cassette, host, user_message, test_config()).await
}

async fn replay_cassette(
    cassette: &AssistantCassette,
    host: FakeHost,
    user_message: &str,
) -> (RigTurnOutcome, FakeLedger) {
    let (replayed, ledger) = try_replay(cassette, host, user_message).await;
    (
        replayed.expect("replay succeeds without a provider"),
        ledger,
    )
}

async fn replay_cassette_with_config(
    cassette: &AssistantCassette,
    host: FakeHost,
    user_message: &str,
    config: AgentLoopConfig,
) -> (RigTurnOutcome, FakeLedger) {
    let (replayed, ledger) = try_replay_with_config(cassette, host, user_message, config).await;
    (
        replayed.expect("replay succeeds without a provider"),
        ledger,
    )
}

fn assert_same_outcome(recorded: &RigTurnOutcome, replayed: &RigTurnOutcome) {
    assert_eq!(recorded.reply, replayed.reply, "reply replays");
    assert_eq!(recorded.actions, replayed.actions, "plan actions replay");
    assert_eq!(recorded.trace, replayed.trace, "tool trace replays");
    assert_eq!(
        recorded.input_tokens, replayed.input_tokens,
        "usage replays"
    );
    assert_eq!(
        recorded.output_tokens, replayed.output_tokens,
        "usage replays"
    );
    assert_eq!(recorded.steps, replayed.steps, "steps replay");
    assert_eq!(
        recorded.truncated, replayed.truncated,
        "terminal status replays"
    );
}

#[tokio::test]
async fn cassette_records_run_association_and_replays_a_plain_answer() {
    let (recorded, cassette, _) =
        record_script(vec![text_turn("Hello.")], FakeHost::default(), "hi").await;
    let context = test_context();
    assert_eq!(cassette.conversation_id, context.conversation_id);
    assert_eq!(cassette.project_id, context.project_id);
    assert_eq!(cassette.route, context.route);
    assert_eq!(cassette.prompt_version, crate::ASSISTANT_PROMPT_VERSION);
    assert!(
        cassette.rig_version.contains("rig-agent"),
        "{}",
        cassette.rig_version
    );
    assert!(!cassette.effect_log.records.is_empty());
    assert_ne!(cassette.run_spec_hash, 0);
    assert_eq!(
        cassette.semantic_contract_id.as_deref(),
        Some("contract-id")
    );

    let host = FakeHost::default();
    let (replayed, _) = replay_cassette(&cassette, host, "hi").await;
    assert_same_outcome(&recorded, &replayed);
}

#[tokio::test]
async fn cassette_replays_a_single_read() {
    let host = FakeHost::default().with_read("get_project_overview", json!({"reports": 3}));
    let (recorded, cassette, _) = record_script(
        vec![
            stream_call("c1", "get_project_overview", json!({})),
            text_turn("Three."),
        ],
        host,
        "how many?",
    )
    .await;
    assert_eq!(recorded.trace.len(), 1);

    let replay_host = FakeHost::default().with_read("get_project_overview", json!({"reports": 3}));
    let (replayed, _) = replay_cassette(&cassette, replay_host.clone(), "how many?").await;
    assert_same_outcome(&recorded, &replayed);
    // The replay re-executed the read against its own host.
    assert_eq!(replay_host.calls.lock().expect("calls").len(), 1);
}

#[tokio::test]
async fn cassette_replays_multiple_reads_in_order() {
    let read = |title: &str| FakeHost::default().with_read("get_report", json!({"title": title}));
    let report = Uuid::from_u128(1);
    let script = || {
        vec![
            vec![
                MockStreamEvent::tool_call(
                    "c1".to_owned(),
                    "get_report".to_owned(),
                    json!({"report_id": report}),
                )
                .with_call_id("c1".to_owned()),
                MockStreamEvent::tool_call(
                    "c2".to_owned(),
                    "get_report".to_owned(),
                    json!({"report_id": report}),
                )
                .with_call_id("c2".to_owned()),
                MockStreamEvent::final_response_with_total_tokens(4),
            ],
            text_turn("Two reads."),
        ]
    };
    let (recorded, cassette, _) = record_script(script(), read("T"), "read twice").await;
    assert_eq!(recorded.trace.len(), 2);

    let (replayed, _) = replay_cassette(&cassette, read("T"), "read twice").await;
    assert_same_outcome(&recorded, &replayed);
    let tools: Vec<String> = replayed
        .trace
        .iter()
        .map(|entry| entry.call.name.clone())
        .collect();
    assert_eq!(tools, vec!["get_report", "get_report"]);
}

#[tokio::test]
async fn cassette_replays_a_tool_error() {
    let host = FakeHost::default();
    host.forced_error
        .lock()
        .expect("error lock")
        .replace(AiError::InvalidContext("nope".to_owned()));
    let (recorded, cassette, _) = record_script(
        vec![
            stream_call("c1", "get_project_overview", json!({})),
            text_turn("It failed."),
        ],
        host,
        "try it",
    )
    .await;
    assert_eq!(recorded.trace[0].output, json!({"error": "nope"}));

    let replay_host = FakeHost::default();
    replay_host
        .forced_error
        .lock()
        .expect("error lock")
        .replace(AiError::InvalidContext("nope".to_owned()));
    let (replayed, _) = replay_cassette(&cassette, replay_host, "try it").await;
    assert_same_outcome(&recorded, &replayed);
}

#[tokio::test]
async fn cassette_replays_proposals_without_applying_them() {
    let ids = || json!([Uuid::from_u128(1)]);
    let proposal = |id: &str| {
        vec![
            MockStreamEvent::tool_call(
                id.to_owned(),
                "screen_reports".to_owned(),
                json!({
                    "report_ids": ids(),
                    "decision": "exclude",
                    "summary": "Exclude off-topic",
                    "rationale": "Protocol requires humans",
                }),
            )
            .with_call_id(id.to_owned()),
            MockStreamEvent::final_response_with_total_tokens(4),
        ]
    };
    let script = vec![proposal("c1"), proposal("c2"), text_turn("Two plans.")];
    let (recorded, cassette, _) = record_script(script, FakeHost::default(), "exclude both").await;
    assert_eq!(recorded.actions.len(), 2);
    assert!(recorded.actions.iter().all(|action| action.executable));

    let (replayed, _) = replay_cassette(&cassette, FakeHost::default(), "exclude both").await;
    assert_same_outcome(&recorded, &replayed);
    // Proposals were recreated as plan data, never applied: replay used a
    // host with no persistence and still produced the same actions.
}

#[tokio::test]
async fn cassette_replays_a_manual_only_step() {
    let (recorded, cassette, _) = record_script(
        vec![
            stream_call(
                "c1",
                "request_protocol_publish",
                json!({"summary": "Publish it", "rationale": "done"}),
            ),
            text_turn("Do it yourself."),
        ],
        FakeHost::default(),
        "publish?",
    )
    .await;
    assert!(!recorded.actions[0].executable);

    let (replayed, _) = replay_cassette(&cassette, FakeHost::default(), "publish?").await;
    assert_same_outcome(&recorded, &replayed);
}

#[tokio::test]
async fn cassette_replays_a_corrected_false_claim() {
    let (recorded, cassette, _) = record_script(
        vec![
            text_turn("I've queued it; confirm to apply."),
            text_turn("Nothing was queued."),
        ],
        FakeHost::default(),
        "queue it",
    )
    .await;
    assert_eq!(recorded.reply, "Nothing was queued.");

    let (replayed, _) = replay_cassette(&cassette, FakeHost::default(), "queue it").await;
    assert_same_outcome(&recorded, &replayed);
}

#[tokio::test]
async fn cassette_replays_turn_exhaustion() {
    let host = FakeHost::default().with_read("get_project_overview", json!({"reports": 1}));
    let context = test_context();
    let model = MockCompletionModel::from_stream_turns(vec![
        stream_call("c1", "get_project_overview", json!({})),
        text_turn("unreached"),
    ]);
    let factory = Arc::new(StaticModelFactory::new(model.erase()));
    let recorder = rig_cassette::effect_log::EffectLogRecorder::keeping_stream_events();
    let outcome = run_rig_turn(
        RigTurn {
            factory,
            run_id: Uuid::new_v4(),
            semantic_contract_id: None,
            build_provenance: None,
            context,
            history: Vec::new(),
            user_message: "go".to_owned(),
            host: Arc::new(host),
            ledger: Arc::new(FakeLedger::default()),
            prices: PriceBook::default(),
            config: AgentLoopConfig {
                max_steps: 1,
                ..test_config()
            },
            recorder: Some(recorder),
            progress: None,
        },
        |_| {},
    )
    .await
    .expect("exhausted turn settles truncated");
    assert!(outcome.truncated);
    let cassette = outcome.cassette.expect("exhaustion still records");

    // Replay runs under the same turn budget: config drift fails closed.
    let config = AgentLoopConfig {
        max_steps: 1,
        ..test_config()
    };
    let (replayed, _) = replay_cassette_with_config(
        &cassette,
        FakeHost::default().with_read("get_project_overview", json!({"reports": 1})),
        "go",
        config,
    )
    .await;
    assert!(replayed.truncated);
    assert_same_outcome(
        &RigTurnOutcome {
            cassette: None,
            ..outcome
        },
        &replayed,
    );
}

#[tokio::test]
async fn provider_failure_records_no_cassette() {
    let context = test_context();
    let model = MockCompletionModel::from_stream_turns(vec![vec![MockStreamEvent::error(
        "boom".to_owned(),
    )]]);
    let factory = Arc::new(StaticModelFactory::new(model.erase()));
    let recorder = rig_cassette::effect_log::EffectLogRecorder::keeping_stream_events();
    let outcome = run_rig_turn(
        RigTurn {
            factory,
            run_id: Uuid::new_v4(),
            semantic_contract_id: None,
            build_provenance: None,
            context,
            history: Vec::new(),
            user_message: "go".to_owned(),
            host: Arc::new(FakeHost::default()),
            ledger: Arc::new(FakeLedger::default()),
            prices: PriceBook::default(),
            config: test_config(),
            recorder: Some(recorder),
            progress: None,
        },
        |_| {},
    )
    .await;
    assert!(matches!(outcome, Err(AiError::Gateway(_))));
}

#[tokio::test]
async fn cassette_round_trips_through_json() {
    let (_, cassette, _) =
        record_script(vec![text_turn("Hello.")], FakeHost::default(), "hi").await;
    let json = serde_json::to_value(&cassette).expect("cassette serializes");
    let restored: AssistantCassette = serde_json::from_value(json).expect("cassette deserializes");
    assert_eq!(restored.run_id, cassette.run_id);
    assert_eq!(restored.run_spec_hash, cassette.run_spec_hash);
    assert_eq!(
        restored.effect_log.records.len(),
        cassette.effect_log.records.len()
    );
}

#[tokio::test]
async fn tampered_run_spec_fails_closed() {
    let (_, mut cassette, _) =
        record_script(vec![text_turn("Hello.")], FakeHost::default(), "hi").await;
    cassette.effect_log.header.run_spec = Some(cassette.run_spec_hash.wrapping_add(1));
    let (result, _) = try_replay(&cassette, FakeHost::default(), "hi").await;
    assert!(
        result.is_err(),
        "a log stamped for another run spec must not replay"
    );
}

#[tokio::test]
async fn tampered_hook_stack_fails_closed() {
    let (_, mut cassette, _) =
        record_script(vec![text_turn("Hello.")], FakeHost::default(), "hi").await;
    cassette.effect_log.header.hooks.clear();
    let (result, _) = try_replay(&cassette, FakeHost::default(), "hi").await;
    assert!(
        result.is_err(),
        "a log from another hook stack must not replay"
    );
}

#[tokio::test]
async fn replay_with_a_different_prompt_fails_closed() {
    let (_, cassette, _) =
        record_script(vec![text_turn("Hello.")], FakeHost::default(), "hi").await;
    let (result, _) = try_replay(&cassette, FakeHost::default(), "different question").await;
    assert!(
        result.is_err(),
        "a replayer must refuse a model request the log never served"
    );
}

#[tokio::test]
async fn cassette_replays_rejected_tool_arguments() {
    let (recorded, cassette, _) = record_script(
        vec![
            stream_call(
                "c1",
                "screen_reports",
                json!({"report_ids": ["nope"], "decision": "exclude"}),
            ),
            text_turn("Sorry."),
        ],
        FakeHost::default(),
        "exclude it",
    )
    .await;
    assert!(recorded.actions.is_empty());
    assert!(recorded.trace[0].output.get("error").is_some());

    let (replayed, _) = replay_cassette(&cassette, FakeHost::default(), "exclude it").await;
    assert_same_outcome(&recorded, &replayed);
}

#[tokio::test]
async fn dropping_observation_does_not_cancel_the_run() {
    // The channel split: execution proceeds even when nobody observes.
    let context = test_context();
    let model = MockCompletionModel::from_stream_turns(vec![
        stream_call("c1", "get_project_overview", json!({})),
        text_turn("Done."),
    ]);
    let factory = Arc::new(StaticModelFactory::new(model.erase()));
    let host = FakeHost::default().with_read("get_project_overview", json!({"reports": 1}));
    let (future, events) = run_rig_turn_channel(RigTurn {
        factory,
        run_id: Uuid::new_v4(),
        semantic_contract_id: None,
        build_provenance: None,
        context,
        history: Vec::new(),
        user_message: "go".to_owned(),
        host: Arc::new(host),
        ledger: Arc::new(FakeLedger::default()),
        prices: PriceBook::default(),
        config: test_config(),
        recorder: None,
        progress: None,
    })
    .expect("channel splits");
    drop(events);
    let outcome = future.await.expect("unobserved run still completes");
    assert_eq!(outcome.reply, "Done.");
    assert_eq!(outcome.trace.len(), 1);
}
