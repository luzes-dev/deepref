//! Worker side of the visual workflow engine.
//!
//! * `handle_step` runs the workflow handler of one `durable_run_step` job:
//!   records the outcome and lets the planner queue what comes next.
//! * `run_scheduler` fires due schedules, polls publication alerts and fails
//!   steps whose job died.
//! * The autonomy gate is process-wide; the autonomy feature installs its
//!   implementation with [`set_autonomy_gate`] at start-up.

use std::{
    sync::{Arc, OnceLock},
    time::Duration,
};

use chrono::Utc;
use deepref_ai::AiGateway;
use deepref_application::{
    jobs::{ClaimedJob, JobLease},
    workflows::{AutonomyGate, DefaultAutonomyGate, PublicationQuery},
};
use deepref_app::{
    AlertHit, DueAlert, NodeOutcome, StepClaim, begin_node_execution, claim_due_workflows,
    fail_abandoned_steps, finish_node_execution, record_alert_poll, record_node_retry,
};
use serde::Deserialize;
use serde_json::{Value, json};
use sqlx::PgPool;
use tokio::sync::watch;
use uuid::Uuid;

use crate::delivery::DeliveryAction;

pub(crate) mod net;
mod nodes;
pub(crate) mod pubmed;

static AUTONOMY_GATE: OnceLock<Arc<dyn AutonomyGate>> = OnceLock::new();

/// Install the autonomy gate used by blocks that change review state. Only the
/// first call has an effect. Without it every block gets `Suggest`.
pub fn set_autonomy_gate(gate: Arc<dyn AutonomyGate>) -> bool {
    AUTONOMY_GATE.set(gate).is_ok()
}

pub(crate) fn autonomy_gate() -> Arc<dyn AutonomyGate> {
    AUTONOMY_GATE
        .get()
        .cloned()
        .unwrap_or_else(|| Arc::new(DefaultAutonomyGate))
}

/// Why a block could not finish.
#[derive(Debug, Clone)]
pub struct NodeError {
    pub message: String,
    pub retryable: bool,
}

impl NodeError {
    pub fn retryable(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            retryable: true,
        }
    }

    pub fn permanent(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            retryable: false,
        }
    }
}

impl From<sqlx::Error> for NodeError {
    fn from(_: sqlx::Error) -> Self {
        Self::retryable("The database was busy. This will be tried again.")
    }
}

