//! Durable assistant agent runs: execution owned by the worker.
//!
//! HTTP persists the user message, creates a queued run with fixed output
//! identities (answer message id, plan id) and enqueues a durable job. The
//! worker claims the job, transitions the run to running, executes the Rig
//! agent, persists batched events, the effect log, the final answer and the
//! plan, then marks the run terminal.
//!
//! Retry safety: a recovered lease re-drives a `running` run by continuing
//! its append-only event log, but terminal runs are never re-executed, and
//! the final message and plan are written idempotently under their fixed
//! ids. One run always yields at most one final answer and one plan.
//! Terminal redeliveries settle nothing and append nothing.

use chrono::{DateTime, Utc};
use serde_json::{Value, json};
use sqlx::{PgPool, Postgres, Row, Transaction, postgres::PgRow};
use uuid::Uuid;

use crate::{AppendAssistantMessage, AssistantError, AssistantMessageRecord, NewAssistantPlan};

/// Assistant agent job kind in the durable PostgreSQL job queue.
pub const ASSISTANT_AGENT_RUN_JOB_KIND: &str = "assistant_agent_run";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AssistantAgentRunStatus {
    Queued,
    Running,
    Completed,
    Failed,
    Cancelled,
}

impl AssistantAgentRunStatus {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Running => "running",
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
        }
    }

    fn parse(value: &str) -> Option<Self> {
        match value {
            "queued" => Some(Self::Queued),
            "running" => Some(Self::Running),
            "completed" => Some(Self::Completed),
            "failed" => Some(Self::Failed),
            "cancelled" => Some(Self::Cancelled),
            _ => None,
        }
    }

    pub const fn terminal(self) -> bool {
        matches!(self, Self::Completed | Self::Failed | Self::Cancelled)
    }
}

#[derive(Debug, Clone)]
pub struct NewAssistantAgentRun {
    pub id: Uuid,
    pub conversation_id: Uuid,
    pub project_id: Uuid,
    pub trigger_message_id: Uuid,
    pub actor_kind: String,
    pub actor_id: String,
    pub model_route: Value,
    pub semantic_contract_id: Option<String>,
    pub runtime_version: String,
    pub build_provenance: Value,
    pub answer_message_id: Uuid,
    pub plan_id: Uuid,
}

#[derive(Debug, Clone)]
pub struct AssistantAgentRunRecord {
    pub id: Uuid,
    pub conversation_id: Uuid,
    pub project_id: Uuid,
    pub trigger_message_id: Uuid,
    pub actor_kind: String,
    pub actor_id: String,
    pub status: AssistantAgentRunStatus,
    pub model_route: Value,
    pub semantic_contract_id: Option<String>,
    pub runtime_version: String,
    pub build_provenance: Value,
    pub rig_run: Option<Value>,
    pub effect_log: Option<Value>,
    pub run_spec_hash: Option<String>,
    pub answer_message_id: Option<Uuid>,
    pub plan_id: Option<Uuid>,
    pub error: Option<Value>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub completed_at: Option<DateTime<Utc>>,
}

fn run_from_row(row: &PgRow) -> Result<AssistantAgentRunRecord, AssistantError> {
    let status: String = row.get("status");
    let status = AssistantAgentRunStatus::parse(&status).ok_or_else(|| {
        AssistantError::InvalidInput(format!("unknown assistant run status {status}"))
    })?;
    Ok(AssistantAgentRunRecord {
        id: row.get("id"),
        conversation_id: row.get("conversation_id"),
        project_id: row.get("project_id"),
        trigger_message_id: row.get("trigger_message_id"),
        actor_kind: row.get("actor_kind"),
        actor_id: row.get("actor_id"),
        status,
        model_route: row.get("model_route"),
        semantic_contract_id: row.get("semantic_contract_id"),
        runtime_version: row.get("runtime_version"),
        build_provenance: row.get("build_provenance"),
        rig_run: row.get("rig_run"),
        effect_log: row.get("effect_log"),
        run_spec_hash: row.get("run_spec_hash"),
        answer_message_id: row.get("answer_message_id"),
        plan_id: row.get("plan_id"),
        error: row.get("error"),
        created_at: row.get("created_at"),
        updated_at: row.get("updated_at"),
        completed_at: row.get("completed_at"),
    })
}

