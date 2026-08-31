use std::{
    collections::BTreeMap,
    sync::{Arc, RwLock},
};

use rig::{
    completion::{AssistantContent, CompletionModel, Message},
    embeddings::EmbeddingModel,
};
use serde_json::json;
use tracing::debug;

use crate::{
    AiError, AiFuture, CompletionRequest, Embedding, GatewayCompletion, GroundingContextBuilder,
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

pub struct RoutedGateway {
    adapters: RwLock<BTreeMap<(String, String), Arc<dyn AiGateway>>>,
}

impl Default for RoutedGateway {
    fn default() -> Self {
        Self {
            adapters: RwLock::new(BTreeMap::new()),
        }
    }
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
        let key = (request.route.provider.clone(), request.route.model.clone());
        let adapter = match self.adapters.read() {
            Ok(adapters) => adapters.get(&key).cloned(),
            Err(_) => {
                return Box::pin(async {
                    Err(AiError::Gateway(
                        "gateway registry lock is poisoned".to_owned(),
                    ))
                });
            }
        };
        Box::pin(async move {
            let adapter = adapter.ok_or_else(|| {
                AiError::Gateway("no adapter is registered for the resolved route".to_owned())
            })?;
            adapter.complete(request).await
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
    M::Response: Send + Sync + 'static,
    M::StreamingResponse: Send + Sync + 'static,
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
            let output_json = match response.choice.first_ref() {
                AssistantContent::Text(text) => text.text.clone(),
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
