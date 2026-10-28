//! Database credential contract. This supplements application authorization;
//! it is not tenant RLS and never accepts a caller-controlled GUC as authority.
use anyhow::{Context, Result};
use serde::Serialize;
use sqlx::{PgPool, Row};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DatabaseRolePurpose {
    Runtime,
    Migration,
    Maintenance,
}

impl DatabaseRolePurpose {
    pub const fn capability(self) -> &'static str {
        match self {
            Self::Runtime => "deepref_runtime",
            Self::Migration => "deepref_migration_owner",
            Self::Maintenance => "deepref_maintenance",
        }
    }
}

#[derive(Debug, Serialize)]
pub struct DatabaseRoleCheck {
    pub purpose: &'static str,
    pub contract_active: bool,
    pub role_verified: bool,
}

/// Requires real catalog privileges and memberships; configuration strings and
/// session variables cannot satisfy this check. Query and lock waits are bounded.
pub async fn verify_database_role(
    pool: &PgPool,
    purpose: DatabaseRolePurpose,
) -> Result<DatabaseRoleCheck> {
    tokio::time::timeout(std::time::Duration::from_secs(3), verify(pool, purpose))
        .await
        .context("database role verification deadline exceeded")?
}

async fn verify(pool: &PgPool, purpose: DatabaseRolePurpose) -> Result<DatabaseRoleCheck> {
    let mut transaction = crate::operational_read::begin(pool).await?;
    let active: bool = sqlx::query_scalar(
        "SELECT activated_at IS NOT NULL FROM public.database_role_contract WHERE singleton",
    )
    .fetch_one(&mut *transaction)
    .await
    .context("read database role activation")?;
    anyhow::ensure!(
        active,
        "database roles are not activated; run the explicit administrator provisioning step"
    );
    // Expected source comes from immutable versioned migration bytes, never
    // from a mutable catalog row or caller-controlled configuration.
    for expected in PROTECTED_FUNCTIONS {
        let safe: bool = sqlx::query_scalar(
            "SELECT p.proowner='deepref_migration_owner'::regrole
                AND p.prosecdef=$2 AND p.prosrc=$3 AND l.lanname='plpgsql'
                AND p.proconfig @> $4::text[] AND cardinality(p.proconfig)=cardinality($4::text[])
                AND NOT EXISTS(SELECT 1 FROM aclexplode(coalesce(p.proacl,acldefault('f',p.proowner))) a
                    WHERE a.grantee=0 AND a.privilege_type='EXECUTE')
             FROM pg_proc p JOIN pg_language l ON l.oid=p.prolang
             WHERE p.oid=$1::regprocedure",
        )
        .bind(expected.signature).bind(expected.definer).bind(expected.body()?)
        .bind(expected.configuration).fetch_one(&mut *transaction).await?;
        anyhow::ensure!(
            safe,
            "protected database function source, language, owner, configuration or EXECUTE grants violate the role contract"
        );
    }
    let triggers_safe: bool = sqlx::query_scalar(
        "SELECT count(*)=2 AND bool_and(
            t.tgfoid='public.reject_admin_audit_event_changes()'::regprocedure
            AND t.tgtype=expected.kind AND t.tgenabled='O' AND NOT t.tgisinternal
            AND t.tgqual IS NULL AND t.tgnargs=0 AND octet_length(t.tgargs)=0
            AND cardinality(t.tgattr::smallint[])=0 AND t.tgconstraint=0)
         FROM (VALUES ('admin_audit_events_append_only_trigger',27),
                      ('admin_audit_events_no_truncate_trigger',34)) expected(name,kind)
         JOIN pg_trigger t ON t.tgname=expected.name
         AND t.tgrelid='public.admin_audit_events'::regclass",
    )
    .fetch_one(&mut *transaction)
    .await?;
    anyhow::ensure!(
        triggers_safe,
        "protected audit trigger bindings violate the role contract"
    );
    let role = purpose.capability();
    let row=sqlx::query(
        "SELECT pg_has_role(session_user,$1::name,'USAGE') AS assigned,
           EXISTS(SELECT 1 FROM pg_roles r WHERE pg_has_role(session_user,r.oid,'MEMBER') AND
             (r.rolsuper OR r.rolcreatedb OR r.rolcreaterole OR r.rolreplication OR r.rolbypassrls
              OR r.rolname NOT IN(session_user,$1))) AS unsafe_membership,
           EXISTS(SELECT 1 FROM pg_auth_members m JOIN pg_roles r ON r.oid=m.member
             WHERE r.rolname=session_user AND m.admin_option) AS admin_option,
           current_user=session_user OR current_user=$1 AS expected_current_role,
           EXISTS(SELECT 1 FROM pg_roles r WHERE r.rolname=$1 AND
             (r.rolcanlogin OR r.rolsuper OR r.rolcreatedb OR r.rolcreaterole OR r.rolreplication OR r.rolbypassrls)) AS unsafe_capability,
           EXISTS(SELECT 1 FROM pg_auth_members m JOIN pg_roles r ON r.oid=m.member WHERE r.rolname=$1) AS capability_membership,
           EXISTS(SELECT 1 FROM pg_class c JOIN pg_namespace n ON n.oid=c.relnamespace
             WHERE n.nspname='public' AND c.relkind IN('r','p','v','m')
               AND (has_table_privilege(session_user,c.oid,'TRUNCATE,REFERENCES,TRIGGER')
                 OR has_any_column_privilege(session_user,c.oid,'REFERENCES'))) AS unsafe_table_grant,
           EXISTS(SELECT 1 FROM pg_class c JOIN pg_namespace n ON n.oid=c.relnamespace
             WHERE n.nspname='public' AND c.relkind IN('r','p','v','m')
               AND (has_table_privilege(session_user,c.oid,'INSERT,UPDATE,DELETE,TRUNCATE,REFERENCES,TRIGGER')
                 OR has_any_column_privilege(session_user,c.oid,'INSERT,UPDATE,REFERENCES'))) AS maintenance_mutation,
           EXISTS(SELECT 1 FROM pg_class c JOIN pg_namespace n ON n.oid=c.relnamespace
             JOIN pg_attribute a ON a.attrelid=c.oid AND a.attnum>0 AND NOT a.attisdropped
             WHERE n.nspname='public' AND c.relkind IN('r','p','v','m')
               AND has_column_privilege(session_user,c.oid,a.attnum,'SELECT')
               AND NOT ((c.relname='admin_audit_events' AND a.attname IN('event_type','occurred_at'))
                 OR (c.relname='fulfilled_account_deletions' AND a.attname='completed_at')
                 OR (c.relname='user_sessions' AND a.attname IN('revoked_at','expires_at'))
                 OR c.relname='database_role_contract')) AS maintenance_excess_read,
           EXISTS(SELECT 1 FROM pg_class c JOIN pg_namespace n ON n.oid=c.relnamespace
             CROSS JOIN LATERAL aclexplode(coalesce(c.relacl,acldefault('S',c.relowner))) a
             WHERE n.nspname='public' AND c.relkind='S' AND a.grantee=0) AS public_sequence_access,
           EXISTS(SELECT 1 FROM pg_class c JOIN pg_namespace n ON n.oid=c.relnamespace
             WHERE n.nspname='public' AND c.relkind='S'
               AND has_sequence_privilege(session_user,c.oid,'UPDATE')) AS runtime_sequence_mutation,
           EXISTS(SELECT 1 FROM pg_class c JOIN pg_namespace n ON n.oid=c.relnamespace
             WHERE n.nspname='public' AND c.relkind='S'
               AND has_sequence_privilege(session_user,c.oid,'USAGE,SELECT,UPDATE')) AS maintenance_sequence_access,
           EXISTS(SELECT 1 FROM pg_default_acl d CROSS JOIN LATERAL aclexplode(d.defaclacl) a
             WHERE d.defaclrole='deepref_migration_owner'::regrole AND d.defaclobjtype='S'
               AND d.defaclnamespace IN(0,'public'::regnamespace)
               AND (a.grantee IN(0,'deepref_maintenance'::regrole)
                 OR (a.grantee='deepref_runtime'::regrole AND a.privilege_type='UPDATE'))) AS unsafe_sequence_defaults,
           has_schema_privilege(session_user,'public','CREATE') AS schema_create,
           has_database_privilege(session_user,current_database(),'CREATE') AS database_create,
           has_database_privilege(session_user,current_database(),'TEMPORARY') AS database_temp,
           (has_table_privilege(session_user,'public.admin_audit_events','UPDATE,DELETE,TRUNCATE') OR has_any_column_privilege(session_user,'public.admin_audit_events','UPDATE,REFERENCES')) AS audit_mutation,
           (has_table_privilege(session_user,'public.fulfilled_account_deletions','UPDATE,DELETE,TRUNCATE') OR has_any_column_privilege(session_user,'public.fulfilled_account_deletions','UPDATE,REFERENCES')) AS proof_mutation,
           EXISTS(SELECT 1 FROM pg_proc p JOIN pg_namespace n ON n.oid=p.pronamespace WHERE n.nspname='public' AND p.prosecdef
             AND has_function_privilege(session_user,p.oid,'EXECUTE') AND p.oid<>CASE WHEN $1='deepref_runtime' THEN 'public.redact_deleted_user_audit(uuid)'::regprocedure WHEN $1='deepref_maintenance' THEN 'public.purge_retention_records(integer)'::regprocedure ELSE p.oid END) AS unsafe_definer,
           (has_table_privilege(session_user,'public._sqlx_migrations','INSERT,UPDATE,DELETE,TRUNCATE') OR has_any_column_privilege(session_user,'public._sqlx_migrations','INSERT,UPDATE,REFERENCES')) AS migration_mutation,
           (has_table_privilege(session_user,'public.database_role_contract','INSERT,UPDATE,DELETE,TRUNCATE') OR has_any_column_privilege(session_user,'public.database_role_contract','INSERT,UPDATE,REFERENCES')) AS contract_mutation,
           EXISTS(SELECT 1 FROM pg_class c JOIN pg_namespace n ON n.oid=c.relnamespace
             WHERE n.nspname='public' AND pg_has_role(session_user,c.relowner,'MEMBER')) AS owns_objects,
           has_function_privilege(session_user,'public.purge_retention_records(integer)','EXECUTE') AS can_purge"
    ).bind(role).fetch_one(&mut *transaction).await?;
    anyhow::ensure!(
        row.try_get::<bool, _>("assigned")?
            && row.try_get::<bool, _>("expected_current_role")?
            && !row.try_get::<bool, _>("unsafe_membership")?
            && !row.try_get::<bool, _>("admin_option")?
            && !row.try_get::<bool, _>("unsafe_capability")?
            && !row.try_get::<bool, _>("capability_membership")?,
        "database login is privileged, unassigned, or has unrelated role membership"
    );
    match purpose {
        DatabaseRolePurpose::Runtime => {
            for field in [
                "schema_create",
                "database_create",
                "database_temp",
                "audit_mutation",
                "proof_mutation",
                "unsafe_definer",
                "migration_mutation",
                "contract_mutation",
                "owns_objects",
                "can_purge",
                "unsafe_table_grant",
                "runtime_sequence_mutation",
                "public_sequence_access",
                "unsafe_sequence_defaults",
            ] {
                anyhow::ensure!(
                    !row.try_get::<bool, _>(field)?,
                    "runtime database permissions exceed the role contract ({field})"
                );
            }
            let has_dml:bool=sqlx::query_scalar("SELECT has_table_privilege(session_user,'public.jobs','SELECT') AND has_table_privilege(session_user,'public.jobs','INSERT') AND has_table_privilege(session_user,'public.jobs','UPDATE') AND has_table_privilege(session_user,'public.admin_audit_events','INSERT')")
                .fetch_one(&mut *transaction).await?;
            anyhow::ensure!(has_dml, "runtime database permissions are incomplete");
        }
        DatabaseRolePurpose::Maintenance => {
            for field in [
                "schema_create",
                "database_create",
                "database_temp",
                "audit_mutation",
                "proof_mutation",
                "unsafe_definer",
                "migration_mutation",
                "contract_mutation",
                "owns_objects",
                "maintenance_mutation",
                "maintenance_excess_read",
                "maintenance_sequence_access",
                "public_sequence_access",
                "unsafe_sequence_defaults",
            ] {
                anyhow::ensure!(
                    !row.try_get::<bool, _>(field)?,
                    "maintenance database permissions exceed the role contract ({field})"
                );
            }
            anyhow::ensure!(
                row.try_get::<bool, _>("can_purge")?,
                "maintenance retention function is not granted"
            );
        }
        DatabaseRolePurpose::Migration => {
            anyhow::ensure!(
                row.try_get::<bool, _>("owns_objects")?
                    && row.try_get::<bool, _>("schema_create")?,
                "migration ownership is incomplete"
            );
            let current: bool = sqlx::query_scalar("SELECT current_user='deepref_migration_owner'")
                .fetch_one(&mut *transaction)
                .await?;
            anyhow::ensure!(
                current,
                "migration connections must SET ROLE deepref_migration_owner before any DDL"
            );
        }
    }
    transaction.commit().await?;
    Ok(DatabaseRoleCheck {
        purpose: role,
        contract_active: true,
        role_verified: true,
    })
}

