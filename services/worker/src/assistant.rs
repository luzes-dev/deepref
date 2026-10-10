//! Durable assistant agent runs: execution owned by the worker.
//!
//! HTTP persists the user message, creates a queued run and enqueues a job;
//! this module claims the job and drives the Rig agent to settlement. The
//! browser observes persisted run events; it never owns execution, so a
//! disconnect cannot cancel a valid run.
//!
//! Retry safety: terminal runs are never re-executed, the final message and
//! plan are written idempotently under fixed ids, and a recovered lease
//! re-drives a `running` run by continuing its append-only event log.
//! Provider failures requeue; budget exhaustion and malformed runs fail
//! closed.

use std::{sync::Arc, time::Duration};

use deepref_ai::{
    AgentLoopConfig, AiError, ResolvedModel,
    runtime::{AssistantToolHost, DeepRefAgentContext, RigTurn},
};
use deepref_domain::{Actor, ActorKind, ProjectId};
use serde::Deserialize;
use serde_json::{Value, json};
use sqlx::PgPool;
use uuid::Uuid;

use super::delivery::DeliveryAction;
use deepref_postgres::{
    AppendAssistantMessage, NewAssistantPlan, PostgresAssistantToolHost, PostgresUsageLedger,
    begin_assistant_agent_run, get_assistant_agent_run,
};

