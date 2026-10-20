//! Operator-confirmed decisions about pre-canonical human actor labels.
//!
//! A label is never inferred from a browser, session, email address or display
//! name. These functions are exposed only through the server operator CLI.

use deepref_domain::UserId;
use serde::{Deserialize, Serialize};
use serde_json::json;
use sqlx::{PgPool, Postgres, Row, Transaction};
use thiserror::Error;
use uuid::Uuid;

use crate::identity::{AuditPrincipal, AuditRequest, insert_admin_audit};

/// Serializes reviewer validation with operator changes to the legacy identity map.
pub(crate) const IDENTITY_MAPPING_LOCK_KEY: i64 = 73_003_203;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LegacyActorKind {
    User,
}

impl LegacyActorKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::User => "user",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "user" => Some(Self::User),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LegacyActorDecisionKind {
    Mapped,
    NonIdentity,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LegacyActorDecision {
    pub actor_kind: LegacyActorKind,
    pub actor_id: String,
    pub decision: LegacyActorDecisionKind,
    pub user_id: UserId,
    pub operator_user_id: UserId,
    pub decided_at: chrono::DateTime<chrono::Utc>,
    pub source: String,
    #[serde(default)]
    pub affected_finalized_cohorts: Vec<Uuid>,
}

#[derive(Debug, Error)]
pub enum LegacyActorError {
    #[error(transparent)]
    Database(#[from] sqlx::Error),
    #[error("legacy actor kind must be user")]
    UnknownKind,
    #[error("legacy actor ID must contain 1 to 200 non-whitespace characters")]
    InvalidActorId,
    #[error("decision source must contain 1 to 200 non-whitespace characters")]
    InvalidSource,
    #[error("operator must be an active user")]
    InactiveOperator,
    #[error("target user does not exist")]
    MissingUser,
    #[error("legacy actor label is already mapped to a different user")]
    ConflictingMapping,
    #[error("legacy actor label is already recorded as not this user")]
    ConflictingNonIdentity,
}

/// Confirm that one historical user label belongs to `user_id`. Repeating the
/// same decision is idempotent and does not add another audit event.
pub async fn map_legacy_actor(
    pool: &PgPool,
    actor_kind: &str,
    actor_id: &str,
    user_id: UserId,
    operator_user_id: UserId,
    source: &str,
) -> Result<LegacyActorDecision, LegacyActorError> {
    let kind = parse_label(actor_kind, actor_id, source)?;
    let source = source.trim();
    let mut tx = pool.begin().await?;
    lock_identity_mappings_exclusive(&mut tx).await?;
    lock_legacy_actor_label(&mut tx, actor_id).await?;
    ensure_operator(&mut tx, operator_user_id).await?;
    ensure_user(&mut tx, user_id).await?;

    let negative_exists: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM legacy_actor_non_identities \
         WHERE legacy_actor_kind=$1 AND legacy_actor_id=$2 AND user_id=$3)",
    )
    .bind(kind.as_str())
    .bind(actor_id)
    .bind(user_id.as_uuid())
    .fetch_one(&mut *tx)
    .await?;
    if negative_exists {
        return Err(LegacyActorError::ConflictingNonIdentity);
    }

    let inserted = sqlx::query(
        "INSERT INTO legacy_actor_mappings \
         (legacy_actor_kind,legacy_actor_id,user_id,confirmed_by,source) \
         VALUES ($1,$2,$3,$4,$5) \
         ON CONFLICT (legacy_actor_kind,legacy_actor_id) DO NOTHING \
         RETURNING confirmed_at",
    )
    .bind(kind.as_str())
    .bind(actor_id)
    .bind(user_id.as_uuid())
    .bind(operator_user_id.as_uuid())
    .bind(source)
    .fetch_optional(&mut *tx)
    .await?;

    let affected_finalized_cohorts = affected_finalized_cohorts(&mut tx, actor_id).await?;

