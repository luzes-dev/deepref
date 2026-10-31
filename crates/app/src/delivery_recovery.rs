//! Operator recovery of terminal deliveries: quarantined outbox events and
//! dead-lettered (quarantined) inbox events. Listing is bounded and keyset
//! paginated; inspection never returns payloads. Replay is audited in
//! `delivery_recovery_audit` in the same transaction as the state change, and
//! keyed by an operator-supplied request id so a retried command is a no-op
//! that returns the first outcome.
use anyhow::Context;
use chrono::{DateTime, Utc};
use serde::Serialize;
use sqlx::{PgPool, Row};
use uuid::Uuid;

pub const MAX_LIST_LIMIT: i64 = 100;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DeliveryKind {
    Outbox,
    Inbox,
}

impl DeliveryKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Outbox => "outbox",
            Self::Inbox => "inbox",
        }
    }
}

impl std::str::FromStr for DeliveryKind {
    type Err = anyhow::Error;
    fn from_str(value: &str) -> anyhow::Result<Self> {
        match value {
            "outbox" => Ok(Self::Outbox),
            "inbox" => Ok(Self::Inbox),
            _ => anyhow::bail!("delivery kind must be outbox or inbox"),
        }
    }
}

/// A terminal delivery reference: an outbox event UUID or an inbox row id.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeliveryRef {
    Outbox(Uuid),
    Inbox(i64),
}

impl DeliveryRef {
    pub fn parse(kind: DeliveryKind, value: &str) -> anyhow::Result<Self> {
        Ok(match kind {
            DeliveryKind::Outbox => {
                Self::Outbox(value.parse().context("invalid outbox event UUID")?)
            }
            DeliveryKind::Inbox => {
                let id: i64 = value.parse().context("invalid inbox event id")?;
                anyhow::ensure!(id > 0, "inbox event id must be positive");
                Self::Inbox(id)
            }
        })
    }

    pub fn kind(&self) -> DeliveryKind {
        match self {
            Self::Outbox(_) => DeliveryKind::Outbox,
            Self::Inbox(_) => DeliveryKind::Inbox,
        }
    }

    fn text(&self) -> String {
        match self {
            Self::Outbox(id) => id.to_string(),
            Self::Inbox(id) => id.to_string(),
        }
    }
}

/// One terminal delivery. Error text is truncated; payloads are never read.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct TerminalDelivery {
    pub kind: DeliveryKind,
    pub reference: String,
    pub message_type: String,
    pub source: Option<String>,
    pub state: String,
    pub attempts: i32,
    pub max_attempts: i32,
    pub created_at: DateTime<Utc>,
    pub last_error: Option<String>,
    pub quarantined_consumers: Vec<String>,
}

const ERROR_PREVIEW: i32 = 300;

fn bounded_limit(limit: i64) -> anyhow::Result<i64> {
    anyhow::ensure!(
        (1..=MAX_LIST_LIMIT).contains(&limit),
        "limit must be 1..={MAX_LIST_LIMIT}"
    );
    Ok(limit)
}

/// Quarantined rows of `kind`, oldest first, after the optional cursor.
pub async fn list_terminal_deliveries(
    pool: &PgPool,
    kind: DeliveryKind,
    after: Option<&str>,
    limit: i64,
) -> anyhow::Result<Vec<TerminalDelivery>> {
    let limit = bounded_limit(limit)?;
    match kind {
        DeliveryKind::Outbox => {
            let after = after
                .map(|value| value.parse::<Uuid>().context("invalid --after outbox UUID"))
                .transpose()?;
            let rows = sqlx::query(OUTBOX_SELECT_LIST)
                .bind(after)
                .bind(limit)
                .bind(ERROR_PREVIEW)
                .fetch_all(pool)
                .await?;
            rows.iter().map(outbox_row).collect()
        }
        DeliveryKind::Inbox => {
            let after = after
                .map(|value| value.parse::<i64>().context("invalid --after inbox id"))
                .transpose()?;
            let rows = sqlx::query(INBOX_SELECT_LIST)
                .bind(after)
                .bind(limit)
                .bind(ERROR_PREVIEW)
                .fetch_all(pool)
                .await?;
            rows.iter().map(inbox_row).collect()
        }
    }
}

