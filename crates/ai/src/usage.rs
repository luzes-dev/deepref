//! Provider-neutral reservation and settlement contracts.
//!
//! PostgreSQL owns quota correctness. This crate computes a conservative
//! per-invocation bound from a captured price snapshot and requires the host
//! ledger to reserve it before the provider sees the request.

use std::{sync::Arc, time::Duration};

use chrono::NaiveDate;
use deepref_domain::ProjectId;
use uuid::Uuid;

use crate::{
    AiError, AiFuture, AiGateway, AiUsageContext, CompletionRequest, GatewayCompletion,
    PreparedCompletionRequest, PriceBook, RouteCapabilities, UsagePrincipal,
};

const INPUT_BOUND_SAFETY_NUMERATOR: u64 = 5;
const INPUT_BOUND_SAFETY_DENOMINATOR: u64 = 4;

/// Adds the documented provider-template expansion to the exact wire JSON
/// size. Schemas are counted again in their rendered form because templates
/// move them into system text rather than leaving them in the JSON envelope.
fn template_inclusive_input_bound(
    serialized_body: &[u8],
    capabilities: RouteCapabilities,
) -> Result<u64, AiError> {
    let body: serde_json::Value =
        serde_json::from_slice(serialized_body).map_err(|_| AiError::UnboundedInput)?;
    let messages = body
        .get("messages")
        .and_then(serde_json::Value::as_array)
        .ok_or(AiError::UnboundedInput)?;
    let tools = body
        .get("tools")
        .and_then(serde_json::Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default();

    let mut tool_call_count = 0_u64;
    let mut tool_argument_count = 0_u64;
    for message in messages {
        if message
            .get("content")
            .and_then(serde_json::Value::as_array)
            .is_some_and(|parts| {
                parts.iter().any(|part| {
                    part.get("type").and_then(serde_json::Value::as_str) != Some("text")
                })
            })
        {
            // Current allowlisted billing contracts cover text and function
            // tools only; multimodal template expansion has no reviewed bound.
            return Err(AiError::UnboundedInput);
        }
        let Some(calls) = message
            .get("tool_calls")
            .and_then(serde_json::Value::as_array)
        else {
            continue;
        };
        for call in calls {
            tool_call_count = tool_call_count
                .checked_add(1)
                .ok_or(AiError::UnboundedInput)?;
            let arguments = call
                .pointer("/function/arguments")
                .or_else(|| call.get("arguments"));
            let argument_count = match arguments {
                Some(serde_json::Value::Object(arguments)) => arguments.len(),
                Some(serde_json::Value::String(arguments)) => {
                    match serde_json::from_str::<serde_json::Value>(arguments) {
                        Ok(serde_json::Value::Object(arguments)) => arguments.len(),
                        _ => 1,
                    }
                }
                Some(_) => 1,
                None => 0,
            };
            tool_argument_count = tool_argument_count
                .checked_add(u64::try_from(argument_count).map_err(|_| AiError::UnboundedInput)?)
                .ok_or(AiError::UnboundedInput)?;
        }
    }

    let rendered_schema_bytes = tools.iter().try_fold(0_u64, |sum, tool| {
        let schema_bytes = serde_json::to_vec(tool)
            .map_err(|_| AiError::UnboundedInput)?
            .len();
        sum.checked_add(u64::try_from(schema_bytes).map_err(|_| AiError::UnboundedInput)?)
            .ok_or(AiError::UnboundedInput)
    })?;
    let message_count = u64::try_from(messages.len()).map_err(|_| AiError::UnboundedInput)?;
    let tool_count = u64::try_from(tools.len())
        .map_err(|_| AiError::UnboundedInput)?
        .checked_add(tool_call_count)
        .ok_or(AiError::UnboundedInput)?;
    let raw_bound = u64::try_from(serialized_body.len())
        .map_err(|_| AiError::UnboundedInput)?
        .checked_add(capabilities.template_overhead_bytes)
        .and_then(|bound| {
            message_count
                .checked_mul(capabilities.per_message_overhead_bytes)
                .and_then(|overhead| bound.checked_add(overhead))
        })
        .and_then(|bound| {
            tool_count
                .checked_mul(capabilities.per_tool_wrapper_bytes)
                .and_then(|overhead| bound.checked_add(overhead))
        })
        .and_then(|bound| {
            tool_argument_count
                .checked_mul(capabilities.per_tool_argument_wrapper_bytes)
                .and_then(|overhead| bound.checked_add(overhead))
        })
        .and_then(|bound| bound.checked_add(rendered_schema_bytes))
        .ok_or(AiError::UnboundedInput)?;
    raw_bound
        .checked_mul(INPUT_BOUND_SAFETY_NUMERATOR)
        .map(|bound| bound.div_ceil(INPUT_BOUND_SAFETY_DENOMINATOR))
        .ok_or(AiError::UnboundedInput)
}

/// Retries evidence persistence on a fresh ledger transaction with bounded
/// backoff. If all writes fail, the original reservation is still held; the
/// unresolved transition is best-effort and the structured log retains the
/// operator evidence without storing prompt content.
pub(crate) async fn observe_or_hold(
    ledger: &dyn UsageLedger,
    reservation: &UsageReservation,
    observation: UsageObservation,
) -> Result<(), AiError> {
    let mut last_error = None;
    for attempt in 0..3 {
        match ledger.observe(reservation, observation.clone()).await {
            Ok(()) => return Ok(()),
            Err(error) => {
                last_error = Some(error);
                if attempt < 2 {
                    tokio::time::sleep(Duration::from_millis(25 * (1 << attempt))).await;
                }
            }
        }
    }

    let error =
        last_error.unwrap_or_else(|| AiError::Persistence("observation retry failed".to_owned()));
    let hold_error = ledger
        .unresolved(reservation, None, "observation_persistence_failed")
        .await
        .err();
    tracing::error!(
        reservation_id = %reservation.id,
        provider_request_id = ?observation.provider_request_id,
        provider_response_id = ?observation.provider_response_id,
        input_tokens = ?observation.input_tokens,
        output_tokens = ?observation.output_tokens,
        reasoning_tokens = ?observation.reasoning_tokens,
        commercial_units = ?observation.commercial_units,
        hold_persisted = hold_error.is_none(),
        hold_error = ?hold_error,
        error = %error,
        "AI usage evidence persistence failed after retries; reservation remains held"
    );
    Err(error)
}

/// A cost snapshot captured before the provider request is sent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UsagePriceSnapshot {
    pub version: String,
    pub input_micros_per_million_tokens: i64,
    pub output_micros_per_million_tokens: i64,
    pub reasoning_micros_per_million_tokens: Option<i64>,
}

