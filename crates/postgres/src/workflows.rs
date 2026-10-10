//! Persistence for the visual workflow engine: definitions and versions, runs
//! and node executions, schedule / alert bookkeeping, and the data queries the
//! workflow blocks read from.

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use deepref_application::AutomationDomainEvent;
use deepref_application::workflows::{
    InputSource, NodeRunSnapshot, NodeRunState, ValidationContext, ValidationReport, WorkflowGraph,
    WorkflowStatus, WorkflowTrigger, email_is_configured, extract_secrets, generate_secret,
    generate_token, input_sources, next_fire_after, node_type, plan_run, trigger_of_graph,
    validate_graph,
};
use deepref_domain::{Actor, ProjectId};
use serde::Serialize;
use serde_json::{Map, Value, json};
use sqlx::{Acquire, PgPool, Postgres, Row, Transaction, postgres::PgRow};
use thiserror::Error;
use uuid::Uuid;

/// Largest trigger payload kept on a run.
pub const MAX_TRIGGER_DATA_BYTES: usize = 256 * 1024;
/// Largest output a single block may pass to the next one.
pub const MAX_NODE_OUTPUT_BYTES: usize = 4 * 1024 * 1024;
/// Input / output snapshots longer than this are not returned in run logs.
pub const NODE_SNAPSHOT_LIMIT_BYTES: i64 = 32 * 1024;
/// Attempts per block execution (the first try plus retries).
pub const NODE_MAX_ATTEMPTS: i32 = 4;
pub const MAX_WORKFLOW_RUN_LIST_LIMIT: i64 = 100;
pub const WORKFLOW_STEP_JOB_KIND: &str = "workflow_step";

