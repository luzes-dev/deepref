//! Provider-neutral AI usage fact export (handoff 09 slice B).
//!
//! Export emits by replaying retained usage facts, never from the settlement
//! path. Each immutable usage fact id (the ledger row id, or a correction
//! record id) is the outbox event id, so `insert_outbox_event`'s
//! `ON CONFLICT (event_id) DO NOTHING` makes re-emission a committed no-op and
//! exactly one outbox event exists per fact. There is no export state table and
//! no scan cursor: "pending emission" is derived from the absence of that
//! outbox row, so no cursor can fall behind a late-committing settlement
//! transaction and skip a fact forever. Concurrent replay runs the same
//! derived scan and converge on the same single row per fact.
//!
//! The replay is bounded (one call inspects at most `limit` rows per fact
//! source) and keysets retained facts by `(settled_at, id)`. Facts settled
//! while export is disabled are emitted by a later replay, never lost.

use std::sync::Arc;

use anyhow::Context;
use chrono::{DateTime, Utc};
use deepref_application::{
    outbox::{
        DeliveryContext, NewOutboxEvent, OutboxHandlerError, OutboxMessage, OutboxRegistry,
        TypedOutboxHandler,
    },
    usage_export::{
        AiUsageFact, USAGE_EXPORT_CONSUMER, USAGE_EXPORT_MAX_ATTEMPTS, UsageExportCounters,
        UsageExportOutcome, UsageExportSink, UsagePrincipalKind,
    },
};
use serde::{Deserialize, Serialize};
use sqlx::{PgPool, Postgres, Row, Transaction};
use uuid::Uuid;

use crate::outbox::{insert_outbox_event, record_outbox_receipt};

/// What one bounded replay call inspects.
#[derive(Debug, Clone, Copy)]
pub struct UsageExportReplayParams {
    /// The deployment's export switch. While it is false the replay emits
    /// nothing at all; the retained facts wait and a later replay catches up.
    pub enabled: bool,
    /// Lower bound on the fact settlement time to consider.
    pub since: Option<DateTime<Utc>>,
    /// Most fact rows and most correction rows this call inspects.
    pub limit: i64,
}

impl UsageExportReplayParams {
    pub fn enabled(limit: i64) -> Self {
        Self {
            enabled: true,
            since: None,
            limit,
        }
    }

    pub fn disabled() -> Self {
        Self {
            enabled: false,
            since: None,
            limit: 1,
        }
    }
}

/// Export backlog as SQL reports it. Counters carry no workspace or cost
/// labels; ages are relative to `now()` on the database. A fact counts as
/// emitted only when the outbox event with its id *is* its export event; an
/// unrelated event sharing the id is an identity conflict, not emission.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct UsageExportBacklog {
    /// Outbox events of the usage fact contract, emitted so far.
    pub emitted_events: i64,
    /// Emitted events not yet delivered to a sink.
    pub pending_events: i64,
    pub oldest_pending_age_seconds: Option<i64>,
    /// Attributable usage facts without an export event.
    pub unemitted_facts: i64,
    /// Correction records without an export event.
    pub unemitted_corrections: i64,
    pub oldest_unemitted_age_seconds: Option<i64>,
    /// Facts whose id is held by an outbox event that is not their export
    /// event. They are reported here and refused by the replay; never counted
    /// as emitted.
    pub conflicting_facts: i64,
    /// Facts no workspace can be resolved for. They are reported, never
    /// silently exported to no customer and never hidden.
    pub unattributed_facts: i64,
}

/// What one bounded replay call did.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UsageExportReplaySummary {
    pub enabled: bool,
    pub facts_scanned: u64,
    pub facts_emitted: u64,
    pub facts_already_emitted: u64,
    pub corrections_scanned: u64,
    pub corrections_emitted: u64,
    pub corrections_already_emitted: u64,
    /// True when both scans reached the end of their retained history within
    /// the call's limit.
    pub complete: bool,
    pub backlog: UsageExportBacklog,
}

/// Export failures the caller can act on. An outbox event that is not this
/// fact's export event is a collision, never a successful earlier emission:
/// the replay refuses to lose the fact silently.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum UsageExportError {
    #[error(
        "usage fact {fact_id} collides with outbox event {fact_id} held by {message_type}; refusing to treat it as emitted"
    )]
    EventIdentityConflict { fact_id: Uuid, message_type: String },
    #[error("usage export replay failed: {0}")]
    Replay(String),
}