/// One delivery in any state, or `None` when absent.
pub async fn inspect_delivery(
    pool: &PgPool,
    reference: &DeliveryRef,
) -> anyhow::Result<Option<TerminalDelivery>> {
    match reference {
        DeliveryRef::Outbox(id) => sqlx::query(OUTBOX_SELECT_ONE)
            .bind(id)
            .bind(ERROR_PREVIEW)
            .fetch_optional(pool)
            .await?
            .as_ref()
            .map(outbox_row)
            .transpose(),
        DeliveryRef::Inbox(id) => sqlx::query(INBOX_SELECT_ONE)
            .bind(id)
            .bind(ERROR_PREVIEW)
            .fetch_optional(pool)
            .await?
            .as_ref()
            .map(inbox_row)
            .transpose(),
    }
}

const OUTBOX_SELECT_LIST: &str = "SELECT e.event_id, e.message_type, e.delivery_state,
        e.attempts, e.max_attempts, e.created_at, left(e.last_error, $3) AS last_error,
        COALESCE((SELECT array_agg(r.consumer ORDER BY r.consumer) FROM outbox_receipts r
                  WHERE r.event_id = e.event_id AND r.state = 'quarantined'), '{}') AS consumers
     FROM outbox_events e
     WHERE e.delivery_state = 'quarantined' AND ($1::uuid IS NULL OR e.event_id > $1)
     ORDER BY e.event_id LIMIT $2";
const OUTBOX_SELECT_ONE: &str = "SELECT e.event_id, e.message_type, e.delivery_state,
        e.attempts, e.max_attempts, e.created_at, left(e.last_error, $2) AS last_error,
        COALESCE((SELECT array_agg(r.consumer ORDER BY r.consumer) FROM outbox_receipts r
                  WHERE r.event_id = e.event_id AND r.state = 'quarantined'), '{}') AS consumers
     FROM outbox_events e WHERE e.event_id = $1";
const INBOX_SELECT_LIST: &str = "SELECT id, provider, event_type, processing_state, attempts,
    max_attempts, received_at, left(COALESCE(quarantine_reason, last_error), $3) AS last_error
    FROM integration_inbox_events
    WHERE processing_state = 'quarantined' AND ($1::bigint IS NULL OR id > $1)
    ORDER BY id LIMIT $2";
const INBOX_SELECT_ONE: &str = "SELECT id, provider, event_type, processing_state, attempts,
    max_attempts, received_at, left(COALESCE(quarantine_reason, last_error), $2) AS last_error
    FROM integration_inbox_events WHERE id = $1";

fn outbox_row(row: &sqlx::postgres::PgRow) -> anyhow::Result<TerminalDelivery> {
    Ok(TerminalDelivery {
        kind: DeliveryKind::Outbox,
        reference: row.try_get::<Uuid, _>("event_id")?.to_string(),
        message_type: row.try_get("message_type")?,
        source: None,
        state: row.try_get("delivery_state")?,
        attempts: row.try_get("attempts")?,
        max_attempts: row.try_get("max_attempts")?,
        created_at: row.try_get("created_at")?,
        last_error: row.try_get("last_error")?,
        quarantined_consumers: row.try_get("consumers")?,
    })
}

