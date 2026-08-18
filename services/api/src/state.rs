use std::{
    sync::Arc,
    time::{Duration, Instant},
};

use async_nats::jetstream;
use deepref_graph::GraphRepository;
use sqlx::PgPool;

#[derive(Clone)]
pub struct AppState {
    pub pool: PgPool,
    pub jetstream: Option<jetstream::Context>,
    pub graph: Option<Arc<GraphRepository>>,
    pub graph_retry_after: Duration,
    pub started_at: Instant,
}

impl AppState {
    pub fn new(
        pool: PgPool,
        jetstream: Option<jetstream::Context>,
        graph: Option<Arc<GraphRepository>>,
        graph_retry_after: Duration,
    ) -> Self {
        Self {
            pool,
            jetstream,
            graph,
            graph_retry_after,
            started_at: Instant::now(),
        }
    }

    pub fn core(pool: PgPool) -> Self {
        Self::new(pool, None, None, Duration::from_secs(30))
    }
}