    let (stored_user_id, operator, decided_at, stored_source) = if let Some(row) = inserted {
        insert_admin_audit(
            &mut tx,
            "legacy_actor.mapped",
            AuditPrincipal::User(operator_user_id),
            Some(user_id),
            None,
            None,
            json!({
                "legacy_actor_kind": kind.as_str(),
                "legacy_actor_id": actor_id,
                "source": source,
                "affected_finalized_cohorts": affected_finalized_cohorts,
            }),
            AuditRequest::default(),
        )
        .await?;
        (
            user_id.as_uuid(),
            operator_user_id.as_uuid(),
            row.try_get("confirmed_at")?,
            source.to_owned(),
        )
    } else {
        let row = sqlx::query(
            "SELECT user_id,confirmed_by,confirmed_at,source FROM legacy_actor_mappings \
             WHERE legacy_actor_kind=$1 AND legacy_actor_id=$2",
        )
        .bind(kind.as_str())
        .bind(actor_id)
        .fetch_one(&mut *tx)
        .await?;
        (
            row.try_get("user_id")?,
            row.try_get("confirmed_by")?,
            row.try_get("confirmed_at")?,
            row.try_get("source")?,
        )
    };
    if stored_user_id != user_id.as_uuid() {
        return Err(LegacyActorError::ConflictingMapping);
    }
    tx.commit().await?;
    Ok(LegacyActorDecision {
        actor_kind: kind,
        actor_id: actor_id.to_owned(),
        decision: LegacyActorDecisionKind::Mapped,
        user_id: UserId::new(stored_user_id),
        operator_user_id: UserId::new(operator),
        decided_at,
        source: stored_source,
        affected_finalized_cohorts,
    })
}

/// Record that a label does not belong to a particular user. An existing
/// mapping to another user already has that meaning, but the explicit denial
/// remains useful operator provenance when it was requested.
pub async fn record_legacy_actor_non_identity(
    pool: &PgPool,
    actor_kind: &str,
    actor_id: &str,
    user_id: UserId,
    operator_user_id: UserId,
    source: &str,
) -> Result<LegacyActorDecision, LegacyActorError> {
    let kind = parse_label(actor_kind, actor_id, source)?;
    let source = source.trim();
    let mut tx = pool.begin().await?;
    lock_identity_mappings_exclusive(&mut tx).await?;
    lock_legacy_actor_label(&mut tx, actor_id).await?;
    ensure_operator(&mut tx, operator_user_id).await?;
    ensure_user(&mut tx, user_id).await?;

    let mapped_to_user: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM legacy_actor_mappings \
         WHERE legacy_actor_kind=$1 AND legacy_actor_id=$2 AND user_id=$3)",
    )
    .bind(kind.as_str())
    .bind(actor_id)
    .bind(user_id.as_uuid())
    .fetch_one(&mut *tx)
    .await?;
    if mapped_to_user {
        return Err(LegacyActorError::ConflictingMapping);
    }

    let inserted = sqlx::query(
        "INSERT INTO legacy_actor_non_identities \
         (legacy_actor_kind,legacy_actor_id,user_id,recorded_by,source) \
         VALUES ($1,$2,$3,$4,$5) \
         ON CONFLICT (legacy_actor_kind,legacy_actor_id,user_id) DO NOTHING \
         RETURNING recorded_at",
    )
    .bind(kind.as_str())
    .bind(actor_id)
    .bind(user_id.as_uuid())
    .bind(operator_user_id.as_uuid())
    .bind(source)
    .fetch_optional(&mut *tx)
    .await?;

    let (operator, decided_at, stored_source) = if let Some(row) = inserted {
        insert_admin_audit(
            &mut tx,
            "legacy_actor.non_identity_recorded",
            AuditPrincipal::User(operator_user_id),
            Some(user_id),
            None,
            None,
            json!({
                "legacy_actor_kind": kind.as_str(),
                "legacy_actor_id": actor_id,
                "source": source,
                "affected_finalized_cohorts": [],
            }),
            AuditRequest::default(),
        )
        .await?;
        (
            operator_user_id.as_uuid(),
            row.try_get("recorded_at")?,
            source.to_owned(),
        )
    } else {
        let row = sqlx::query(
            "SELECT recorded_by,recorded_at,source FROM legacy_actor_non_identities \
             WHERE legacy_actor_kind=$1 AND legacy_actor_id=$2 AND user_id=$3",
        )
        .bind(kind.as_str())
        .bind(actor_id)
        .bind(user_id.as_uuid())
        .fetch_one(&mut *tx)
        .await?;
        (
            row.try_get("recorded_by")?,
            row.try_get("recorded_at")?,
            row.try_get("source")?,
        )
    };
    tx.commit().await?;
    Ok(LegacyActorDecision {
        actor_kind: kind,
        actor_id: actor_id.to_owned(),
        decision: LegacyActorDecisionKind::NonIdentity,
        user_id,
        operator_user_id: UserId::new(operator),
        decided_at,
        source: stored_source,
        affected_finalized_cohorts: Vec::new(),
    })
}