pub async fn create_assistant_agent_run(
    pool: &PgPool,
    input: &NewAssistantAgentRun,
) -> Result<AssistantAgentRunRecord, AssistantError> {
    let mut tx = pool.begin().await?;
    let record = create_assistant_agent_run_tx(&mut tx, input).await;
    match record {
        Ok(record) => {
            tx.commit().await?;
            Ok(record)
        }
        Err(error) => {
            drop(tx);
            Err(
                clarify_run_reference_error(pool, input.project_id, input.conversation_id, error)
                    .await,
            )
        }
    }
}

/// The run insert names its missing reference only as a foreign-key
/// violation. When the conversation or project vanished between validation
/// and insert (a delete racing submit), report which one so callers answer
/// 404 instead of 400. Anything else (notably a bogus trigger message id
/// passed straight to [`create_assistant_agent_run`]) keeps its error.
async fn clarify_run_reference_error(
    pool: &PgPool,
    project_id: Uuid,
    conversation_id: Uuid,
    error: AssistantError,
) -> AssistantError {
    let AssistantError::InvalidInput(ref message) = error else {
        return error;
    };
    if message != RUN_REFERENCE_MISSING {
        return error;
    }
    if !crate::project_exists(pool, project_id)
        .await
        .unwrap_or(true)
    {
        return AssistantError::ProjectNotFound;
    }
    match crate::get_assistant_conversation(pool, project_id, conversation_id).await {
        Ok(_) => error,
        Err(AssistantError::ConversationNotFound) => AssistantError::ConversationNotFound,
        Err(other) => other,
    }
}

/// Persists the user message, the queued run and the durable job in one
/// transaction: HTTP submits a turn atomically, so a crash can never leave a
/// queued run without a job or a job without its run.
#[allow(clippy::too_many_arguments)]
pub async fn submit_assistant_agent_run(
    pool: &PgPool,
    project_id: Uuid,
    conversation_id: Uuid,
    content: &str,
    actor_kind: &str,
    actor_id: &str,
    model_route: &Value,
    runtime_version: &str,
) -> Result<AssistantAgentRunRecord, AssistantError> {
    let mut tx = pool.begin().await?;
    let trigger = crate::assistant::append_assistant_message_tx(
        &mut tx,
        &AppendAssistantMessage {
            id: None,
            conversation_id,
            role: "user".to_owned(),
            content: content.to_owned(),
            tool_calls: None,
            tool_results: None,
            metadata: None,
        },
    )
    .await?;
    let run_id = Uuid::new_v4();
    let record = create_assistant_agent_run_tx(
        &mut tx,
        &NewAssistantAgentRun {
            id: run_id,
            conversation_id,
            project_id,
            trigger_message_id: trigger.id,
            actor_kind: actor_kind.to_owned(),
            actor_id: actor_id.to_owned(),
            model_route: model_route.clone(),
            // Assistant turns are not calibrated reviews: no contract.
            semantic_contract_id: None,
            runtime_version: runtime_version.to_owned(),
            build_provenance: Value::Object(Default::default()),
            answer_message_id: Uuid::new_v4(),
            plan_id: Uuid::new_v4(),
        },
    )
    .await;
    let record = match record {
        Ok(record) => record,
        Err(error) => {
            drop(tx);
            return Err(
                clarify_run_reference_error(pool, project_id, conversation_id, error).await,
            );
        }
    };
    crate::jobs::enqueue_job(
        &mut tx,
        &crate::jobs::job(
            Uuid::new_v4(),
            deepref_domain::ProjectId::new(project_id),
            ASSISTANT_AGENT_RUN_JOB_KIND,
            serde_json::json!({ "assistant_run_id": run_id }),
            format!("assistant-run:{run_id}"),
        ),
    )
    .await
    .map_err(|error| {
        AssistantError::InvalidInput(format!("assistant job enqueue failed: {error}"))
    })?;
    tx.commit().await?;
    Ok(record)
}

/// Foreign-key signal for a run insert whose conversation, project or
/// trigger message is missing. [`clarify_run_reference_error`] refines this
/// into a 404-grade error when a racing delete removed the conversation or
/// project.
const RUN_REFERENCE_MISSING: &str =
    "assistant run references a missing conversation, project or message";

