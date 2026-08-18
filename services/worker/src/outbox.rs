use async_nats::jetstream;
use sqlx::{PgPool, Row};
use uuid::Uuid;

pub async fn run_publisher(pool: PgPool, jetstream: jetstream::Context) {
    let mut interval = tokio::time::interval(std::time::Duration::from_secs(1));
    loop {
        interval.tick().await;
        if let Err(error) = publish_once(&pool, &jetstream).await {
            tracing::error!(%error, "failed to publish outbox batch");
        }
    }
}

pub async fn publish_once(pool: &PgPool, jetstream: &jetstream::Context) -> anyhow::Result<usize> {
    let rows = sqlx::query(
        r#"UPDATE event_outbox SET locked_at=now(), attempts=attempts+1, last_error=NULL
        WHERE id IN (SELECT id FROM event_outbox WHERE published_at IS NULL AND exhausted_at IS NULL
          AND next_attempt_at <= now() AND (locked_at IS NULL OR locked_at < now()-interval '30 seconds')
          ORDER BY next_attempt_at,created_at LIMIT 50 FOR UPDATE SKIP LOCKED)
        RETURNING id,subject,payload,attempts,max_attempts"#,
    ).fetch_all(pool).await?;
    let count = rows.len();
    for row in rows {
        let id: Uuid = row.get("id");
        let subject: String = row.get("subject");
        let payload: serde_json::Value = row.get("payload");
        let attempts: i32 = row.get("attempts");
        let max_attempts: i32 = row.get("max_attempts");
        let result = match jetstream
            .publish(subject, serde_json::to_vec(&payload)?.into())
            .await
        {
            Ok(ack) => ack.await.map(|_| ()).map_err(|error| error.to_string()),
            Err(error) => Err(error.to_string()),
        };
        match result {
            Ok(()) => {
                sqlx::query("UPDATE event_outbox SET published_at=now(),locked_at=NULL,last_error=NULL WHERE id=$1")
                .bind(id).execute(pool).await?;
            }
            Err(error) => {
                sqlx::query(
                    "UPDATE event_outbox SET locked_at=NULL,last_error=$2, \
                     exhausted_at=CASE WHEN $3 >= $4 THEN now() ELSE exhausted_at END, \
                     next_attempt_at=now()+(LEAST(300,power(2,LEAST($3,8))::int)*interval '1 second') WHERE id=$1",
                ).bind(id).bind(error).bind(attempts).bind(max_attempts).execute(pool).await?;
            }
        }
    }
    Ok(count)
}
