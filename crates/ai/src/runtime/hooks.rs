//! DeepRef lifecycle hooks for the Rig assistant runtime.
//!
//! Rig owns the agent state machine; these hooks keep DeepRef authoritative
//! for budget, usage accounting, telemetry and plan consistency:
//!
//! * budget: checked before the run and before every model call; exhaustion
//!   stops the run with [`BUDGET_STOP_REASON`], which the turn driver maps
//!   to [`AiError::BudgetExceeded`].
//! * token cap: checked before every model call AND after every model turn.
//!   The pre-call check stops the next call; the post-turn check stops a run
//!   whose latest turn just spent past `max_total_tokens`, so a tool-free
//!   turn cannot buy one more call (not even the plan-consistency re-prompt).
//!   Tool-bearing turns still continue so their already-requested tools run;
//!   stopping there would drop committed work the trace must keep.
//! * usage: every model turn's reported tokens are priced with DeepRef's
//!   price book and recorded in the usage ledger (bookkeeping never fails
//!   the turn, mirroring `MeteredGateway`).
//! * plan consistency: a tool-free reply that claims a change was queued
//!   while the turn collected no [`PlanAction`][crate::PlanAction] gets one
//!   corrective re-prompt, exactly like the custom loop; a surviving claim
//!   is annotated by the turn driver afterwards.
//! * telemetry: run/model/tool lifecycle logged through `tracing`.

use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};

use rig_agent::agent::{
    AgentHook, CompletionCallAction, CompletionCallEvent, HookContext, InvalidToolCallAction,
    InvalidToolCallContext, ModelTurnAction, ModelTurnFinished, OutcomeAction, OutcomeEvent,
    RetryRequest, RunSettled, RunStart, RunStartAction, SettledOutcome,
};
use rig_core::completion::AssistantContent;

use crate::{
    AgentLoopConfig, AiError, PriceBook, TokenUsage, UsageEntry, UsageLedger,
    claims_pending_change,
    runtime::{
        context::DeepRefAgentContext,
        tools::{PlanCollector, ToolHostScope},
    },
};

/// Stop reason when the project budget is exhausted. The turn driver maps
/// exactly this reason to [`AiError::BudgetExceeded`].
pub const BUDGET_STOP_REASON: &str = "project AI budget is exhausted";
/// Stop reason when the turn's token cap is reached. The driver synthesizes
/// the same truncated reply the custom loop produces.
pub const TOKEN_STOP_REASON: &str = "assistant turn token budget is exhausted";

/// Shared per-turn state for DeepRef's hooks.
#[derive(Clone)]
pub struct DeepRefHooks {
    scope: ToolHostScope,
    ledger: Arc<dyn UsageLedger>,
    prices: Arc<PriceBook>,
    usage: Arc<Mutex<TokenUsage>>,
    consistency_retried: Arc<AtomicBool>,
    config: AgentLoopConfig,
}

impl DeepRefHooks {
    pub fn new(
        scope: ToolHostScope,
        ledger: Arc<dyn UsageLedger>,
        prices: PriceBook,
        config: AgentLoopConfig,
    ) -> Self {
        Self {
            scope,
            ledger,
            prices: Arc::new(prices),
            usage: Arc::new(Mutex::new(TokenUsage::default())),
            consistency_retried: Arc::new(AtomicBool::new(false)),
            config,
        }
    }

    /// Total tokens spent by model calls so far this turn.
    pub fn usage(&self) -> TokenUsage {
        self.usage.lock().map(|usage| *usage).unwrap_or_default()
    }

    /// Whether the plan-consistency hook already used its one corrective
    /// re-prompt. The driver annotates a surviving claim afterwards.
    pub(crate) fn consistency_retried(&self) -> bool {
        self.consistency_retried.load(Ordering::SeqCst)
    }

    pub fn context(&self) -> &DeepRefAgentContext {
        &self.scope.context
    }

    pub fn plans(&self) -> &PlanCollector {
        &self.scope.plans
    }

    async fn budget_exhausted(&self) -> Result<bool, AiError> {
        Ok(self
            .ledger
            .budget(self.scope.context.project_id)
            .await?
            .exhausted())
    }

    async fn record_turn_usage(&self, input_tokens: u64, output_tokens: u64) {
        if let Ok(mut usage) = self.usage.lock() {
            usage.input_tokens += input_tokens;
            usage.output_tokens += output_tokens;
        }
        let route = &self.scope.context.route;
        let cost = self.prices.estimate_cost_micros(
            &route.provider,
            &route.model,
            input_tokens,
            output_tokens,
        );
        let entry = UsageEntry {
            project_id: Some(self.scope.context.project_id.as_uuid()),
            profile: route.profile.as_str().to_owned(),
            provider: route.provider.clone(),
            model: route.model.clone(),
            purpose: "chat",
            input_tokens,
            output_tokens,
            cost_micros: cost,
        };
        if let Err(error) = self.ledger.record(entry).await {
            // The provider call already happened; never fail the turn
            // because bookkeeping failed.
            tracing::warn!(?error, "failed to record assistant usage");
        }
    }
}

