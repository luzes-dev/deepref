//! Usage ledger port and the metering gateway that enforces the per-project
//! monthly budget before every provider call.

use std::sync::Arc;

use deepref_domain::ProjectId;
use uuid::Uuid;

use crate::{AiError, AiFuture, AiGateway, CompletionRequest, GatewayCompletion, PriceBook};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UsageEntry {
    pub project_id: Option<Uuid>,
    pub profile: String,
    pub provider: String,
    pub model: String,
    /// `structured` or `chat`.
    pub purpose: &'static str,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cost_micros: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BudgetSnapshot {
    pub budget_micros: i64,
    pub spent_micros: i64,
}

impl BudgetSnapshot {
    pub const fn remaining_micros(&self) -> i64 {
        let remaining = self.budget_micros - self.spent_micros;
        if remaining < 0 { 0 } else { remaining }
    }
    pub const fn exhausted(&self) -> bool {
        self.spent_micros >= self.budget_micros
    }
}

pub trait UsageLedger: Send + Sync {
    /// Current calendar-month (UTC) spend and the project's monthly budget.
    fn budget<'a>(&'a self, project_id: ProjectId) -> AiFuture<'a, BudgetSnapshot>;
    fn record<'a>(&'a self, entry: UsageEntry) -> AiFuture<'a, ()>;
}

/// Wraps a gateway: refuses calls once the project's month spend reaches its
/// budget, prices every call, and records it in the ledger. The budget is a
/// soft guard in USD-equivalent: a subscription has no per-token bill, so the
/// prices are the published per-token rates for each provider and model.
pub struct MeteredGateway<G> {
    inner: G,
    ledger: Arc<dyn UsageLedger>,
    prices: Arc<PriceBook>,
}

impl<G> MeteredGateway<G> {
    /// Prices calls with the built-in table until [`Self::with_prices`] is used.
    pub fn new(inner: G, ledger: Arc<dyn UsageLedger>) -> Self {
        Self {
            inner,
            ledger,
            prices: Arc::new(PriceBook::default()),
        }
    }

    /// Prices calls with the configured table, which overrides the built-in one.
    #[must_use]
    pub fn with_prices(mut self, prices: PriceBook) -> Self {
        self.prices = Arc::new(prices);
        self
    }

    async fn ensure_budget(&self, project_id: Option<ProjectId>) -> Result<(), AiError> {
        if let Some(project_id) = project_id
            && self.ledger.budget(project_id).await?.exhausted()
        {
            return Err(AiError::BudgetExceeded);
        }
        Ok(())
    }
}