/// Services the assistant processor needs beyond the pool. The thin
/// [`handle_assistant_agent_run`] entry builds these from the environment;
/// tests inject fakes through [`handle_assistant_agent_run_with`].
pub struct AssistantWorkerServices {
    pub model_factory: Arc<dyn deepref_ai::runtime::AgentModelFactory>,
    pub prices: deepref_ai::PriceBook,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct AssistantAgentRunJobPayload {
    assistant_run_id: Uuid,
}

const RETRY_DELAY: Duration = Duration::from_secs(30);

pub async fn handle_assistant_agent_run(
    pool: &PgPool,
    job: &deepref_application::jobs::ClaimedJob,
    owner: &str,
) -> anyhow::Result<DeliveryAction> {
    let _ = owner;
    let config = deepref_config::AiProviderConfig::from_env()?;
    let Some(api_key) = config.api_key() else {
        return Ok(DeliveryAction::Terminate);
    };
    let factory = Arc::new(deepref_ai::runtime::OpenAiCompatModelFactory::new(
        config.base_url.clone(),
        api_key.to_owned(),
    ));
    let prices = deepref_ai::PriceBook::new(config.price_overrides.iter().map(|price| {
        (
            price.provider.clone(),
            price.model.clone(),
            deepref_ai::ModelPrice {
                input_micros_per_million_tokens: price.input_micros_per_million_tokens,
                output_micros_per_million_tokens: price.output_micros_per_million_tokens,
            },
        )
    }));
    handle_assistant_agent_run_with(
        pool,
        job,
        &AssistantWorkerServices {
            model_factory: factory,
            prices,
        },
    )
    .await
}

pub async fn handle_assistant_agent_run_with(
    pool: &PgPool,
    job: &deepref_application::jobs::ClaimedJob,
    services: &AssistantWorkerServices,
) -> anyhow::Result<DeliveryAction> {
    let payload: AssistantAgentRunJobPayload = serde_json::from_value(job.payload.clone())
        .map_err(|_| anyhow::anyhow!("assistant job payload is malformed"))?;
    if get_assistant_agent_run(pool, payload.assistant_run_id)
        .await?
        .is_none()
    {
        return Ok(DeliveryAction::Terminate);
    }
    // Terminal check first: a duplicate delivery after settlement acks
    // without appending anything. The attempts-exhausted fail runs after it
    // so a terminal redelivery never gains a bogus error event, and so the
    // run is already `running` when the fail flip applies (failing a merely
    // queued run would append an event while stranding its status).
    let Some(claimed) = begin_assistant_agent_run(pool, payload.assistant_run_id).await? else {
        return Ok(DeliveryAction::Ack);
    };
    // Attempts exhausted: fail the run closed instead of retrying forever.
    if job.attempts >= job.max_attempts {
        fail_run(
            pool,
            payload.assistant_run_id,
            "ai_attempts_exhausted",
            "the assistant run used up its retry budget",
        )
        .await?;
        return Ok(DeliveryAction::Terminate);
    }
    let outcome = drive_assistant_run(pool, job, &claimed, services).await;
    match outcome {
        Ok(()) => Ok(DeliveryAction::Ack),
        Err(RunFailure::Retryable(error)) => {
            tracing::warn!(run_id = %payload.assistant_run_id, %error, "assistant run failed transiently; requeued");
            Ok(DeliveryAction::Nak(RETRY_DELAY))
        }
        Err(RunFailure::Fatal(error)) => {
            tracing::warn!(run_id = %payload.assistant_run_id, %error, "assistant run failed");
            Ok(DeliveryAction::Terminate)
        }
    }
}

#[derive(Debug)]
enum RunFailure {
    Retryable(anyhow::Error),
    Fatal(anyhow::Error),
}

async fn drive_assistant_run(
    pool: &PgPool,
    job: &deepref_application::jobs::ClaimedJob,
    claimed: &deepref_postgres::ClaimedAssistantAgentRun,
    services: &AssistantWorkerServices,
) -> Result<(), RunFailure> {
    let record = &claimed.record;
    let attempts_exhausted = job.attempts >= job.max_attempts;
    let route: ResolvedModel =
        serde_json::from_value(record.model_route.clone()).map_err(|error| {
            RunFailure::Fatal(anyhow::anyhow!(
                "assistant run model route is invalid: {error}"
            ))
        })?;
    let actor_kind = ActorKind::parse(&record.actor_kind)
        .ok_or_else(|| RunFailure::Fatal(anyhow::anyhow!("assistant run actor kind is invalid")))?;
    let actor = Actor::new(actor_kind, record.actor_id.clone())
        .map_err(|_| RunFailure::Fatal(anyhow::anyhow!("assistant run actor is invalid")))?;
    let history = claimed
        .history
        .iter()
        // The trigger message itself becomes the prompt, not history.
        .filter(|message| message.id != record.trigger_message_id)
        .filter(|message| !message.content.trim().is_empty())
        .filter_map(|message| match message.role.as_str() {
            "user" => Some(deepref_ai::ChatMessage::User(message.content.clone())),
            "assistant" => Some(deepref_ai::ChatMessage::Assistant {
                content: message.content.clone(),
                tool_calls: Vec::new(),
            }),
            _ => None,
        })
        .collect::<Vec<_>>();
    let context = DeepRefAgentContext {
        project_id: ProjectId::new(record.project_id),
        conversation_id: record.conversation_id,
        actor: actor.clone(),
        route: route.clone(),
    };
    let host: Arc<dyn AssistantToolHost> = Arc::new(PostgresAssistantToolHost::new(pool));
    let ledger = Arc::new(PostgresUsageLedger::new(pool));
    let recorder = rig_cassette::effect_log::EffectLogRecorder::keeping_stream_events();
    let turn = RigTurn {
        factory: services.model_factory.clone(),
        run_id: record.id,
        semantic_contract_id: record.semantic_contract_id.clone(),
        build_provenance: Some(record.build_provenance.clone()),
        context,
        history,
        user_message: claimed.user_message.content.clone(),
        host,
        ledger,
        prices: services.prices.clone(),
        config: AgentLoopConfig::default(),
        recorder: Some(recorder),
        progress: None,
    };
    let model_label = route.model.clone();
    let (future, mut events) =
        deepref_ai::runtime::run_rig_turn_channel(turn).map_err(|error| {
            RunFailure::Fatal(anyhow::anyhow!("assistant run failed to start: {error}"))
        })?;
    tokio::pin!(future);
    let mut text_buffer = String::new();
    let outcome = loop {
        tokio::select! {
            settled = &mut future => break settled,
            event = events.recv() => {
                let Some(event) = event else { continue };
                persist_event(pool, record.id, &mut text_buffer, &event).await?;
            }
        }
    };
    while let Ok(event) = events.try_recv() {
        persist_event(pool, record.id, &mut text_buffer, &event).await?;
    }
    flush_text(pool, record.id, &mut text_buffer).await?;
    match outcome {
        Ok(finished) => {
            emit_plan_event(pool, record.id, record.plan_id, finished.actions.len()).await?;
            persist_event_row(
                pool,
                record.id,
                "done",
                &json!({
                    "message_id": record.answer_message_id,
                    "input_tokens": finished.input_tokens,
                    "output_tokens": finished.output_tokens,
                }),
            )
            .await?;
            let cassette = finished.cassette.as_ref();
            let completed = deepref_postgres::CompletedAssistantAgentRun {
                answer: answer_message(record, &model_label, &finished),
                plan: plan_record(record, &actor, &model_label, &finished)?,
                effect_log: cassette
                    .map(|cassette| {
                        serde_json::to_value(&cassette.effect_log).map_err(|error| {
                            RunFailure::Fatal(anyhow::anyhow!(
                                "assistant cassette is not serializable: {error}"
                            ))
                        })
                    })
                    .transpose()?,
                run_spec_hash: cassette.map(|cassette| format!("{:016x}", cassette.run_spec_hash)),
            };
            if !deepref_postgres::complete_assistant_agent_run(pool, record.id, &completed)
                .await
                .map_err(|error| RunFailure::Fatal(error.into()))?
            {
                tracing::warn!(run_id = %record.id, "assistant run completion lost a status race");
            }
            Ok(())
        }
        Err(AiError::BudgetExceeded) => {
            fail_run(
                pool,
                record.id,
                "ai_budget_exceeded",
                "AI budget for this month reached",
            )
            .await
            .map_err(RunFailure::Fatal)?;
            Ok(())
        }
        Err(error) if retryable(&error) => {
            if attempts_exhausted {
                fail_run(pool, record.id, "ai_attempts_exhausted", &error.to_string())
                    .await
                    .map_err(RunFailure::Fatal)?;
                Ok(())
            } else {
                Err(RunFailure::Retryable(anyhow::anyhow!("{error}")))
            }
        }
        Err(error) => {
            fail_run(pool, record.id, "ai_run_failed", &error.to_string())
                .await
                .map_err(RunFailure::Fatal)?;
            Ok(())
        }
    }
}

fn retryable(error: &AiError) -> bool {
    matches!(error, AiError::Gateway(_) | AiError::Persistence(_))
}

async fn fail_run(pool: &PgPool, run_id: Uuid, code: &str, message: &str) -> anyhow::Result<()> {
    // One atomic transaction appends the `error` event and flips the run to
    // failed, or appends nothing when the run already settled.
    deepref_postgres::fail_assistant_agent_run(
        pool,
        run_id,
        &json!({"code": code, "message": message}),
    )
    .await
    .map_err(|error| anyhow::anyhow!("assistant failure bookkeeping failed: {error:?}"))?;
    Ok(())
}

/// Persists one driver event, batching answer text: a row per flush, never
/// per token.
async fn persist_event(
    pool: &PgPool,
    run_id: Uuid,
    text_buffer: &mut String,
    event: &deepref_ai::AssistantStreamEvent,
) -> Result<(), RunFailure> {
    match event {
        deepref_ai::AssistantStreamEvent::Token { delta } => {
            text_buffer.push_str(delta);
            Ok(())
        }
        deepref_ai::AssistantStreamEvent::Replace { text } => {
            text_buffer.clear();
            persist_event_row(pool, run_id, "replace", &json!({"text": text})).await
        }
        deepref_ai::AssistantStreamEvent::ToolStart {
            tool,
            tool_call_id,
            args,
        } => {
            flush_text(pool, run_id, text_buffer).await?;
            persist_event_row(
                pool,
                run_id,
                "tool_start",
                &json!({"tool": tool, "tool_call_id": tool_call_id, "args": args}),
            )
            .await
        }
        deepref_ai::AssistantStreamEvent::ToolComplete {
            tool,
            tool_call_id,
            output,
        } => {
            persist_event_row(
                pool,
                run_id,
                "tool_complete",
                &json!({
                    "tool": tool,
                    "tool_call_id": tool_call_id,
                    "output": stored_output(output),
                }),
            )
            .await
        }
        _ => Ok(()),
    }
}

async fn flush_text(
    pool: &PgPool,
    run_id: Uuid,
    text_buffer: &mut String,
) -> Result<(), RunFailure> {
    if text_buffer.is_empty() {
        return Ok(());
    }
    let text = std::mem::take(text_buffer);
    persist_event_row(pool, run_id, "text", &json!({"delta": text})).await
}

async fn persist_event_row(
    pool: &PgPool,
    run_id: Uuid,
    kind: &str,
    payload: &Value,
) -> Result<(), RunFailure> {
    deepref_postgres::append_assistant_run_event(pool, run_id, kind, payload)
        .await
        .map_err(|error| RunFailure::Fatal(error.into()))?;
    Ok(())
}

fn stored_output(output: &Value) -> Value {
    const LIMIT: usize = 4_000;
    let text = output.to_string();
    if text.chars().count() <= LIMIT {
        output.clone()
    } else {
        json!({
            "truncated": true,
            "preview": text.chars().take(LIMIT).collect::<String>(),
        })
    }
}

fn answer_message(
    record: &deepref_postgres::AssistantAgentRunRecord,
    model: &str,
    finished: &deepref_ai::runtime::RigTurnOutcome,
) -> AppendAssistantMessage {
    let calls: Vec<Value> = finished
        .trace
        .iter()
        .map(|entry| {
            json!({
                "id": entry.call.id,
                "tool": entry.call.name,
                "args": entry.call.arguments,
            })
        })
        .collect();
    let results: Vec<Value> = finished
        .trace
        .iter()
        .map(|entry| {
            json!({
                "tool_call_id": entry.call.id,
                "tool": entry.call.name,
                "output": stored_output(&entry.output),
                "proposal_review_run_id": null,
            })
        })
        .collect();
    AppendAssistantMessage {
        id: record.answer_message_id,
        conversation_id: record.conversation_id,
        role: "assistant".to_owned(),
        content: finished.reply.clone(),
        tool_calls: (!calls.is_empty()).then(|| Value::Array(calls)),
        tool_results: (!results.is_empty()).then(|| Value::Array(results)),
        metadata: Some(json!({
            "message_id": record.answer_message_id,
            "plan_id": record.plan_id,
            "model": model,
            "prompt_version": deepref_ai::ASSISTANT_PROMPT_VERSION,
            "input_tokens": finished.input_tokens,
            "output_tokens": finished.output_tokens,
            "steps": finished.steps,
            "truncated": finished.truncated,
        })),
    }
}

fn plan_record(
    record: &deepref_postgres::AssistantAgentRunRecord,
    actor: &Actor,
    model: &str,
    finished: &deepref_ai::runtime::RigTurnOutcome,
) -> Result<Option<NewAssistantPlan>, RunFailure> {
    if finished.actions.is_empty() {
        return Ok(None);
    }
    let plan_id = record
        .plan_id
        .ok_or_else(|| RunFailure::Fatal(anyhow::anyhow!("assistant run has no plan id")))?;
    let summary = finished
        .actions
        .iter()
        .map(|action| action.summary.as_str())
        .collect::<Vec<_>>()
        .join("; ");
    let actions = serde_json::to_value(&finished.actions).map_err(|_| {
        RunFailure::Fatal(anyhow::anyhow!(
            "assistant plan actions are not serializable"
        ))
    })?;
    let evidence = Value::Array(
        finished
            .trace
            .iter()
            .filter(|entry| deepref_ai::is_read_tool(&entry.call.name))
            .map(|entry| {
                json!({
                    "tool": entry.call.name,
                    "args": entry.call.arguments,
                    "output_sha256": deepref_ai::hash_json(&entry.output).ok(),
                })
            })
            .collect(),
    );
    Ok(Some(NewAssistantPlan {
        id: plan_id,
        project_id: record.project_id,
        conversation_id: record.conversation_id,
        summary,
        actions,
        created_by_kind: actor.kind().as_str().to_owned(),
        created_by_id: actor.id().to_owned(),
        model: model.to_owned(),
        prompt_version: deepref_ai::ASSISTANT_PROMPT_VERSION.to_owned(),
        evidence,
    }))
}

/// Emits the plan observation event. The completion transaction performs
/// the idempotent message and plan inserts afterwards.
///
/// The event deliberately precedes the commit: the `done` event must exist
/// before the run flips terminal or observers can see a terminal status with
/// no `done` row and end the stream early. The HTTP `plan` frame therefore
/// retries its plan lookup briefly until that commit lands instead of
/// resolving to Null on first delivery.
async fn emit_plan_event(
    pool: &PgPool,
    run_id: Uuid,
    plan_id: Option<Uuid>,
    action_count: usize,
) -> Result<(), RunFailure> {
    if action_count == 0 {
        return Ok(());
    }
    persist_event_row(
        pool,
        run_id,
        "plan",
        &json!({
            "plan_id": plan_id,
            "actions": action_count,
        }),
    )
    .await
}
