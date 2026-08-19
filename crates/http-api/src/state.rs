use std::time::Instant;

use sqlx::PgPool;

#[derive(Clone)]
pub struct AppState {
    pub pool: PgPool,
    pub started_at: Instant,
}

impl AppState {
    pub fn new(pool: PgPool) -> Self {
        Self {
            pool,
            started_at: Instant::now(),
        }
    }

    pub fn core(pool: PgPool) -> Self {
        Self::new(pool)
    }
}