impl From<anyhow::Error> for UsageExportError {
    fn from(error: anyhow::Error) -> Self {
        Self::Replay(format!("{error:#}"))
    }
}

impl From<serde_json::Error> for UsageExportError {
    fn from(error: serde_json::Error) -> Self {
        Self::Replay(error.to_string())
    }
}

impl From<sqlx::Error> for UsageExportError {
    fn from(error: sqlx::Error) -> Self {
        Self::Replay(error.to_string())
    }
}

/// Which retained source one scan page reads. Both project the same columns,
/// so one row reader builds both event shapes.
#[derive(Debug, Clone, Copy)]
enum ScanTarget {
    Facts,
    Corrections,
}

impl ScanTarget {
    const fn sql(self) -> &'static str {
        match self {
            // `settled_at` is the committed fact timestamp. Legacy facts keep
            // their own creation time; reservation facts were written in the
            // settlement transaction that stamped their reservation.
            Self::Facts => {
                r#"
                SELECT l.id AS fact_id, NULL::uuid AS correction_of,
                       COALESCE(l.workspace_id, p.workspace_id) AS workspace_id,
                       l.project_id, l.principal_kind,
                       l.provider, l.model,
                       l.input_tokens, l.output_tokens, l.reasoning_tokens,
                       l.cost_micros, l.commercial_units, l.price_version,
                       COALESCE(l.period_start,
                         date_trunc('month', l.created_at AT TIME ZONE 'UTC')::date) AS period_start,
                       l.created_at AS settled_at
                FROM ai_usage_ledger l
                LEFT JOIN projects p ON p.id = l.project_id
                WHERE COALESCE(l.workspace_id, p.workspace_id) IS NOT NULL
                  AND NOT EXISTS (
                    SELECT 1 FROM outbox_events e
                    WHERE e.event_id = l.id AND e.message_type = $5
                      AND e.payload->>'fact_id' = l.id::text)
                  AND ($1::timestamptz IS NULL OR l.created_at >= $1)
                  AND ($2::timestamptz IS NULL OR (l.created_at, l.id) > ($2::timestamptz, $3::uuid))
                ORDER BY l.created_at, l.id
                LIMIT $4"#
            }
            // A correction is its own immutable fact referencing the
            // original. It repeats the original's identity and usage counters
            // with the corrected cost and price version; the original event is
            // never edited.
            Self::Corrections => {
                r#"
                SELECT c.id AS fact_id, c.ledger_id AS correction_of,
                       COALESCE(l.workspace_id, p.workspace_id) AS workspace_id,
                       l.project_id, l.principal_kind,
                       l.provider, l.model,
                       l.input_tokens, l.output_tokens, l.reasoning_tokens,
                       c.corrected_cost_micros AS cost_micros, l.commercial_units,
                       c.corrected_price_version AS price_version,
                       COALESCE(l.period_start,
                         date_trunc('month', l.created_at AT TIME ZONE 'UTC')::date) AS period_start,
                       c.created_at AS settled_at
                FROM ai_usage_corrections c
                JOIN ai_usage_ledger l ON l.id = c.ledger_id
                LEFT JOIN projects p ON p.id = l.project_id
                WHERE COALESCE(l.workspace_id, p.workspace_id) IS NOT NULL
                  AND NOT EXISTS (
                    SELECT 1 FROM outbox_events e
                    WHERE e.event_id = c.id AND e.message_type = $5
                      AND e.payload->>'fact_id' = c.id::text)
                  AND ($1::timestamptz IS NULL OR c.created_at >= $1)
                  AND ($2::timestamptz IS NULL OR (c.created_at, c.id) > ($2::timestamptz, $3::uuid))
                ORDER BY c.created_at, c.id
                LIMIT $4"#
            }
        }
    }
}

/// One scan page plus whether the scan reached the end of its window.
struct ScanPage {
    facts: Vec<AiUsageFact>,
    complete: bool,
}

/// Rows one scan statement reads at most. The replay limit bounds the whole
/// call; pages keep each transaction small so a long replay stays resumable.
const REPLAY_PAGE_ROWS: u64 = 200;