impl From<deepref_app::WorkflowError> for NodeError {
    fn from(error: deepref_app::WorkflowError) -> Self {
        match error {
            deepref_app::WorkflowError::Database(_) => {
                Self::retryable("The database was busy. This will be tried again.")
            }
            other => Self::permanent(other.to_string()),
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct StepPayload {
    run_id: Uuid,
    step_id: Uuid,
    handler_kind: String,
}

fn backoff(attempt: i32) -> Duration {
    match attempt {
        ..=1 => Duration::from_secs(10),
        2 => Duration::from_secs(60),
        _ => Duration::from_secs(300),
    }
}

/// Execute one block of a workflow run under the job claim `lease`. The step
/// is fenced on that claim: a second execution under the same claim is
/// skipped, and an outcome from a claim that lost the step is dropped.
pub async fn handle_step(
    pool: &PgPool,
    job: &ClaimedJob,
    lease: &JobLease,
    ai_gateway: Option<Arc<dyn AiGateway>>,
    runtime: &deepref_config::RuntimeConfig,
) -> anyhow::Result<DeliveryAction> {
    let payload: StepPayload = serde_json::from_value(job.payload.clone())?;
    anyhow::ensure!(payload.handler_kind == "workflow", "wrong durable workflow handler");
    let identity: Option<Uuid> = sqlx::query_scalar(
        "SELECT run_id FROM durable_steps WHERE id=$1 AND handler_kind='workflow'",
    ).bind(payload.step_id).fetch_optional(pool).await?;
    anyhow::ensure!(identity == Some(payload.run_id), "durable workflow step/run mismatch");
    let execution = match begin_node_execution(pool, payload.step_id, lease).await? {
        StepClaim::Stale => return Ok(DeliveryAction::Ack),
        StepClaim::Execute(execution) => *execution,
    };
    let graph:Value=sqlx::query_scalar("SELECT graph FROM workflow_run_details WHERE id=$1")
        .bind(payload.run_id).fetch_one(pool).await?;
    let graph:deepref_application::workflows::WorkflowGraph=serde_json::from_value(graph)?;
    if !deepref_application::workflows::catalog::graph_is_available(&graph,runtime.workflow.niche_nodes_enabled) {
        finish_node_execution(pool,payload.step_id,lease,NodeOutcome::Failed{
            message:"Niche workflow nodes are disabled.".to_owned(),
        }).await?;
        return Ok(DeliveryAction::Terminate);
    }
    let context = nodes::Context {
        pool,
        ai: ai_gateway.as_deref(),
        execution: &execution,
        runtime,
    };
    let finished = match nodes::execute(&context).await {
        Ok(outcome) => finish_node_execution(pool, payload.step_id, lease, outcome).await?,
        Err(error) if error.retryable && job.attempts < job.max_attempts => {
            record_node_retry(pool, payload.step_id, lease, &error.message).await?;
            return Ok(DeliveryAction::Nak(backoff(job.attempts)));
        }
        Err(error) => {
            finish_node_execution(
                pool,
                payload.step_id,
                lease,
                NodeOutcome::Failed {
                    message: error.message,
                },
            )
            .await?
        }
    };
    if !finished {
        tracing::warn!(
            job_id = %job.id,
            node_run_id = %payload.step_id,
            attempt = lease.attempt,
            "workflow step outcome dropped because a newer claim owns the step"
        );
    }
    Ok(DeliveryAction::Ack)
}

/// Background loop for schedules, publication alerts and orphaned steps.
pub async fn run_scheduler(pool: PgPool, interval: Duration, mut shutdown: watch::Receiver<bool>) {
    let mut ticker = tokio::time::interval(interval);
    loop {
        tokio::select! {
            _ = ticker.tick() => {}
            changed = shutdown.changed() => {
                if changed.is_err() || *shutdown.borrow() {
                    return;
                }
                continue;
            }
        }
        if let Err(error) = scheduler_tick(&pool).await {
            tracing::error!(%error, "workflow scheduler tick failed");
        }
    }
}

pub async fn scheduler_tick(pool: &PgPool) -> anyhow::Result<()> {
    let (fired, alerts) = claim_due_workflows(pool, Utc::now(), 20).await?;
    if fired > 0 {
        tracing::info!(fired, "scheduled workflows started");
    }
    for alert in alerts {
        poll_alert(pool, alert).await;
    }
    let reaped = fail_abandoned_steps(pool).await?;
    if reaped > 0 {
        tracing::warn!(reaped, "workflow steps failed after their job died");
    }
    Ok(())
}

async fn poll_alert(pool: &PgPool, alert: DueAlert) {
    let state = alert.poll_state.clone();
    let result = poll_alert_sources(pool, &alert).await;
    let (hits, error) = match result {
        Ok(hits) => (hits, None),
        Err(error) => (Vec::new(), Some(error.message)),
    };
    if let Err(error) = record_alert_poll(pool, &alert, hits, error.as_deref(), state).await {
        tracing::error!(%error, workflow_id=%alert.workflow_id, "could not record the alert poll");
    }
}

async fn poll_alert_sources(pool: &PgPool, alert: &DueAlert) -> Result<Vec<AlertHit>, NodeError> {
    let config = alert
        .graph
        .trigger_node()
        .map(|node| node.config.clone())
        .unwrap_or_else(|| json!({}));
    let query = PublicationQuery::from_config(config.get("query"));
    let terms = query.pubmed_terms();
    if terms.is_empty() {
        return Err(NodeError::permanent("The search words are empty."));
    }
    let sources = config
        .get("sources")
        .and_then(Value::as_str)
        .unwrap_or("both");
    let last_polled = alert
        .poll_state
        .get("last_polled_at")
        .and_then(Value::as_str)
        .and_then(|text| chrono::DateTime::parse_from_rfc3339(text).ok())
        .map(|date| date.with_timezone(&Utc));
    let now = Utc::now();
    // Overlap by a day so nothing indexed late is missed; the seen-set removes
    // the repeats.
    let since = last_polled.map_or_else(
        || now - chrono::Duration::days(2),
        |date| date - chrono::Duration::days(1),
    );
    let days = u32::try_from((now - since).num_days().clamp(1, 60)).unwrap_or(1);
    let mailto = crate::store::load_runtime_settings(pool)
        .await
        .map(|settings| settings.crossref_mailto)
        .unwrap_or_default();
    let client = net::trusted_client()?;
    let pubmed_client = deepref_integrations::PubmedClient::new(client.clone(), mailto.clone());
    let mut items: Vec<Value> = Vec::new();
    let mut failures: Vec<NodeError> = Vec::new();
    if matches!(sources, "both" | "pubmed") {
        match pubmed::search_pubmed(pool, &pubmed_client, &terms, days).await {
            Ok(found) => items.extend(found),
            Err(error) => failures.push(error),
        }
    }
    if matches!(sources, "both" | "crossref") {
        match deepref_integrations::search_crossref(
            &client,
            &query.crossref_terms(),
            &since.format("%Y-%m-%d").to_string(),
            &mailto,
        )
        .await
        {
            Ok(found) => items.extend(found),
            Err(error) => failures.push(match error {
                deepref_integrations::CrossrefSearchError::Retryable(message) => NodeError::retryable(message),
                deepref_integrations::CrossrefSearchError::Permanent(message) => NodeError::permanent(message),
            }),
        }
    }
    if items.is_empty()
        && let Some(error) = failures.into_iter().next()
    {
        return Err(error);
    }
    let mut hits: Vec<AlertHit> = Vec::new();
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    for item in items {
        let doi = item
            .get("doi")
            .and_then(Value::as_str)
            .map(str::to_lowercase);
        let pmid = item.get("pmid").and_then(Value::as_str).map(str::to_owned);
        let identifier = match (&doi, &pmid) {
            (Some(doi), _) => format!("doi:{doi}"),
            (None, Some(pmid)) => format!("pmid:{pmid}"),
            _ => continue,
        };
        if seen.insert(identifier.clone()) {
            hits.push(AlertHit { identifier, item });
        }
    }
    Ok(hits)
}