impl UsagePriceSnapshot {
    fn token_cost(rate: i64, tokens: u64) -> Result<i64, AiError> {
        if rate < 0 {
            return Err(AiError::InvalidUsageBound);
        }
        let numerator = u128::from(tokens)
            .checked_mul(rate as u128)
            .ok_or(AiError::InvalidUsageBound)?;
        i64::try_from(numerator.div_ceil(1_000_000)).map_err(|_| AiError::InvalidUsageBound)
    }

    pub fn maximum_cost_micros(
        &self,
        input_token_bound: Option<u64>,
        max_output_tokens: Option<u64>,
        max_reasoning_tokens: Option<u64>,
        other_cost_micros: i64,
        rounding_margin_micros: i64,
    ) -> Result<i64, AiError> {
        let input = input_token_bound.ok_or(AiError::UnboundedInput)?;
        let output = max_output_tokens.ok_or(AiError::UnboundedOutput)?;
        if other_cost_micros < 0
            || rounding_margin_micros < 0
            || self.version.trim().is_empty()
            || self.version.len() > 128
        {
            return Err(AiError::InvalidUsageBound);
        }
        let mut total = Self::token_cost(self.input_micros_per_million_tokens, input)?
            .checked_add(Self::token_cost(
                self.output_micros_per_million_tokens,
                output,
            )?)
            .ok_or(AiError::InvalidUsageBound)?;
        if let Some(rate) = self.reasoning_micros_per_million_tokens {
            total = total
                .checked_add(Self::token_cost(
                    rate,
                    max_reasoning_tokens.ok_or(AiError::InvalidUsageBound)?,
                )?)
                .ok_or(AiError::InvalidUsageBound)?;
        }
        total
            .checked_add(other_cost_micros)
            .and_then(|value| value.checked_add(rounding_margin_micros))
            .ok_or(AiError::InvalidUsageBound)
    }

