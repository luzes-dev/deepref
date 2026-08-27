use std::sync::Arc;
use std::time::Instant;

use deepref_ai::{AiGateway, RoutedGateway};
use deepref_documents::DocumentStore;
use sqlx::PgPool;

#[derive(Clone)]
pub struct AppState {
    pub pool: PgPool,
    pub started_at: Instant,
    pub document_store: Option<DocumentStore>,
    pub ai_gateway: Arc<dyn AiGateway>,
}

impl AppState {
    pub fn new(pool: PgPool) -> Self {
        Self {
            pool,
            started_at: Instant::now(),
            document_store: None,
            ai_gateway: Arc::new(RoutedGateway::default()),
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
        self
    }

    pub fn core(pool: PgPool) -> Self {
        Self::new(pool)
    }
}
