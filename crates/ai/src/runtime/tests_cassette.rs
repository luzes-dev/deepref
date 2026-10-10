//! Cassette record/replay tests: a scripted turn records an effect log, and
//! replaying it without a provider reproduces the same outcome. Tool
//! dispatches execute live against a deterministic host during replay, so
//! plan collection, validation and policy behave exactly like the recording.

use std::sync::Arc;

use rig_core::test_utils::{MockCompletionModel, MockStreamEvent};
use serde_json::json;
use uuid::Uuid;

use super::test_fixtures::*;
use super::*;
use crate::{AgentLoopConfig, AiError, PriceBook, runtime::model::StaticModelFactory};

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
