#![allow(clippy::expect_used, clippy::unwrap_used)]
//! Operator reconciliation must expose liabilities after tenant deletion.

use deepref_http_api::{ai_usage_reconcile, config::ApiConfig};
use serde_json::{Value, json};
use uuid::Uuid;

#[tokio::test]
async fn reconciliation_report_includes_deleted_tenant_nonterminal_liabilities() {
    let Some(url) = deepref_app::test_isolation::isolated_url_from_env("DATABASE_URL").await else {
        return;
    };
    let pool = sqlx::PgPool::connect(&url).await.unwrap();
    let config = ApiConfig::from_source(&|name| match name {
        "APP_ENV" => Some("development".into()),
        "DATABASE_URL" => Some(url.clone()),
        _ => None,
    })
    .unwrap();
    let deleted = Uuid::new_v4();
    let terminal_only = Uuid::new_v4();
    for (workspace, state) in [
        (deleted, "open"),
        (deleted, "observed"),
        (deleted, "unresolved"),
        (deleted, "settled"),
        (terminal_only, "settled"),
    ] {
        sqlx::query(
            "INSERT INTO ai_usage_reservations
               (id,workspace_id,run_id,run_budget_id,job_id,attempt_fence,profile,provider,model,purpose,
                input_token_bound,max_output_tokens,input_micros_per_million_tokens,output_micros_per_million_tokens,
                price_version,request_fingerprint,reserved_cost_micros,period_start,state,actual_cost_micros,settled_at)
             VALUES($1,$2,$3,$3,$4,1,'reasoning','test-provider','test-model','structured',
                    0,0,0,0,'test-price',repeat('a',64),0,current_date,$5,
                    CASE WHEN $5='settled' THEN 0 END,CASE WHEN $5='settled' THEN now() END)",
        ).bind(Uuid::new_v4()).bind(workspace).bind(Uuid::new_v4()).bind(Uuid::new_v4()).bind(state)
            .execute(&pool).await.unwrap();
    }
    let expected =
        json!({"workspace_id":deleted,"open":1,"observed":1,"unresolved":1,"stale_fenced":3});
    let all: Vec<Value> =
        serde_json::from_str(&ai_usage_reconcile(&config, None).await.unwrap()).unwrap();
    assert!(
        all.contains(&expected),
        "unfiltered report exposes deleted tenant liabilities: {all:?}"
    );
    assert!(
        !all.iter()
            .any(|r| r["workspace_id"] == terminal_only.to_string()),
        "terminal history alone is excluded"
    );
    let filtered: Vec<Value> =
        serde_json::from_str(&ai_usage_reconcile(&config, Some(deleted)).await.unwrap()).unwrap();
    assert_eq!(filtered, vec![expected]);
    let empty: Vec<Value> = serde_json::from_str(
        &ai_usage_reconcile(&config, Some(Uuid::new_v4()))
            .await
            .unwrap(),
    )
    .unwrap();
    assert!(empty.is_empty());
    let states: Vec<String> = sqlx::query_scalar(
        "SELECT state FROM ai_usage_reservations WHERE workspace_id=$1 ORDER BY state",
    )
    .bind(deleted)
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(
        states,
        ["observed", "open", "settled", "unresolved"],
        "the report remains read-only"
    );
}