pub(crate) async fn create_assistant_agent_run_tx(
    tx: &mut Transaction<'_, Postgres>,
    input: &NewAssistantAgentRun,
) -> Result<AssistantAgentRunRecord, AssistantError> {
    if input.id.is_nil()
        || input.conversation_id.is_nil()
        || input.project_id.is_nil()
        || input.trigger_message_id.is_nil()
        || input.answer_message_id.is_nil()
        || input.plan_id.is_nil()
    {
        return Err(AssistantError::InvalidInput(
            "assistant run identifiers must not be nil".to_owned(),
        ));
    }
    if !input.model_route.is_object() {
        return Err(AssistantError::InvalidInput(
            "assistant run model route must be an object".to_owned(),
        ));
    }
    let row = sqlx::query(
        "INSERT INTO assistant_agent_runs
           (id, conversation_id, project_id, trigger_message_id, actor_kind, actor_id,
            status, model_route, semantic_contract_id, runtime_version, build_provenance,
            answer_message_id, plan_id)
         VALUES ($1,$2,$3,$4,$5,$6,'queued',$7,$8,$9,$10,$11,$12)
         RETURNING id",
    )
    .bind(input.id)
    .bind(input.conversation_id)
    .bind(input.project_id)
    .bind(input.trigger_message_id)
    .bind(&input.actor_kind)
    .bind(&input.actor_id)
    .bind(&input.model_route)
    .bind(&input.semantic_contract_id)
    .bind(&input.runtime_version)
    .bind(&input.build_provenance)
    .bind(input.answer_message_id)
    .bind(input.plan_id)
    .fetch_one(&mut **tx)
    .await
    .map_err(|err| {
        if let sqlx::Error::Database(ref db_err) = err
            && db_err.code().as_deref() == Some("23503")
        {
            return AssistantError::InvalidInput(RUN_REFERENCE_MISSING.to_owned());
        }
        AssistantError::Database(err)
    })?;
    let id: Uuid = row.get("id");
    let row = sqlx::query(
        "SELECT id, conversation_id, project_id, trigger_message_id, actor_kind, actor_id, \
          status, model_route, semantic_contract_id, runtime_version, build_provenance, \
          rig_run, effect_log, run_spec_hash, answer_message_id, plan_id, error, \
          created_at, updated_at, completed_at FROM assistant_agent_runs WHERE id=$1",
    )
    .bind(id)
    .fetch_one(&mut **tx)
    .await?;
    run_from_row(&row)
}

pub async fn get_assistant_agent_run(
    pool: &PgPool,
    run_id: Uuid,
) -> Result<Option<AssistantAgentRunRecord>, AssistantError> {
    let row = sqlx::query(
        "SELECT id, conversation_id, project_id, trigger_message_id, actor_kind, actor_id, \
          status, model_route, semantic_contract_id, runtime_version, build_provenance, \
          rig_run, effect_log, run_spec_hash, answer_message_id, plan_id, error, \
          created_at, updated_at, completed_at FROM assistant_agent_runs WHERE id=$1",
    )
    .bind(run_id)
    .fetch_optional(pool)
    .await?;
    row.map(|row| run_from_row(&row)).transpose()
}

/// Inputs a worker needs after claiming a run.
#[derive(Debug, Clone)]
pub struct ClaimedAssistantAgentRun {
    pub record: AssistantAgentRunRecord,
    pub user_message: AssistantMessageRecord,
    pub history: Vec<AssistantMessageRecord>,
}