async fn scan_page(
    tx: &mut Transaction<'_, Postgres>,
    target: ScanTarget,
    params: &UsageExportReplayParams,
    cursor: Option<(DateTime<Utc>, Uuid)>,
    page_rows: i64,
) -> anyhow::Result<ScanPage> {
    let (cursor_time, cursor_id) = match cursor {
        Some((time, id)) => (Some(time), Some(id)),
        None => (None, None),
    };
    let rows = sqlx::query(target.sql())
        .bind(params.since)
        .bind(cursor_time)
        .bind(cursor_id)
        .bind(page_rows)
        .bind(AiUsageFact::TYPE)
        .fetch_all(&mut **tx)
        .await?;
    let complete = rows.len() < usize::try_from(page_rows).context("page size fits usize")?;
    let facts = rows
        .into_iter()
        .map(|row| {
            let principal_kind: Option<String> = row.get("principal_kind");
            let principal_kind = match principal_kind.as_deref() {
                Some(value) => Some(
                    UsagePrincipalKind::parse(value)
                        .ok_or_else(|| anyhow::anyhow!("unknown usage principal kind {value}"))?,
                ),
                None => None,
            };
            Ok(AiUsageFact {
                fact_id: row.get("fact_id"),
                workspace_id: row.get("workspace_id"),
                project_id: row.get("project_id"),
                principal_kind,
                provider: row.get("provider"),
                model: row.get("model"),
                input_tokens: row.get("input_tokens"),
                output_tokens: row.get("output_tokens"),
                reasoning_tokens: row.get("reasoning_tokens"),
                provider_cost_micros: row.get("cost_micros"),
                commercial_units: row.get("commercial_units"),
                // Null only for facts settled before price versions existed.
                price_version: row.get("price_version"),
                period_start: row.get("period_start"),
                settled_at: row.get("settled_at"),
                correction_of: row.get("correction_of"),
            })
        })
        .collect::<anyhow::Result<Vec<_>>>()?;
    Ok(ScanPage { facts, complete })
}

#[derive(Debug, Default)]
struct TargetCounts {
    scanned: u64,
    emitted: u64,
    already_emitted: u64,
}

/// Insert one fact's export event. `false` means this fact's event already
/// exists (a replayed or concurrent emission). An outbox row that is not this
/// fact's event is an identity collision and raises instead of being treated
/// as a successful earlier emission.
async fn insert_fact_event(
    tx: &mut Transaction<'_, Postgres>,
    fact: &AiUsageFact,
) -> Result<bool, UsageExportError> {
    let event = NewOutboxEvent::for_message(fact.fact_id, fact)?
        .with_max_attempts(USAGE_EXPORT_MAX_ATTEMPTS);
    if insert_outbox_event(tx, &event).await? {
        return Ok(true);
    }
    let existing = sqlx::query(
        "SELECT message_type, payload->>'fact_id' AS fact_id FROM outbox_events WHERE event_id = $1",
    )
    .bind(fact.fact_id)
    .fetch_optional(&mut **tx)
    .await?;
    let this_fact = existing.as_ref().is_some_and(|row| {
        row.get::<String, _>("message_type") == AiUsageFact::TYPE
            && row.get::<Option<String>, _>("fact_id").as_deref()
                == Some(fact.fact_id.to_string().as_str())
    });
    if this_fact {
        return Ok(false);
    }
    Err(UsageExportError::EventIdentityConflict {
        fact_id: fact.fact_id,
        message_type: existing
            .map(|row| row.get::<String, _>("message_type"))
            .unwrap_or_else(|| "<missing>".to_owned()),
    })
}

async fn emit_target(
    pool: &PgPool,
    counters: &UsageExportCounters,
    target: ScanTarget,
    params: &UsageExportReplayParams,
) -> Result<(TargetCounts, bool), UsageExportError> {
    let limit = u64::try_from(params.limit).context("limit fits u64")?;
    let mut counts = TargetCounts::default();
    let mut cursor: Option<(DateTime<Utc>, Uuid)> = None;
    loop {
        let remaining = limit.saturating_sub(counts.scanned).max(1);
        let page_rows =
            i64::try_from(remaining.min(REPLAY_PAGE_ROWS)).context("page size fits i64")?;
        let mut tx = pool.begin().await?;
        let page = scan_page(&mut tx, target, params, cursor, page_rows).await?;
        let mut emitted = 0_u64;
        for fact in &page.facts {
            if insert_fact_event(&mut tx, fact).await? {
                emitted = emitted.saturating_add(1);
            } else {
                counts.already_emitted = counts.already_emitted.saturating_add(1);
            }
            cursor = Some((fact.settled_at, fact.fact_id));
        }
        tx.commit().await?;
        counters.record_emitted(emitted);
        counts.emitted = counts.emitted.saturating_add(emitted);
        counts.scanned = counts.scanned.saturating_add(page.facts.len() as u64);
        if page.complete || page.facts.is_empty() {
            return Ok((counts, true));
        }
        if counts.scanned >= limit {
            return Ok((counts, false));
        }
    }
}

