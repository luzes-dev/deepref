use sqlx::PgPool;

#[derive(Clone)]
pub(crate) struct AppState {
    pub(crate) pool: PgPool,
}
