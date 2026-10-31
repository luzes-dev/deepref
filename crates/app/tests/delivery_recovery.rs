use deepref_app::delivery_recovery::{
    DeliveryKind, DeliveryRef, ReplayOutcome, inspect_delivery, list_terminal_deliveries,
    replay_terminal_delivery,
};
use sqlx::PgPool;

macro_rules! ensure_eq {
    ($left:expr, $right:expr $(, $($message:tt)+)?) => {
        anyhow::ensure!($left == $right $(, $($message)+)?)
    };
}
use uuid::Uuid;

async fn database() -> Option<PgPool> {
    deepref_app::test_isolation::isolated_pool_from_env("DATABASE_URL", 16).await
}

async fn quarantined_inbox(pool: &PgPool) -> anyhow::Result<i64> {
    Ok(sqlx::query_scalar(
        "INSERT INTO integration_inbox_events
           (provider, provider_event_id, event_type, payload, processing_state, attempts,
            quarantine_reason)
         VALUES ('workos', $1, 'user.created', '{\"secret\":\"payload\"}', 'quarantined', 8,
                 'handler failed')
         RETURNING id",
    )
    .bind(Uuid::new_v4().to_string())
    .fetch_one(pool)
    .await?)
}

async fn quarantined_outbox(pool: &PgPool) -> anyhow::Result<Uuid> {
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO outbox_events
           (event_id, message_type, message_version, payload, expected_consumers,
            delivery_state, attempts, last_error)
         VALUES ($1, 'fixture.event', 1, '{\"secret\":\"payload\"}', ARRAY['consumer'],
                 'quarantined', 8, 'consumer failed')",
    )
    .bind(id)
    .execute(pool)
    .await?;
    sqlx::query(
        "INSERT INTO outbox_receipts (event_id, consumer, state, attempts)
         VALUES ($1, 'consumer', 'quarantined', 8)",
    )
    .bind(id)
    .execute(pool)
    .await?;
    Ok(id)
}

async fn audit_rows(pool: &PgPool, reference: &str) -> anyhow::Result<i64> {
    Ok(
        sqlx::query_scalar("SELECT count(*) FROM delivery_recovery_audit WHERE delivery_ref = $1")
            .bind(reference)
            .fetch_one(pool)
            .await?,
    )
}

#[tokio::test]
async fn list_inspect_and_replay_are_bounded_payload_free_and_audited() -> anyhow::Result<()> {
    let Some(pool) = database().await else {
        return Ok(());
    };
    let inbox = quarantined_inbox(&pool).await?;
    let outbox = quarantined_outbox(&pool).await?;
    let operator = Uuid::new_v4();

    let listed = list_terminal_deliveries(&pool, DeliveryKind::Inbox, None, 10).await?;
    ensure_eq!(listed.len(), 1);
    anyhow::ensure!(
        list_terminal_deliveries(&pool, DeliveryKind::Inbox, None, 101)
            .await
            .is_err()
    );
    let page =
        list_terminal_deliveries(&pool, DeliveryKind::Inbox, Some(&inbox.to_string()), 10).await?;
    anyhow::ensure!(page.is_empty(), "cursor must exclude the seen row");
    let listed = list_terminal_deliveries(&pool, DeliveryKind::Outbox, None, 10).await?;
    ensure_eq!(listed[0].quarantined_consumers, vec!["consumer".to_owned()]);
    let shown = serde_json::to_string(&listed)?;
    anyhow::ensure!(!shown.contains("payload"), "payload leaked: {shown}");

    let request = Uuid::new_v4();
    let receipt = replay_terminal_delivery(
        &pool,
        &DeliveryRef::Outbox(outbox),
        request,
        operator,
        "fixed",
    )
    .await?;
    ensure_eq!(receipt.outcome, ReplayOutcome::Replayed);
    let shown = inspect_delivery(&pool, &DeliveryRef::Outbox(outbox))
        .await?
        .unwrap();
    ensure_eq!(shown.state, "pending");
    ensure_eq!(shown.attempts, 0);
    anyhow::ensure!(shown.quarantined_consumers.is_empty());

    // Re-quarantine, then retry the same command: the request id must not
    // replay a second time.
    sqlx::query("UPDATE outbox_events SET delivery_state='quarantined' WHERE event_id=$1")
        .bind(outbox)
        .execute(&pool)
        .await?;
    let repeat = replay_terminal_delivery(
        &pool,
        &DeliveryRef::Outbox(outbox),
        request,
        operator,
        "fixed",
    )
    .await?;
    anyhow::ensure!(repeat.repeated_request);
    ensure_eq!(
        inspect_delivery(&pool, &DeliveryRef::Outbox(outbox))
            .await?
            .unwrap()
            .state,
        "quarantined"
    );
    ensure_eq!(audit_rows(&pool, &outbox.to_string()).await?, 1);
    // A request id belongs to exactly one delivery.
    anyhow::ensure!(
        replay_terminal_delivery(&pool, &DeliveryRef::Inbox(inbox), request, operator, "x")
            .await
            .is_err()
    );
    // Replaying a non-terminal delivery changes nothing but is still audited.
    let receipt = replay_terminal_delivery(
        &pool,
        &DeliveryRef::Inbox(i64::MAX),
        Uuid::new_v4(),
        operator,
        "probe",
    )
    .await?;
    ensure_eq!(receipt.outcome, ReplayOutcome::NotQuarantined);
    Ok(())
}

#[tokio::test]
async fn concurrent_replays_reopen_a_dead_letter_exactly_once() -> anyhow::Result<()> {
    let Some(pool) = database().await else {
        return Ok(());
    };
    let inbox = quarantined_inbox(&pool).await?;
    let operator = Uuid::new_v4();
    let shared = Uuid::new_v4();
    let mut tasks = Vec::new();
    for index in 0..8 {
        let pool = pool.clone();
        // Half reuse one request id (a retried command), half are distinct.
        let request = if index % 2 == 0 {
            shared
        } else {
            Uuid::new_v4()
        };
        tasks.push(tokio::spawn(async move {
            replay_terminal_delivery(&pool, &DeliveryRef::Inbox(inbox), request, operator, "race")
                .await
        }));
    }
    let mut replayed = 0;
    for task in tasks {
        let receipt = task.await??;
        if receipt.outcome == ReplayOutcome::Replayed && !receipt.repeated_request {
            replayed += 1;
        }
    }
    ensure_eq!(replayed, 1, "exactly one replay may reopen the row");
    ensure_eq!(audit_rows(&pool, &inbox.to_string()).await?, 5);
    let state: String =
        sqlx::query_scalar("SELECT processing_state FROM integration_inbox_events WHERE id=$1")
            .bind(inbox)
            .fetch_one(&pool)
            .await?;
    ensure_eq!(state, "received");
    Ok(())
}
