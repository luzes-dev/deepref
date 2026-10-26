//! The 04a consumer commits its queue effect and receipt together. Expensive
//! computation remains in the existing dedicated Projection workload pool.
use deepref_application::{
    graph::{METRICS_PROJECTION_CONSUMER, MetricsRecomputeRequested},
    outbox::{DeliveryContext, OutboxHandlerError, OutboxRegistry, TypedOutboxHandler},
};
use sqlx::PgPool;
use std::sync::Arc;

pub fn register_metrics_projection(
    pool: &PgPool,
    registry: &mut OutboxRegistry,
) -> anyhow::Result<()> {
    let pool = pool.clone();
    let handler: TypedOutboxHandler<MetricsRecomputeRequested> = Arc::new(
        move |request: MetricsRecomputeRequested, context: DeliveryContext| {
            let pool = pool.clone();
            Box::pin(async move {
                if request.revision < 0 {
                    return Err(OutboxHandlerError::Terminal(
                        "negative metrics revision".to_owned(),
                    ));
                }
                deliver(&pool, request, context)
                    .await
                    .map_err(|error| OutboxHandlerError::Transient(format!("{error:#}")))
            })
        },
    );
    registry.register::<MetricsRecomputeRequested>(&[(METRICS_PROJECTION_CONSUMER, handler)])?;
    Ok(())
}

async fn deliver(
    pool: &PgPool,
    request: MetricsRecomputeRequested,
    context: DeliveryContext,
) -> anyhow::Result<()> {
    let mut tx = pool.begin().await?;
    if crate::outbox::record_outbox_receipt(&mut tx, &context).await? {
        // A project can be deleted while its committed event is waiting.
        // Lock membership in the authoritative project before enqueueing.
        let exists = sqlx::query("SELECT id FROM projects WHERE id=$1 FOR KEY SHARE")
            .bind(request.project_id.as_uuid())
            .fetch_optional(&mut *tx)
            .await?
            .is_some();
        if exists {
            crate::jobs::enqueue_job(&mut tx, &crate::jobs::job(
                context.event_id, request.project_id, deepref_domain::JobKind::RecomputeMetrics,
                serde_json::json!({"project_id":request.project_id,"revision":request.revision}),
                format!("recompute_metrics:{}:{}",request.project_id.as_uuid(),request.revision),
            )).await?;
        }
    }
    tx.commit().await?;
    Ok(())
}