    pub fn actual_cost_micros(&self, usage: &UsageObservation) -> Result<i64, AiError> {
        let input_tokens = usage.input_tokens.ok_or(AiError::InvalidUsageBound)?;
        let output_tokens = usage.output_tokens.ok_or(AiError::InvalidUsageBound)?;
        let reasoning = self
            .reasoning_micros_per_million_tokens
            .map(|rate| {
                Self::token_cost(
                    rate,
                    usage.reasoning_tokens.ok_or(AiError::InvalidUsageBound)?,
                )
            })
            .transpose()?
            .unwrap_or(0);
        Self::token_cost(self.input_micros_per_million_tokens, input_tokens)?
            .checked_add(Self::token_cost(
                self.output_micros_per_million_tokens,
                output_tokens,
            )?)
            .and_then(|value| value.checked_add(reasoning))
            .ok_or(AiError::InvalidUsageBound)
    }
}

/// Stable identity of one model call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UsageReservationRequest {
    pub reservation_id: Uuid,
    /// SHA-256 of the exact provider payload bytes; prompt text is not stored.
    pub request_fingerprint: String,
    pub attribution: AiUsageContext,
    pub project_id: Option<ProjectId>,
    pub profile: String,
    pub provider: String,
    pub model: String,
    pub purpose: String,
    pub input_token_bound: Option<u64>,
    pub max_output_tokens: Option<u64>,
    pub max_reasoning_tokens: Option<u64>,
    pub other_cost_micros: i64,
    pub rounding_margin_micros: i64,
    pub price: UsagePriceSnapshot,
}

impl UsageReservationRequest {
    pub fn reserved_cost_micros(&self) -> Result<i64, AiError> {
        self.attribution.validate()?;
        if self.project_id.is_none() && self.attribution.workspace_id.is_none() {
            return Err(AiError::MissingUsageWorkspace);
        }
        if self.profile.trim().is_empty()
            || self.provider.trim().is_empty()
            || self.model.trim().is_empty()
            || self.purpose.trim().is_empty()
        {
            return Err(AiError::InvalidUsageBound);
        }
        if self.request_fingerprint.len() != 64
            || !self
                .request_fingerprint
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit())
        {
            return Err(AiError::InvalidUsageBound);
        }
        self.price.maximum_cost_micros(
            self.input_token_bound,
            self.max_output_tokens,
            self.max_reasoning_tokens,
            self.other_cost_micros,
            self.rounding_margin_micros,
        )
    }
}

/// Reservation persisted before provider dispatch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UsageReservation {
    pub id: Uuid,
    pub period_start: NaiveDate,
    pub reserved_cost_micros: i64,
    pub price: UsagePriceSnapshot,
}

/// Additional bounded liability requested before a call continues or streams
/// beyond its original reservation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UsageTopUpRequest {
    /// SHA-256 of the full frozen adapter request after the top-up.
    pub request_fingerprint: String,
    pub additional_input_token_bound: u64,
    pub additional_output_tokens: u64,
    pub additional_reasoning_tokens: u64,
    pub other_cost_micros: i64,
    pub rounding_margin_micros: i64,
}

impl UsageTopUpRequest {
    pub fn reserved_cost_micros(&self, price: &UsagePriceSnapshot) -> Result<i64, AiError> {
        if self.request_fingerprint.len() != 64
            || !self
                .request_fingerprint
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit())
        {
            return Err(AiError::InvalidUsageBound);
        }
        price.maximum_cost_micros(
            Some(self.additional_input_token_bound),
            Some(self.additional_output_tokens),
            price
                .reasoning_micros_per_million_tokens
                .map(|_| self.additional_reasoning_tokens),
            self.other_cost_micros,
            self.rounding_margin_micros,
        )
    }
}

/// Provider-reported token usage for one invocation.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct UsageObservation {
    /// Missing provider counters remain unknown rather than becoming zero.
    pub input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    pub reasoning_tokens: Option<u64>,
    /// Commercial meters have not been selected. This remains nullable and
    /// is never derived from provider cost.
    pub commercial_units: Option<i64>,
    pub provider_request_id: Option<String>,
    pub provider_response_id: Option<String>,
}

impl UsageObservation {
    pub fn has_token_usage(&self) -> bool {
        self.input_tokens.is_some() && self.output_tokens.is_some()
    }

    pub fn validate(&self) -> Result<(), AiError> {
        let invalid_id = |id: &Option<String>| {
            id.as_ref().is_some_and(|value| {
                value.trim().is_empty() || value.len() > 256 || value.chars().any(char::is_control)
            })
        };
        if self.commercial_units.is_some_and(|units| units < 0)
            || invalid_id(&self.provider_request_id)
            || invalid_id(&self.provider_response_id)
        {
            return Err(AiError::InvalidUsageBound);
        }
        Ok(())
    }
}