struct ProtectedFunction {
    signature: &'static str,
    marker: &'static str,
    source: &'static str,
    definer: bool,
    configuration: &'static [&'static str],
}
impl ProtectedFunction {
    fn body(&self) -> Result<&'static str> {
        self.source
            .split_once(self.marker)
            .and_then(|(_, definition)| definition.split_once("AS $$"))
            .and_then(|(_, body)| body.split_once("$$;"))
            .map(|(body, _)| body)
            .context("read embedded canonical database function")
    }
}
const ROLE_SQL: &str =
    include_str!("../migrations/0188_database_roles_and_bounded_audit_retention.sql");
const REDACTION_SQL: &str =
    include_str!("../migrations/0117_authorized_account_redaction_and_address_digests.sql");
const PROTECTED_FUNCTIONS: &[ProtectedFunction] = &[
    ProtectedFunction {
        signature: "public.purge_retention_records(integer)",
        marker: "CREATE FUNCTION public.purge_retention_records(",
        source: ROLE_SQL,
        definer: true,
        configuration: &["search_path=pg_catalog", "lock_timeout=250ms"],
    },
    ProtectedFunction {
        signature: "public.redact_deleted_user_audit(uuid)",
        marker: "CREATE FUNCTION redact_deleted_user_audit(",
        source: REDACTION_SQL,
        definer: true,
        configuration: &["search_path=pg_catalog"],
    },
    ProtectedFunction {
        signature: "public.reject_admin_audit_event_changes()",
        marker: "CREATE OR REPLACE FUNCTION reject_admin_audit_event_changes()",
        source: ROLE_SQL,
        definer: false,
        configuration: &["search_path=pg_catalog"],
    },
];