#[derive(Debug, Error)]
pub enum WorkflowError {
    #[error("workflow database operation failed")]
    Database(#[from] sqlx::Error),
    #[error("workflow JSON value is invalid: {0}")]
    Serialization(#[from] serde_json::Error),
    #[error("the workflow was not found")]
    NotFound,
    #[error("the run was not found")]
    RunNotFound,
    #[error("{0}")]
    Conflict(String),
    #[error("the workflow has problems that must be fixed before it can be published")]
    Invalid(ValidationReport),
    #[error("{0}")]
    InvalidInput(String),
    #[error("the workflow has not been published yet")]
    NotPublished,
    #[error("the run has already finished")]
    RunFinished,
    #[error("stored workflow value is invalid: {0}")]
    InvalidStoredValue(String),
}

fn project_uuid(project_id: ProjectId) -> Uuid {
    project_id.as_uuid()
}

// ---------------------------------------------------------------------------
// Records

#[derive(Debug, Clone)]
pub struct WorkflowRecord {
    pub id: Uuid,
    pub project_id: Uuid,
    pub name: String,
    pub description: String,
    pub status: WorkflowStatus,
    pub draft_graph: WorkflowGraph,
    pub draft_revision: i64,
    pub published_version_id: Option<Uuid>,
    pub published_version: Option<i32>,
    pub trigger_kind: Option<String>,
    pub has_unpublished_changes: bool,
    pub webhook_signature_required: bool,
    pub next_fire_at: Option<DateTime<Utc>>,
    pub last_fired_at: Option<DateTime<Utc>>,
    pub poll_state: Value,
    pub created_by_kind: String,
    pub created_by_id: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub struct WorkflowVersionRecord {
    pub id: Uuid,
    pub workflow_id: Uuid,
    pub version: i32,
    pub graph: WorkflowGraph,
    pub trigger_kind: String,
    pub note: Option<String>,
    pub published_by_kind: String,
    pub published_by_id: String,
    pub published_at: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub struct WebhookConfig {
    pub token: String,
    pub secret: String,
    pub signature_required: bool,
    pub email_token: String,
}

/// What a public endpoint needs to know about the workflow behind a token.
#[derive(Debug, Clone)]
pub struct EndpointTarget {
    pub workflow_id: Uuid,
    pub project_id: Uuid,
    pub secret: String,
    pub signature_required: bool,
    pub enabled: bool,
    pub version_id: Option<Uuid>,
    pub trigger_kind: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct NodeRunRecord {
    pub id: Uuid,
    pub node_id: String,
    pub node_type: String,
    pub iteration: String,
    pub status: String,
    pub attempts: i32,
    pub input: Option<Value>,
    pub output: Option<Value>,
    pub input_preview: Option<String>,
    pub output_preview: Option<String>,
    pub note: Option<String>,
    pub error: Option<String>,
    pub created_at: DateTime<Utc>,
    pub started_at: Option<DateTime<Utc>>,
    pub finished_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone)]
pub struct WorkflowRunRecord {
    pub id: Uuid,
    pub project_id: Uuid,
    pub workflow_id: Uuid,
    pub workflow_name: String,
    pub version: Option<i32>,
    pub trigger_kind: String,
    pub status: String,
    pub test_mode: bool,
    pub actor_kind: String,
    pub actor_id: String,
    pub error: Option<String>,
    pub created_at: DateTime<Utc>,
    pub started_at: Option<DateTime<Utc>>,
    pub finished_at: Option<DateTime<Utc>>,
    pub trigger_data: Value,
    pub graph: WorkflowGraph,
    pub nodes: Vec<NodeRunRecord>,
}

const WORKFLOW_SELECT: &str = "SELECT w.id, w.project_id, w.name, w.description, w.status,
        w.draft_graph, w.draft_revision, w.published_version_id,
        v.version AS published_version, w.trigger_kind,
        (v.graph IS DISTINCT FROM w.draft_graph) AS has_unpublished_changes,
        w.webhook_signature_required, w.next_fire_at, w.last_fired_at, w.poll_state,
        w.created_by_kind, w.created_by_id, w.created_at, w.updated_at
     FROM workflows w
     LEFT JOIN workflow_versions v ON v.id = w.published_version_id";

fn parse_graph(value: Value) -> Result<WorkflowGraph, WorkflowError> {
    serde_json::from_value(value)
        .map_err(|error| WorkflowError::InvalidStoredValue(error.to_string()))
}

fn workflow_from_row(row: &PgRow) -> Result<WorkflowRecord, WorkflowError> {
    let status: String = row.get("status");
    let has_unpublished: Option<bool> = row.get("has_unpublished_changes");
    Ok(WorkflowRecord {
        id: row.get("id"),
        project_id: row.get("project_id"),
        name: row.get("name"),
        description: row.get("description"),
        status: WorkflowStatus::parse(&status).ok_or_else(|| {
            WorkflowError::InvalidStoredValue(format!("workflow status {status}"))
        })?,
        draft_graph: parse_graph(row.get("draft_graph"))?,
        draft_revision: row.get("draft_revision"),
        published_version_id: row.get("published_version_id"),
        published_version: row.get("published_version"),
        trigger_kind: row.get("trigger_kind"),
        has_unpublished_changes: has_unpublished.unwrap_or(true),
        webhook_signature_required: row.get("webhook_signature_required"),
        next_fire_at: row.get("next_fire_at"),
        last_fired_at: row.get("last_fired_at"),
        poll_state: row.get("poll_state"),
        created_by_kind: row.get("created_by_kind"),
        created_by_id: row.get("created_by_id"),
        created_at: row.get("created_at"),
        updated_at: row.get("updated_at"),
    })
}

fn version_from_row(row: &PgRow) -> Result<WorkflowVersionRecord, WorkflowError> {
    Ok(WorkflowVersionRecord {
        id: row.get("id"),
        workflow_id: row.get("workflow_id"),
        version: row.get("version"),
        graph: parse_graph(row.get("graph"))?,
        trigger_kind: row.get("trigger_kind"),
        note: row.get("note"),
        published_by_kind: row.get("published_by_kind"),
        published_by_id: row.get("published_by_id"),
        published_at: row.get("published_at"),
    })
}

fn is_unique_violation(error: &sqlx::Error) -> bool {
    error
        .as_database_error()
        .and_then(|database| database.code())
        .is_some_and(|code| code == "23505")
}

// ---------------------------------------------------------------------------
// Definitions

pub async fn create_workflow(
    pool: &PgPool,
    project_id: ProjectId,
    name: &str,
    description: &str,
    graph: WorkflowGraph,
    actor: &Actor,
) -> Result<WorkflowRecord, WorkflowError> {
    let name = name.trim();
    if name.is_empty() || name.len() > 200 {
        return Err(WorkflowError::InvalidInput(
            "The name must have between 1 and 200 characters.".to_owned(),
        ));
    }
    let mut graph = graph;
    let secrets = extract_secrets(&mut graph);
    let id = Uuid::new_v4();
    let mut tx = pool.begin().await?;
    let inserted = sqlx::query(
        "INSERT INTO workflows
           (id, project_id, name, description, draft_graph, webhook_token, webhook_secret,
            email_token, created_by_kind, created_by_id)
         VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10)",
    )
    .bind(id)
    .bind(project_uuid(project_id))
    .bind(name)
    .bind(description)
    .bind(serde_json::to_value(&graph)?)
    .bind(generate_token())
    .bind(generate_secret())
    .bind(generate_token())
    .bind(actor.kind().as_str())
    .bind(actor.id())
    .execute(&mut *tx)
    .await;
    match inserted {
        Ok(_) => {}
        Err(error) if is_unique_violation(&error) => {
            return Err(WorkflowError::Conflict(
                "Another automation in this project already has that name.".to_owned(),
            ));
        }
        Err(error) => return Err(error.into()),
    }
    write_secrets(&mut tx, id, &secrets).await?;
    tx.commit().await?;
    get_workflow(pool, project_id, id).await
}

async fn write_secrets(
    tx: &mut Transaction<'_, Postgres>,
    workflow_id: Uuid,
    secrets: &[deepref_application::workflows::SecretWrite],
) -> Result<(), sqlx::Error> {
    for secret in secrets {
        match &secret.value {
            Some(value) => {
                sqlx::query(
                    "INSERT INTO workflow_secrets (workflow_id,node_id,key,value)
                     VALUES ($1,$2,$3,$4)
                     ON CONFLICT (workflow_id,node_id,key)
                     DO UPDATE SET value=EXCLUDED.value, updated_at=now()",
                )
                .bind(workflow_id)
                .bind(&secret.node_id)
                .bind(&secret.key)
                .bind(value)
                .execute(&mut **tx)
                .await?;
            }
            None => {
                sqlx::query(
                    "DELETE FROM workflow_secrets WHERE workflow_id=$1 AND node_id=$2 AND key=$3",
                )
                .bind(workflow_id)
                .bind(&secret.node_id)
                .bind(&secret.key)
                .execute(&mut **tx)
                .await?;
            }
        }
    }
    // Drop secrets of blocks that no longer exist.
    Ok(())
}

pub async fn list_workflows(
    pool: &PgPool,
    project_id: ProjectId,
) -> Result<Vec<WorkflowRecord>, WorkflowError> {
    let rows = sqlx::query(sqlx::AssertSqlSafe(format!(
        "{WORKFLOW_SELECT} WHERE w.project_id=$1 ORDER BY w.updated_at DESC, w.id"
    )))
    .bind(project_uuid(project_id))
    .fetch_all(pool)
    .await?;
    rows.iter().map(workflow_from_row).collect()
}

pub async fn get_workflow(
    pool: &PgPool,
    project_id: ProjectId,
    workflow_id: Uuid,
) -> Result<WorkflowRecord, WorkflowError> {
    let row = sqlx::query(sqlx::AssertSqlSafe(format!(
        "{WORKFLOW_SELECT} WHERE w.project_id=$1 AND w.id=$2"
    )))
    .bind(project_uuid(project_id))
    .bind(workflow_id)
    .fetch_optional(pool)
    .await?
    .ok_or(WorkflowError::NotFound)?;
    workflow_from_row(&row)
}

#[derive(Debug, Clone, Default)]
pub struct UpdateWorkflow {
    pub name: Option<String>,
    pub description: Option<String>,
    pub graph: Option<WorkflowGraph>,
    /// Reject the save when someone else changed the draft in the meantime.
    pub expected_revision: Option<i64>,
}

pub async fn update_workflow(
    pool: &PgPool,
    project_id: ProjectId,
    workflow_id: Uuid,
    update: UpdateWorkflow,
) -> Result<WorkflowRecord, WorkflowError> {
    let mut tx = pool.begin().await?;
    let current = sqlx::query(
        "SELECT draft_revision FROM workflows WHERE project_id=$1 AND id=$2 FOR UPDATE",
    )
    .bind(project_uuid(project_id))
    .bind(workflow_id)
    .fetch_optional(&mut *tx)
    .await?
    .ok_or(WorkflowError::NotFound)?;
    let revision: i64 = current.get("draft_revision");
    if update
        .expected_revision
        .is_some_and(|expected| expected != revision)
    {
        return Err(WorkflowError::Conflict(
            "This automation was changed somewhere else. Reload it before saving.".to_owned(),
        ));
    }
    if let Some(name) = &update.name {
        let name = name.trim();
        if name.is_empty() || name.len() > 200 {
            return Err(WorkflowError::InvalidInput(
                "The name must have between 1 and 200 characters.".to_owned(),
            ));
        }
    }
    let mut graph_value = None;
    if let Some(mut graph) = update.graph {
        let secrets = extract_secrets(&mut graph);
        write_secrets(&mut tx, workflow_id, &secrets).await?;
        // Forget secrets of blocks that were deleted from the canvas.
        let live: Vec<String> = graph.nodes.iter().map(|node| node.id.clone()).collect();
        sqlx::query(
            "DELETE FROM workflow_secrets WHERE workflow_id=$1 AND NOT (node_id = ANY($2))",
        )
        .bind(workflow_id)
        .bind(&live)
        .execute(&mut *tx)
        .await?;
        graph_value = Some(serde_json::to_value(&graph)?);
    }
    let result = sqlx::query(
        "UPDATE workflows SET
           name = COALESCE($3, name),
           description = COALESCE($4, description),
           draft_graph = COALESCE($5, draft_graph),
           draft_revision = draft_revision + 1,
           updated_at = now()
         WHERE project_id=$1 AND id=$2",
    )
    .bind(project_uuid(project_id))
    .bind(workflow_id)
    .bind(update.name.as_deref().map(str::trim))
    .bind(update.description.as_deref())
    .bind(graph_value)
    .execute(&mut *tx)
    .await;
    match result {
        Ok(_) => {}
        Err(error) if is_unique_violation(&error) => {
            return Err(WorkflowError::Conflict(
                "Another automation in this project already has that name.".to_owned(),
            ));
        }
        Err(error) => return Err(error.into()),
    }
    tx.commit().await?;
    get_workflow(pool, project_id, workflow_id).await
}

pub async fn delete_workflow(
    pool: &PgPool,
    project_id: ProjectId,
    workflow_id: Uuid,
) -> Result<(), WorkflowError> {
    let deleted = sqlx::query("DELETE FROM workflows WHERE project_id=$1 AND id=$2")
        .bind(project_uuid(project_id))
        .bind(workflow_id)
        .execute(pool)
        .await?
        .rows_affected();
    if deleted == 0 {
        return Err(WorkflowError::NotFound);
    }
    Ok(())
}

/// What the checks need beyond the graph: whether this server can send mail, and
/// the stored Slack webhook of each block (the graph only holds a placeholder).
pub async fn validation_context(
    pool: &PgPool,
    workflow_id: Uuid,
    graph: &WorkflowGraph,
) -> Result<ValidationContext, WorkflowError> {
    let mut secrets = HashMap::new();
    for node in graph
        .nodes
        .iter()
        .filter(|node| node.node_type == "integration.slack")
    {
        if let Some(webhook) = get_workflow_secret(pool, workflow_id, &node.id, "webhook").await? {
            secrets.insert((node.id.clone(), "webhook".to_owned()), webhook);
        }
    }
    Ok(ValidationContext {
        email_configured: email_is_configured(
            &std::env::var("SMTP_HOST").unwrap_or_default(),
            &std::env::var("SMTP_FROM").unwrap_or_default(),
        ),
        secrets,
    })
}

/// Validate the draft without publishing it.
pub async fn validate_workflow_draft(
    pool: &PgPool,
    project_id: ProjectId,
    workflow_id: Uuid,
) -> Result<ValidationReport, WorkflowError> {
    let workflow = get_workflow(pool, project_id, workflow_id).await?;
    let ctx = validation_context(pool, workflow_id, &workflow.draft_graph).await?;
    Ok(validate_graph(&workflow.draft_graph, &ctx))
}

fn next_fire_for(trigger: &WorkflowTrigger, now: DateTime<Utc>) -> Option<DateTime<Utc>> {
    match trigger {
        WorkflowTrigger::Schedule(schedule) | WorkflowTrigger::PublicationAlert { schedule } => {
            next_fire_after(schedule, now).ok().flatten()
        }
        _ => None,
    }
}

pub async fn publish_workflow(
    pool: &PgPool,
    project_id: ProjectId,
    workflow_id: Uuid,
    note: Option<&str>,
    actor: &Actor,
) -> Result<WorkflowVersionRecord, WorkflowError> {
    let mut tx = pool.begin().await?;
    let row = sqlx::query(
        "SELECT draft_graph, status FROM workflows WHERE project_id=$1 AND id=$2 FOR UPDATE",
    )
    .bind(project_uuid(project_id))
    .bind(workflow_id)
    .fetch_optional(&mut *tx)
    .await?
    .ok_or(WorkflowError::NotFound)?;
    let graph = parse_graph(row.get("draft_graph"))?;
    let status: String = row.get("status");
    let ctx = validation_context(pool, workflow_id, &graph).await?;
    let report = validate_graph(&graph, &ctx);
    if !report.is_ok() {
        return Err(WorkflowError::Invalid(report));
    }
    let trigger = trigger_of_graph(&graph).ok_or_else(|| {
        WorkflowError::InvalidInput("The trigger is not configured correctly.".to_owned())
    })?;
    let version: i32 = sqlx::query_scalar(
        "SELECT COALESCE(MAX(version),0)+1 FROM workflow_versions WHERE workflow_id=$1",
    )
    .bind(workflow_id)
    .fetch_one(&mut *tx)
    .await?;
    let version_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO workflow_versions
           (id, project_id, workflow_id, version, graph, trigger_kind, note,
            published_by_kind, published_by_id)
         VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9)",
    )
    .bind(version_id)
    .bind(project_uuid(project_id))
    .bind(workflow_id)
    .bind(version)
    .bind(serde_json::to_value(&graph)?)
    .bind(trigger.kind_key())
    .bind(note)
    .bind(actor.kind().as_str())
    .bind(actor.id())
    .execute(&mut *tx)
    .await?;
    let next_fire = if status == "enabled" {
        next_fire_for(&trigger, Utc::now())
    } else {
        None
    };
    sqlx::query(
        "UPDATE workflows SET published_version_id=$3, trigger_kind=$4, next_fire_at=$5,
                updated_at=now()
         WHERE project_id=$1 AND id=$2",
    )
    .bind(project_uuid(project_id))
    .bind(workflow_id)
    .bind(version_id)
    .bind(trigger.kind_key())
    .bind(next_fire)
    .execute(&mut *tx)
    .await?;
    let row = sqlx::query(
        "SELECT id, workflow_id, version, graph, trigger_kind, note, published_by_kind,
                published_by_id, published_at
         FROM workflow_versions WHERE id=$1",
    )
    .bind(version_id)
    .fetch_one(&mut *tx)
    .await?;
    tx.commit().await?;
    version_from_row(&row)
}

pub async fn set_workflow_enabled(
    pool: &PgPool,
    project_id: ProjectId,
    workflow_id: Uuid,
    enabled: bool,
) -> Result<WorkflowRecord, WorkflowError> {
    let mut tx = pool.begin().await?;
    let row = sqlx::query(
        "SELECT v.graph FROM workflows w
         LEFT JOIN workflow_versions v ON v.id = w.published_version_id
         WHERE w.project_id=$1 AND w.id=$2 FOR UPDATE OF w",
    )
    .bind(project_uuid(project_id))
    .bind(workflow_id)
    .fetch_optional(&mut *tx)
    .await?
    .ok_or(WorkflowError::NotFound)?;
    let graph: Option<Value> = row.get("graph");
    let next_fire = if enabled {
        let graph = parse_graph(graph.ok_or(WorkflowError::NotPublished)?)?;
        trigger_of_graph(&graph).and_then(|trigger| next_fire_for(&trigger, Utc::now()))
    } else {
        None
    };
    sqlx::query(
        "UPDATE workflows SET status=$3, next_fire_at=$4, updated_at=now()
         WHERE project_id=$1 AND id=$2",
    )
    .bind(project_uuid(project_id))
    .bind(workflow_id)
    .bind(if enabled { "enabled" } else { "disabled" })
    .bind(next_fire)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    get_workflow(pool, project_id, workflow_id).await
}

pub async fn list_workflow_versions(
    pool: &PgPool,
    project_id: ProjectId,
    workflow_id: Uuid,
) -> Result<Vec<WorkflowVersionRecord>, WorkflowError> {
    get_workflow(pool, project_id, workflow_id).await?;
    let rows = sqlx::query(
        "SELECT id, workflow_id, version, graph, trigger_kind, note, published_by_kind,
                published_by_id, published_at
         FROM workflow_versions WHERE workflow_id=$1 ORDER BY version DESC",
    )
    .bind(workflow_id)
    .fetch_all(pool)
    .await?;
    rows.iter().map(version_from_row).collect()
}

pub async fn get_workflow_version(
    pool: &PgPool,
    project_id: ProjectId,
    workflow_id: Uuid,
    version: i32,
) -> Result<WorkflowVersionRecord, WorkflowError> {
    get_workflow(pool, project_id, workflow_id).await?;
    let row = sqlx::query(
        "SELECT id, workflow_id, version, graph, trigger_kind, note, published_by_kind,
                published_by_id, published_at
         FROM workflow_versions WHERE workflow_id=$1 AND version=$2",
    )
    .bind(workflow_id)
    .bind(version)
    .fetch_optional(pool)
    .await?
    .ok_or(WorkflowError::NotFound)?;
    version_from_row(&row)
}

// ---------------------------------------------------------------------------
// Endpoint tokens

pub async fn get_webhook_config(
    pool: &PgPool,
    project_id: ProjectId,
    workflow_id: Uuid,
) -> Result<WebhookConfig, WorkflowError> {
    let row = sqlx::query(
        "SELECT webhook_token, webhook_secret, webhook_signature_required, email_token
         FROM workflows WHERE project_id=$1 AND id=$2",
    )
    .bind(project_uuid(project_id))
    .bind(workflow_id)
    .fetch_optional(pool)
    .await?
    .ok_or(WorkflowError::NotFound)?;
    Ok(WebhookConfig {
        token: row.get("webhook_token"),
        secret: row.get("webhook_secret"),
        signature_required: row.get("webhook_signature_required"),
        email_token: row.get("email_token"),
    })
}

/// Which credential to replace.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RotateTarget {
    /// New webhook address token and a new signing secret.
    Webhook,
    /// Only the signing secret.
    WebhookSecret,
    /// New inbound e-mail address token.
    Email,
}

