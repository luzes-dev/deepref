use std::{
    collections::BTreeMap,
    sync::{Arc, RwLock},
};

use rig_core::{
    DynModel,
    completion::{CompletionRequest as RigCompletionRequest, Message},
    operation::{Completion, Embedding as RigEmbeddingOperation},
};
use serde_json::json;
use tracing::debug;

use crate::{
    AiError, AiFuture, ChatCompletion, ChatGateway, ChatRequest, ChatTextSink, CompletionRequest,
    Embedding, GatewayCompletion, GroundingContextBuilder, ProviderEndpoint,
    register_provider_endpoint,
};

pub trait AiGateway: Send + Sync {
    fn complete<'a>(&'a self, request: CompletionRequest) -> AiFuture<'a, GatewayCompletion>;
}

pub trait EmbeddingGateway: Send + Sync {
    fn embed<'a>(
        &'a self,
        model: &'a crate::ResolvedModel,
        text: &'a str,
    ) -> AiFuture<'a, Embedding>;
}

/// Model name that matches every model of a provider.
pub const ANY_MODEL: &str = "*";

type Adapters<T> = RwLock<BTreeMap<(String, String), Arc<T>>>;

pub struct RoutedGateway {
    adapters: Adapters<dyn AiGateway>,
    chat_adapters: Adapters<dyn ChatGateway>,
}

impl Default for RoutedGateway {
    fn default() -> Self {
        Self {
            adapters: RwLock::new(BTreeMap::new()),
            chat_adapters: RwLock::new(BTreeMap::new()),
        }
    }
}

fn lookup<T: ?Sized>(
    registry: &Adapters<T>,
    provider: &str,
    model: &str,
) -> Result<Option<Arc<T>>, AiError> {
    let adapters = registry
        .read()
        .map_err(|_| AiError::Gateway("gateway registry lock is poisoned".to_owned()))?;
    Ok(adapters
        .get(&(provider.to_owned(), model.to_owned()))
        .or_else(|| adapters.get(&(provider.to_owned(), ANY_MODEL.to_owned())))
        .cloned())
}

impl RoutedGateway {
    pub fn register(
        &self,
        provider: impl Into<String>,
        model: impl Into<String>,
        gateway: Arc<dyn AiGateway>,
    ) -> Result<(), AiError> {
        self.adapters
            .write()
            .map_err(|_| AiError::Gateway("gateway registry lock is poisoned".to_owned()))?
            .insert((provider.into(), model.into()), gateway);
        Ok(())
    }
    pub fn register_chat(
        &self,
        provider: impl Into<String>,
        model: impl Into<String>,
        gateway: Arc<dyn ChatGateway>,
    ) -> Result<(), AiError> {
        self.chat_adapters
            .write()
            .map_err(|_| AiError::Gateway("gateway registry lock is poisoned".to_owned()))?
            .insert((provider.into(), model.into()), gateway);
        Ok(())
    }
    /// Registers one provider adapter for both structured completion and chat.
    pub fn register_provider<G>(
        &self,
        provider: impl Into<String> + Clone,
        model: impl Into<String> + Clone,
        gateway: Arc<G>,
    ) -> Result<(), AiError>
    where
        G: AiGateway + ChatGateway + 'static,
    {
        self.register(provider.clone(), model.clone(), gateway.clone())?;
        self.register_chat(provider, model, gateway)
    }
    pub fn register_adapter<G>(
        &self,
        provider: impl Into<String>,
        model: impl Into<String>,
        gateway: G,
    ) -> Result<(), AiError>
    where
        G: AiGateway + 'static,
    {
        self.register(provider, model, Arc::new(gateway))
    }
}

impl AiGateway for RoutedGateway {
    fn complete<'a>(&'a self, request: CompletionRequest) -> AiFuture<'a, GatewayCompletion> {
        Box::pin(async move {
            let adapter = lookup(
                &self.adapters,
                &request.route.provider,
                &request.route.model,
            )?
            .ok_or_else(|| {
                AiError::Gateway("no adapter is registered for the resolved route".to_owned())
            })?;
            adapter.complete(request).await
        })
    }
}

impl ChatGateway for RoutedGateway {
    fn chat<'a>(&'a self, request: ChatRequest) -> AiFuture<'a, ChatCompletion> {
        Box::pin(async move {
            let adapter = lookup(
                &self.chat_adapters,
                &request.route.provider,
                &request.route.model,
            )?
            .ok_or_else(|| {
                AiError::Gateway("no chat adapter is registered for the resolved route".to_owned())
            })?;
            adapter.chat(request).await
        })
    }

    fn chat_streaming<'a>(
        &'a self,
        request: ChatRequest,
        on_text: ChatTextSink<'a>,
    ) -> AiFuture<'a, ChatCompletion> {
        Box::pin(async move {
            let adapter = lookup(
                &self.chat_adapters,
                &request.route.provider,
                &request.route.model,
            )?
            .ok_or_else(|| {
                AiError::Gateway("no chat adapter is registered for the resolved route".to_owned())
            })?;
            adapter.chat_streaming(request, on_text).await
        })
    }
}

