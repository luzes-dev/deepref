//! Live-path parity tests: the Rig runtime enforces the same domain rules as
//! the custom loop (same policy, same project isolation, same plan
//! semantics) without touching a provider.

use std::sync::{Arc, Mutex, atomic::Ordering};

use rig_core::completion::Usage;
use rig_core::test_utils::{MockCompletionModel, MockStreamEvent};
use serde_json::json;
use uuid::Uuid;

use super::test_fixtures::*;
use super::*;
use crate::{
    AgentLoopConfig, AiError, ChatMessage, ChatToolCall, PriceBook, agent_tool_declarations,
    runtime::{
        AgentProgress, OpenAiCompatModelFactory, PlanCollector, ToolHostScope,
        deepref_dynamic_tools, model::StaticModelFactory, run_rig_turn_channel,
    },
};
use deepref_domain::ProjectId;

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
async fn token_cap_beats_the_consistency_reprompt() {
    // A tool-free turn that both crosses the token cap and claims a queued
    // change must stop on the cap: the corrective re-prompt is itself another
    // model call and must not slip past `max_total_tokens`.
    let log = Arc::new(Mutex::new(ProgressLog::default()));
    let mut progress = ProgressRecorder { log: log.clone() };
    let big_claim = vec![
        MockStreamEvent::text("I've queued it; confirm to apply.".to_owned()),
        MockStreamEvent::final_response(Usage::new().input_tokens(100).output_tokens(10)),
    ];
    let config = AgentLoopConfig {
        max_total_tokens: 5,
        ..test_config()
    };
    let (outcome, model) = run_script(
        vec![big_claim, text_turn("unreached")],
        FakeHost::default(),
        FakeLedger::default(),
        config,
        Vec::new(),
        "queue it",
        Some(&mut progress),
    )
    .await;
    let outcome = outcome.expect("capped turn still settles");
    assert!(outcome.truncated);
    assert_eq!(outcome.steps, 1);
    assert_eq!(
        model.requests().len(),
        1,
        "no corrective re-prompt may run while over the token cap"
    );
    assert_eq!(
        log.lock().expect("log lock").discards,
        0,
        "no re-prompt means no discarded text"
    );
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
    let factory = OpenAiCompatModelFactory::new("https://proxy.example/v1", "key");
    let route = test_context().route;
    factory.model(&route).expect("model constructs");
    let empty = OpenAiCompatModelFactory::new("", "");
    assert!(empty.model(&route).is_err());
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