pub async fn rotate_endpoint_credentials(
    pool: &PgPool,
    project_id: ProjectId,
    workflow_id: Uuid,
    target: RotateTarget,
) -> Result<WebhookConfig, WorkflowError> {
    let (sql, token, secret) = match target {
        RotateTarget::Webhook => (
            "UPDATE workflows SET webhook_token=$3, webhook_secret=$4, updated_at=now()
             WHERE project_id=$1 AND id=$2",
            generate_token(),
            generate_secret(),
        ),
        RotateTarget::WebhookSecret => (
            "UPDATE workflows SET webhook_secret=$4, updated_at=now()
             WHERE project_id=$1 AND id=$2 AND $3 IS NOT NULL",
            generate_token(),
            generate_secret(),
        ),
        RotateTarget::Email => (
            "UPDATE workflows SET email_token=$3, updated_at=now()
             WHERE project_id=$1 AND id=$2 AND $4 IS NOT NULL",
            generate_token(),
            generate_secret(),
        ),
    };
    let updated = sqlx::query(sql)
        .bind(project_uuid(project_id))
        .bind(workflow_id)
        .bind(token)
        .bind(secret)
        .execute(pool)
        .await?
        .rows_affected();
    if updated == 0 {
        return Err(WorkflowError::NotFound);
    }
    get_webhook_config(pool, project_id, workflow_id).await
}

pub async fn set_webhook_signature_required(
    pool: &PgPool,
    project_id: ProjectId,
    workflow_id: Uuid,
    required: bool,
) -> Result<WebhookConfig, WorkflowError> {
    let updated = sqlx::query(
        "UPDATE workflows SET webhook_signature_required=$3, updated_at=now()
         WHERE project_id=$1 AND id=$2",
    )
    .bind(project_uuid(project_id))
    .bind(workflow_id)
    .bind(required)
    .execute(pool)
    .await?
    .rows_affected();
    if updated == 0 {
        return Err(WorkflowError::NotFound);
    }
    get_webhook_config(pool, project_id, workflow_id).await
}

fn endpoint_target(row: &PgRow) -> EndpointTarget {
    let status: String = row.get("status");
    EndpointTarget {
        workflow_id: row.get("id"),
        project_id: row.get("project_id"),
        secret: row.get("webhook_secret"),
        signature_required: row.get("webhook_signature_required"),
        enabled: status == "enabled",
        version_id: row.get("published_version_id"),
        trigger_kind: row.get("trigger_kind"),
    }
}

pub async fn find_workflow_by_webhook_token(
    pool: &PgPool,
    token: &str,
) -> Result<Option<EndpointTarget>, WorkflowError> {
    let row = sqlx::query(
        "SELECT id, project_id, webhook_secret, webhook_signature_required, status,
                published_version_id, trigger_kind
         FROM workflows WHERE webhook_token=$1",
    )
    .bind(token)
    .fetch_optional(pool)
    .await?;
    Ok(row.as_ref().map(endpoint_target))
}

pub async fn find_workflow_by_email_token(
    pool: &PgPool,
    token: &str,
) -> Result<Option<EndpointTarget>, WorkflowError> {
    let row = sqlx::query(
        "SELECT id, project_id, webhook_secret, webhook_signature_required, status,
                published_version_id, trigger_kind
         FROM workflows WHERE email_token=$1",
    )
    .bind(token)
    .fetch_optional(pool)
    .await?;
    Ok(row.as_ref().map(endpoint_target))
}

pub async fn get_workflow_secret(
    pool: &PgPool,
    workflow_id: Uuid,
    node_id: &str,
    key: &str,
) -> Result<Option<String>, WorkflowError> {
    Ok(sqlx::query_scalar(
        "SELECT value FROM workflow_secrets WHERE workflow_id=$1 AND node_id=$2 AND key=$3",
    )
    .bind(workflow_id)
    .bind(node_id)
    .bind(key)
    .fetch_optional(pool)
    .await?)
}

// ---------------------------------------------------------------------------
// Runs

#[derive(Debug, Clone)]
pub struct StartRun {
    pub project_id: Uuid,
    pub workflow_id: Uuid,
    pub version_id: Option<Uuid>,
    pub graph: WorkflowGraph,
    pub trigger_kind: String,
    pub trigger_data: Value,
    pub idempotency_key: String,
    pub actor_kind: String,
    pub actor_id: String,
    pub test_mode: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StartedRun {
    pub run_id: Uuid,
    pub created: bool,
}

/// Shape a trigger's payload into the output of the trigger node: each output
/// port takes the payload entry of the same name, or the whole payload.
fn trigger_outputs(node_type_id: &str, data: &Value) -> (Value, Vec<String>) {
    let mut outputs = Map::new();
    let mut fired = Vec::new();
    if let Some(def) = node_type(node_type_id) {
        for port in &def.outputs {
            let value = data.get(&port.id).cloned().unwrap_or_else(|| data.clone());
            outputs.insert(port.id.clone(), value);
            fired.push(port.id.clone());
        }
    }
    (Value::Object(outputs), fired)
}

/// Create a run (idempotent per `(workflow, idempotency_key)`) and queue its
/// first steps. Runs inside the caller's transaction so a platform event and
/// the runs it causes commit together.
pub async fn start_run_in_transaction(
    tx: &mut Transaction<'_, Postgres>,
    request: &StartRun,
) -> Result<StartedRun, WorkflowError> {
    if serde_json::to_vec(&request.trigger_data)?.len() > MAX_TRIGGER_DATA_BYTES {
        return Err(WorkflowError::InvalidInput(
            "The information that started this automation is too large.".to_owned(),
        ));
    }
    let run_id = Uuid::new_v4();
    let inserted: Option<Uuid> = sqlx::query_scalar(
        "INSERT INTO workflow_runs
           (id, project_id, workflow_id, version_id, graph, trigger_kind, trigger_data,
            idempotency_key, test_mode, status, actor_kind, actor_id, started_at)
         VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,'running',$10,$11,now())
         ON CONFLICT (workflow_id, idempotency_key) DO NOTHING
         RETURNING id",
    )
    .bind(run_id)
    .bind(request.project_id)
    .bind(request.workflow_id)
    .bind(request.version_id)
    .bind(serde_json::to_value(&request.graph)?)
    .bind(&request.trigger_kind)
    .bind(&request.trigger_data)
    .bind(&request.idempotency_key)
    .bind(request.test_mode)
    .bind(&request.actor_kind)
    .bind(&request.actor_id)
    .fetch_optional(&mut **tx)
    .await?;
    let Some(run_id) = inserted else {
        let existing: Uuid = sqlx::query_scalar(
            "SELECT id FROM workflow_runs WHERE workflow_id=$1 AND idempotency_key=$2",
        )
        .bind(request.workflow_id)
        .bind(&request.idempotency_key)
        .fetch_one(&mut **tx)
        .await?;
        return Ok(StartedRun {
            run_id: existing,
            created: false,
        });
    };
    let trigger = request
        .graph
        .trigger_node()
        .ok_or_else(|| WorkflowError::InvalidInput("The automation has no trigger.".to_owned()))?;
    let (output, fired) = trigger_outputs(&trigger.node_type, &request.trigger_data);
    sqlx::query(
        "INSERT INTO workflow_node_runs
           (project_id, run_id, node_id, iteration, node_type, status, attempts, output,
            fired_ports, started_at, finished_at)
         VALUES ($1,$2,$3,'',$4,'completed',1,$5,$6,now(),now())",
    )
    .bind(request.project_id)
    .bind(run_id)
    .bind(&trigger.id)
    .bind(&trigger.node_type)
    .bind(output)
    .bind(&fired)
    .execute(&mut **tx)
    .await?;
    advance_run_in_transaction(tx, run_id).await?;
    Ok(StartedRun {
        run_id,
        created: true,
    })
}

/// Start a run in its own transaction.
pub async fn start_run(pool: &PgPool, request: &StartRun) -> Result<StartedRun, WorkflowError> {
    let mut tx = pool.begin().await?;
    let started = start_run_in_transaction(&mut tx, request).await?;
    tx.commit().await?;
    Ok(started)
}

