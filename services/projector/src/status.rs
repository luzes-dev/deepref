use chrono::Utc;
use deepref_events::{
    DomainPayload, EntityType, EventEnvelope, ProjectionCompleted, ProjectionFailed,
    SUBJECT_PROJECTION_COMPLETED, SUBJECT_PROJECTION_FAILED,
};
use sqlx::{PgPool, Row};

pub async fn set_rebuild_state(
    pool: &PgPool,
    state: &str,
    watermark: i64,
    error: Option<&str>,
) -> anyhow::Result<()> {
    sqlx::query(
        "INSERT INTO projection_state (projection_name,project_id,state,watermark,lag,last_error,rebuild_state) \
         VALUES ('graph',NULL,$1,$2,$2,$3,$1) ON CONFLICT \
         (projection_name,(COALESCE(project_id,'00000000-0000-0000-0000-000000000000'::uuid))) \
         DO UPDATE SET state=EXCLUDED.state,watermark=EXCLUDED.watermark, \
         lag=GREATEST(0,EXCLUDED.watermark-projection_state.revision),last_error=EXCLUDED.last_error, \
         rebuild_state=EXCLUDED.rebuild_state,updated_at=now()",
    ).bind(state).bind(watermark).bind(error).execute(pool).await?;
    Ok(())
}

pub async fn record_success(
    pool: &PgPool,
    source: &EventEnvelope<DomainPayload>,
) -> anyhow::Result<()> {
    let watermark: i64 = sqlx::query_scalar("SELECT COALESCE(max(revision),0) FROM domain_events")
        .fetch_one(pool)
        .await?;
    let lag = (watermark - source.revision).max(0);
    let mut tx = pool.begin().await?;
    sqlx::query(
        "INSERT INTO projection_state (projection_name,project_id,state,revision,watermark,lag,last_success_at) \
         VALUES ('graph',NULL,'ready',$1,$2,$3,now()) ON CONFLICT \
         (projection_name,(COALESCE(project_id,'00000000-0000-0000-0000-000000000000'::uuid))) \
         DO UPDATE SET state='ready',revision=GREATEST(projection_state.revision,EXCLUDED.revision), \
         watermark=EXCLUDED.watermark,lag=GREATEST(0,EXCLUDED.watermark-GREATEST(projection_state.revision,EXCLUDED.revision)), \
         last_success_at=now(),last_error=NULL,updated_at=now()",
    ).bind(source.revision).bind(watermark).bind(lag).execute(&mut *tx).await?;
    let revision: i64 = sqlx::query_scalar("SELECT nextval('graph_domain_revision_seq')")
        .fetch_one(&mut *tx)
        .await?;
    let event = EventEnvelope::v1(
        SUBJECT_PROJECTION_COMPLETED,
        "deepref.projector",
        EntityType::Projection,
        "graph",
        revision,
        source.correlation_id,
        Some(source.event_id),
        DomainPayload::ProjectionCompleted(ProjectionCompleted {
            projection: "graph".into(),
            project_id: None,
            revision: source.revision,
            lag,
            completed_at: Utc::now(),
        }),
    );
    persist_event(&mut tx, &event, SUBJECT_PROJECTION_COMPLETED).await?;
    tx.commit().await?;
    Ok(())
}

pub async fn record_failure(
    pool: &PgPool,
    source: &EventEnvelope<DomainPayload>,
    error: &str,
) -> anyhow::Result<()> {
    let mut tx = pool.begin().await?;
    sqlx::query(
        "UPDATE projection_state SET state='failed',last_error=$1,updated_at=now() \
         WHERE projection_name='graph' AND project_id IS NULL",
    )
    .bind(error)
    .execute(&mut *tx)
    .await?;
    let revision: i64 = sqlx::query_scalar("SELECT nextval('graph_domain_revision_seq')")
        .fetch_one(&mut *tx)
        .await?;
    let event = EventEnvelope::v1(
        SUBJECT_PROJECTION_FAILED,
        "deepref.projector",
        EntityType::Projection,
        "graph",
        revision,
        source.correlation_id,
        Some(source.event_id),
        DomainPayload::ProjectionFailed(ProjectionFailed {
            projection: "graph".into(),
            project_id: None,
            revision: source.revision,
            error_code: "PROJECTION_FAILED".into(),
        }),
    );
    persist_event(&mut tx, &event, SUBJECT_PROJECTION_FAILED).await?;
    tx.commit().await?;
    Ok(())
}

async fn persist_event(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    event: &EventEnvelope<DomainPayload>,
    subject: &str,
) -> anyhow::Result<()> {
    sqlx::query(
        "INSERT INTO domain_events (event_id,schema_version,event_type,entity_type,entity_key,revision,payload,correlation_id,causation_id,created_at) \
         VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10) ON CONFLICT DO NOTHING",
    ).bind(event.event_id).bind(event.schema_version as i16).bind(&event.event_type)
     .bind(event.entity_type.as_str()).bind(&event.entity_key).bind(event.revision)
     .bind(serde_json::to_value(&event.payload)?).bind(event.correlation_id)
     .bind(event.causation_id).bind(event.occurred_at).execute(&mut **tx).await?;
    sqlx::query(
        "INSERT INTO event_outbox (id,subject,payload) VALUES ($1,$2,$3) ON CONFLICT DO NOTHING",
    )
    .bind(event.event_id)
    .bind(subject)
    .bind(serde_json::to_value(event)?)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

pub async fn snapshot(pool: &PgPool) -> anyhow::Result<serde_json::Value> {
    let row = sqlx::query(
        "SELECT state,revision,watermark,lag,last_success_at,last_error,rebuild_state \
         FROM projection_state WHERE projection_name='graph' AND project_id IS NULL",
    )
    .fetch_one(pool)
    .await?;
    Ok(serde_json::json!({
        "state": row.get::<String,_>("state"), "revision": row.get::<i64,_>("revision"),
        "watermark": row.get::<i64,_>("watermark"), "lag": row.get::<i64,_>("lag"),
        "last_success_at": row.get::<Option<chrono::DateTime<Utc>>,_>("last_success_at"),
        "last_error": row.get::<Option<String>,_>("last_error"),
        "rebuild_state": row.get::<Option<String>,_>("rebuild_state"),
    }))
}
