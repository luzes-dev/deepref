use std::{
    collections::BTreeMap,
    sync::{Arc, RwLock},
};

use rig_core::{
    completion::{AssistantContent, CompletionModel, Message},
    embeddings::EmbeddingModel,
};
use serde_json::json;
use tracing::debug;

use crate::{
    AiError, AiFuture, ChatCompletion, ChatGateway, ChatRequest, ChatTextSink, CompletionRequest,
    Embedding, GatewayCompletion, GroundingContextBuilder,
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

/// Rig adapter. The DeepRef route identity selects the model per request and
/// every supported parameter is forwarded into Rig's request builder.
pub struct RigGateway<M> {
    model: M,
}
impl<M> RigGateway<M> {
    pub const fn new(model: M) -> Self {
        Self { model }
    }
}

impl<M> AiGateway for RigGateway<M>
where
    M: CompletionModel + Clone + Send + Sync + 'static,
{
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
            let response = self
                .model
                .completion_request(Message::user(prompt))
                .model(request.route.model.clone())
                .preamble(request.system_prompt)
                .temperature_opt(request.route.parameters.temperature.map(f64::from))
                .max_tokens_opt(request.route.parameters.max_tokens.map(u64::from))
                .additional_params_opt((!additional.is_empty()).then(|| json!(additional)))
                .output_schema(schema)
                .send()
                .await
                .map_err(|_| AiError::Gateway("provider completion failed".to_owned()))?;
            let output_json = match response.choice.first() {
                Some(AssistantContent::Text(text)) => text.text.clone(),
                _ => {
                    return Err(AiError::Gateway(
                        "provider did not return structured text".to_owned(),
                    ));
                }
            };
            debug!(ai.provider = %request.route.provider, ai.model = %request.route.model, "structured completion finished");
            Ok(GatewayCompletion {
                output_json,
                input_tokens: response.usage.input_tokens,
                output_tokens: response.usage.output_tokens,
                cost_micros: None,
            })
        })
    }
}

pub struct RigEmbeddingGateway<M> {
    model: M,
}
impl<M> RigEmbeddingGateway<M> {
    pub const fn new(model: M) -> Self {
        Self { model }
    }
}
impl<M> EmbeddingGateway for RigEmbeddingGateway<M>
where
    M: EmbeddingModel + Send + Sync + 'static,
{
    fn embed<'a>(
        &'a self,
        model: &'a crate::ResolvedModel,
        text: &'a str,
    ) -> AiFuture<'a, Embedding> {
        Box::pin(async move {
            model.validate()?;
            let mut values = self
                .model
                .embed_texts(vec![text.to_owned()])
                .await
                .map_err(|_| AiError::Gateway("embedding provider failed".to_owned()))?;
            let value = values.pop().ok_or_else(|| {
                AiError::Gateway("embedding provider returned no value".to_owned())
            })?;
            Embedding::new(value.vec.into_iter().map(|item| item as f32).collect())
        })
    }
}

/// Builds the production gateway stack for an OpenAI-compatible provider
/// (`opencode-go`, or the legacy `zai`): the provider adapter registered for
/// every model of `provider`, wrapped so the project budget is enforced, every
/// call is priced with `prices`, and usage is recorded. Routes must carry the
/// same `provider` label, because the router looks adapters up by it.
pub fn build_metered_provider(
    provider: &str,
    base_url: &str,
    api_key: &str,
    ledger: Arc<dyn crate::UsageLedger>,
    prices: crate::PriceBook,
) -> Result<Arc<crate::MeteredGateway<RoutedGateway>>, AiError> {
    let dialect = crate::ProviderDialect::from_id(provider)
        .ok_or_else(|| AiError::Gateway("AI provider is not supported".to_owned()))?;
    let routed = RoutedGateway::default();
    routed.register_provider(
        provider,
        ANY_MODEL,
        Arc::new(crate::OpenAiCompatGateway::new(dialect, base_url, api_key)?),
    )?;
    Ok(Arc::new(
        crate::MeteredGateway::new(routed, ledger).with_prices(prices),
    ))
}