fn snapshot_from_row(row: &PgRow) -> Result<NodeRunSnapshot, WorkflowError> {
    let status: String = row.get("status");
    Ok(NodeRunSnapshot {
        node_id: row.get("node_id"),
        iteration: row.get("iteration"),
        state: NodeRunState::parse(&status)
            .ok_or_else(|| WorkflowError::InvalidStoredValue(format!("node status {status}")))?,
        fired_ports: row.get("fired_ports"),
        items: usize::try_from(row.get::<i32, _>("items")).unwrap_or_default(),
    })
}

async fn load_snapshots(
    tx: &mut Transaction<'_, Postgres>,
    run_id: Uuid,
) -> Result<Vec<NodeRunSnapshot>, WorkflowError> {
    let rows = sqlx::query(
        "SELECT node_id, iteration, status, fired_ports, items FROM workflow_node_runs WHERE run_id=$1",
    )
    .bind(run_id)
    .fetch_all(&mut **tx)
    .await?;
    rows.iter().map(snapshot_from_row).collect()
}

/// One notification per failed run, written in the transaction that made the
/// run `failed`, so it commits or rolls back with that transition.
async fn notify_run_failed(
    tx: &mut Transaction<'_, Postgres>,
    project_id: Uuid,
    workflow_id: Uuid,
    run_id: Uuid,
    reason: Option<&str>,
) -> Result<(), WorkflowError> {
    let name: Option<String> = sqlx::query_scalar("SELECT name FROM workflows WHERE id=$1")
        .bind(workflow_id)
        .fetch_optional(&mut **tx)
        .await?;
    let title: String = format!("{} failed", name.as_deref().unwrap_or("Automation"))
        .chars()
        .take(200)
        .collect();
    let body: Option<String> = reason.map(|text| text.chars().take(500).collect());
    crate::record_run_notification_once_in_transaction(
        tx,
        &crate::NotificationDraft::error(
            "workflow_run.failed",
            Some(project_id),
            &title,
            body,
            json!({ "workflow_id": workflow_id, "run_id": run_id }),
        ),
        run_id,
    )
    .await?;
    Ok(())
}

/// Re-plan a run after something changed: queue newly ready steps, record
/// skipped ones and finish the run when nothing is left to do.
pub async fn advance_run_in_transaction(
    tx: &mut Transaction<'_, Postgres>,
    run_id: Uuid,
) -> Result<(), WorkflowError> {
    let row = sqlx::query(
        "SELECT project_id, workflow_id, graph, status, test_mode
         FROM workflow_runs WHERE id=$1 FOR UPDATE",
    )
    .bind(run_id)
    .fetch_optional(&mut **tx)
    .await?
    .ok_or(WorkflowError::RunNotFound)?;
    let status: String = row.get("status");
    if matches!(status.as_str(), "completed" | "failed" | "cancelled") {
        return Ok(());
    }
    let project_id: Uuid = row.get("project_id");
    let workflow_id: Uuid = row.get("workflow_id");
    let test_mode: bool = row.get("test_mode");
    let graph = parse_graph(row.get("graph"))?;
    let snapshots = load_snapshots(tx, run_id).await?;
    let plan = plan_run(&graph, &snapshots);

    for (node_id, iteration, reason) in &plan.skipped {
        let node_type = graph
            .node(node_id)
            .map_or_else(String::new, |node| node.node_type.clone());
        sqlx::query(
            "INSERT INTO workflow_node_runs
               (project_id, run_id, node_id, iteration, node_type, status, note, finished_at)
             VALUES ($1,$2,$3,$4,$5,'skipped',$6,now())
             ON CONFLICT (run_id,node_id,iteration) DO NOTHING",
        )
        .bind(project_id)
        .bind(run_id)
        .bind(node_id)
        .bind(iteration)
        .bind(node_type)
        .bind(reason)
        .execute(&mut **tx)
        .await?;
    }
    for planned in &plan.ready {
        let node_type = graph
            .node(&planned.node_id)
            .map_or_else(String::new, |node| node.node_type.clone());
        let node_run_id: Option<Uuid> = sqlx::query_scalar(
            "INSERT INTO workflow_node_runs
               (project_id, run_id, node_id, iteration, node_type, status)
             VALUES ($1,$2,$3,$4,$5,'queued')
             ON CONFLICT (run_id,node_id,iteration) DO NOTHING
             RETURNING id",
        )
        .bind(project_id)
        .bind(run_id)
        .bind(&planned.node_id)
        .bind(&planned.iteration)
        .bind(node_type)
        .fetch_optional(&mut **tx)
        .await?;
        if let Some(node_run_id) = node_run_id {
            sqlx::query(
                "INSERT INTO jobs (id,project_id,kind,payload,priority,max_attempts,dedupe_key,available_at)
                 VALUES ($1,$2,$3,$4,0,$5,$6, now() + ($7::double precision * interval '1 second'))
                 ON CONFLICT (dedupe_key) DO NOTHING",
            )
            .bind(Uuid::new_v4())
            .bind(project_id)
            .bind(WORKFLOW_STEP_JOB_KIND)
            .bind(json!({ "node_run_id": node_run_id }))
            .bind(NODE_MAX_ATTEMPTS)
            .bind(format!("workflow_step:{node_run_id}"))
            .bind(planned.delay_secs as f64)
            .execute(&mut **tx)
            .await?;
        }
    }
    if plan.finished {
        let first_error: Option<String> = if plan.failed {
            sqlx::query_scalar(
                "SELECT error FROM workflow_node_runs
                 WHERE run_id=$1 AND status='failed' ORDER BY finished_at LIMIT 1",
            )
            .bind(run_id)
            .fetch_optional(&mut **tx)
            .await?
            .flatten()
        } else {
            None
        };
        let stop_reason: Option<String> = first_error.or_else(|| {
            plan.failed
                .then(|| "A step did not finish, so the automation stopped.".to_owned())
        });
        sqlx::query("UPDATE workflow_runs SET status=$2, error=$3, finished_at=now() WHERE id=$1")
            .bind(run_id)
            .bind(if plan.failed { "failed" } else { "completed" })
            .bind(stop_reason.as_deref())
            .execute(&mut **tx)
            .await?;
        if plan.failed && !test_mode {
            notify_run_failed(tx, project_id, workflow_id, run_id, stop_reason.as_deref()).await?;
        }
        sqlx::query("UPDATE workflows SET last_fired_at=now() WHERE id=$1")
            .bind(workflow_id)
            .execute(&mut **tx)
            .await?;
    }
    Ok(())
}

/// Request cancellation: queued steps are cancelled at once, a running step
/// finishes its current attempt but nothing further is planned.
pub async fn cancel_workflow_run(
    pool: &PgPool,
    project_id: ProjectId,
    run_id: Uuid,
) -> Result<(), WorkflowError> {
    let mut tx = pool.begin().await?;
    let status: Option<String> = sqlx::query_scalar(
        "SELECT status FROM workflow_runs WHERE project_id=$1 AND id=$2 FOR UPDATE",
    )
    .bind(project_uuid(project_id))
    .bind(run_id)
    .fetch_optional(&mut *tx)
    .await?;
    let status = status.ok_or(WorkflowError::RunNotFound)?;
    if matches!(status.as_str(), "completed" | "failed" | "cancelled") {
        return Err(WorkflowError::RunFinished);
    }
    sqlx::query(
        "UPDATE workflow_node_runs SET status='cancelled', finished_at=now(),
                note='Cancelled before it started.'
         WHERE run_id=$1 AND status='queued'",
    )
    .bind(run_id)
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        "UPDATE workflow_runs SET status='cancelled', finished_at=now(),
                error='Cancelled by a person.'
         WHERE id=$1",
    )
    .bind(run_id)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(())
}

const NODE_RUN_SELECT: &str = "SELECT id, node_id, node_type, iteration, status, attempts,
        CASE WHEN octet_length(input::text) > $2 THEN NULL ELSE input END AS input,
        CASE WHEN octet_length(input::text) > $2 THEN left(input::text, 2000) END AS input_preview,
        CASE WHEN octet_length(output::text) > $2 THEN NULL ELSE output END AS output,
        CASE WHEN octet_length(output::text) > $2 THEN left(output::text, 2000) END AS output_preview,
        note, error, created_at, started_at, finished_at
     FROM workflow_node_runs";

fn node_run_from_row(row: &PgRow) -> NodeRunRecord {
    NodeRunRecord {
        id: row.get("id"),
        node_id: row.get("node_id"),
        node_type: row.get("node_type"),
        iteration: row.get("iteration"),
        status: row.get("status"),
        attempts: row.get("attempts"),
        input: row.get("input"),
        output: row.get("output"),
        input_preview: row.get("input_preview"),
        output_preview: row.get("output_preview"),
        note: row.get("note"),
        error: row.get("error"),
        created_at: row.get("created_at"),
        started_at: row.get("started_at"),
        finished_at: row.get("finished_at"),
    }
}

const RUN_SELECT: &str = "SELECT r.id, r.project_id, r.workflow_id, w.name AS workflow_name,
        v.version, r.trigger_kind, r.status, r.test_mode, r.actor_kind, r.actor_id,
        r.error, r.created_at, r.started_at, r.finished_at, r.trigger_data, r.graph
     FROM workflow_runs r
     JOIN workflows w ON w.id = r.workflow_id
     LEFT JOIN workflow_versions v ON v.id = r.version_id";

fn run_from_row(
    row: &PgRow,
    nodes: Vec<NodeRunRecord>,
) -> Result<WorkflowRunRecord, WorkflowError> {
    Ok(WorkflowRunRecord {
        id: row.get("id"),
        project_id: row.get("project_id"),
        workflow_id: row.get("workflow_id"),
        workflow_name: row.get("workflow_name"),
        version: row.get("version"),
        trigger_kind: row.get("trigger_kind"),
        status: row.get("status"),
        test_mode: row.get("test_mode"),
        actor_kind: row.get("actor_kind"),
        actor_id: row.get("actor_id"),
        error: row.get("error"),
        created_at: row.get("created_at"),
        started_at: row.get("started_at"),
        finished_at: row.get("finished_at"),
        trigger_data: row.get("trigger_data"),
        graph: parse_graph(row.get("graph"))?,
        nodes,
    })
}