fn inbox_row(row: &sqlx::postgres::PgRow) -> anyhow::Result<TerminalDelivery> {
    Ok(TerminalDelivery {
        kind: DeliveryKind::Inbox,
        reference: row.try_get::<i64, _>("id")?.to_string(),
        message_type: row.try_get("event_type")?,
        source: Some(row.try_get("provider")?),
        state: row.try_get("processing_state")?,
        attempts: row.try_get("attempts")?,
        max_attempts: row.try_get("max_attempts")?,
        created_at: row.try_get("received_at")?,
        last_error: row.try_get("last_error")?,
        quarantined_consumers: Vec::new(),
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ReplayOutcome {
    /// The row was quarantined and is now due for delivery again.
    Replayed,
    /// The row was absent or not quarantined; nothing changed.
    NotQuarantined,
}

impl ReplayOutcome {
    fn as_str(self) -> &'static str {
        match self {
            Self::Replayed => "replayed",
            Self::NotQuarantined => "not_quarantined",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ReplayReceipt {
    pub request_id: Uuid,
    pub kind: DeliveryKind,
    pub reference: String,
    pub outcome: ReplayOutcome,
    /// True when this request id was already recorded; nothing changed now.
    pub repeated_request: bool,
}

/// Replay one terminal delivery and audit it atomically. Reusing a request id
/// for a different delivery is refused.
pub async fn replay_terminal_delivery(
    pool: &PgPool,
    reference: &DeliveryRef,
    request_id: Uuid,
    operator_user_id: Uuid,
    reason: &str,
) -> anyhow::Result<ReplayReceipt> {
    let reason = reason.trim();
    anyhow::ensure!(
        (1..=500).contains(&reason.chars().count()),
        "reason must be 1..=500 characters"
    );
    let kind = reference.kind();
    let reference_text = reference.text();
    let mut tx = pool.begin().await?;
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1::text, 0))")
        .bind(request_id.to_string())
        .execute(&mut *tx)
        .await?;
    if let Some(row) = sqlx::query(
        "SELECT delivery_kind, delivery_ref, outcome FROM delivery_recovery_audit WHERE request_id = $1",
    )
    .bind(request_id)
    .fetch_optional(&mut *tx)
    .await?
    {
        let recorded_kind: String = row.try_get("delivery_kind")?;
        let recorded_ref: String = row.try_get("delivery_ref")?;
        anyhow::ensure!(
            recorded_kind == kind.as_str() && recorded_ref == reference_text,
            "request id {request_id} was already used for a different delivery"
        );
        let outcome: String = row.try_get("outcome")?;
        tx.rollback().await?;
        return Ok(ReplayReceipt {
            request_id,
            kind,
            reference: reference_text,
            outcome: if outcome == "replayed" {
                ReplayOutcome::Replayed
            } else {
                ReplayOutcome::NotQuarantined
            },
            repeated_request: true,
        });
    }
    let replayed = match reference {
        DeliveryRef::Outbox(id) => crate::outbox::replay_outbox_event_in(&mut tx, *id).await?,
        DeliveryRef::Inbox(id) => crate::inbox::requeue_inbox_event_in(&mut tx, *id).await?,
    };
    let outcome = if replayed {
        ReplayOutcome::Replayed
    } else {
        ReplayOutcome::NotQuarantined
    };
    sqlx::query(
        "INSERT INTO delivery_recovery_audit
           (request_id, operator_user_id, delivery_kind, delivery_ref, outcome, reason)
         VALUES ($1, $2, $3, $4, $5, $6)",
    )
    .bind(request_id)
    .bind(operator_user_id)
    .bind(kind.as_str())
    .bind(&reference_text)
    .bind(outcome.as_str())
    .bind(reason)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    tracing::info!(
        kind = kind.as_str(),
        outcome = outcome.as_str(),
        "operator replayed a terminal delivery"
    );
    Ok(ReplayReceipt {
        request_id,
        kind,
        reference: reference_text,
        outcome,
        repeated_request: false,
    })
}

/// A parsed `deepref-server deliveries` action.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeliveryAction {
    List {
        kind: DeliveryKind,
        after: Option<String>,
        limit: i64,
    },
    Inspect {
        reference: DeliveryRef,
    },
    Replay {
        reference: DeliveryRef,
        request_id: Uuid,
        operator_user_id: Uuid,
        reason: String,
    },
}

pub const USAGE: &str = "usage: deepref-server deliveries list <outbox|inbox> [--after <cursor>] [--limit <1..100>] | deliveries inspect <outbox|inbox> <id> | deliveries replay <outbox|inbox> <id> --request <uuid> --operator <user-uuid> --reason <text>";

impl DeliveryAction {
    pub fn parse(args: &[String]) -> anyhow::Result<Self> {
        let [action, kind, rest @ ..] = args else {
            anyhow::bail!(USAGE);
        };
        let kind: DeliveryKind = kind.parse().context(USAGE)?;
        match action.as_str() {
            "list" => {
                let mut after = None;
                let mut limit = None;
                let mut flags = rest.iter();
                while let Some(flag) = flags.next() {
                    let value = flags.next().context(USAGE)?;
                    match flag.as_str() {
                        "--after" if after.is_none() => after = Some(value.clone()),
                        "--limit" if limit.is_none() => {
                            limit = Some(bounded_limit(value.parse().context(USAGE)?)?)
                        }
                        _ => anyhow::bail!(USAGE),
                    }
                }
                Ok(Self::List {
                    kind,
                    after,
                    limit: limit.unwrap_or(25),
                })
            }
            "inspect" => match rest {
                [id] => Ok(Self::Inspect {
                    reference: DeliveryRef::parse(kind, id)?,
                }),
                _ => anyhow::bail!(USAGE),
            },
            "replay" => {
                let [id, flags @ ..] = rest else {
                    anyhow::bail!(USAGE);
                };
                let mut request = None;
                let mut operator = None;
                let mut reason = None;
                let mut flags = flags.iter();
                while let Some(flag) = flags.next() {
                    let value = flags.next().context(USAGE)?;
                    match flag.as_str() {
                        "--request" if request.is_none() => {
                            request = Some(value.parse().context("invalid --request UUID")?)
                        }
                        "--operator" if operator.is_none() => {
                            operator = Some(value.parse().context("invalid --operator UUID")?)
                        }
                        "--reason" if reason.is_none() => reason = Some(value.clone()),
                        _ => anyhow::bail!(USAGE),
                    }
                }
                Ok(Self::Replay {
                    reference: DeliveryRef::parse(kind, id)?,
                    request_id: request.context(USAGE)?,
                    operator_user_id: operator.context(USAGE)?,
                    reason: reason.context(USAGE)?,
                })
            }
            _ => anyhow::bail!(USAGE),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(items: &[&str]) -> Vec<String> {
        items.iter().map(|item| (*item).to_owned()).collect()
    }

    #[test]
    fn parser_bounds_listing_and_requires_audit_fields_for_replay() {
        assert_eq!(
            DeliveryAction::parse(&args(&["list", "inbox"])).unwrap(),
            DeliveryAction::List {
                kind: DeliveryKind::Inbox,
                after: None,
                limit: 25
            }
        );
        let request = Uuid::from_u128(1);
        let operator = Uuid::from_u128(2);
        assert_eq!(
            DeliveryAction::parse(&args(&[
                "replay",
                "inbox",
                "7",
                "--request",
                &request.to_string(),
                "--operator",
                &operator.to_string(),
                "--reason",
                "provider fixed",
            ]))
            .unwrap(),
            DeliveryAction::Replay {
                reference: DeliveryRef::Inbox(7),
                request_id: request,
                operator_user_id: operator,
                reason: "provider fixed".into()
            }
        );
        for bad in [
            vec!["list", "inbox", "--limit", "0"],
            vec!["list", "inbox", "--limit", "101"],
            vec!["list", "jobs"],
            vec!["inspect", "outbox", "not-a-uuid"],
            vec!["inspect", "inbox", "0"],
            vec![
                "replay",
                "inbox",
                "7",
                "--operator",
                &operator.to_string(),
                "--reason",
                "x",
            ],
            vec![
                "replay",
                "inbox",
                "7",
                "--request",
                &request.to_string(),
                "--reason",
                "x",
            ],
            vec![
                "replay",
                "inbox",
                "7",
                "--request",
                &request.to_string(),
                "--operator",
                &operator.to_string(),
            ],
            vec!["purge", "inbox"],
        ] {
            assert!(DeliveryAction::parse(&args(&bad)).is_err(), "{bad:?}");
        }
    }
}
