//! Workspace machine credentials. Only a verifier is persisted. Lifecycle
//! writes lock the workspace and live manager facts, then use the central PDP.
//! No key is a browser session or an accountable human scientific actor.

use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use chrono::{DateTime, Utc};
use deepref_application::{
    authorization::{Authorizer, AuthzError, PolicyDecisionPoint},
    identity::OpaqueSecret,
};
use deepref_domain::{
    ApiKeyId, Principal, UserId, WorkspaceId,
    authorization::{Permission, Resource},
};
use hmac::{Hmac, KeyInit, Mac};
use serde::Serialize;
use serde_json::json;
use sha2::Sha256;
use sqlx::{PgPool, Postgres, Row, Transaction};
use uuid::Uuid;

use crate::{
    authorization::TransactionAuthorizationStore,
    identity::{AuditPrincipal, AuditRequest, insert_admin_audit},
};

pub const KEY_PREFIX: &str = "dr_key_v1_";
const VERIFIER_CONTEXT: &[u8] = b"deepref-api-key-v1";
const MAX_ACTIVE_KEYS: i64 = 100;

#[derive(Debug, thiserror::Error)]
pub enum ApiKeyError {
    #[error("invalid API key name, scopes or expiry")]
    Invalid,
    #[error("resource not found")]
    Denied,
    #[error("workspace API key limit reached")]
    Limit,
    #[error("secure randomness is unavailable")]
    Random,
    #[error(transparent)]
    Database(#[from] sqlx::Error),
    #[error(transparent)]
    Authorization(#[from] AuthzError),
}

/// Validated boundary input. Machine writes are intentionally not enabled:
/// scientific decisions must keep honest human provenance.
#[derive(Debug)]
pub struct KeySpec {
    name: String,
    scopes: Vec<Permission>,
    expires_at: Option<DateTime<Utc>>,
}

impl KeySpec {
    pub fn parse(
        name: String,
        scopes: Vec<String>,
        expires_at: Option<DateTime<Utc>>,
        now: DateTime<Utc>,
    ) -> Result<Self, ApiKeyError> {
        let name = name.trim().to_owned();
        if name.is_empty()
            || name.chars().count() > 100
            || name.chars().any(char::is_control)
            || scopes.is_empty()
            || scopes.len() > 2
            || expires_at.is_some_and(|at| at <= now)
        {
            return Err(ApiKeyError::Invalid);
        }
        let mut parsed = Vec::with_capacity(scopes.len());
        for raw in scopes {
            let scope = Permission::parse(&raw)
                .filter(|scope| scope.machine_allowed())
                .ok_or(ApiKeyError::Invalid)?;
            if parsed.contains(&scope) {
                return Err(ApiKeyError::Invalid);
            }
            parsed.push(scope);
        }
        Ok(Self {
            name,
            scopes: parsed,
            expires_at,
        })
    }
}

#[derive(Debug, Serialize)]
pub struct ApiKeyRecord {
    pub id: Uuid,
    pub workspace_id: Uuid,
    pub created_by_user_id: Uuid,
    pub name: String,
    pub prefix: String,
    pub scopes: Vec<String>,
    pub expires_at: Option<DateTime<Utc>>,
    pub last_used_at: Option<DateTime<Utc>>,
    pub revoked_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
}

/// The only result containing the full credential. Debug is redacted by the
/// secret wrapper; list/get operations cannot reconstruct it from a record.
#[derive(Debug)]
pub struct IssuedApiKey {
    pub record: ApiKeyRecord,
    pub secret: OpaqueSecret,
}

fn record(row: &sqlx::postgres::PgRow) -> Result<ApiKeyRecord, sqlx::Error> {
    Ok(ApiKeyRecord {
        id: row.try_get("id")?,
        workspace_id: row.try_get("workspace_id")?,
        created_by_user_id: row.try_get("created_by_user_id")?,
        name: row.try_get("name")?,
        prefix: row.try_get("prefix")?,
        scopes: row.try_get("scopes")?,
        expires_at: row.try_get("expires_at")?,
        last_used_at: row.try_get("last_used_at")?,
        revoked_at: row.try_get("revoked_at")?,
        created_at: row.try_get("created_at")?,
    })
}

fn verifier(secret: &[u8]) -> Result<Hmac<Sha256>, ApiKeyError> {
    let mut mac =
        <Hmac<Sha256> as KeyInit>::new_from_slice(secret).map_err(|_| ApiKeyError::Invalid)?;
    mac.update(VERIFIER_CONTEXT);
    Ok(mac)
}

struct Generated {
    id: Uuid,
    hash: Vec<u8>,
    token: OpaqueSecret,
}

fn generate() -> Result<Generated, ApiKeyError> {
    let id = Uuid::new_v4();
    let mut secret = [0_u8; 32];
    getrandom::fill(&mut secret).map_err(|_| ApiKeyError::Random)?;
    let hash = verifier(&secret)?.finalize().into_bytes().to_vec();
    let token = OpaqueSecret::new(format!(
        "{KEY_PREFIX}{id}.{}",
        URL_SAFE_NO_PAD.encode(secret)
    ));
    Ok(Generated { id, hash, token })
}

/// Parses a bounded, canonical wire token. No raw input goes into errors/logs.
fn parse(token: &str) -> Option<(Uuid, [u8; 32])> {
    if token.len() != KEY_PREFIX.len() + 36 + 1 + 43 {
        return None;
    }
    let (selector, encoded) = token.strip_prefix(KEY_PREFIX)?.split_once('.')?;
    let id = Uuid::parse_str(selector).ok()?;
    if id.to_string() != selector {
        return None;
    }
    let decoded = URL_SAFE_NO_PAD.decode(encoded).ok()?;
    let bytes: [u8; 32] = decoded.try_into().ok()?;
    (URL_SAFE_NO_PAD.encode(bytes) == encoded).then_some((id, bytes))
}

/// Resolve the secret into a machine identity. Active creator membership and
/// status are checked here AND in the PDP. No provider availability is needed.
pub async fn authenticate(pool: &PgPool, token: &str) -> Result<Option<Principal>, ApiKeyError> {
    let Some((id, secret)) = parse(token) else {
        return Ok(None);
    };
    let row = sqlx::query(
        "SELECT k.secret_hash, k.workspace_id FROM api_keys k
         JOIN users u ON u.id = k.created_by_user_id AND u.status = 'active'
         JOIN workspace_memberships m ON m.user_id = u.id AND m.workspace_id = k.workspace_id AND m.status = 'active'
         WHERE k.id = $1 AND k.revoked_at IS NULL AND (k.expires_at IS NULL OR k.expires_at > now())"
    ).bind(id).fetch_optional(pool).await?;
    let Some(row) = row else {
        return Ok(None);
    };
    let hash: Vec<u8> = row.try_get("secret_hash")?;
    // RustCrypto's verify_slice compares the complete tag in constant time.
    if verifier(&secret)?.verify_slice(&hash).is_err() {
        return Ok(None);
    }
    let workspace: Uuid = row.try_get("workspace_id")?;
    sqlx::query("UPDATE api_keys SET last_used_at = now() WHERE id = $1 AND (last_used_at IS NULL OR last_used_at < now() - interval '1 minute')")
        .bind(id).execute(pool).await?;
    Ok(Some(Principal::service(
        ApiKeyId::new(id),
        WorkspaceId::new(workspace),
    )))
}

async fn manager<'a>(
    pool: &PgPool,
    actor: UserId,
    workspace: WorkspaceId,
) -> Result<Transaction<'a, Postgres>, ApiKeyError> {
    let mut tx = pool.begin().await?;
    // Serializes quota/rotation within a workspace and fences account/membership
    // changes until commit. Never infer the role here: the central PDP decides.
    // Follow account deletion's user -> workspace -> membership lock order.
    let user: Option<Uuid> = sqlx::query_scalar("SELECT id FROM users WHERE id = $1 FOR SHARE")
        .bind(actor.as_uuid())
        .fetch_optional(&mut *tx)
        .await?;
    if user.is_none() {
        return Err(ApiKeyError::Denied);
    }
    let workspace_row: Option<Uuid> =
        sqlx::query_scalar("SELECT id FROM workspaces WHERE id = $1 FOR UPDATE")
            .bind(workspace.as_uuid())
            .fetch_optional(&mut *tx)
            .await?;
    if workspace_row.is_none() {
        return Err(ApiKeyError::Denied);
    }
    let membership: Option<Uuid> = sqlx::query_scalar(
        "SELECT id FROM workspace_memberships WHERE workspace_id = $1 AND user_id = $2 FOR SHARE",
    )
    .bind(workspace.as_uuid())
    .bind(actor.as_uuid())
    .fetch_optional(&mut *tx)
    .await?;
    if membership.is_none() {
        return Err(ApiKeyError::Denied);
    }
    PolicyDecisionPoint::new(TransactionAuthorizationStore::new(&mut tx))
        .require(
            &Principal::user(actor),
            Permission::ApiKeyManage,
            Resource::Workspace(workspace),
        )
        .await
        .map_err(|error| match error {
            AuthzError::Denied(_) => ApiKeyError::Denied,
            other => other.into(),
        })?;
    Ok(tx)
}

