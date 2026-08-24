use std::time::Instant;

use deepref_documents::DocumentStore;
use sqlx::PgPool;

#[derive(Clone)]
pub struct AppState {
    pub pool: PgPool,
    pub started_at: Instant,
    pub document_store: Option<DocumentStore>,
}

impl AppState {
    pub fn new(pool: PgPool) -> Self {
        Self {
            pool,
            started_at: Instant::now(),
            document_store: None,
        }
    }

    pub fn with_document_store(mut self, document_store: DocumentStore) -> Self {
        self.document_store = Some(document_store);
        self
    }

    pub fn core(pool: PgPool) -> Self {
        Self::new(pool)
    }
}
