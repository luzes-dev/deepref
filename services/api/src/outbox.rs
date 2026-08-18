use async_nats::jetstream;
use serde::Serialize;
use sqlx::{PgPool, Postgres, Row, Transaction};
use uuid::Uuid;

use crate::error::ApiError;

pub(crate) async fn enqueue<T: Serialize>(
    tx: &mut Transaction<'_, Postgres>,
    id: Uuid,
    subject: &str,
    payload: &T,
) -> Result<(), ApiError> {
    sqlx::query(
        r#"
        INSERT INTO event_outbox (id, subject, payload)
        VALUES ($1, $2, $3)
        ON CONFLICT (id) DO NOTHING
        "#,
    )
    .bind(id)
    .bind(subject)
    .bind(serde_json::to_value(payload)?)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

pub(crate) async fn run_publisher(pool: PgPool, jetstream: jetstream::Context) {
    let mut interval = tokio::time::interval(std::time::Duration::from_secs(1));
    loop {
        interval.tick().await;
        if let Err(error) = publish_batch(&pool, &jetstream).await {
            tracing::error!(?error, "failed to publish outbox batch");
        }
    }
}

async fn publish_batch(pool: &PgPool, jetstream: &jetstream::Context) -> Result<(), ApiError> {
    let rows = sqlx::query(
        r#"
        UPDATE event_outbox
        SET locked_at = now(), attempts = attempts + 1, last_error = NULL
        WHERE id IN (
          SELECT id FROM event_outbox
          WHERE published_at IS NULL AND exhausted_at IS NULL
            AND next_attempt_at <= now()
            AND (locked_at IS NULL OR locked_at < now() - interval '30 seconds')
          ORDER BY next_attempt_at, created_at
          LIMIT 50
          FOR UPDATE SKIP LOCKED
        )
        RETURNING id, subject, payload, attempts, max_attempts
        "#,
    )
    .fetch_all(pool)
    .await?;

    for row in rows {
        let id: Uuid = row.get("id");
        let subject: String = row.get("subject");
        let payload: serde_json::Value = row.get("payload");
        let attempts: i32 = row.get("attempts");
        let max_attempts: i32 = row.get("max_attempts");
        let bytes = serde_json::to_vec(&payload)?;

        match jetstream.publish(subject.clone(), bytes.into()).await {
            Ok(ack) => match ack.await {
                Ok(_) => mark_published(pool, id).await?,
                Err(error) => {
                    mark_failed(pool, id, attempts, max_attempts, &error.to_string()).await?
                }
            },
            Err(error) => mark_failed(pool, id, attempts, max_attempts, &error.to_string()).await?,
        }
    }

    Ok(())
}

async fn mark_published(pool: &PgPool, id: Uuid) -> Result<(), ApiError> {
    sqlx::query(
        r#"
        UPDATE event_outbox
        SET published_at = now(), locked_at = NULL, last_error = NULL
        WHERE id = $1
        "#,
    )
    .bind(id)
    .execute(pool)
    .await?;
    Ok(())
}

async fn mark_failed(
    pool: &PgPool,
    id: Uuid,
    attempts: i32,
    max_attempts: i32,
    error: &str,
) -> Result<(), ApiError> {
    sqlx::query(
        r#"
        UPDATE event_outbox
        SET locked_at = NULL, last_error = $2,
            exhausted_at = CASE WHEN $3 >= $4 THEN now() ELSE exhausted_at END,
            next_attempt_at = now() + (LEAST(300, power(2, LEAST($3, 8))::int) * interval '1 second')
        WHERE id = $1
        "#,
    )
    .bind(id)
    .bind(error)
    .bind(attempts)
    .bind(max_attempts)
    .execute(pool)
    .await?;
    Ok(())
}