pub async fn list_workflow_runs(
    pool: &PgPool,
    project_id: ProjectId,
    workflow_id: Option<Uuid>,
    limit: i64,
) -> Result<Vec<WorkflowRunRecord>, WorkflowError> {
    if !(1..=MAX_WORKFLOW_RUN_LIST_LIMIT).contains(&limit) {
        return Err(WorkflowError::InvalidInput(format!(
            "The limit must be between 1 and {MAX_WORKFLOW_RUN_LIST_LIMIT}."
        )));
    }
    // Run snapshots carry raw node input and output, including screening judgments
    // written by `data.find_records`, so a blind audit withholds them project-wide.
    let rows = sqlx::query(sqlx::AssertSqlSafe(format!(
        "{RUN_SELECT} WHERE r.project_id=$1 AND ($2::uuid IS NULL OR r.workflow_id=$2)
         AND ai_first_run_visible(r.project_id, r.id)
         ORDER BY r.created_at DESC, r.id DESC LIMIT $3"
    )))
    .bind(project_uuid(project_id))
    .bind(workflow_id)
    .bind(limit)
    .fetch_all(pool)
    .await?;
    rows.iter()
        .map(|row| run_from_row(row, Vec::new()))
        .collect()
}

/// The AI verdicts of a run's "Run an AI review" screening, for the run history.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReviewCounts {
    pub included: i64,
    pub excluded: i64,
    pub unsure: i64,
}

/// The verdict counts of each run in `run_ids` that has a screening step with
/// finished verdicts. Runs that used no AI review screening are not in the map.
pub async fn review_counts_for_runs(
    pool: &PgPool,
    run_ids: &[Uuid],
) -> Result<HashMap<Uuid, ReviewCounts>, WorkflowError> {
    if run_ids.is_empty() {
        return Ok(HashMap::new());
    }
    let rows = sqlx::query(
        "SELECT run_id,
            SUM(jsonb_array_length(output->'included'))::bigint AS included,
            SUM(jsonb_array_length(output->'excluded'))::bigint AS excluded,
            SUM(jsonb_array_length(output->'unsure'))::bigint AS unsure
         FROM workflow_node_runs
         WHERE run_id = ANY($1) AND node_type = 'ai.run_review'
           AND jsonb_typeof(output->'included') = 'array'
           AND jsonb_typeof(output->'excluded') = 'array'
           AND jsonb_typeof(output->'unsure') = 'array'
         GROUP BY run_id",
    )
    .bind(run_ids)
    .fetch_all(pool)
    .await?;
    Ok(rows
        .iter()
        .map(|row| {
            (
                row.get("run_id"),
                ReviewCounts {
                    included: row.get("included"),
                    excluded: row.get("excluded"),
                    unsure: row.get("unsure"),
                },
            )
        })
        .collect())
}

pub async fn get_workflow_run(
    pool: &PgPool,
    project_id: ProjectId,
    run_id: Uuid,
) -> Result<WorkflowRunRecord, WorkflowError> {
    // The node log repeats raw node input and output. Withholding the run while a
    // blind audit is active keeps those judgments out of an auditor's reach.
    let row = sqlx::query(sqlx::AssertSqlSafe(format!(
        "{RUN_SELECT} WHERE r.project_id=$1 AND r.id=$2 AND ai_first_run_visible(r.project_id, r.id)"
    )))
    .bind(project_uuid(project_id))
    .bind(run_id)
    .fetch_optional(pool)
    .await?
    .ok_or(WorkflowError::RunNotFound)?;
    let nodes = sqlx::query(sqlx::AssertSqlSafe(format!(
        "{NODE_RUN_SELECT} WHERE run_id=$1 ORDER BY COALESCE(started_at, finished_at, created_at), created_at"
    )))
    .bind(run_id)
    .bind(NODE_SNAPSHOT_LIMIT_BYTES)
    .fetch_all(pool)
    .await?;
    run_from_row(&row, nodes.iter().map(node_run_from_row).collect())
}

/// Whether an active blind audit withholds this project's run inspection. Starting a
/// test run would produce a snapshot nobody may read, so callers refuse before writing.
pub async fn workflow_run_inspection_withheld(
    pool: &PgPool,
    project_id: ProjectId,
) -> Result<bool, WorkflowError> {
    let withheld: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM ai_screening_cohorts WHERE project_id=$1 AND status='auditing')",
    )
    .bind(project_uuid(project_id))
    .fetch_one(pool)
    .await?;
    Ok(withheld)
}

// ---------------------------------------------------------------------------
// Step execution

/// Everything the worker needs to execute one block.
#[derive(Debug, Clone)]
pub struct NodeExecution {
    pub node_run_id: Uuid,
    pub run_id: Uuid,
    pub project_id: Uuid,
    pub workflow_id: Uuid,
    pub node_id: String,
    pub node_type: String,
    pub iteration: String,
    pub config: Value,
    pub attempts: i32,
    pub test_mode: bool,
    pub actor_kind: String,
    pub actor_id: String,
    pub trigger_data: Value,
    /// What a step that is waiting for the AI kept from its last look.
    pub wait_state: Option<Value>,
    /// Input port id to value.
    pub inputs: Map<String, Value>,
}

/// What happened when a step was asked to start.
#[derive(Debug)]
pub enum StepClaim {
    Execute(Box<NodeExecution>),
    /// The step is no longer wanted (run cancelled or already finished).
    Stale,
}

