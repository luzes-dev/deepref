//! DeepRef lifecycle hooks for the Rig assistant runtime.
//!
//! Every completion reserves its conservative bound before Rig dispatches it.
//! A finished turn persists observed usage and settles it. Provider errors
//! keep the reservation unresolved unless the adapter proves zero liability;
//! accounting failures stop the run.

use std::{
    collections::BTreeMap,
    future::Future,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
};

use crate::{
    AgentLoopConfig, AiError, AiUsageContext, AssistantRequestBounds, PriceBook, TokenUsage,
    UsageLedger, UsageObservation, UsageReservation, assistant_input_bound_tokens,
    assistant_reservation_request, claims_pending_change,
    runtime::{
        context::DeepRefAgentContext,
        tools::{PlanCollector, ToolHostScope},
    },
    usage::observe_or_hold,
};
use rig_agent::agent::{
    AgentHook, CompletionCallAction, CompletionCallEvent, DispatchAction, DispatchEvent,
    HookContext, InvalidToolCallAction, InvalidToolCallContext, ModelTurnAction, ModelTurnFinished,
    OutcomeAction, OutcomeEvent, RetryRequest, RunSettled, RunStart, RunStartAction,
    SettledOutcome,
};
use rig_core::{ErrorKind, ErrorReport, effect::EffectKind, message::AssistantContent};

pub const BUDGET_STOP_REASON: &str = "project AI budget is exhausted";
pub const TOKEN_STOP_REASON: &str = "assistant turn token budget is exhausted";
const RESERVATION_STOP_REASON: &str = "AI usage reservation failed";
const SETTLEMENT_STOP_REASON: &str = "AI usage settlement failed";

#[derive(Clone)]
pub struct DeepRefHooks {
    scope: ToolHostScope,
    ledger: Arc<dyn UsageLedger>,
    prices: Arc<PriceBook>,
    usage_context: AiUsageContext,
    usage: Arc<Mutex<TokenUsage>>,
    reservations: Arc<Mutex<BTreeMap<usize, UsageReservation>>>,
    reservation_error: Arc<Mutex<Option<AiError>>>,
    consistency_retried: Arc<AtomicBool>,
    config: AgentLoopConfig,
}

impl DeepRefHooks {
    pub fn new(
        scope: ToolHostScope,
        ledger: Arc<dyn UsageLedger>,
        prices: PriceBook,
        usage_context: AiUsageContext,
        config: AgentLoopConfig,
    ) -> Self {
        Self {
            scope,
            ledger,
            prices: Arc::new(prices),
            usage_context,
            usage: Arc::new(Mutex::new(TokenUsage::default())),
            reservations: Arc::new(Mutex::new(BTreeMap::new())),
            reservation_error: Arc::new(Mutex::new(None)),
            consistency_retried: Arc::new(AtomicBool::new(false)),
            config,
        }
    }

    pub fn usage(&self) -> TokenUsage {
        self.usage.lock().map(|usage| *usage).unwrap_or_default()
    }

    pub(crate) fn take_reservation_error(&self) -> Option<AiError> {
        self.reservation_error
            .lock()
            .ok()
            .and_then(|mut error| error.take())
    }

    fn record_error(&self, error: AiError) {
        if let Ok(mut stored) = self.reservation_error.lock()
            && stored.is_none()
        {
            *stored = Some(error);
        }
    }

    pub(crate) fn consistency_retried(&self) -> bool {
        self.consistency_retried.load(Ordering::SeqCst)
    }

    pub fn context(&self) -> &DeepRefAgentContext {
        &self.scope.context
    }

    pub fn plans(&self) -> &PlanCollector {
        &self.scope.plans
    }

    fn take_reservation(&self, turn: usize) -> Option<UsageReservation> {
        self.reservations
            .lock()
            .ok()
            .and_then(|mut reservations| reservations.remove(&turn))
    }

    async fn settle_turn_usage(
        &self,
        turn: usize,
        input_tokens: Option<u64>,
        output_tokens: Option<u64>,
        reasoning_tokens: Option<u64>,
        provider_request_id: Option<String>,
        provider_response_id: Option<String>,
    ) -> Result<(), AiError> {
        if let Ok(mut usage) = self.usage.lock() {
            if let Some(input_tokens) = input_tokens {
                usage.input_tokens = usage.input_tokens.saturating_add(input_tokens);
            }
            if let Some(output_tokens) = output_tokens {
                usage.output_tokens = usage.output_tokens.saturating_add(output_tokens);
            }
        }
        let reservation = self.take_reservation(turn).ok_or_else(|| {
            AiError::Persistence("assistant model call had no usage reservation".to_owned())
        })?;
        let observation = UsageObservation {
            input_tokens,
            output_tokens,
            reasoning_tokens,
            commercial_units: None,
            provider_request_id,
            provider_response_id,
        };
        observation.validate()?;
        observe_or_hold(self.ledger.as_ref(), &reservation, observation.clone()).await?;
        if !observation.has_token_usage() {
            tracing::warn!("assistant AI usage counters are missing; reservation remains held");
            return Ok(());
        }
        let actual_cost_micros = reservation.price.actual_cost_micros(&observation)?;
        self.ledger
            .settle(&reservation, observation, &self.prices.version())
            .await?;
        tracing::info!(actual_cost_micros, "assistant AI usage settled");
        Ok(())
    }

