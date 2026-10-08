use std::sync::Arc;
use std::time::Instant;

use deepref_ai::{AiGateway, ChatGateway, RoutedGateway};
use deepref_documents::DocumentStore;
use sqlx::PgPool;

/// What the API can say about the configured AI provider without exposing
/// any secret.
#[derive(Debug, Clone, Default)]
pub struct AiRuntimeInfo {
    pub configured: bool,
    pub provider: Option<String>,
    pub model: Option<String>,
}

#[derive(Clone)]
pub struct AppState {
    pub pool: PgPool,
    pub started_at: Instant,
    pub document_store: Option<DocumentStore>,
    pub ai_gateway: Arc<dyn AiGateway>,
    /// Tool-calling chat for the assistant; `None` when no provider is set.
    pub chat_gateway: Option<Arc<dyn ChatGateway>>,
    pub ai_info: AiRuntimeInfo,
}

impl AppState {
    pub fn new(pool: PgPool) -> Self {
        Self {
            pool,
            started_at: Instant::now(),
            document_store: None,
            ai_gateway: Arc::new(RoutedGateway::default()),
            chat_gateway: None,
            ai_info: AiRuntimeInfo::default(),
        }
    }

    pub fn with_document_store(mut self, document_store: DocumentStore) -> Self {
        self.document_store = Some(document_store);
        self
    }

    pub fn with_ai_gateway<G>(mut self, gateway: G) -> Self
    where
        G: AiGateway + 'static,
    {
        self.ai_gateway = Arc::new(gateway);
        self.ai_info.configured = true;
        self
    }

    pub fn with_chat_gateway<G>(mut self, gateway: G) -> Self
    where
        G: ChatGateway + 'static,
    {
        self.chat_gateway = Some(Arc::new(gateway));
        self.ai_info.configured = true;
        self
    }

    /// Installs one provider stack that serves both structured completions
    /// and assistant chat.
    pub fn with_ai_provider<G>(
        mut self,
        gateway: Arc<G>,
        provider: impl Into<String>,
        model: impl Into<String>,
    ) -> Self
    where
        G: AiGateway + ChatGateway + 'static,
    {
        self.ai_gateway = gateway.clone();
        self.chat_gateway = Some(gateway);
        self.ai_info = AiRuntimeInfo {
            configured: true,
            provider: Some(provider.into()),
            model: Some(model.into()),
        };
        self
    }

    pub fn core(pool: PgPool) -> Self {
        Self::new(pool)
    }
}
