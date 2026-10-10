use std::sync::Arc;
use std::time::Instant;

use deepref_ai::{AiGateway, RoutedGateway};
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
    pub ai_info: AiRuntimeInfo,
}

impl AppState {
    pub fn new(pool: PgPool) -> Self {
        Self {
            pool,
            started_at: Instant::now(),
            document_store: None,
            ai_gateway: Arc::new(RoutedGateway::default()),
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

    /// Installs the structured-completion provider stack. Assistant turns run
    /// durably in the worker; nothing serves chat gateways anymore.
    pub fn with_ai_provider<G>(
        mut self,
        gateway: Arc<G>,
        provider: impl Into<String>,
        model: impl Into<String>,
    ) -> Self
    where
        G: AiGateway + 'static,
    {
        self.ai_gateway = gateway;
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