async fn audit(
    tx: &mut Transaction<'_, Postgres>,
    actor: UserId,
    workspace: WorkspaceId,
    event: &str,
    id: Uuid,
    request: AuditRequest<'_>,
) -> Result<(), ApiKeyError> {
    insert_admin_audit(
        tx,
        event,
        AuditPrincipal::User(actor),
        None,
        Some(workspace.as_uuid()),
        None,
        json!({ "key_id": id }),
        request,
    )
    .await?;
    Ok(())
}

async fn insert(
    tx: &mut Transaction<'_, Postgres>,
    actor: UserId,
    workspace: WorkspaceId,
    spec: &KeySpec,
    generated: &Generated,
    request: AuditRequest<'_>,
) -> Result<ApiKeyRecord, ApiKeyError> {
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM api_keys WHERE workspace_id = $1 AND revoked_at IS NULL AND (expires_at IS NULL OR expires_at > now())")
        .bind(workspace.as_uuid()).fetch_one(&mut **tx).await?;
    if count >= MAX_ACTIVE_KEYS {
        return Err(ApiKeyError::Limit);
    }
    let scopes: Vec<_> = spec.scopes.iter().map(|scope| scope.as_str()).collect();
    let row = sqlx::query("INSERT INTO api_keys (id, workspace_id, created_by_user_id, name, prefix, secret_hash, scopes, expires_at) VALUES ($1,$2,$3,$4,$5,$6,$7,$8) RETURNING *")
        .bind(generated.id).bind(workspace.as_uuid()).bind(actor.as_uuid()).bind(&spec.name)
        .bind(KEY_PREFIX).bind(&generated.hash).bind(scopes).bind(spec.expires_at).fetch_one(&mut **tx).await?;
    audit(
        tx,
        actor,
        workspace,
        "api_key.created",
        generated.id,
        request,
    )
    .await?;
    Ok(record(&row)?)
}

