//! Provider-neutral reservation and settlement contracts.
//!
//! PostgreSQL owns quota correctness. This crate computes a conservative
//! per-invocation bound from a captured price snapshot and requires the host
//! ledger to reserve it before the provider sees the request.

use std::sync::Arc;

use chrono::NaiveDate;
use deepref_domain::ProjectId;
use uuid::Uuid;

use crate::{
    AiError, AiFuture, AiGateway, AiUsageContext, CompletionRequest, GatewayCompletion, PriceBook,
    UsagePrincipal,
};

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
        let reasoning = self
            .reasoning_micros_per_million_tokens
            .map(|rate| Self::token_cost(rate, usage.reasoning_tokens))
            .transpose()?
            .unwrap_or(0);
        Self::token_cost(self.input_micros_per_million_tokens, usage.input_tokens)?
            .checked_add(Self::token_cost(
                self.output_micros_per_million_tokens,
                usage.output_tokens,
            )?)
            .and_then(|value| value.checked_add(reasoning))
            .ok_or(AiError::InvalidUsageBound)
    }
}

/// Stable identity of one model call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UsageReservationRequest {
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
    pub additional_input_token_bound: u64,
    pub additional_output_tokens: u64,
    pub additional_reasoning_tokens: u64,
    pub other_cost_micros: i64,
    pub rounding_margin_micros: i64,
}

impl UsageTopUpRequest {
    pub fn reserved_cost_micros(&self, price: &UsagePriceSnapshot) -> Result<i64, AiError> {
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
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub reasoning_tokens: u64,
    /// Commercial meters have not been selected. This remains nullable and
    /// is never derived from provider cost.
    pub commercial_units: Option<i64>,
}

impl UsageObservation {
    pub fn validate(&self) -> Result<(), AiError> {
        if self.commercial_units.is_some_and(|units| units < 0) {
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
    /// usage fact and provider-neutral outbox event atomically.
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

    fn input_bound_tokens(request: &CompletionRequest) -> Result<u64, AiError> {
        let value = serde_json::json!({
            "system_prompt": request.system_prompt,
            "user_prompt": request.user_prompt,
            "evidence": request.evidence,
            "schema": request.schema,
        });
        let bytes = serde_json::to_vec(&value).map_err(|_| AiError::UnboundedInput)?;
        let bytes = u64::try_from(bytes.len()).map_err(|_| AiError::UnboundedInput)?;
        // Byte-level tokenizers cannot produce more tokens than input bytes.
        // The fixed envelope covers message roles and provider serialization.
        bytes.checked_add(512).ok_or(AiError::UnboundedInput)
    }

    fn reservation_request(
        &self,
        request: &CompletionRequest,
    ) -> Result<UsageReservationRequest, AiError> {
        let attribution = request
            .usage_context
            .clone()
            .ok_or(AiError::MissingUsageWorkspace)?;
        let price = self
            .prices
            .price(&request.route.provider, &request.route.model);
        let max_output_tokens = request.route.parameters.max_tokens.map(u64::from);
        let price = crate::UsagePriceSnapshot {
            version: self.prices.version(),
            input_micros_per_million_tokens: price.input_micros_per_million_tokens,
            output_micros_per_million_tokens: price.output_micros_per_million_tokens,
            reasoning_micros_per_million_tokens: price.reasoning_micros_per_million_tokens,
        };
        Ok(UsageReservationRequest {
            attribution,
            project_id: request.project_id,
            profile: request.route.profile.as_str().to_owned(),
            provider: request.route.provider.clone(),
            model: request.route.model.clone(),
            purpose: "structured".to_owned(),
            input_token_bound: Some(Self::input_bound_tokens(request)?),
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
            let reservation_request = self.reservation_request(&request)?;
            // Validate every bound before opening a database reservation.
            reservation_request.reserved_cost_micros()?;
            let reservation = self.ledger.reserve(reservation_request).await?;
            let mut completion = match self.inner.complete(request).await {
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
            };
            self.ledger
                .observe(&reservation, observation.clone())
                .await?;
            self.ledger
                .settle(&reservation, observation, &self.prices.version())
                .await?;
            completion.cost_micros =
                Some(reservation.price.actual_cost_micros(&UsageObservation {
                    input_tokens: completion.input_tokens,
                    output_tokens: completion.output_tokens,
                    reasoning_tokens: completion.reasoning_tokens,
                    commercial_units: None,
                })?);
            Ok(completion)
        })
    }
}

/// Input token bound for a Rig assistant completion. The prompt, history,
/// preamble and full host-owned tool declarations are all known before the
/// model call; serialization failure refuses the call.
pub(crate) fn assistant_input_bound_tokens(
    system_prompt: &str,
    prompt: &rig_core::completion::Message,
    history: &[rig_core::completion::Message],
    tools: &[crate::ToolDeclaration],
) -> Result<u64, AiError> {
    let wire_shape = serde_json::json!({
        "system_prompt": system_prompt,
        "prompt": prompt,
        "history": history,
        "tools": tools,
    });
    let bytes = serde_json::to_vec(&wire_shape).map_err(|_| AiError::UnboundedInput)?;
    u64::try_from(bytes.len())
        .ok()
        .and_then(|bytes| bytes.checked_add(1024))
        .ok_or(AiError::UnboundedInput)
}

/// Builds one reservation request for an assistant model turn.
pub(crate) fn assistant_reservation_request(
    attribution: AiUsageContext,
    project_id: ProjectId,
    route: &crate::ResolvedModel,
    turn: usize,
    input_token_bound: Option<u64>,
    max_output_tokens: Option<u64>,
    prices: &PriceBook,
) -> UsageReservationRequest {
    let model_price = prices.price(&route.provider, &route.model);
    UsageReservationRequest {
        attribution: AiUsageContext {
            step_id: Some(format!("turn:{turn}")),
            ..attribution
        },
        project_id: Some(project_id),
        profile: route.profile.as_str().to_owned(),
        provider: route.provider.clone(),
        model: route.model.clone(),
        purpose: "assistant".to_owned(),
        input_token_bound,
        max_output_tokens,
        max_reasoning_tokens: model_price
            .reasoning_micros_per_million_tokens
            .map(|_| max_output_tokens.unwrap_or_default()),
        other_cost_micros: 0,
        rounding_margin_micros: 1,
        price: UsagePriceSnapshot {
            version: prices.version(),
            input_micros_per_million_tokens: model_price.input_micros_per_million_tokens,
            output_micros_per_million_tokens: model_price.output_micros_per_million_tokens,
            reasoning_micros_per_million_tokens: model_price.reasoning_micros_per_million_tokens,
        },
    }
}

/// Constructs host attribution for a human or service call.
pub fn service_usage_context(run_id: Uuid, service: impl Into<String>) -> AiUsageContext {
    AiUsageContext {
        workspace_id: None,
        principal: Some(UsagePrincipal::Service(service.into())),
        run_id,
        step_id: None,
        job_id: None,
        attempt_fence: None,
        run_limit_micros: None,
    }
}