/// Evidence strong enough to release a reservation without a usage fact.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ZeroLiabilityProof {
    ProviderRejected { status: u16 },
}

impl ZeroLiabilityProof {
    pub fn validate(self) -> Result<(), AiError> {
        match self {
            Self::ProviderRejected { status } if (400..500).contains(&status) && status != 408 => {
                Ok(())
            }
            Self::ProviderRejected { .. } => Err(AiError::InvalidUsageBound),
        }
    }
}

pub trait UsageLedger: Send + Sync {
    /// Atomically reserves the conservative upper bound before dispatch.
    fn reserve<'a>(&'a self, request: UsageReservationRequest) -> AiFuture<'a, UsageReservation>;

    /// Atomically increases an open reservation before additional provider
    /// liability is incurred. Implementations must use the captured price and
    /// original period, and enforce every parent and child cap in one unit.
    fn top_up<'a>(
        &'a self,
        reservation: &'a UsageReservation,
        request: UsageTopUpRequest,
    ) -> AiFuture<'a, UsageReservation>;

    /// Saves provider evidence before final settlement, leaving a recoverable
    /// observation if the following ledger transaction fails.
    fn observe<'a>(
        &'a self,
        reservation: &'a UsageReservation,
        observation: UsageObservation,
    ) -> AiFuture<'a, ()>;

    /// Settles a previously observed invocation and commits its immutable
    /// usage fact and bucket adjustments atomically.
    fn settle<'a>(
        &'a self,
        reservation: &'a UsageReservation,
        observation: UsageObservation,
        current_price_version: &'a str,
    ) -> AiFuture<'a, ()>;

    /// Keeps a possible provider liability reserved for reconciliation.
    fn unresolved<'a>(
        &'a self,
        reservation: &'a UsageReservation,
        observation: Option<UsageObservation>,
        reason: &'a str,
    ) -> AiFuture<'a, ()>;

    /// Releases only when the caller supplies proof that the provider
    /// rejected the request before accepting work.
    fn void_zero_liability<'a>(
        &'a self,
        reservation: &'a UsageReservation,
        proof: ZeroLiabilityProof,
    ) -> AiFuture<'a, ()>;
}

/// Optional provider-side usage lookup. No provider adapter is enabled yet.
pub trait ProviderUsageLookup: Send + Sync {
    fn usage_by_request_id<'a>(
        &'a self,
        provider: &'a str,
        request_id: &'a str,
    ) -> AiFuture<'a, Option<UsageObservation>>;
}

/// Wraps the provider gateway so every model call is reserved before dispatch
/// and every result is either settled, voided with proof, or held unresolved.
pub struct MeteredGateway<G> {
    inner: G,
    ledger: Arc<dyn UsageLedger>,
    prices: Arc<PriceBook>,
}

impl<G> MeteredGateway<G> {
    pub fn new(inner: G, ledger: Arc<dyn UsageLedger>) -> Self {
        Self {
            inner,
            ledger,
            prices: Arc::new(PriceBook::default()),
        }
    }

    #[must_use]
    pub fn with_prices(mut self, prices: PriceBook) -> Self {
        self.prices = Arc::new(prices);
        self
    }

    fn reservation_request(
        &self,
        prepared: &PreparedCompletionRequest,
        reservation_id: Uuid,
    ) -> Result<UsageReservationRequest, AiError> {
        let request = &prepared.request;
        let attribution = request
            .usage_context
            .clone()
            .ok_or(AiError::MissingUsageWorkspace)?;
        let price = self
            .prices
            .price(&request.route.provider, &request.route.model)?;
        let capabilities = self
            .prices
            .capabilities(&request.route.provider, &request.route.model)?;
        let max_output_tokens = request.route.parameters.max_tokens.map(u64::from);
        if max_output_tokens
            .is_none_or(|tokens| tokens == 0 || tokens > capabilities.max_output_tokens)
        {
            return Err(AiError::UnboundedOutput);
        }
        let price = crate::UsagePriceSnapshot {
            version: self.prices.version(),
            input_micros_per_million_tokens: price.input_micros_per_million_tokens,
            output_micros_per_million_tokens: price.output_micros_per_million_tokens,
            reasoning_micros_per_million_tokens: price.reasoning_micros_per_million_tokens,
        };
        let input_token_bound =
            template_inclusive_input_bound(&prepared.serialized_body, capabilities)?;
        let request_fingerprint = crate::sha256_bytes(&prepared.serialized_body);
        Ok(UsageReservationRequest {
            reservation_id,
            request_fingerprint,
            attribution,
            project_id: request.project_id,
            profile: request.route.profile.as_str().to_owned(),
            provider: request.route.provider.clone(),
            model: request.route.model.clone(),
            purpose: "structured".to_owned(),
            input_token_bound: Some(input_token_bound),
            max_reasoning_tokens: price
                .reasoning_micros_per_million_tokens
                .map(|_| max_output_tokens.unwrap_or_default()),
            max_output_tokens,
            other_cost_micros: 0,
            rounding_margin_micros: 1,
            price,
        })
    }
}