/// Emit one bounded batch of missing export events. Idempotent and safe to run
/// concurrently: re-emission of an already emitted fact is a committed no-op.
pub async fn replay_usage_facts(
    pool: &PgPool,
    counters: &UsageExportCounters,
    params: UsageExportReplayParams,
) -> Result<UsageExportReplaySummary, UsageExportError> {
    if params.limit <= 0 {
        return Err(UsageExportError::Replay(
            "replay limit must be positive".to_owned(),
        ));
    }
    let backlog = usage_export_backlog(pool).await?;
    if backlog.unattributed_facts > 0 {
        tracing::warn!(
            unattributed_facts = backlog.unattributed_facts,
            "usage facts without a resolvable workspace are not exported; attribute them first"
        );
    }
    if !params.enabled {
        return Ok(UsageExportReplaySummary {
            enabled: false,
            facts_scanned: 0,
            facts_emitted: 0,
            facts_already_emitted: 0,
            corrections_scanned: 0,
            corrections_emitted: 0,
            corrections_already_emitted: 0,
            complete: backlog.unemitted_facts == 0 && backlog.unemitted_corrections == 0,
            backlog,
        });
    }
    let (facts, facts_complete) = emit_target(pool, counters, ScanTarget::Facts, &params).await?;
    let (corrections, corrections_complete) =
        emit_target(pool, counters, ScanTarget::Corrections, &params).await?;
    let backlog = usage_export_backlog(pool).await?;
    Ok(UsageExportReplaySummary {
        enabled: true,
        facts_scanned: facts.scanned,
        facts_emitted: facts.emitted,
        facts_already_emitted: facts.already_emitted,
        corrections_scanned: corrections.scanned,
        corrections_emitted: corrections.emitted,
        corrections_already_emitted: corrections.already_emitted,
        complete: facts_complete && corrections_complete,
        backlog,
    })
}

/// Current export backlog, including identity conflicts and facts no workspace
/// resolves for. A fact is emitted only when its outbox event is its export
/// event.
pub async fn usage_export_backlog(pool: &PgPool) -> anyhow::Result<UsageExportBacklog> {
    let message_type = AiUsageFact::TYPE;
    let row = sqlx::query(
        r#"
        WITH facts AS (
          SELECT l.id AS fact_id, l.created_at AS settled_at, 'usage'::text AS fact_kind,
                 COALESCE(l.workspace_id, p.workspace_id) AS workspace_id
          FROM ai_usage_ledger l
          LEFT JOIN projects p ON p.id = l.project_id
          UNION ALL
          SELECT c.id, c.created_at, 'correction',
                 COALESCE(l.workspace_id, p.workspace_id)
          FROM ai_usage_corrections c
          JOIN ai_usage_ledger l ON l.id = c.ledger_id
          LEFT JOIN projects p ON p.id = l.project_id
        ),
        emission AS (
          SELECT f.*,
                 EXISTS (SELECT 1 FROM outbox_events e
                   WHERE e.event_id = f.fact_id AND e.message_type = $1
                     AND e.payload->>'fact_id' = f.fact_id::text) AS emitted,
                 EXISTS (SELECT 1 FROM outbox_events e WHERE e.event_id = f.fact_id)
                   AS identity_taken
          FROM facts f
        )
        SELECT
          (SELECT count(*) FROM outbox_events
            WHERE message_type = $1 AND message_version = 1)::bigint AS emitted_events,
          (SELECT count(*) FROM outbox_events
            WHERE message_type = $1 AND message_version = 1
              AND delivery_state <> 'delivered')::bigint AS pending_events,
          (SELECT EXTRACT(EPOCH FROM (now() - min(created_at)))::bigint FROM outbox_events
            WHERE message_type = $1 AND message_version = 1
              AND delivery_state <> 'delivered') AS oldest_pending_age_seconds,
          (SELECT count(*) FROM emission
            WHERE fact_kind = 'usage' AND workspace_id IS NOT NULL
              AND NOT emitted)::bigint AS unemitted_facts,
          (SELECT count(*) FROM emission
            WHERE fact_kind = 'correction' AND workspace_id IS NOT NULL
              AND NOT emitted)::bigint AS unemitted_corrections,
          (SELECT EXTRACT(EPOCH FROM (now() - min(settled_at)))::bigint FROM emission
            WHERE workspace_id IS NOT NULL AND NOT emitted) AS oldest_unemitted_age_seconds,
          (SELECT count(*) FROM emission
            WHERE NOT emitted AND identity_taken)::bigint AS conflicting_facts,
          (SELECT count(*) FROM emission
            WHERE workspace_id IS NULL AND NOT emitted)::bigint AS unattributed_facts
        "#,
    )
    .bind(message_type)
    .fetch_one(pool)
    .await?;
    Ok(UsageExportBacklog {
        emitted_events: row.get("emitted_events"),
        pending_events: row.get("pending_events"),
        oldest_pending_age_seconds: row.get("oldest_pending_age_seconds"),
        unemitted_facts: row.get("unemitted_facts"),
        unemitted_corrections: row.get("unemitted_corrections"),
        oldest_unemitted_age_seconds: row.get("oldest_unemitted_age_seconds"),
        conflicting_facts: row.get("conflicting_facts"),
        unattributed_facts: row.get("unattributed_facts"),
    })
}

