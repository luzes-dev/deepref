use sqlx::PgPool;

/// One bounded pass of the AI second reviewer. The gate, the batch cap and the
/// per-record attempt limit live in `deepref_postgres::sweep_second_reviews`;
/// this only runs it on the reconciler's tick and reports the outcome.
pub async fn sweep(pool: &PgPool) {
    if let Err(error) = deepref_postgres::sweep_ai_first(pool).await {
        tracing::error!(%error,"AI-first sweep failed");
    }
    match deepref_postgres::sweep_second_reviews(pool).await {
        Ok(report) if report.scheduled > 0 || report.released > 0 => tracing::info!(
            scheduled = report.scheduled,
            released = report.released,
            "automatic second review sweep completed"
        ),
        Ok(_) => tracing::debug!("automatic second review sweep found nothing to schedule"),
        Err(error) => tracing::error!(%error, "automatic second review sweep failed"),
    }
}