impl<G: AiGateway> AiGateway for MeteredGateway<G> {
    fn complete<'a>(&'a self, request: CompletionRequest) -> AiFuture<'a, GatewayCompletion> {
        Box::pin(async move {
            let prepared = self.inner.prepare(request)?;
            let reservation_id = Uuid::new_v4();
            let reservation_request = self.reservation_request(&prepared, reservation_id)?;
            // Validate every bound before opening a database reservation.
            reservation_request.reserved_cost_micros()?;
            let reservation = self.ledger.reserve(reservation_request).await?;
            let mut completion = match self.inner.complete_prepared(prepared).await {
                Ok(completion) => completion,
                Err(AiError::ProviderRejected { status }) => {
                    self.ledger
                        .void_zero_liability(
                            &reservation,
                            ZeroLiabilityProof::ProviderRejected { status },
                        )
                        .await?;
                    return Err(AiError::ProviderRejected { status });
                }
                Err(AiError::ProviderRejectedSubscriptionLimit { status }) => {
                    self.ledger
                        .void_zero_liability(
                            &reservation,
                            ZeroLiabilityProof::ProviderRejected { status },
                        )
                        .await?;
                    return Err(AiError::SubscriptionLimit);
                }
                Err(error) => {
                    self.ledger
                        .unresolved(&reservation, None, "provider_outcome_ambiguous")
                        .await?;
                    return Err(error);
                }
            };
            let observation = UsageObservation {
                input_tokens: completion.input_tokens,
                output_tokens: completion.output_tokens,
                reasoning_tokens: completion.reasoning_tokens,
                commercial_units: None,
                provider_request_id: completion.provider_request_id.clone(),
                provider_response_id: completion.provider_response_id.clone(),
            };
            observe_or_hold(self.ledger.as_ref(), &reservation, observation.clone()).await?;
            if !observation.has_token_usage() {
                completion.cost_micros = None;
                return Ok(completion);
            }
            self.ledger
                .settle(&reservation, observation.clone(), &self.prices.version())
                .await?;
            completion.cost_micros = Some(reservation.price.actual_cost_micros(&observation)?);
            Ok(completion)
        })
    }
}

/// Input bound from Rig's OpenAI wire conversion, after history, tools,
/// schemas, parameters and JSON escaping have been rendered. The route's
/// published-template overhead and safety factor are applied to those exact
/// wire bytes before admission.
pub(crate) fn assistant_input_bound_tokens(
    request: &rig_core::completion::CompletionRequest,
    model: &str,
    capabilities: RouteCapabilities,
) -> Result<u64, AiError> {
    use rig_core::{
        providers::openai::{
            OpenAIConfig,
            wire::{OpenAiWire, Route},
        },
        wire::{Body, Mode, Wire},
    };

    let wire = OpenAiWire::new(
        OpenAIConfig::new("deepref-ai-usage-bound").with_route(Route::Chat),
        model.to_owned(),
    );
    let encoded = wire
        .encode(request.clone(), Mode::Unary)
        .map_err(|_| AiError::UnboundedInput)?;
    let bytes = match encoded.request.into_body() {
        Body::Bytes(bytes) => bytes,
        Body::Multipart(_) => return Err(AiError::UnboundedInput),
    };
    template_inclusive_input_bound(&bytes, capabilities)
}

/// Frozen values from the assistant dispatch request used by the ledger.
pub(crate) struct AssistantRequestBounds {
    pub turn: usize,
    pub input_token_bound: Option<u64>,
    pub max_output_tokens: Option<u64>,
    pub request_fingerprint: String,
}