pub async fn create(
    pool: &PgPool,
    actor: UserId,
    workspace: WorkspaceId,
    spec: KeySpec,
    request: AuditRequest<'_>,
) -> Result<IssuedApiKey, ApiKeyError> {
    let generated = generate()?;
    let mut tx = manager(pool, actor, workspace).await?;
    let record = insert(&mut tx, actor, workspace, &spec, &generated, request).await?;
    tx.commit().await?;
    Ok(IssuedApiKey {
        record,
        secret: generated.token,
    })
}

pub async fn list(
    pool: &PgPool,
    actor: UserId,
    workspace: WorkspaceId,
) -> Result<Vec<ApiKeyRecord>, ApiKeyError> {
    let mut tx = manager(pool, actor, workspace).await?;
    let rows = sqlx::query("SELECT id, workspace_id, created_by_user_id, name, prefix, scopes, expires_at, last_used_at, revoked_at, created_at FROM api_keys WHERE workspace_id = $1 ORDER BY (revoked_at IS NULL AND (expires_at IS NULL OR expires_at > now())) DESC, created_at DESC, id LIMIT 200")
        .bind(workspace.as_uuid()).fetch_all(&mut *tx).await?;
    let records = rows.iter().map(record).collect::<Result<Vec<_>, _>>()?;
    tx.commit().await?;
    Ok(records)
}

pub async fn revoke(
    pool: &PgPool,
    actor: UserId,
    workspace: WorkspaceId,
    id: ApiKeyId,
    request: AuditRequest<'_>,
) -> Result<(), ApiKeyError> {
    let mut tx = manager(pool, actor, workspace).await?;
    let row = sqlx::query(
        "SELECT revoked_at FROM api_keys WHERE id = $1 AND workspace_id = $2 FOR UPDATE",
    )
    .bind(id.as_uuid())
    .bind(workspace.as_uuid())
    .fetch_optional(&mut *tx)
    .await?
    .ok_or(ApiKeyError::Denied)?;
    if row
        .try_get::<Option<DateTime<Utc>>, _>("revoked_at")?
        .is_none()
    {
        sqlx::query("UPDATE api_keys SET revoked_at = now() WHERE id = $1")
            .bind(id.as_uuid())
            .execute(&mut *tx)
            .await?;
        audit(
            &mut tx,
            actor,
            workspace,
            "api_key.revoked",
            id.as_uuid(),
            request,
        )
        .await?;
    }
    tx.commit().await?;
    Ok(())
}