/// Takes a queued run (or takes over a running one after lease recovery)
/// for execution. Terminal runs are never re-executed: the caller must ack
/// the job without work.
///
/// Recovery never deletes events. The log is append-only per run with
/// gap-free monotonic seqs starting at 0, so live SSE observers holding an
/// `after_seq` stay valid across a redrive. When prior events exist, one
/// `status` boundary row scopes the new attempt: everything after it belongs
/// to the latest drive.
pub async fn begin_assistant_agent_run(
    pool: &PgPool,
    run_id: Uuid,
) -> Result<Option<ClaimedAssistantAgentRun>, AssistantError> {
    let mut tx = pool.begin().await?;
    let row = sqlx::query(
        "SELECT id, conversation_id, project_id, trigger_message_id, actor_kind, actor_id, \
          status, model_route, semantic_contract_id, runtime_version, build_provenance, \
          rig_run, effect_log, run_spec_hash, answer_message_id, plan_id, error, \
          created_at, updated_at, completed_at FROM assistant_agent_runs WHERE id=$1 FOR UPDATE",
    )
    .bind(run_id)
    .fetch_optional(&mut *tx)
    .await?;
    let Some(row) = row else {
        return Ok(None);
    };
    let record = run_from_row(&row)?;
    if record.status.terminal() {
        return Ok(None);
    }
    sqlx::query(
        "UPDATE assistant_agent_runs
         SET status='running', updated_at=now()
         WHERE id=$1 AND status IN ('queued','running')",
    )
    .bind(run_id)
    .execute(&mut *tx)
    .await?;
    let next_seq: Option<i64> =
        sqlx::query_scalar("SELECT MAX(seq) FROM assistant_run_events WHERE run_id=$1")
            .bind(run_id)
            .fetch_one(&mut *tx)
            .await?;
    let history = sqlx::query(
        "SELECT id, conversation_id, role, content, tool_calls, tool_results, metadata, created_at
         FROM assistant_messages
         WHERE conversation_id=$1
         ORDER BY created_at ASC",
    )
    .bind(record.conversation_id)
    .fetch_all(&mut *tx)
    .await?
    .iter()
    .map(|row| {
        Ok::<AssistantMessageRecord, AssistantError>(AssistantMessageRecord {
            id: row.get("id"),
            conversation_id: row.get("conversation_id"),
            role: row.get("role"),
            content: row.get("content"),
            tool_calls: row.get("tool_calls"),
            tool_results: row.get("tool_results"),
            metadata: row.get("metadata"),
            created_at: row.get("created_at"),
        })
    })
    .collect::<Result<Vec<_>, _>>()?;
    let user_message = history
        .iter()
        .find(|message| message.id == record.trigger_message_id)
        .cloned()
        .ok_or_else(|| {
            AssistantError::InvalidInput("assistant run trigger message is missing".to_owned())
        })?;
    if let Some(seq) = next_seq {
        // A redrive continues the same monotonic log: mark where the new
        // attempt starts so observers never mistake two attempts' tool
        // events for one. Fresh runs (no prior events) need no marker.
        sqlx::query(
            "INSERT INTO assistant_run_events (run_id, seq, kind, payload)
             VALUES ($1,$2,'status',$3)",
        )
        .bind(run_id)
        .bind(seq + 1)
        .bind(json!({"message": "Retrying after interruption"}))
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;
    Ok(Some(ClaimedAssistantAgentRun {
        record,
        user_message,
        history,
    }))
}

/// Final answer plus plan persistence for a completed run. Idempotent under
/// the run's fixed output identities: a retried worker reuses the same
/// message and plan rows, so one run yields at most one final answer and
/// one plan.
pub struct CompletedAssistantAgentRun {
    pub answer: AppendAssistantMessage,
    pub plan: Option<NewAssistantPlan>,
    pub effect_log: Option<Value>,
    pub run_spec_hash: Option<String>,
}

pub async fn complete_assistant_agent_run(
    pool: &PgPool,
    run_id: Uuid,
    completion: &CompletedAssistantAgentRun,
) -> Result<bool, AssistantError> {
    let mut tx = pool.begin().await?;
    let status: Option<String> =
        sqlx::query_scalar("SELECT status FROM assistant_agent_runs WHERE id=$1 FOR UPDATE")
            .bind(run_id)
            .fetch_optional(&mut *tx)
            .await?;
    if status.as_deref() != Some("running") {
        return Ok(false);
    }
    ensure_message(&mut tx, &completion.answer).await?;
    if let Some(plan) = &completion.plan {
        ensure_plan(&mut tx, plan).await?;
    }
    sqlx::query(
        "UPDATE assistant_agent_runs
         SET status='completed', effect_log=$2, run_spec_hash=$3,
             updated_at=now(), completed_at=now()
         WHERE id=$1",
    )
    .bind(run_id)
    .bind(&completion.effect_log)
    .bind(&completion.run_spec_hash)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(true)
}

async fn ensure_message(
    tx: &mut Transaction<'_, Postgres>,
    message: &AppendAssistantMessage,
) -> Result<(), AssistantError> {
    let id = message.id.unwrap_or_else(Uuid::new_v4);
    let exists: bool =
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM assistant_messages WHERE id=$1)")
            .bind(id)
            .fetch_one(&mut **tx)
            .await?;
    if exists {
        return Ok(());
    }
    crate::assistant::append_assistant_message_tx(tx, message).await?;
    Ok(())
}