pub async fn begin_node_execution(
    pool: &PgPool,
    node_run_id: Uuid,
) -> Result<StepClaim, WorkflowError> {
    let mut tx = pool.begin().await?;
    let row = sqlx::query(
        "SELECT n.run_id, n.project_id, n.node_id, n.node_type, n.iteration, n.status,
                n.wait_state, r.status AS run_status, r.graph, r.test_mode, r.workflow_id,
                r.actor_kind, r.actor_id, r.trigger_data
         FROM workflow_node_runs n JOIN workflow_runs r ON r.id = n.run_id
         WHERE n.id=$1 FOR UPDATE OF n",
    )
    .bind(node_run_id)
    .fetch_optional(&mut *tx)
    .await?;
    let Some(row) = row else {
        return Ok(StepClaim::Stale);
    };
    let status: String = row.get("status");
    let run_status: String = row.get("run_status");
    if !matches!(status.as_str(), "queued" | "running")
        || !matches!(run_status.as_str(), "queued" | "running")
    {
        return Ok(StepClaim::Stale);
    }
    let run_id: Uuid = row.get("run_id");
    let graph = parse_graph(row.get("graph"))?;
    let node_id: String = row.get("node_id");
    let iteration: String = row.get("iteration");
    let node = graph
        .node(&node_id)
        .ok_or_else(|| WorkflowError::InvalidStoredValue(format!("missing node {node_id}")))?;

    let snapshots = load_snapshots(&mut tx, run_id).await?;
    let sources = input_sources(&graph, &snapshots, &node_id, &iteration);
    let inputs = resolve_inputs(&mut tx, run_id, &graph, &node_id, &sources).await?;

    let input_value = Value::Object(inputs.clone());
    let stored_input = if serde_json::to_vec(&input_value)?.len() > MAX_NODE_OUTPUT_BYTES {
        json!({ "note": "too large to keep" })
    } else {
        input_value
    };
    let attempts: i32 = sqlx::query_scalar(
        "UPDATE workflow_node_runs
         SET status='running', attempts=attempts+1, input=$2, error=NULL,
             started_at=COALESCE(started_at, now())
         WHERE id=$1 RETURNING attempts",
    )
    .bind(node_run_id)
    .bind(stored_input)
    .fetch_one(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(StepClaim::Execute(Box::new(NodeExecution {
        node_run_id,
        run_id,
        project_id: row.get("project_id"),
        workflow_id: row.get("workflow_id"),
        node_id,
        node_type: row.get("node_type"),
        iteration,
        config: node.config.clone(),
        attempts,
        test_mode: row.get("test_mode"),
        actor_kind: row.get("actor_kind"),
        actor_id: row.get("actor_id"),
        trigger_data: row.get("trigger_data"),
        wait_state: row.get("wait_state"),
        inputs,
    })))
}

async fn resolve_inputs(
    tx: &mut Transaction<'_, Postgres>,
    run_id: Uuid,
    graph: &WorkflowGraph,
    node_id: &str,
    sources: &[InputSource],
) -> Result<Map<String, Value>, WorkflowError> {
    let source_nodes: Vec<String> = sources
        .iter()
        .map(|source| source.source_node.clone())
        .collect();
    let rows = sqlx::query(
        "SELECT node_id, iteration, status, fired_ports, output
         FROM workflow_node_runs WHERE run_id=$1 AND node_id = ANY($2)",
    )
    .bind(run_id)
    .bind(&source_nodes)
    .fetch_all(&mut **tx)
    .await?;
    type Stored = (String, Vec<String>, Option<Value>);
    let mut outputs: HashMap<(String, String), Stored> = HashMap::new();
    for row in &rows {
        outputs.insert(
            (row.get("node_id"), row.get("iteration")),
            (row.get("status"), row.get("fired_ports"), row.get("output")),
        );
    }
    let multiple_ports: Vec<String> = graph
        .node(node_id)
        .and_then(|node| node_type(&node.node_type))
        .map(|def| {
            def.inputs
                .iter()
                .filter(|port| port.multiple)
                .map(|port| port.id.clone())
                .collect()
        })
        .unwrap_or_default();
    let mut inputs: Map<String, Value> = Map::new();
    for source in sources {
        let multiple = multiple_ports.iter().any(|port| port == &source.port);
        for iteration in &source.iterations {
            let Some((status, fired, output)) =
                outputs.get(&(source.source_node.clone(), iteration.clone()))
            else {
                continue;
            };
            if status != "completed" || !fired.contains(&source.source_port) {
                continue;
            }
            let Some(mut value) = output
                .as_ref()
                .and_then(|map| map.get(&source.source_port))
                .cloned()
            else {
                continue;
            };
            if let Some(index) = source.loop_index {
                value = value
                    .as_array()
                    .and_then(|items| items.get(index))
                    .cloned()
                    .unwrap_or(Value::Null);
            }
            match inputs.get_mut(&source.port) {
                Some(Value::Array(existing)) if multiple => existing.push(value),
                _ if multiple => {
                    inputs.insert(source.port.clone(), Value::Array(vec![value]));
                }
                _ => {
                    inputs.insert(source.port.clone(), value);
                }
            }
        }
    }
    Ok(inputs)
}

/// The outcome of executing one block.
#[derive(Debug, Clone)]
pub enum NodeOutcome {
    Completed {
        outputs: Map<String, Value>,
        /// Output ports that carry data; the rest are dead branches.
        fired: Vec<String>,
        /// Items fanned out (`for_each`).
        items: usize,
        note: Option<String>,
    },
    Skipped {
        note: String,
    },
    Failed {
        message: String,
    },
    /// The step is not finished: it waits for something outside the flow (the
    /// AI) and is looked at again after `delay_secs`. It stays queued, and the
    /// plan does not move, until a later look completes it.
    Waiting {
        delay_secs: u64,
        /// Numbers the looks, so a look is queued at most once.
        poll: u32,
        note: String,
        wait_state: Value,
    },
}

/// Record the outcome of a step and plan what comes next.
pub async fn finish_node_execution(
    pool: &PgPool,
    node_run_id: Uuid,
    outcome: NodeOutcome,
) -> Result<(), WorkflowError> {
    let mut tx = pool.begin().await?;
    let (run_id, project_id): (Uuid, Uuid) =
        sqlx::query_as("SELECT run_id, project_id FROM workflow_node_runs WHERE id=$1 FOR UPDATE")
            .bind(node_run_id)
            .fetch_optional(&mut *tx)
            .await?
            .ok_or(WorkflowError::NotFound)?;
    match outcome {
        NodeOutcome::Waiting {
            delay_secs,
            poll,
            note,
            wait_state,
        } => {
            // The waiting step and its next look are written in one
            // transaction: a crash cannot leave the step queued with nothing to
            // run it. The look is keyed by its number, so a duplicate look is
            // dropped by the queue.
            sqlx::query(
                "UPDATE workflow_node_runs
                 SET status='queued', note=$2, wait_state=$3, error=NULL
                 WHERE id=$1",
            )
            .bind(node_run_id)
            .bind(note)
            .bind(wait_state)
            .execute(&mut *tx)
            .await?;
            sqlx::query(
                "INSERT INTO jobs (id,project_id,kind,payload,priority,max_attempts,dedupe_key,available_at)
                 VALUES ($1,$2,$3,$4,0,$5,$6, now() + ($7::double precision * interval '1 second'))
                 ON CONFLICT (dedupe_key) DO NOTHING",
            )
            .bind(Uuid::new_v4())
            .bind(project_id)
            .bind(WORKFLOW_STEP_JOB_KIND)
            .bind(json!({ "node_run_id": node_run_id }))
            .bind(NODE_MAX_ATTEMPTS)
            .bind(format!("workflow_step:{node_run_id}:wait:{poll}"))
            .bind(delay_secs as f64)
            .execute(&mut *tx)
            .await?;
            tx.commit().await?;
            return Ok(());
        }
        NodeOutcome::Completed {
            outputs,
            fired,
            items,
            note,
        } => {
            let output = Value::Object(outputs);
            if serde_json::to_vec(&output)?.len() > MAX_NODE_OUTPUT_BYTES {
                sqlx::query(
                    "UPDATE workflow_node_runs SET status='failed', finished_at=now(),
                            error='This step produced more information than an automation can pass along. Narrow it down, for example with a filter.'
                     WHERE id=$1",
                )
                .bind(node_run_id)
                .execute(&mut *tx)
                .await?;
            } else {
                sqlx::query(
                    "UPDATE workflow_node_runs
                     SET status='completed', output=$2, fired_ports=$3, items=$4, note=$5,
                         error=NULL, finished_at=now(), wait_state=NULL
                     WHERE id=$1",
                )
                .bind(node_run_id)
                .bind(output)
                .bind(&fired)
                .bind(i32::try_from(items).unwrap_or(i32::MAX))
                .bind(note)
                .execute(&mut *tx)
                .await?;
            }
        }
        NodeOutcome::Skipped { note } => {
            sqlx::query(
                "UPDATE workflow_node_runs SET status='skipped', note=$2, finished_at=now(), wait_state=NULL WHERE id=$1",
            )
            .bind(node_run_id)
            .bind(note)
            .execute(&mut *tx)
            .await?;
        }
        NodeOutcome::Failed { message } => {
            sqlx::query(
                "UPDATE workflow_node_runs SET status='failed', error=$2, finished_at=now(), wait_state=NULL WHERE id=$1",
            )
            .bind(node_run_id)
            .bind(truncate_message(&message))
            .execute(&mut *tx)
            .await?;
        }
    }
    advance_run_in_transaction(&mut tx, run_id).await?;
    tx.commit().await?;
    Ok(())
}

/// A retryable failure: keep the step queued and show the reason in the log.
pub async fn record_node_retry(
    pool: &PgPool,
    node_run_id: Uuid,
    message: &str,
) -> Result<(), WorkflowError> {
    sqlx::query(
        "UPDATE workflow_node_runs SET status='queued', error=$2 WHERE id=$1 AND status='running'",
    )
    .bind(node_run_id)
    .bind(truncate_message(message))
    .execute(pool)
    .await?;
    Ok(())
}

fn truncate_message(message: &str) -> String {
    message.chars().take(1000).collect()
}

/// Fail steps whose job died without recording an outcome (for example the
/// worker crashed repeatedly), so their run does not hang forever.
pub async fn fail_abandoned_steps(pool: &PgPool) -> Result<u64, WorkflowError> {
    // Every look of a step names the step in its payload, including the looks
    // queued while a step waits for the AI.
    let rows = sqlx::query(
        "SELECT n.id FROM jobs j
         JOIN workflow_node_runs n ON n.id = (j.payload->>'node_run_id')::uuid
         WHERE j.kind = 'workflow_step' AND j.state IN ('dead','failed')
           AND n.status IN ('queued','running')
         LIMIT 50",
    )
    .fetch_all(pool)
    .await?;
    let mut count = 0;
    for row in rows {
        let id: Uuid = row.get("id");
        finish_node_execution(
            pool,
            id,
            NodeOutcome::Failed {
                message: "This step stopped unexpectedly and could not be completed.".to_owned(),
            },
        )
        .await?;
        count += 1;
    }
    Ok(count)
}

// ---------------------------------------------------------------------------
// Schedules and publication alerts

#[derive(Debug, Clone)]
pub struct DueAlert {
    pub workflow_id: Uuid,
    pub project_id: Uuid,
    pub version_id: Uuid,
    pub graph: WorkflowGraph,
    pub poll_state: Value,
}

/// Fire every due scheduled workflow and reserve every due publication alert
/// for polling. Returns the alerts the caller must poll.
pub async fn claim_due_workflows(
    pool: &PgPool,
    now: DateTime<Utc>,
    limit: i64,
) -> Result<(u64, Vec<DueAlert>), WorkflowError> {
    let mut tx = pool.begin().await?;
    let rows = sqlx::query(
        "SELECT w.id, w.project_id, w.published_version_id, w.next_fire_at, w.poll_state, v.graph
         FROM workflows w JOIN workflow_versions v ON v.id = w.published_version_id
         WHERE w.status='enabled' AND w.next_fire_at IS NOT NULL AND w.next_fire_at <= $1
         ORDER BY w.next_fire_at
         LIMIT $2
         FOR UPDATE OF w SKIP LOCKED",
    )
    .bind(now)
    .bind(limit)
    .fetch_all(&mut *tx)
    .await?;
    let mut fired = 0;
    let mut alerts = Vec::new();
    for row in rows {
        let id: Uuid = row.get("id");
        let project_id: Uuid = row.get("project_id");
        let version_id: Uuid = row.get("published_version_id");
        let due_at: DateTime<Utc> = row.get("next_fire_at");
        let graph = parse_graph(row.get("graph"))?;
        let trigger = trigger_of_graph(&graph);
        let next = trigger
            .as_ref()
            .and_then(|trigger| next_fire_for(trigger, now));
        sqlx::query("UPDATE workflows SET next_fire_at=$2 WHERE id=$1")
            .bind(id)
            .bind(next)
            .execute(&mut *tx)
            .await?;
        match trigger {
            Some(WorkflowTrigger::Schedule(_)) => {
                let request = StartRun {
                    project_id,
                    workflow_id: id,
                    version_id: Some(version_id),
                    graph,
                    trigger_kind: "schedule".to_owned(),
                    trigger_data: json!({ "data": { "scheduled_for": due_at, "fired_at": now } }),
                    idempotency_key: format!("schedule:{}", due_at.to_rfc3339()),
                    actor_kind: "system".to_owned(),
                    actor_id: "workflow-schedule".to_owned(),
                    test_mode: false,
                };
                // A savepoint keeps one broken workflow from blocking the rest.
                let mut savepoint = tx.begin().await?;
                match start_run_in_transaction(&mut savepoint, &request).await {
                    Ok(started) => {
                        savepoint.commit().await?;
                        fired += u64::from(started.created);
                    }
                    Err(_) => savepoint.rollback().await?,
                }
            }
            Some(WorkflowTrigger::PublicationAlert { .. }) => alerts.push(DueAlert {
                workflow_id: id,
                project_id,
                version_id,
                graph,
                poll_state: row.get("poll_state"),
            }),
            _ => {}
        }
    }
    tx.commit().await?;
    Ok((fired, alerts))
}

/// One publication found by an alert.
#[derive(Debug, Clone)]
pub struct AlertHit {
    /// Stable identifier used for de-duplication (`doi:...` or `pmid:...`).
    pub identifier: String,
    pub item: Value,
}

/// Store the outcome of one alert poll: unseen publications start a run, the
/// poll status is saved for the canvas to show.
pub async fn record_alert_poll(
    pool: &PgPool,
    alert: &DueAlert,
    hits: Vec<AlertHit>,
    error: Option<&str>,
    poll_state: Value,
) -> Result<u64, WorkflowError> {
    let mut tx = pool.begin().await?;
    let mut fresh: Vec<Value> = Vec::new();
    let mut fresh_ids: Vec<String> = Vec::new();
    for hit in hits {
        let inserted = sqlx::query(
            "INSERT INTO workflow_seen_items (workflow_id, identifier) VALUES ($1,$2)
             ON CONFLICT DO NOTHING",
        )
        .bind(alert.workflow_id)
        .bind(&hit.identifier)
        .execute(&mut *tx)
        .await?
        .rows_affected();
        if inserted == 1 {
            fresh_ids.push(hit.identifier);
            fresh.push(hit.item);
        }
    }
    let mut state = poll_state;
    if let Value::Object(map) = &mut state {
        map.insert("last_polled_at".to_owned(), json!(Utc::now()));
        map.insert("last_error".to_owned(), json!(error));
        map.insert("last_new_count".to_owned(), json!(fresh.len()));
    }
    sqlx::query("UPDATE workflows SET poll_state=$2 WHERE id=$1")
        .bind(alert.workflow_id)
        .bind(state)
        .execute(&mut *tx)
        .await?;
    let mut started = 0;
    if !fresh.is_empty() {
        fresh_ids.sort();
        let digest = sha256_hex(&fresh_ids.join("|"));
        let request = StartRun {
            project_id: alert.project_id,
            workflow_id: alert.workflow_id,
            version_id: Some(alert.version_id),
            graph: alert.graph.clone(),
            trigger_kind: "publication_alert".to_owned(),
            trigger_data: json!({ "records": fresh }),
            idempotency_key: format!("alert:{digest}"),
            actor_kind: "system".to_owned(),
            actor_id: "workflow-alert".to_owned(),
            test_mode: false,
        };
        started = u64::from(start_run_in_transaction(&mut tx, &request).await?.created);
    }
    tx.commit().await?;
    Ok(started)
}

fn sha256_hex(text: &str) -> String {
    // Avoid a direct sha2 dependency: PostgreSQL-independent, tiny FNV-1a pair
    // is enough for an idempotency key over already-unique identifiers.
    let mut h1: u64 = 0xcbf2_9ce4_8422_2325;
    let mut h2: u64 = 0x8422_2325_cbf2_9ce4;
    for byte in text.bytes() {
        h1 ^= u64::from(byte);
        h1 = h1.wrapping_mul(0x0100_0000_01b3);
        h2 = h2.rotate_left(5) ^ u64::from(byte);
        h2 = h2.wrapping_mul(0x0100_0000_01b3);
    }
    format!("{h1:016x}{h2:016x}")
}

// ---------------------------------------------------------------------------
// Platform events

/// Start every enabled workflow listening to this platform event. Called from
/// the same transaction that made the event authoritative. A failure in one
/// workflow never disturbs the source transaction.
pub async fn dispatch_workflow_event(
    tx: &mut Transaction<'_, Postgres>,
    event: &AutomationDomainEvent,
) -> Result<u64, sqlx::Error> {
    let project_id = event.project_id().as_uuid();
    let rows = sqlx::query(
        "SELECT w.id, w.published_version_id, v.graph
         FROM workflows w JOIN workflow_versions v ON v.id = w.published_version_id
         WHERE w.project_id=$1 AND w.trigger_kind=$2 AND w.status='enabled'
         ORDER BY w.id",
    )
    .bind(project_id)
    .bind(event.workflow_trigger_key())
    .fetch_all(&mut **tx)
    .await?;
    if rows.is_empty() {
        return Ok(0);
    }
    let (actor_kind, actor_id) = event.actor();
    let trigger_data = event_trigger_data(tx, event).await;
    let mut created = 0;
    for row in rows {
        let Ok(graph) = parse_graph(row.get("graph")) else {
            continue;
        };
        let request = StartRun {
            project_id,
            workflow_id: row.get("id"),
            version_id: row.get("published_version_id"),
            graph,
            trigger_kind: event.workflow_trigger_key().to_owned(),
            trigger_data: trigger_data.clone(),
            idempotency_key: event.source_identity(),
            actor_kind: actor_kind.as_str().to_owned(),
            actor_id: actor_id.to_owned(),
            test_mode: false,
        };
        let mut savepoint = tx.begin().await?;
        match start_run_in_transaction(&mut savepoint, &request).await {
            Ok(started) => {
                savepoint.commit().await?;
                created += u64::from(started.created);
            }
            Err(_) => savepoint.rollback().await?,
        }
    }
    Ok(created)
}

async fn event_trigger_data(
    tx: &mut Transaction<'_, Postgres>,
    event: &AutomationDomainEvent,
) -> Value {
    let project_id = event.project_id().as_uuid();
    let report_id: Option<Uuid> = match event {
        AutomationDomainEvent::ReportAdded { report_id, .. } => Some(*report_id),
        AutomationDomainEvent::ReportIncluded {
            screening_event_id, ..
        }
        | AutomationDomainEvent::ScreeningDecisionRecorded {
            screening_event_id, ..
        } => sqlx::query_scalar("SELECT report_id FROM screening_events WHERE id=$1")
            .bind(screening_event_id)
            .fetch_optional(&mut **tx)
            .await
            .ok()
            .flatten(),
        AutomationDomainEvent::AppraisalCompleted {
            appraisal_event_id, ..
        } => sqlx::query_scalar("SELECT report_id FROM appraisal_events WHERE id=$1")
            .bind(appraisal_event_id)
            .fetch_optional(&mut **tx)
            .await
            .ok()
            .flatten(),
        AutomationDomainEvent::FullTextAttached { document_id, .. }
        | AutomationDomainEvent::DocumentParsed { document_id, .. } => {
            sqlx::query_scalar("SELECT report_id FROM documents WHERE id=$1")
                .bind(document_id)
                .fetch_optional(&mut **tx)
                .await
                .ok()
                .flatten()
        }
        _ => None,
    };
    let record = match report_id {
        Some(report_id) => record_items_in(tx, project_id, &[report_id])
            .await
            .ok()
            .and_then(|mut items| items.pop()),
        None => None,
    };
    match event {
        AutomationDomainEvent::ReportAdded { .. }
        | AutomationDomainEvent::ReportIncluded { .. }
        | AutomationDomainEvent::AppraisalCompleted { .. }
        | AutomationDomainEvent::ScreeningDecisionRecorded { .. } => {
            json!({ "record": record.unwrap_or(Value::Null) })
        }
        AutomationDomainEvent::FullTextAttached { document_id, .. }
        | AutomationDomainEvent::DocumentParsed { document_id, .. } => json!({
            "document": { "document_id": document_id, "report_id": report_id },
            "record": record.unwrap_or(Value::Null),
        }),
        AutomationDomainEvent::StudyCreated { study_event_id, .. } => {
            let study = sqlx::query(
                "SELECT s.id, s.title, s.design FROM study_events e
                 JOIN studies s ON s.id = e.study_id WHERE e.id=$1",
            )
            .bind(study_event_id)
            .fetch_optional(&mut **tx)
            .await
            .ok()
            .flatten()
            .map(|row| {
                json!({
                    "study_id": row.get::<Uuid, _>("id"),
                    "title": row.get::<String, _>("title"),
                    "design": row.get::<Option<String>, _>("design"),
                })
            });
            json!({ "study": study.unwrap_or(Value::Null) })
        }
        AutomationDomainEvent::AcquisitionCompleted { acquisition_id, .. } => {
            json!({ "data": { "acquisition_id": acquisition_id } })
        }
        AutomationDomainEvent::ProtocolPublished {
            protocol_version_id,
            ..
        } => {
            json!({ "data": { "protocol_version_id": protocol_version_id } })
        }
        AutomationDomainEvent::AiProposalCreated { proposal_id, .. } => {
            json!({ "data": { "proposal_id": proposal_id } })
        }
    }
}

// ---------------------------------------------------------------------------
// Data queries used by blocks

const RECORD_ITEM_SQL: &str = "SELECT jsonb_build_object(
    'report_id', r.id,
    'title', r.title,
    'abstract', r.abstract_text,
    'year', r.publication_year,
    'journal', r.journal,
    'url', r.url,
    'doi', (SELECT i.value FROM report_identifiers i
            WHERE i.report_id = r.id AND i.scheme = 'doi' ORDER BY i.created_at LIMIT 1),
    'pmid', (SELECT i.value FROM report_identifiers i
             WHERE i.report_id = r.id AND i.scheme = 'pmid' ORDER BY i.created_at LIMIT 1),
    'authors', COALESCE((SELECT jsonb_agg(COALESCE(a->>'literal',
                          btrim(concat_ws(' ', a->>'given', a->>'family'))))
                         FROM jsonb_array_elements(r.authors) a), '[]'::jsonb),
    'screening', jsonb_build_object(
        'title_abstract', COALESCE(s.title_abstract_status, 'unscreened'),
        'full_text', COALESCE(s.full_text_status, 'not_required'),
        'final', COALESCE(s.final_status, 'unscreened')),
    'study_id', (SELECT sr.study_id FROM study_reports sr
                 WHERE sr.project_id = pr.project_id AND sr.report_id = r.id LIMIT 1)
  ) AS item
  FROM project_reports pr
  JOIN reports r ON r.id = pr.report_id
  LEFT JOIN screening_state s ON s.project_id = pr.project_id AND s.report_id = pr.report_id";