pub async fn list_legacy_actor_decisions(
    pool: &PgPool,
) -> Result<Vec<LegacyActorDecision>, LegacyActorError> {
    let rows = sqlx::query(
        "SELECT legacy_actor_kind,legacy_actor_id,'mapped' AS decision,user_id,confirmed_by AS operator_user_id,confirmed_at AS decided_at,source \
           FROM legacy_actor_mappings \
         UNION ALL \
         SELECT legacy_actor_kind,legacy_actor_id,'non_identity',user_id,recorded_by,recorded_at,source \
           FROM legacy_actor_non_identities \
         ORDER BY legacy_actor_kind,legacy_actor_id,user_id,decision",
    )
    .fetch_all(pool)
    .await?;
    rows.into_iter()
        .map(|row| {
            let kind_text: String = row.try_get("legacy_actor_kind")?;
            let kind = LegacyActorKind::parse(&kind_text).ok_or(sqlx::Error::ColumnDecode {
                index: "legacy_actor_kind".to_owned(),
                source: Box::new(std::io::Error::other("unknown legacy actor kind")),
            })?;
            let decision_text: String = row.try_get("decision")?;
            let decision = match decision_text.as_str() {
                "mapped" => LegacyActorDecisionKind::Mapped,
                "non_identity" => LegacyActorDecisionKind::NonIdentity,
                _ => {
                    return Err(sqlx::Error::ColumnDecode {
                        index: "decision".to_owned(),
                        source: Box::new(std::io::Error::other("unknown legacy actor decision")),
                    }
                    .into());
                }
            };
            Ok(LegacyActorDecision {
                actor_kind: kind,
                actor_id: row.try_get("legacy_actor_id")?,
                decision,
                user_id: UserId::new(row.try_get("user_id")?),
                operator_user_id: UserId::new(row.try_get("operator_user_id")?),
                decided_at: row.try_get("decided_at")?,
                source: row.try_get("source")?,
                affected_finalized_cohorts: Vec::new(),
            })
        })
        .collect()
}

fn parse_label(
    actor_kind: &str,
    actor_id: &str,
    source: &str,
) -> Result<LegacyActorKind, LegacyActorError> {
    let kind = LegacyActorKind::parse(actor_kind).ok_or(LegacyActorError::UnknownKind)?;
    if actor_id.trim().is_empty() || actor_id.len() > 200 {
        return Err(LegacyActorError::InvalidActorId);
    }
    if source.trim().is_empty() || source.trim().len() > 200 {
        return Err(LegacyActorError::InvalidSource);
    }
    Ok(kind)
}

async fn lock_legacy_actor_label(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    actor_id: &str,
) -> Result<(), LegacyActorError> {
    // Serialize map and denial decisions for this label. Hash collisions only
    // cause harmless extra serialization, never merge distinct decisions.
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 0))")
        .bind(actor_id)
        .fetch_one(&mut **tx)
        .await?;
    Ok(())
}

pub(crate) async fn lock_identity_mappings_shared(
    tx: &mut Transaction<'_, Postgres>,
) -> Result<(), sqlx::Error> {
    sqlx::query("SELECT pg_advisory_xact_lock_shared($1::bigint)")
        .bind(IDENTITY_MAPPING_LOCK_KEY)
        .fetch_one(&mut **tx)
        .await?;
    Ok(())
}

async fn lock_identity_mappings_exclusive(
    tx: &mut Transaction<'_, Postgres>,
) -> Result<(), sqlx::Error> {
    sqlx::query("SELECT pg_advisory_xact_lock($1::bigint)")
        .bind(IDENTITY_MAPPING_LOCK_KEY)
        .fetch_one(&mut **tx)
        .await?;
    Ok(())
}

async fn affected_finalized_cohorts(
    tx: &mut Transaction<'_, Postgres>,
    actor_id: &str,
) -> Result<Vec<Uuid>, sqlx::Error> {
    sqlx::query_scalar(
        "SELECT DISTINCT c.id
         FROM ai_screening_cohorts c
         JOIN ai_screening_audit_labels l ON l.cohort_id=c.id
         JOIN ai_screening_cohort_members m
           ON m.cohort_id=l.cohort_id AND m.report_id=l.report_id
         WHERE c.status='finalized' AND (m.sampled OR m.audit_control)
           AND l.actor_id=$1
           AND NOT ai_first_audit_has_two_distinct_reviewers(c.id,l.report_id)
         ORDER BY c.id",
    )
    .bind(actor_id)
    .fetch_all(&mut **tx)
    .await
}

async fn ensure_operator(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    operator: UserId,
) -> Result<(), LegacyActorError> {
    let active: bool =
        sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM users WHERE id=$1 AND status='active')")
            .bind(operator.as_uuid())
            .fetch_one(&mut **tx)
            .await?;
    if !active {
        return Err(LegacyActorError::InactiveOperator);
    }
    Ok(())
}

async fn ensure_user(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    user: UserId,
) -> Result<(), LegacyActorError> {
    let exists: bool = sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM users WHERE id=$1)")
        .bind(user.as_uuid())
        .fetch_one(&mut **tx)
        .await?;
    if !exists {
        return Err(LegacyActorError::MissingUser);
    }
    Ok(())
}
