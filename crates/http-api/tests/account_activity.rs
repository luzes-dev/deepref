use axum::http::{Method, StatusCode};
use uuid::Uuid;

mod support;
use support::authz::Harness;

#[tokio::test]
async fn authenticated_product_command_records_durable_activity_with_one_connection()
-> anyhow::Result<()> {
    let Some(pool) = deepref_app::test_isolation::isolated_pool_from_env("DATABASE_URL", 1).await
    else {
        return Ok(());
    };
    let h = Harness::on(pool);
    let person = h.sign_in("product-activity", "activity@example.org").await;
    let project = h.create_project(&person, "Human research").await;
    let previous: Uuid = sqlx::query_scalar(
        "UPDATE account_human_activity SET occurred_at=clock_timestamp()-interval '23 months'
         WHERE user_id=$1 RETURNING generation",
    )
    .bind(person.user.as_uuid())
    .fetch_one(&h.pool)
    .await?;

    let (status, body) = h
        .json(
            &person,
            Method::POST,
            &format!("/projects/{project}/studies"),
            r#"{"title":"A human-created study"}"#,
        )
        .await;
    anyhow::ensure!(
        status == StatusCode::CREATED,
        "genuine product command must succeed before testing evidence: {status}, {body}"
    );
    let (generation, kind, occurred_at): (Uuid, String, chrono::DateTime<chrono::Utc>) =
        sqlx::query_as(
            "SELECT generation,evidence_kind,occurred_at FROM account_human_activity WHERE user_id=$1",
        )
        .bind(person.user.as_uuid())
        .fetch_one(&h.pool)
        .await?;
    anyhow::ensure!(
        generation != previous,
        "durable authenticated product command produced no new human activity evidence"
    );
    anyhow::ensure!(
        kind == "product_command",
        "product command evidence must declare its provenance: {kind}"
    );
    let now: chrono::DateTime<chrono::Utc> = sqlx::query_scalar("SELECT clock_timestamp()")
        .fetch_one(&h.pool)
        .await?;
    anyhow::ensure!(
        occurred_at <= now && occurred_at > now - chrono::Duration::minutes(1),
        "command evidence must use actual database time: evidence={occurred_at}, now={now}"
    );
    Ok(())
}
