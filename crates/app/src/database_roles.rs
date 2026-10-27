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
    let active: bool = sqlx::query_scalar(
        "SELECT activated_at IS NOT NULL FROM public.database_role_contract WHERE singleton",
    )
    .fetch_one(pool)
    .await
    .context("read database role activation")?;
    anyhow::ensure!(
        active,
        "database roles are not activated; run the explicit administrator provisioning step"
    );
    // Verify the canonical protected functions as well as the caller's ACLs.
    // A correctly named membership cannot conceal owner/search_path or PUBLIC
    // EXECUTE drift introduced during deployment.
    let protected_functions: bool = sqlx::query_scalar(
        "SELECT count(*)=3 AND bool_and(
            p.proowner='deepref_migration_owner'::regrole
            AND p.prosecdef=expected.definer
            AND coalesce(p.proconfig @> ARRAY['search_path=pg_catalog'],false)
            AND NOT EXISTS(SELECT 1 FROM aclexplode(coalesce(p.proacl,acldefault('f',p.proowner))) a
                WHERE a.grantee=0 AND a.privilege_type='EXECUTE'))
         FROM (VALUES
            ('public.purge_retention_records(integer)'::regprocedure,true),
            ('public.redact_deleted_user_audit(uuid)'::regprocedure,true),
            ('public.reject_admin_audit_event_changes()'::regprocedure,false)
         ) expected(oid,definer) JOIN pg_proc p ON p.oid=expected.oid",
    )
    .fetch_one(pool)
    .await?;
    anyhow::ensure!(
        protected_functions,
        "protected database function ownership, search_path or EXECUTE grants violate the role contract"
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
    ).bind(role).fetch_one(pool).await?;
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
            ] {
                anyhow::ensure!(
                    !row.try_get::<bool, _>(field)?,
                    "runtime database permissions exceed the role contract ({field})"
                );
            }
            let has_dml:bool=sqlx::query_scalar("SELECT has_table_privilege(session_user,'public.jobs','SELECT') AND has_table_privilege(session_user,'public.jobs','INSERT') AND has_table_privilege(session_user,'public.jobs','UPDATE') AND has_table_privilege(session_user,'public.admin_audit_events','INSERT')")
                .fetch_one(pool).await?;
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
                .fetch_one(pool)
                .await?;
            anyhow::ensure!(
                current,
                "migration connections must SET ROLE deepref_migration_owner before any DDL"
            );
        }
    }
    Ok(DatabaseRoleCheck {
        purpose: role,
        contract_active: true,
        role_verified: true,
    })
}

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