async fn record_items_in(
    tx: &mut Transaction<'_, Postgres>,
    project_id: Uuid,
    report_ids: &[Uuid],
) -> Result<Vec<Value>, sqlx::Error> {
    sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
        "{RECORD_ITEM_SQL} WHERE pr.project_id=$1 AND r.id = ANY($2)"
    )))
    .bind(project_id)
    .bind(report_ids)
    .fetch_all(&mut **tx)
    .await
}

/// Records of a project by id, in the shape blocks pass around.
pub async fn load_record_items(
    pool: &PgPool,
    project_id: ProjectId,
    report_ids: &[Uuid],
) -> Result<Vec<Value>, WorkflowError> {
    Ok(sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
        "{RECORD_ITEM_SQL} WHERE pr.project_id=$1 AND r.id = ANY($2) ORDER BY pr.created_at, r.id"
    )))
    .bind(project_uuid(project_id))
    .bind(report_ids)
    .fetch_all(pool)
    .await?)
}

#[derive(Debug, Clone)]
pub struct RecordFilter {
    /// `final`, `title_abstract` or `full_text`.
    pub stage: String,
    /// `any`, `unscreened`, `include`, `exclude` or `maybe`.
    pub status: String,
    pub text: Option<String>,
    pub limit: i64,
}

pub async fn find_record_items(
    pool: &PgPool,
    project_id: ProjectId,
    filter: &RecordFilter,
) -> Result<Vec<Value>, WorkflowError> {
    let column = match filter.stage.as_str() {
        "title_abstract" => "COALESCE(s.title_abstract_status, 'unscreened')",
        "full_text" => "COALESCE(s.full_text_status, 'not_required')",
        _ => "COALESCE(s.final_status, 'unscreened')",
    };
    let pattern = filter
        .text
        .as_deref()
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .map(|text| {
            format!(
                "%{}%",
                text.replace('\\', "\\\\")
                    .replace('%', "\\%")
                    .replace('_', "\\_")
            )
        });
    Ok(sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
        "{RECORD_ITEM_SQL}
         WHERE pr.project_id=$1
           AND ($2 = 'any' OR {column} = $2)
           AND ($3::text IS NULL
                OR r.title ILIKE $3 OR r.abstract_text ILIKE $3)
         ORDER BY pr.created_at, r.id
         LIMIT $4"
    )))
    .bind(project_uuid(project_id))
    .bind(&filter.status)
    .bind(pattern)
    .bind(filter.limit.clamp(1, 500))
    .fetch_all(pool)
    .await?)
}