/// Rotation is one transaction: no old/new overlap after commit and no
/// revocation if generating or inserting the replacement fails.
pub async fn rotate(
    pool: &PgPool,
    actor: UserId,
    workspace: WorkspaceId,
    id: ApiKeyId,
    request: AuditRequest<'_>,
) -> Result<IssuedApiKey, ApiKeyError> {
    let generated = generate()?;
    let mut tx = manager(pool, actor, workspace).await?;
    let row = sqlx::query("SELECT * FROM api_keys WHERE id = $1 AND workspace_id = $2 AND revoked_at IS NULL AND (expires_at IS NULL OR expires_at > now()) FOR UPDATE")
        .bind(id.as_uuid()).bind(workspace.as_uuid()).fetch_optional(&mut *tx).await?.ok_or(ApiKeyError::Denied)?;
    let old = record(&row)?;
    let spec = KeySpec::parse(old.name, old.scopes, old.expires_at, Utc::now())?;
    sqlx::query("UPDATE api_keys SET revoked_at = now() WHERE id = $1")
        .bind(id.as_uuid())
        .execute(&mut *tx)
        .await?;
    audit(
        &mut tx,
        actor,
        workspace,
        "api_key.revoked",
        id.as_uuid(),
        request,
    )
    .await?;
    let record = insert(&mut tx, actor, workspace, &spec, &generated, request).await?;
    audit(
        &mut tx,
        actor,
        workspace,
        "api_key.rotated",
        generated.id,
        request,
    )
    .await?;
    tx.commit().await?;
    Ok(IssuedApiKey {
        record,
        secret: generated.token,
    })
}

/// Workspace erasure deletes verifiers and free-text names. The independent
/// administrative stream retains revocation facts in the same transaction.
pub(crate) async fn remove_workspace_keys(
    tx: &mut Transaction<'_, Postgres>,
    actor: UserId,
    workspace: WorkspaceId,
    request: AuditRequest<'_>,
) -> Result<(), sqlx::Error> {
    let ids: Vec<Uuid> = sqlx::query_scalar(
        "DELETE FROM api_keys WHERE workspace_id=$1 AND revoked_at IS NULL RETURNING id",
    )
    .bind(workspace.as_uuid())
    .fetch_all(&mut **tx)
    .await?;
    for id in ids {
        insert_admin_audit(
            tx,
            "api_key.revoked",
            AuditPrincipal::User(actor),
            None,
            Some(workspace.as_uuid()),
            None,
            json!({"key_id":id}),
            request,
        )
        .await?;
    }
    // Already-revoked keys disappear with the workspace cascade.
    Ok(())
}

/// Account erasure removes key names/verifiers in shared workspaces too,
/// keeping the creator UserId and security history in the separate audit.
pub(crate) async fn remove_user_keys(
    tx: &mut Transaction<'_, Postgres>,
    user: UserId,
    request: AuditRequest<'_>,
) -> Result<u64, sqlx::Error> {
    let rows = sqlx::query(
        "DELETE FROM api_keys WHERE created_by_user_id=$1 RETURNING id, workspace_id, revoked_at",
    )
    .bind(user.as_uuid())
    .fetch_all(&mut **tx)
    .await?;
    let removed = rows.len() as u64;
    for row in rows {
        if row
            .try_get::<Option<DateTime<Utc>>, _>("revoked_at")?
            .is_none()
        {
            let id: Uuid = row.try_get("id")?;
            let workspace: Uuid = row.try_get("workspace_id")?;
            insert_admin_audit(
                tx,
                "api_key.revoked",
                AuditPrincipal::System,
                None,
                Some(workspace),
                None,
                json!({"key_id":id}),
                request,
            )
            .await?;
        }
    }
    Ok(removed)
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn tokens_are_canonical_unpredictable_and_debug_redacted() {
        let a = generate().unwrap_or_else(|error| unreachable!("{error}"));
        let b = generate().unwrap_or_else(|error| unreachable!("{error}"));
        assert_ne!(a.token.expose(), b.token.expose());
        let (id, secret) =
            parse(a.token.expose()).unwrap_or_else(|| unreachable!("generated token"));
        assert_eq!(id, a.id);
        assert!(verifier(&secret).unwrap().verify_slice(&a.hash).is_ok());
        assert!(verifier(&secret).unwrap().verify_slice(&b.hash).is_err());
        assert!(!format!("{:?}", a.token).contains(a.token.expose()));
        for bad in [
            "",
            "dr_key_v1_",
            "Bearer secret",
            &format!("{}x", a.token.expose()),
            &a.token.expose().to_uppercase(),
        ] {
            assert!(parse(bad).is_none());
        }
    }

    #[test]
    fn spec_rejects_human_permissions_and_duplicate_or_empty_scopes() {
        for scopes in [
            vec![],
            vec!["project:view", "project:view"],
            vec!["screening:perform"],
            vec!["api_key:manage"],
            vec!["unknown"],
        ] {
            assert!(
                KeySpec::parse(
                    "key".into(),
                    scopes.into_iter().map(str::to_owned).collect(),
                    None,
                    Utc::now()
                )
                .is_err()
            );
        }
        assert!(
            KeySpec::parse(
                "key".into(),
                vec!["project:view".into()],
                Some(Utc::now()),
                Utc::now()
            )
            .is_err()
        );
    }
}