/// The `billing_usage_export` consumer: deliver one fact event, then record
/// the receipt in the same transaction. Delivery is at-least-once and the sink
/// receives the same event id on every attempt; the row is marked delivered
/// only after the sink reports [`UsageExportOutcome::Accepted`] or
/// [`UsageExportOutcome::Duplicate`]. An [`UsageExportOutcome::Ambiguous`]
/// outcome is not success: it retries with the same event id.
pub fn billing_usage_export_handler(
    pool: &PgPool,
    sink: Arc<dyn UsageExportSink>,
    counters: Arc<UsageExportCounters>,
) -> TypedOutboxHandler<AiUsageFact> {
    let pool = pool.clone();
    Arc::new(move |fact: AiUsageFact, context: DeliveryContext| {
        let pool = pool.clone();
        let sink = Arc::clone(&sink);
        let counters = Arc::clone(&counters);
        Box::pin(async move {
            if fact.fact_id != context.event_id {
                return Err(OutboxHandlerError::Terminal(
                    "usage fact id does not match its outbox event id".to_owned(),
                ));
            }
            let outcome = sink.deliver(&fact).await;
            counters.record(&outcome);
            tracing::info!(
                event_id = %context.event_id,
                outcome = outcome.label(),
                "usage export fact attempted"
            );
            match &outcome {
                UsageExportOutcome::Accepted | UsageExportOutcome::Duplicate => {
                    let mut tx = pool
                        .begin()
                        .await
                        .map_err(|error| OutboxHandlerError::Transient(error.to_string()))?;
                    record_outbox_receipt(&mut tx, &context)
                        .await
                        .map_err(|error| OutboxHandlerError::Transient(error.to_string()))?;
                    tx.commit()
                        .await
                        .map_err(|error| OutboxHandlerError::Transient(error.to_string()))?;
                    Ok(())
                }
                UsageExportOutcome::RetryableFailure(_) | UsageExportOutcome::Ambiguous(_) => {
                    Err(OutboxHandlerError::Transient(outcome.describe()))
                }
                UsageExportOutcome::PermanentFailure(_) => {
                    Err(OutboxHandlerError::Terminal(outcome.describe()))
                }
            }
        })
    })
}

/// The outbox registry of an enabled usage export: one message contract with
/// its single `billing_usage_export` consumer. While export is disabled the
/// caller registers nothing, so emitted rows would wait without spending an
/// attempt budget — but the emitter is gated too, so no rows exist at all.
pub fn usage_export_registry(
    pool: &PgPool,
    sink: Arc<dyn UsageExportSink>,
    counters: Arc<UsageExportCounters>,
) -> anyhow::Result<OutboxRegistry> {
    let mut registry = OutboxRegistry::new();
    registry
        .register::<AiUsageFact>(&[(
            USAGE_EXPORT_CONSUMER,
            billing_usage_export_handler(pool, sink, counters),
        )])
        .context("register the usage export contract")?;
    Ok(registry)
}