/// Records still waiting for a decision at a screening stage.
pub async fn screening_queue_items(
    pool: &PgPool,
    project_id: ProjectId,
    stage: &str,
    limit: i64,
) -> Result<Vec<Value>, WorkflowError> {
    let condition = if stage == "full_text" {
        "COALESCE(s.title_abstract_status,'unscreened') = 'include'
         AND COALESCE(s.full_text_status,'not_required') IN ('unscreened')"
    } else {
        "COALESCE(s.title_abstract_status,'unscreened') = 'unscreened'"
    };
    Ok(sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
        "{RECORD_ITEM_SQL} WHERE pr.project_id=$1 AND {condition}
         ORDER BY pr.created_at, r.id LIMIT $2"
    )))
    .bind(project_uuid(project_id))
    .bind(limit.clamp(1, 500))
    .fetch_all(pool)
    .await?)
}

/// Full details of one report: the record item plus its identifiers and
/// documents.
pub async fn report_details(
    pool: &PgPool,
    project_id: ProjectId,
    report_id: Uuid,
) -> Result<Option<Value>, WorkflowError> {
    let mut items = load_record_items(pool, project_id, &[report_id]).await?;
    let Some(mut item) = items.pop() else {
        return Ok(None);
    };
    let identifiers: Vec<Value> = sqlx::query(
        "SELECT scheme, value FROM report_identifiers WHERE report_id=$1 ORDER BY scheme, value",
    )
    .bind(report_id)
    .fetch_all(pool)
    .await?
    .iter()
    .map(|row| json!({ "scheme": row.get::<String, _>("scheme"), "value": row.get::<String, _>("value") }))
    .collect();
    let documents: Vec<Value> = sqlx::query(
        "SELECT id, status, original_filename FROM documents
         WHERE project_id=$1 AND report_id=$2 ORDER BY created_at",
    )
    .bind(project_uuid(project_id))
    .bind(report_id)
    .fetch_all(pool)
    .await?
    .iter()
    .map(|row| {
        json!({
            "document_id": row.get::<Uuid, _>("id"),
            "status": row.get::<String, _>("status"),
            "filename": row.get::<Option<String>, _>("original_filename"),
        })
    })
    .collect();
    if let Value::Object(map) = &mut item {
        map.insert("identifiers".to_owned(), Value::Array(identifiers));
        map.insert("documents".to_owned(), Value::Array(documents));
    }
    Ok(Some(item))
}

/// The study a report belongs to, if any.
pub async fn study_of_report(
    pool: &PgPool,
    project_id: ProjectId,
    report_id: Uuid,
) -> Result<Option<Value>, WorkflowError> {
    let row = sqlx::query(
        "SELECT s.id, s.title, s.design FROM study_reports sr
         JOIN studies s ON s.project_id = sr.project_id AND s.id = sr.study_id
         WHERE sr.project_id=$1 AND sr.report_id=$2 LIMIT 1",
    )
    .bind(project_uuid(project_id))
    .bind(report_id)
    .fetch_optional(pool)
    .await?;
    Ok(row.map(|row| {
        json!({
            "study_id": row.get::<Uuid, _>("id"),
            "title": row.get::<String, _>("title"),
            "design": row.get::<Option<String>, _>("design"),
        })
    }))
}

/// Report ids already in the project that carry one of these identifiers.
pub async fn find_reports_by_identifier(
    pool: &PgPool,
    project_id: ProjectId,
    scheme: &str,
    normalized_values: &[String],
) -> Result<Vec<Uuid>, WorkflowError> {
    Ok(sqlx::query_scalar(
        "SELECT DISTINCT i.report_id FROM report_identifiers i
         JOIN project_reports pr ON pr.report_id = i.report_id
         WHERE pr.project_id=$1 AND i.scheme=$2 AND i.normalized_value = ANY($3)",
    )
    .bind(project_uuid(project_id))
    .bind(scheme)
    .bind(normalized_values)
    .fetch_all(pool)
    .await?)
}

/// Report ids of an acquisition's records (used after an import).
pub async fn report_ids_of_import(
    pool: &PgPool,
    project_id: ProjectId,
    run_id: Uuid,
) -> Result<Vec<Uuid>, WorkflowError> {
    Ok(sqlx::query_scalar(
        "SELECT DISTINCT r.report_id FROM records r
         WHERE r.project_id=$1 AND r.acquisition_run_id=$2 AND r.report_id IS NOT NULL",
    )
    .bind(project_uuid(project_id))
    .bind(run_id)
    .fetch_all(pool)
    .await?)
}

/// Resolve an exclusion reason by its label or code for a screening stage.
pub async fn find_exclusion_reason(
    pool: &PgPool,
    project_id: ProjectId,
    stage: &str,
    name: &str,
) -> Result<Option<Uuid>, WorkflowError> {
    Ok(sqlx::query_scalar(
        "SELECT id FROM exclusion_reasons
         WHERE project_id=$1 AND stage=$2 AND (lower(label)=lower($3) OR lower(code)=lower($3))
         LIMIT 1",
    )
    .bind(project_uuid(project_id))
    .bind(stage)
    .bind(name.trim())
    .fetch_optional(pool)
    .await?)
}

/// A stored export file.
#[derive(Debug, Clone)]
pub struct WorkflowFile {
    pub id: Uuid,
    pub name: String,
    pub content_type: String,
    pub content: Vec<u8>,
}

pub async fn create_workflow_file(
    pool: &PgPool,
    project_id: Uuid,
    run_id: Uuid,
    node_id: &str,
    name: &str,
    content_type: &str,
    content: &[u8],
) -> Result<Uuid, WorkflowError> {
    Ok(sqlx::query_scalar(
        "INSERT INTO workflow_files (project_id, run_id, node_id, name, content_type, content)
         VALUES ($1,$2,$3,$4,$5,$6) RETURNING id",
    )
    .bind(project_id)
    .bind(run_id)
    .bind(node_id)
    .bind(name)
    .bind(content_type)
    .bind(content)
    .fetch_one(pool)
    .await?)
}

pub async fn get_workflow_file(
    pool: &PgPool,
    project_id: ProjectId,
    file_id: Uuid,
) -> Result<WorkflowFile, WorkflowError> {
    let row = sqlx::query(
        "SELECT id, name, content_type, content FROM workflow_files WHERE project_id=$1 AND id=$2",
    )
    .bind(project_uuid(project_id))
    .bind(file_id)
    .fetch_optional(pool)
    .await?
    .ok_or(WorkflowError::NotFound)?;
    Ok(WorkflowFile {
        id: row.get("id"),
        name: row.get("name"),
        content_type: row.get("content_type"),
        content: row.get("content"),
    })
}

/// Create a notification for a workflow run (blocks write through the shared
/// notifications table).
/// How long a notification counts as a repeat of the same one, for
/// [`notify_from_workflow`]'s `fingerprint`.
const NOTIFICATION_REPEAT_WINDOW: &str = "7 days";

/// Add a notification from a workflow. With a `fingerprint` (see
/// `notification_fingerprint`), an identical notification already in the
/// project within the last week is not added again. Returns whether a new
/// notification was written.
pub async fn notify_from_workflow(
    pool: &PgPool,
    project_id: Uuid,
    severity: &str,
    title: &str,
    body: Option<&str>,
    payload: Value,
    fingerprint: Option<&str>,
) -> Result<bool, WorkflowError> {
    let severity = match severity {
        "success" => crate::NotificationSeverity::Success,
        "warning" => crate::NotificationSeverity::Warning,
        "error" => crate::NotificationSeverity::Error,
        _ => crate::NotificationSeverity::Info,
    };
    let title: String = title.chars().take(200).collect();
    let body: Option<String> = body.map(|text| text.chars().take(500).collect());
    let mut payload = if payload.is_object() {
        payload
    } else {
        json!({})
    };
    let Some(fingerprint) = fingerprint else {
        let draft = crate::NotificationDraft {
            kind: "workflow".to_owned(),
            severity,
            project_id: Some(project_id),
            title,
            body,
            payload,
        };
        crate::record_notification(pool, &draft).await?;
        return Ok(true);
    };
    if let Value::Object(map) = &mut payload {
        map.insert("fingerprint".to_owned(), json!(fingerprint));
    }
    let draft = crate::NotificationDraft {
        kind: "workflow".to_owned(),
        severity,
        project_id: Some(project_id),
        title,
        body,
        payload,
    };
    // The look for a repeat and the write happen under one lock, so two runs that
    // finish together cannot both add the same notification.
    let mut tx = pool.begin().await?;
    sqlx::query("SELECT pg_advisory_xact_lock(hashtext($1))")
        .bind(format!("notification:{project_id}:{fingerprint}"))
        .execute(&mut *tx)
        .await?;
    let repeat: bool = sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
        "SELECT EXISTS (
           SELECT 1 FROM notifications
           WHERE project_id=$1 AND kind='workflow' AND payload->>'fingerprint'=$2
             AND created_at > now() - interval '{NOTIFICATION_REPEAT_WINDOW}')"
    )))
    .bind(project_id)
    .bind(fingerprint)
    .fetch_one(&mut *tx)
    .await?;
    if repeat {
        tx.rollback().await?;
        return Ok(false);
    }
    crate::record_notification_in_transaction(&mut tx, &draft).await?;
    tx.commit().await?;
    Ok(true)
}

/// The graph of a workflow's published version, with the version id.
pub async fn get_published_graph(
    pool: &PgPool,
    workflow_id: Uuid,
) -> Result<(Uuid, WorkflowGraph), WorkflowError> {
    let row = sqlx::query(
        "SELECT v.id, v.graph FROM workflows w
         JOIN workflow_versions v ON v.id = w.published_version_id
         WHERE w.id=$1",
    )
    .bind(workflow_id)
    .fetch_optional(pool)
    .await?
    .ok_or(WorkflowError::NotPublished)?;
    Ok((row.get("id"), parse_graph(row.get("graph"))?))
}