/// Builds one reservation request for an assistant model turn.
pub(crate) fn assistant_reservation_request(
    attribution: AiUsageContext,
    project_id: ProjectId,
    route: &crate::ResolvedModel,
    bounds: AssistantRequestBounds,
    prices: &PriceBook,
) -> Result<UsageReservationRequest, AiError> {
    let model_price = prices.price(&route.provider, &route.model)?;
    let capabilities = prices.capabilities(&route.provider, &route.model)?;
    if bounds
        .max_output_tokens
        .is_none_or(|tokens| tokens == 0 || tokens > capabilities.max_output_tokens)
    {
        return Err(AiError::UnboundedOutput);
    }
    Ok(UsageReservationRequest {
        reservation_id: Uuid::new_v4(),
        request_fingerprint: bounds.request_fingerprint,
        attribution: AiUsageContext {
            step_id: Some(format!("turn:{}", bounds.turn)),
            ..attribution
        },
        project_id: Some(project_id),
        profile: route.profile.as_str().to_owned(),
        provider: route.provider.clone(),
        model: route.model.clone(),
        purpose: "assistant".to_owned(),
        input_token_bound: bounds.input_token_bound,
        max_output_tokens: bounds.max_output_tokens,
        max_reasoning_tokens: model_price
            .reasoning_micros_per_million_tokens
            .map(|_| bounds.max_output_tokens.unwrap_or_default()),
        other_cost_micros: 0,
        rounding_margin_micros: 1,
        price: UsagePriceSnapshot {
            version: prices.version(),
            input_micros_per_million_tokens: model_price.input_micros_per_million_tokens,
            output_micros_per_million_tokens: model_price.output_micros_per_million_tokens,
            reasoning_micros_per_million_tokens: model_price.reasoning_micros_per_million_tokens,
        },
    })
}