/// Assistant reply text of one finished model turn.
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
        Some("deepref-assistant-hooks-v1".to_owned())
    }

    fn on_run_start(
        &self,
        _ctx: &HookContext,
        _event: RunStart<'_>,
    ) -> impl Future<Output = RunStartAction> + rig_core::wasm_compat::WasmCompatSend {
        let hooks = self.clone();
        async move {
            tracing::info!(
                project_id = %hooks.scope.context.project_id.as_uuid(),
                "assistant turn started"
            );
            match hooks.budget_exhausted().await {
                Ok(false) => RunStartAction::Continue,
                Ok(true) => RunStartAction::Stop(BUDGET_STOP_REASON.to_owned()),
                Err(error) => RunStartAction::Stop(format!("AI budget lookup failed: {error}")),
            }
        }
    }

    fn on_completion_call(
        &self,
        _ctx: &HookContext,
        _event: CompletionCallEvent<'_>,
    ) -> impl Future<Output = CompletionCallAction> + rig_core::wasm_compat::WasmCompatSend {
        let hooks = self.clone();
        async move {
            let usage = hooks.usage();
            if usage.input_tokens + usage.output_tokens >= hooks.config.max_total_tokens {
                return CompletionCallAction::Stop(TOKEN_STOP_REASON.to_owned());
            }
            match hooks.budget_exhausted().await {
                Ok(false) => CompletionCallAction::Continue,
                Ok(true) => CompletionCallAction::Stop(BUDGET_STOP_REASON.to_owned()),
                Err(error) => {
                    CompletionCallAction::Stop(format!("AI budget lookup failed: {error}"))
                }
            }
        }
    }

    fn on_model_turn_finished(
        &self,
        _ctx: &HookContext,
        event: ModelTurnFinished<'_>,
    ) -> impl Future<Output = ModelTurnAction> + rig_core::wasm_compat::WasmCompatSend {
        let hooks = self.clone();
        let input_tokens = event.usage.input_tokens.unwrap_or(0);
        let output_tokens = event.usage.output_tokens.unwrap_or(0);
        let text = turn_text(event.content);
        let has_calls = turn_has_tool_calls(event.content);
        async move {
            hooks.record_turn_usage(input_tokens, output_tokens).await;
            tracing::debug!(
                turn = event.turn,
                input_tokens,
                output_tokens,
                has_tool_calls = has_calls,
                "assistant model turn finished"
            );
            // Token cap first: the pre-call hook can only refuse the *next*
            // call, so a turn that just spent past the cap must stop here —
            // otherwise the consistency re-prompt below would issue one more
            // model call while already over budget. Tool-bearing turns are
            // exempt: their tools were already requested and must still run
            // (the pre-call hook stops the run before the following call).
            // Token cap first: the pre-call hook can only refuse the *next*
            // call, so a turn that just spent past the cap must stop here —
            // otherwise the consistency re-prompt below would issue one more
            // model call while already over budget. Tool-bearing turns are
            // exempt: their tools were already requested and must still run
            // (the pre-call hook stops the run before the following call).
            if !has_calls {
                let spent = hooks.usage();
                if spent.input_tokens + spent.output_tokens >= hooks.config.max_total_tokens {
                    return ModelTurnAction::Stop(TOKEN_STOP_REASON.to_owned());
                }
            }
            // Plan consistency: a tool-free reply claiming a queued change
            // with no plan action gets one corrective re-prompt; a surviving
            // claim is annotated by the turn driver afterwards.
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
            "unknown tool `{}`. Use only the tools declared for this turn; if no change is needed, reply directly.",
            event.tool_name
        );
        async move { Some(InvalidToolCallAction::Retry { feedback }) }
    }

    fn on_outcome(
        &self,
        _ctx: &HookContext,
        event: OutcomeEvent<'_>,
    ) -> impl Future<Output = OutcomeAction> + rig_core::wasm_compat::WasmCompatSend {
        let turn = event.turn;
        async move {
            tracing::debug!(turn, "assistant effect resolved");
            OutcomeAction::Proceed
        }
    }

    fn on_run_settled(
        &self,
        _ctx: &HookContext,
        event: RunSettled<'_>,
    ) -> impl Future<Output = ()> + rig_core::wasm_compat::WasmCompatSend {
        let usage = self.usage();
        let settled = match event.outcome {
            SettledOutcome::Response(_) => "completed",
            SettledOutcome::Error(_) => "error",
        };
        async move {
            tracing::info!(
                input_tokens = usage.input_tokens,
                output_tokens = usage.output_tokens,
                settled,
                "assistant turn settled"
            );
        }
    }
}