/// Rig adapter over an erased 0.44 completion model. The DeepRef route
/// identity selects the model per request and every supported parameter is
/// forwarded into Rig's request builder. Construct the model with PR3's
/// `AgentModelFactory` (or any `impl Into<DynModel<Completion>>`); this
/// adapter only translates DeepRef requests into Rig calls.
pub struct RigGateway {
    model: DynModel<Completion>,
}

impl RigGateway {
    pub fn new(model: impl Into<DynModel<Completion>>) -> Self {
        Self {
            model: model.into(),
        }
    }
}

impl AiGateway for RigGateway {
    fn complete<'a>(&'a self, request: CompletionRequest) -> AiFuture<'a, GatewayCompletion> {
        Box::pin(async move {
            request.route.validate()?;
            let schema =
                serde_json::from_value::<schemars::Schema>(request.schema).map_err(|_| {
                    AiError::Gateway("structured schema could not be prepared".to_owned())
                })?;
            let mut prompt = request.user_prompt;
            if !request.evidence.is_empty() {
                prompt.push_str("\n\n");
                prompt.push_str(&GroundingContextBuilder::render(&request.evidence));
            }
            let mut additional = request.route.parameters.additional.clone();
            if let Some(top_p) = request.route.parameters.top_p {
                additional.insert("top_p".to_owned(), json!(top_p));
            }
            let rig_request = RigCompletionRequest::new(Message::user(prompt))
                .model(request.route.model.clone())
                .preamble(request.system_prompt)
                .temperature(request.route.parameters.temperature.map(f64::from))
                .max_tokens(request.route.parameters.max_tokens.map(u64::from))
                .additional_params((!additional.is_empty()).then(|| json!(additional)))
                .output_schema(schema);
            let response = self
                .model
                .call(rig_request)
                .await
                .map_err(|_| AiError::Gateway("provider completion failed".to_owned()))?;
            debug!(ai.provider = %request.route.provider, ai.model = %request.route.model, "structured completion finished");
            Ok(GatewayCompletion {
                output_json: response.text(),
                input_tokens: response.usage.input_tokens.unwrap_or(0),
                output_tokens: response.usage.output_tokens.unwrap_or(0),
                cost_micros: None,
                served_model: response.model().map(str::to_owned),
                system_fingerprint: None,
            })
        })
    }
}

/// Rig adapter over an erased 0.44 embedding model.
pub struct RigEmbeddingGateway {
    model: DynModel<RigEmbeddingOperation>,
}

impl RigEmbeddingGateway {
    pub fn new(model: impl Into<DynModel<RigEmbeddingOperation>>) -> Self {
        Self {
            model: model.into(),
        }
    }
}

impl EmbeddingGateway for RigEmbeddingGateway {
    fn embed<'a>(
        &'a self,
        model: &'a crate::ResolvedModel,
        text: &'a str,
    ) -> AiFuture<'a, Embedding> {
        Box::pin(async move {
            model.validate()?;
            let value = self
                .model
                .embed_text(text)
                .await
                .map_err(|_| AiError::Gateway("embedding provider failed".to_owned()))?;
            Embedding::new(value.vec.into_iter().map(|item| item as f32).collect())
        })
    }
}

/// Builds the production gateway stack for an OpenAI-compatible provider
/// (`opencode-go`, or the legacy `zai`): the provider adapter registered for
/// every model of `provider`, wrapped so the project budget is enforced, every
/// call is priced with `prices`, and usage is recorded. Routes must carry the
/// same `provider` label, because the router looks adapters up by it.
///
/// This is also the one place where the provider id and its base URL meet, so
/// the normalized endpoint of the adapter is registered here for review
/// identity (see [`register_provider_endpoint`]).
pub fn build_metered_provider(
    provider: &str,
    base_url: &str,
    api_key: &str,
    ledger: Arc<dyn crate::UsageLedger>,
    prices: crate::PriceBook,
) -> Result<Arc<crate::MeteredGateway<RoutedGateway>>, AiError> {
    let dialect = crate::ProviderDialect::from_id(provider)
        .ok_or_else(|| AiError::Gateway("AI provider is not supported".to_owned()))?;
    let endpoint = ProviderEndpoint::from_configured_url(base_url)
        .map_err(|_| AiError::Gateway("AI provider base URL is invalid".to_owned()))?;
    let routed = RoutedGateway::default();
    routed.register_provider(
        provider,
        ANY_MODEL,
        Arc::new(crate::OpenAiCompatGateway::new(dialect, base_url, api_key)?),
    )?;
    // Registered last: the endpoint is only recorded once the adapter that
    // calls it exists.
    register_provider_endpoint(provider, endpoint)?;
    Ok(Arc::new(
        crate::MeteredGateway::new(routed, ledger).with_prices(prices),
    ))
}