/// Constructs host attribution for a human or service call.
pub fn service_usage_context(run_id: Uuid, service: impl Into<String>) -> AiUsageContext {
    AiUsageContext {
        workspace_id: None,
        principal: Some(UsagePrincipal::Service(service.into())),
        run_id,
        run_budget_id: run_id,
        step_id: None,
        job_id: None,
        attempt_fence: None,
        run_limit_micros: None,
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    use super::*;

    const GLM_FIXED_TEMPLATE_PARTS: &str = concat!(
        "[gMASK]<sop><|system|>Reasoning Effort: Max",
        "<|system|>\n# Tools\n",
        "You may call one or more functions to assist with the user query.\n\n",
        "You are provided with function signatures within <tools></tools> XML tags:\n",
        "<tools>",
        "</tools>\nFor each function call, output the function name and arguments within the following XML format:\n",
        "<tool_call>{function-name}<arg_key>{arg-key-1}</arg_key><arg_value>{arg-value-1}</arg_value>",
        "<arg_key>{arg-key-2}</arg_key><arg_value>{arg-value-2}</arg_value>...</tool_call>"
    );

    #[test]
    fn template_bound_covers_system_prompt_tool_schema_and_fixed_rendered_parts() {
        let system_prompt =
            "Use the declared tool only when the supplied evidence is insufficient.";
        let tool = serde_json::json!({
            "type": "function",
            "function": {
                "name": "lookup_report",
                "description": "Find a report by title",
                "parameters": {
                    "type": "object",
                    "properties": {"title": {"type": "string"}},
                    "required": ["title"]
                }
            }
        });
        let body = serde_json::json!({
            "messages": [
                {"role": "system", "content": system_prompt},
                {"role": "user", "content": "Find the matching report."}
            ],
            "tools": [tool]
        });
        let serialized = serde_json::to_vec(&body).expect("wire body serializes");
        let capabilities = PriceBook::default()
            .capabilities("opencode-go", "glm-5.3")
            .expect("published GLM route has a template derivation");
        let bound = template_inclusive_input_bound(&serialized, capabilities)
            .expect("template-inclusive bound is finite");

        let rendered_tool = serde_json::to_string(&body["tools"][0])
            .expect("tool schema serializes for the template");
        let rendered = format!(
            "{GLM_FIXED_TEMPLATE_PARTS}{rendered_tool}\n<|system|>{system_prompt}<|user|>Find the matching report.\n<|assistant|><think></think>"
        );
        assert!(bound >= u64::try_from(rendered.len()).expect("rendered bytes fit"));
    }

    #[derive(Default)]
    struct RetryLedger {
        attempts: AtomicUsize,
        failures_before_success: AtomicUsize,
        unresolved_calls: AtomicUsize,
        held: AtomicBool,
    }

    impl RetryLedger {
        fn failing_observations(count: usize) -> Self {
            Self {
                failures_before_success: AtomicUsize::new(count),
                ..Self::default()
            }
        }
    }

    impl UsageLedger for RetryLedger {
        fn reserve<'a>(
            &'a self,
            _request: UsageReservationRequest,
        ) -> AiFuture<'a, UsageReservation> {
            Box::pin(async { Err(AiError::InvalidUsageBound) })
        }

        fn top_up<'a>(
            &'a self,
            _reservation: &'a UsageReservation,
            _request: UsageTopUpRequest,
        ) -> AiFuture<'a, UsageReservation> {
            Box::pin(async { Err(AiError::InvalidUsageBound) })
        }

        fn observe<'a>(
            &'a self,
            _reservation: &'a UsageReservation,
            _observation: UsageObservation,
        ) -> AiFuture<'a, ()> {
            let attempt = self.attempts.fetch_add(1, Ordering::SeqCst) + 1;
            let failures = self.failures_before_success.load(Ordering::SeqCst);
            Box::pin(async move {
                if attempt <= failures {
                    Err(AiError::Persistence(
                        "temporary observation failure".to_owned(),
                    ))
                } else {
                    Ok(())
                }
            })
        }

        fn settle<'a>(
            &'a self,
            _reservation: &'a UsageReservation,
            _observation: UsageObservation,
            _current_price_version: &'a str,
        ) -> AiFuture<'a, ()> {
            Box::pin(async { Err(AiError::InvalidUsageBound) })
        }

        fn unresolved<'a>(
            &'a self,
            _reservation: &'a UsageReservation,
            _observation: Option<UsageObservation>,
            _reason: &'a str,
        ) -> AiFuture<'a, ()> {
            self.unresolved_calls.fetch_add(1, Ordering::SeqCst);
            self.held.store(true, Ordering::SeqCst);
            Box::pin(async { Ok(()) })
        }

        fn void_zero_liability<'a>(
            &'a self,
            _reservation: &'a UsageReservation,
            _proof: ZeroLiabilityProof,
        ) -> AiFuture<'a, ()> {
            Box::pin(async { Err(AiError::InvalidUsageBound) })
        }
    }

    fn sample_reservation() -> UsageReservation {
        UsageReservation {
            id: Uuid::from_u128(42),
            period_start: NaiveDate::from_ymd_opt(2026, 10, 1).expect("valid date"),
            reserved_cost_micros: 100,
            price: UsagePriceSnapshot {
                version: "test".to_owned(),
                input_micros_per_million_tokens: 1,
                output_micros_per_million_tokens: 1,
                reasoning_micros_per_million_tokens: None,
            },
        }
    }

    fn sample_observation() -> UsageObservation {
        UsageObservation {
            input_tokens: Some(10),
            output_tokens: Some(2),
            reasoning_tokens: None,
            commercial_units: None,
            provider_request_id: Some("req-1".to_owned()),
            provider_response_id: Some("resp-1".to_owned()),
        }
    }

    #[tokio::test]
    async fn observation_retries_three_times_and_keeps_reservation_held_on_total_failure() {
        let reservation = sample_reservation();
        let retry_then_succeed = RetryLedger::failing_observations(2);
        observe_or_hold(&retry_then_succeed, &reservation, sample_observation())
            .await
            .expect("third observation attempt succeeds");
        assert_eq!(retry_then_succeed.attempts.load(Ordering::SeqCst), 3);
        assert_eq!(
            retry_then_succeed.unresolved_calls.load(Ordering::SeqCst),
            0
        );
        assert!(!retry_then_succeed.held.load(Ordering::SeqCst));

        let fail_all = RetryLedger::failing_observations(3);
        assert!(
            observe_or_hold(&fail_all, &reservation, sample_observation())
                .await
                .is_err()
        );
        assert_eq!(fail_all.attempts.load(Ordering::SeqCst), 3);
        assert_eq!(fail_all.unresolved_calls.load(Ordering::SeqCst), 1);
        assert!(fail_all.held.load(Ordering::SeqCst));
    }
}