impl<G: AiGateway> AiGateway for MeteredGateway<G> {
    fn complete<'a>(&'a self, request: CompletionRequest) -> AiFuture<'a, GatewayCompletion> {
        Box::pin(async move {
            self.ensure_budget(request.project_id).await?;
            let project_id = request.project_id;
            let route = request.route.clone();
            let mut completion = self.inner.complete(request).await?;
            let cost = completion.cost_micros.unwrap_or_else(|| {
                self.prices.estimate_cost_micros(
                    &route.provider,
                    &route.model,
                    completion.input_tokens,
                    completion.output_tokens,
                )
            });
            completion.cost_micros = Some(cost);
            if let Err(error) = self
                .ledger
                .record(UsageEntry {
                    project_id: project_id.map(|id| id.as_uuid()),
                    profile: route.profile.as_str().to_owned(),
                    provider: route.provider,
                    model: route.model,
                    purpose: "structured",
                    input_tokens: completion.input_tokens,
                    output_tokens: completion.output_tokens,
                    cost_micros: cost,
                })
                .await
            {
                // The provider call already happened; never discard its result
                // because bookkeeping failed.
                tracing::warn!(?error, "failed to record AI usage");
            }
            Ok(completion)
        })
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{
        Mutex,
        atomic::{AtomicUsize, Ordering},
    };

    use super::*;
    use crate::{ModelParameters, ModelPrice, ModelProfile, ResolvedModel};

    struct Ledger {
        spent: Mutex<i64>,
        budget: i64,
        recorded: Mutex<Vec<UsageEntry>>,
    }
    impl UsageLedger for Ledger {
        fn budget<'a>(&'a self, _project_id: ProjectId) -> AiFuture<'a, BudgetSnapshot> {
            Box::pin(async move {
                Ok(BudgetSnapshot {
                    budget_micros: self.budget,
                    spent_micros: *self
                        .spent
                        .lock()
                        .map_err(|_| AiError::Gateway(String::new()))?,
                })
            })
        }
        fn record<'a>(&'a self, entry: UsageEntry) -> AiFuture<'a, ()> {
            Box::pin(async move {
                *self
                    .spent
                    .lock()
                    .map_err(|_| AiError::Gateway(String::new()))? += entry.cost_micros;
                self.recorded
                    .lock()
                    .map_err(|_| AiError::Gateway(String::new()))?
                    .push(entry);
                Ok(())
            })
        }
    }

    #[derive(Default)]
    struct Inner(AtomicUsize);
    impl AiGateway for Inner {
        fn complete<'a>(&'a self, _request: CompletionRequest) -> AiFuture<'a, GatewayCompletion> {
            Box::pin(async move {
                self.0.fetch_add(1, Ordering::SeqCst);
                Ok(GatewayCompletion {
                    output_json: "{}".to_owned(),
                    input_tokens: 1_000_000,
                    output_tokens: 0,
                    cost_micros: None,
                    served_model: None,
                    system_fingerprint: None,
                })
            })
        }
    }

    fn request(project: Option<ProjectId>) -> CompletionRequest {
        CompletionRequest {
            project_id: project,
            route: ResolvedModel {
                profile: ModelProfile::Reasoning,
                provider: "opencode-go".to_owned(),
                model: "glm-5.3-flash".to_owned(),
                model_version: "glm-5.3-flash".to_owned(),
                parameters: ModelParameters::default(),
                route_id: None,
            },
            system_prompt: "s".to_owned(),
            user_prompt: "u".to_owned(),
            evidence: Vec::new(),
            schema: serde_json::json!({}),
        }
    }

    #[tokio::test]
    async fn refuses_calls_once_the_budget_is_spent() {
        let ledger = Arc::new(Ledger {
            spent: Mutex::new(0),
            budget: 200_000, // US$0.20 buys one 1M-token call at US$0.15
            recorded: Mutex::new(Vec::new()),
        });
        let gateway = MeteredGateway::new(Inner::default(), ledger.clone());
        let project = Some(ProjectId::new(Uuid::from_u128(1)));

        gateway
            .complete(request(project))
            .await
            .unwrap_or_else(|_| unreachable!());
        assert_eq!(
            *ledger.spent.lock().unwrap_or_else(|_| unreachable!()),
            150_000
        );
        gateway
            .complete(request(project))
            .await
            .unwrap_or_else(|_| unreachable!());
        // 300_000 spent >= 200_000 budget: refused before reaching the provider.
        assert_eq!(
            gateway.complete(request(project)).await,
            Err(AiError::BudgetExceeded)
        );
        assert_eq!(gateway.inner.0.load(Ordering::SeqCst), 2);
        // Workspace-level calls (no project) are recorded but never refused.
        gateway
            .complete(request(None))
            .await
            .unwrap_or_else(|_| unreachable!());
    }

    #[tokio::test]
    async fn configured_prices_price_each_provider_and_model_on_the_ledger() {
        fn empty_ledger() -> Arc<Ledger> {
            Arc::new(Ledger {
                spent: Mutex::new(0),
                budget: 5_000_000,
                recorded: Mutex::new(Vec::new()),
            })
        }

        let ledger = empty_ledger();
        let prices = PriceBook::new([(
            "opencode-go".to_owned(),
            "glm-5.3-flash".to_owned(),
            ModelPrice {
                input_micros_per_million_tokens: 2_000_000,
                output_micros_per_million_tokens: 0,
            },
        )]);
        let gateway = MeteredGateway::new(Inner::default(), ledger.clone()).with_prices(prices);
        // `Inner` reports 1M input tokens and no cost, so the ledger prices it.
        gateway
            .complete(request(None))
            .await
            .unwrap_or_else(|_| unreachable!());
        let mut legacy = request(None);
        legacy.route.provider = "zai".to_owned();
        gateway
            .complete(legacy)
            .await
            .unwrap_or_else(|_| unreachable!());
        let recorded = ledger
            .recorded
            .lock()
            .unwrap_or_else(|_| unreachable!())
            .clone();
        assert_eq!(recorded.len(), 2);
        assert_eq!(recorded[0].provider, "opencode-go");
        assert_eq!(recorded[0].cost_micros, 2_000_000);
        // Legacy rows keep their own provider and the built-in price.
        assert_eq!(recorded[1].provider, "zai");
        assert_eq!(recorded[1].cost_micros, 150_000);
    }
}