async fn ensure_plan(
    tx: &mut Transaction<'_, Postgres>,
    plan: &NewAssistantPlan,
) -> Result<(), AssistantError> {
    let exists: bool =
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM assistant_plans WHERE id=$1)")
            .bind(plan.id)
            .fetch_one(&mut **tx)
            .await?;
    if exists {
        return Ok(());
    }
    crate::assistant_plans::create_assistant_plan_tx(tx, plan).await?;
    Ok(())
}

/// Marks a running run failed with a machine-readable error and records the
/// matching `error` event in the same transaction. Runs that already settled
/// (or are missing) are left alone and nothing is appended: a duplicate
/// delivery after settlement must never add a bogus error event. No
/// assistant message is appended on failure, matching the synchronous turn
/// behavior.
pub async fn fail_assistant_agent_run(
    pool: &PgPool,
    run_id: Uuid,
    error: &Value,
) -> Result<bool, AssistantError> {
    let mut tx = pool.begin().await?;
    let status: Option<String> =
        sqlx::query_scalar("SELECT status FROM assistant_agent_runs WHERE id=$1 FOR UPDATE")
            .bind(run_id)
            .fetch_optional(&mut *tx)
            .await?;
    if status.as_deref() != Some("running") {
        return Ok(false);
    }
    let seq: Option<i64> =
        sqlx::query_scalar("SELECT MAX(seq) FROM assistant_run_events WHERE run_id=$1")
            .bind(run_id)
            .fetch_one(&mut *tx)
            .await?;
    sqlx::query(
        "INSERT INTO assistant_run_events (run_id, seq, kind, payload)
         VALUES ($1,$2,'error',$3)",
    )
    .bind(run_id)
    .bind(seq.unwrap_or(-1) + 1)
    .bind(error)
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        "UPDATE assistant_agent_runs
         SET status='failed', error=$2, updated_at=now(), completed_at=now()
         WHERE id=$1",
    )
    .bind(run_id)
    .bind(error)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(true)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssistantRunEventRecord {
    pub run_id: Uuid,
    pub seq: i64,
    pub kind: String,
    pub payload: Value,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

/// Appends one run event with the next sequence number.
///
/// Concurrency invariant: the claimed job lease serializes drivers, so at
/// most one worker appends to a run at a time. `MAX(seq)+1` is allocated
/// inside the insert transaction and `PRIMARY KEY (run_id, seq)` (migration
/// 0057) rejects any concurrent duplicate: a raced append fails loudly and
/// its driver stands down instead of interleaving seqs silently, while the
/// surviving driver continues the same monotonic log.
pub async fn append_assistant_run_event(
    pool: &PgPool,
    run_id: Uuid,
    kind: &str,
    payload: &Value,
) -> Result<i64, AssistantError> {
    let mut tx = pool.begin().await?;
    let seq: Option<i64> =
        sqlx::query_scalar("SELECT MAX(seq) FROM assistant_run_events WHERE run_id=$1")
            .bind(run_id)
            .fetch_one(&mut *tx)
            .await?;
    let seq = seq.unwrap_or(-1) + 1;
    sqlx::query(
        "INSERT INTO assistant_run_events (run_id, seq, kind, payload) VALUES ($1,$2,$3,$4)",
    )
    .bind(run_id)
    .bind(seq)
    .bind(kind)
    .bind(payload)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(seq)
}

pub async fn list_assistant_run_events(
    pool: &PgPool,
    run_id: Uuid,
    after_seq: i64,
    limit: i64,
) -> Result<Vec<AssistantRunEventRecord>, AssistantError> {
    let rows = sqlx::query(
        "SELECT run_id, seq, kind, payload, created_at
         FROM assistant_run_events
         WHERE run_id=$1 AND seq>$2
         ORDER BY seq ASC
         LIMIT $3",
    )
    .bind(run_id)
    .bind(after_seq)
    .bind(limit)
    .fetch_all(pool)
    .await?;
    rows.iter()
        .map(|row| {
            Ok(AssistantRunEventRecord {
                run_id: row.get("run_id"),
                seq: row.get("seq"),
                kind: row.get("kind"),
                payload: row.get("payload"),
                created_at: row.get("created_at"),
            })
        })
        .collect()
}