pub async fn provision_database_roles(
    pool: &PgPool,
    runtime: &str,
    migration: &str,
    maintenance: &str,
) -> Result<()> {
    for value in [runtime, migration, maintenance] {
        anyhow::ensure!(
            !value.is_empty()
                && value.len() <= 63
                && value
                    .bytes()
                    .enumerate()
                    .all(|(index, b)| b.is_ascii_alphabetic()
                        || b == b'_'
                        || (index > 0 && b.is_ascii_digit())),
            "invalid database login identifier"
        );
    }
    let mut transaction = pool.begin().await?;
    sqlx::query("SET LOCAL statement_timeout='10s'")
        .execute(&mut *transaction)
        .await?;
    sqlx::query("SET LOCAL lock_timeout='250ms'")
        .execute(&mut *transaction)
        .await?;
    sqlx::query("SELECT public.configure_database_roles($1,$2,$3)")
        .bind(runtime)
        .bind(migration)
        .bind(maintenance)
        .execute(&mut *transaction)
        .await?;
    transaction.commit().await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn canonical_protected_bodies_remain_compatible_with_applied_migrations() -> Result<()> {
        let corrective = include_str!("../migrations/0205_database_role_contract_repairs.sql");
        for expected in PROTECTED_FUNCTIONS {
            let marker = expected
                .marker
                .replace("CREATE FUNCTION", "CREATE OR REPLACE FUNCTION")
                .replace("FUNCTION reject_", "FUNCTION public.reject_")
                .replace("FUNCTION redact_", "FUNCTION public.redact_");
            // Only this pure unit check rewrites a definition marker; production
            // descriptors and expected bodies are immutable embedded bytes.
            let body = corrective
                .split_once(&marker)
                .and_then(|(_, definition)| definition.split_once("AS $$"))
                .and_then(|(_, body)| body.split_once("$$;"))
                .context("read corrective canonical definition")?
                .0;
            anyhow::ensure!(
                body == expected.body()?,
                "corrective migration changed protected semantics"
            );
        }
        Ok(())
    }
}