    async fn retain_unresolved(&self, turn: usize, reason: &str) -> Result<(), AiError> {
        let Some(reservation) = self.take_reservation(turn) else {
            return Ok(());
        };
        self.ledger.unresolved(&reservation, None, reason).await?;
        tracing::warn!("assistant AI usage remains unresolved");
        Ok(())
    }
}

fn turn_text(content: &[AssistantContent]) -> String {
    content
        .iter()
        .filter_map(|block| match block {
            AssistantContent::Text(text) => Some(text.text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("")
}

fn turn_has_tool_calls(content: &[AssistantContent]) -> bool {
    content
        .iter()
        .any(|block| matches!(block, AssistantContent::ToolCall(_)))
}

impl AgentHook for DeepRefHooks {
    fn name(&self) -> Option<String> {
        Some("deepref-assistant-hooks-v2".to_owned())
    }

    async fn on_run_start(&self, _ctx: &HookContext, _event: RunStart<'_>) -> RunStartAction {
        tracing::info!("assistant turn started");
        RunStartAction::Continue
    }

    fn on_completion_call(
        &self,
        _ctx: &HookContext,
        _event: CompletionCallEvent<'_>,
    ) -> impl Future<Output = CompletionCallAction> + rig_core::wasm_compat::WasmCompatSend {
        let hooks = self.clone();
        async move {
            let usage = hooks.usage();
            if usage.input_tokens.saturating_add(usage.output_tokens)
                >= hooks.config.max_total_tokens
            {
                return CompletionCallAction::Stop(TOKEN_STOP_REASON.to_owned());
            }
            CompletionCallAction::Continue
        }
    }

    fn on_dispatch(
        &self,
        _ctx: &HookContext,
        event: DispatchEvent<'_>,
    ) -> impl Future<Output = DispatchAction> + rig_core::wasm_compat::WasmCompatSend {
        let hooks = self.clone();
        let turn = event.turn;
        let is_completion = event.completion_request().is_some();
        let request = event.completion_request().cloned();
        async move {
            let fail = |error| {
                hooks.record_error(error);
                DispatchAction::Deny(ErrorReport::new(ErrorKind::Other, RESERVATION_STOP_REASON))
            };
            // Tool calls are local, unbilled dispatches. Their prompt/result
            // tokens are included in the next metered completion request.
            if !is_completion {
                return DispatchAction::Proceed;
            }
            let Some(mut request) = request else {
                return fail(AiError::UnboundedInput);
            };
            if request
                .model
                .as_deref()
                .is_some_and(|model| model != hooks.scope.context.route.model)
                || request.additional_params.as_ref().is_some_and(|params| {
                    params.as_object().is_none_or(|object| !object.is_empty())
                })
                || !request.provider_options.is_empty()
                || request.options != Default::default()
            {
                return fail(AiError::UnsupportedUsageRoute {
                    provider: hooks.scope.context.route.provider.clone(),
                    model: hooks.scope.context.route.model.clone(),
                });
            }
            // Rig binds the selected static model on its CompletionModel and
            // leaves `request.model` unset unless a per-call override exists.
            // Freeze that effective model in the accounted request identity.
            request.model = Some(hooks.scope.context.route.model.clone());
            let capabilities = match hooks.prices.capabilities(
                &hooks.scope.context.route.provider,
                &hooks.scope.context.route.model,
            ) {
                Ok(capabilities) => capabilities,
                Err(error) => return fail(error),
            };
            let input_bound = match assistant_input_bound_tokens(
                &request,
                &hooks.scope.context.route.model,
                capabilities,
            ) {
                Ok(bound) => bound,
                Err(error) => return fail(error),
            };
            let serialized = match serde_json::to_vec(&request) {
                Ok(serialized) => serialized,
                Err(_) => return fail(AiError::UnboundedInput),
            };
            let fingerprint = crate::sha256_bytes(&serialized);
            let reservation_request = match assistant_reservation_request(
                hooks.usage_context.clone(),
                hooks.scope.context.project_id,
                &hooks.scope.context.route,
                AssistantRequestBounds {
                    turn,
                    input_token_bound: Some(input_bound),
                    max_output_tokens: request.max_tokens,
                    request_fingerprint: fingerprint,
                },
                &hooks.prices,
            ) {
                Ok(reservation_request) => reservation_request,
                Err(error) => return fail(error),
            };
            let reserved_cost_micros = match reservation_request.reserved_cost_micros() {
                Ok(cost) => cost,
                Err(error) => return fail(error),
            };
            match hooks.ledger.reserve(reservation_request).await {
                Ok(reservation) => {
                    if let Ok(mut reservations) = hooks.reservations.lock() {
                        reservations.insert(turn, reservation);
                        tracing::info!(reserved_cost_micros, "assistant AI usage reserved");
                        DispatchAction::Proceed
                    } else {
                        fail(AiError::Persistence(
                            "assistant reservation state could not be stored".to_owned(),
                        ))
                    }
                }
                Err(error) => fail(error),
            }
        }
    }

    fn on_model_turn_finished(
        &self,
        _ctx: &HookContext,
        event: ModelTurnFinished<'_>,
    ) -> impl Future<Output = ModelTurnAction> + rig_core::wasm_compat::WasmCompatSend {
        let hooks = self.clone();
        let turn = event.turn;
        let input_tokens = event.usage.input_tokens;
        let output_tokens = event.usage.output_tokens;
        let reasoning_tokens = event.usage.reasoning_tokens;
        let provider_request_id = event.identity.provider_request_id.clone();
        let provider_response_id = event.identity.response_id.clone();
        let text = turn_text(event.content);
        let has_calls = turn_has_tool_calls(event.content);
        async move {
            if let Err(error) = hooks
                .settle_turn_usage(
                    turn,
                    input_tokens,
                    output_tokens,
                    reasoning_tokens,
                    provider_request_id,
                    provider_response_id,
                )
                .await
            {
                tracing::error!(error = %error, "assistant AI usage settlement failed");
                hooks.record_error(error);
                return ModelTurnAction::Stop(SETTLEMENT_STOP_REASON.to_owned());
            }
            tracing::debug!(
                turn,
                input_tokens,
                output_tokens,
                reasoning_tokens,
                has_tool_calls = has_calls,
                "assistant model turn finished"
            );
            if !has_calls {
                let spent = hooks.usage();
                if spent.input_tokens.saturating_add(spent.output_tokens)
                    >= hooks.config.max_total_tokens
                {
                    return ModelTurnAction::Stop(TOKEN_STOP_REASON.to_owned());
                }
            }
            if !has_calls
                && hooks.scope.plans.is_empty()
                && claims_pending_change(&text)
                && !hooks.consistency_retried.swap(true, Ordering::SeqCst)
            {
                return ModelTurnAction::Retry(RetryRequest::Feedback(
                    super::plan::NO_PLAN_CORRECTION.to_owned(),
                ));
            }
            ModelTurnAction::Continue
        }
    }

    fn on_invalid_tool_call(
        &self,
        _ctx: &HookContext,
        event: &InvalidToolCallContext,
    ) -> impl Future<Output = Option<InvalidToolCallAction>> + rig_core::wasm_compat::WasmCompatSend
    {
        let feedback = format!(
            "unknown tool {}. Use only the tools declared for this turn; if no change is needed, reply directly.",
            event.tool_name
        );
        async move { Some(InvalidToolCallAction::Retry { feedback }) }
    }

    fn on_outcome(
        &self,
        _ctx: &HookContext,
        event: OutcomeEvent<'_>,
    ) -> impl Future<Output = OutcomeAction> + rig_core::wasm_compat::WasmCompatSend {
        let hooks = self.clone();
        let turn = event.turn;
        let completion_failed =
            matches!(event.kind, EffectKind::Completion { .. }) && event.outcome.is_err();
        async move {
            if completion_failed
                && let Err(error) = hooks
                    .retain_unresolved(turn, "provider_outcome_ambiguous")
                    .await
            {
                hooks.record_error(error);
            }
            tracing::debug!(turn, "assistant effect resolved");
            OutcomeAction::Proceed
        }
    }

    fn on_run_settled(
        &self,
        _ctx: &HookContext,
        event: RunSettled<'_>,
    ) -> impl Future<Output = ()> + rig_core::wasm_compat::WasmCompatSend {
        let hooks = self.clone();
        let settled = match event.outcome {
            SettledOutcome::Response(_) => "completed",
            SettledOutcome::Error(_) => "error",
        };
        async move {
            let pending = hooks
                .reservations
                .lock()
                .map(|mut reservations| {
                    let turns = reservations.keys().copied().collect::<Vec<_>>();
                    turns
                        .into_iter()
                        .filter_map(|turn| {
                            reservations
                                .remove(&turn)
                                .map(|reservation| (turn, reservation))
                        })
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            for (_, reservation) in pending {
                if let Err(error) = hooks
                    .ledger
                    .unresolved(&reservation, None, "run_ended_without_settlement")
                    .await
                {
                    hooks.record_error(error);
                }
            }
            let usage = hooks.usage();
            tracing::info!(
                input_tokens = usage.input_tokens,
                output_tokens = usage.output_tokens,
                settled,
                "assistant turn settled"
            );
        }
    }
}
