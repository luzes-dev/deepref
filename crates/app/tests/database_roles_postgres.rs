use anyhow::{Context, Result};
use deepref_app::database_roles::{
    DatabaseRolePurpose as Purpose, provision_database_roles, verify_database_role,
};
use sqlx::{PgPool, Postgres, QueryBuilder, postgres::PgPoolOptions};
use uuid::Uuid;

// PostgreSQL role identifiers cannot be bind parameters. This fixture-only
// type constructs them exclusively from a closed purpose and a UUID, then
// renders a quoted identifier for the role DDL statements below.
struct FixtureLogin(String);

impl FixtureLogin {
    fn new(purpose: Purpose, tag: Uuid) -> Self {
        let prefix = match purpose {
            Purpose::Runtime => "dr_rt_",
            Purpose::Migration => "dr_mg_",
            Purpose::Maintenance => "dr_mt_",
        };
        Self(format!("{prefix}{}", tag.simple()))
    }

    fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for FixtureLogin {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "\"{}\"", self.0)
    }
}

async fn deny(pool: &PgPool, statement: &'static str) -> Result<()> {
    let error = sqlx::query(statement)
        .execute(pool)
        .await
        .err()
        .context("permission bypass unexpectedly succeeded")?;
    let code = error
        .as_database_error()
        .and_then(|error| error.code())
        .map(|code| code.into_owned())
        .context("expected a database permission refusal")?;
    anyhow::ensure!(
        matches!(code.as_str(), "42501" | "23000" | "3F000"),
        "unexpected refusal category {code}"
    );
    Ok(())
}

#[tokio::test]
async fn real_isolated_login_roles_enforce_audit_and_migration_boundaries() -> Result<()> {
    let Some(admin) = deepref_app::test_isolation::isolated_pool_from_env("DATABASE_URL", 4).await
    else {
        return Ok(());
    };
    let tag = Uuid::new_v4();
    let runtime_name = FixtureLogin::new(Purpose::Runtime, tag);
    let migration_name = FixtureLogin::new(Purpose::Migration, tag);
    let maintenance_name = FixtureLogin::new(Purpose::Maintenance, tag);
    let names = [&runtime_name, &migration_name, &maintenance_name];
    // These exact fixture-only LOGIN roles exist solely on the required
    // disposable server. Every assertion below opens a real authenticated pool.
    for name in names {
        QueryBuilder::<Postgres>::new("CREATE ROLE ")
            .push(name)
            .push(" LOGIN INHERIT PASSWORD 'deepref-role-fixture-only'")
            .build()
            .execute(&admin)
            .await?;
    }
    let mut pools = Vec::new();
    let result = prove_role_contract(&admin, names, &mut pools).await;
    for pool in pools {
        pool.close().await;
    }
    for name in names {
        // Revoke only this exact generated LOGIN's remaining grants even if
        // provisioning rolled back. Capability-owned application objects are
        // distinct and are not owned by these fixture LOGIN identities.
        QueryBuilder::<Postgres>::new("DROP OWNED BY ")
            .push(name)
            .build()
            .execute(&admin)
            .await?;
        QueryBuilder::<Postgres>::new("DROP ROLE ")
            .push(name)
            .build()
            .execute(&admin)
            .await?;
    }
    result
}

async fn prove_role_contract(
    admin: &PgPool,
    names: [&FixtureLogin; 3],
    pools: &mut Vec<PgPool>,
) -> Result<()> {
    let [runtime_name, migration_name, maintenance_name] = names;
    // Table REVOKE alone must not leave older direct column grants behind.
    QueryBuilder::<Postgres>::new(
        "GRANT UPDATE (completed_at) ON public.fulfilled_account_deletions TO ",
    )
    .push(runtime_name)
    .build()
    .execute(admin)
    .await?;
    QueryBuilder::<Postgres>::new(
        "GRANT SELECT (sealed_provider_session) ON public.user_sessions TO ",
    )
    .push(maintenance_name)
    .build()
    .execute(admin)
    .await?;
    anyhow::ensure!(
        provision_database_roles(
            admin,
            runtime_name.as_str(),
            runtime_name.as_str(),
            maintenance_name.as_str()
        )
        .await
        .is_err()
    );
    provision_database_roles(
        admin,
        runtime_name.as_str(),
        migration_name.as_str(),
        maintenance_name.as_str(),
    )
    .await?;
    provision_database_roles(
        admin,
        runtime_name.as_str(),
        migration_name.as_str(),
        maintenance_name.as_str(),
    )
    .await?;
    for name in names {
        let options = (*admin.connect_options())
            .clone()
            .username(name.as_str())
            .password("deepref-role-fixture-only");
        let is_migration = name.as_str() == migration_name.as_str();
        let is_maintenance = name.as_str() == maintenance_name.as_str();
        pools.push(
            PgPoolOptions::new()
                .min_connections(0)
                .max_connections(1)
                .after_connect(move |connection, _| {
                    Box::pin(async move {
                        if is_migration {
                            sqlx::query("SET ROLE deepref_migration_owner")
                                .execute(&mut *connection)
                                .await?;
                        }
                        if is_maintenance {
                            sqlx::query("SET search_path=pg_temp")
                                .execute(&mut *connection)
                                .await?;
                        }
                        Ok(())
                    })
                })
                .connect_with(options)
                .await?,
        );
    }
    let (runtime, migration, maintenance) = (&pools[0], &pools[1], &pools[2]);
    verify_database_role(runtime, Purpose::Runtime).await?;
    prove_review_corrections(admin, runtime, maintenance, names).await?;
    // Deployment checks inspect the actual canonical function contracts.
    sqlx::query("ALTER FUNCTION public.purge_retention_records(integer) RESET search_path")
        .execute(admin)
        .await?;
    anyhow::ensure!(
        verify_database_role(runtime, Purpose::Runtime)
            .await
            .is_err(),
        "unsafe definer search_path must refuse activation"
    );
    sqlx::query(
        "ALTER FUNCTION public.purge_retention_records(integer) SET search_path=pg_catalog",
    )
    .execute(admin)
    .await?;
    sqlx::query("GRANT EXECUTE ON FUNCTION public.purge_retention_records(integer) TO PUBLIC")
        .execute(admin)
        .await?;
    anyhow::ensure!(
        verify_database_role(runtime, Purpose::Runtime)
            .await
            .is_err(),
        "PUBLIC definer execution must refuse activation"
    );
    sqlx::query("REVOKE EXECUTE ON FUNCTION public.purge_retention_records(integer) FROM PUBLIC")
        .execute(admin)
        .await?;
    // Canonical ownership is independently checked, even if the effective
    // runtime grants themselves have not changed.
    sqlx::query("ALTER FUNCTION public.purge_retention_records(integer) OWNER TO postgres")
        .execute(admin)
        .await?;
    anyhow::ensure!(
        verify_database_role(runtime, Purpose::Runtime)
            .await
            .is_err(),
        "an unexpected trusted-function owner must refuse activation"
    );
    sqlx::query(
        "ALTER FUNCTION public.purge_retention_records(integer) OWNER TO deepref_migration_owner",
    )
    .execute(admin)
    .await?;
    verify_database_role(runtime, Purpose::Runtime).await?;
    verify_database_role(migration, Purpose::Migration).await?;
    verify_database_role(maintenance, Purpose::Maintenance).await?;
    anyhow::ensure!(
        verify_database_role(admin, Purpose::Runtime).await.is_err(),
        "superuser must never satisfy runtime activation"
    );
    anyhow::ensure!(
        verify_database_role(runtime, Purpose::Maintenance)
            .await
            .is_err()
    );
    deepref_app::record_worker_heartbeat(runtime, "restricted-runtime-fixture").await?;
    sqlx::query("CREATE TABLE public.role_migration_fixture(value integer)")
        .execute(migration)
        .await?;
    sqlx::query("INSERT INTO public.role_migration_fixture(value) VALUES(1)")
        .execute(runtime)
        .await?;
    let owner:bool=sqlx::query_scalar("SELECT relowner='deepref_migration_owner'::regrole FROM pg_class WHERE oid='public.role_migration_fixture'::regclass").fetch_one(admin).await?;
    anyhow::ensure!(
        owner,
        "migration objects must have the distinct NOLOGIN owner"
    );
    sqlx::query("CREATE TABLE public.role_maintenance_fixture(value integer)")
        .execute(migration)
        .await?;
    sqlx::query("REVOKE ALL ON public.role_maintenance_fixture FROM deepref_runtime")
        .execute(migration)
        .await?;
    sqlx::query("GRANT SELECT ON public.role_maintenance_fixture TO deepref_maintenance")
        .execute(migration)
        .await?;
    provision_database_roles(
        admin,
        runtime_name.as_str(),
        migration_name.as_str(),
        maintenance_name.as_str(),
    )
    .await?;
    deny(
        runtime,
        "INSERT INTO public.role_maintenance_fixture(value) VALUES(1)",
    )
    .await?;
    sqlx::query("SELECT value FROM public.role_maintenance_fixture")
        .fetch_all(maintenance)
        .await?;
    anyhow::ensure!(
        verify_database_role(maintenance, Purpose::Maintenance)
            .await
            .is_err(),
        "unreviewed maintenance reads must refuse activation"
    );
    sqlx::query("REVOKE SELECT ON public.role_maintenance_fixture FROM deepref_maintenance")
        .execute(migration)
        .await?;
    verify_database_role(maintenance, Purpose::Maintenance).await?;
    QueryBuilder::<Postgres>::new(
        "GRANT UPDATE (completed_at) ON public.fulfilled_account_deletions TO ",
    )
    .push(runtime_name)
    .build()
    .execute(admin)
    .await?;
    anyhow::ensure!(
        verify_database_role(runtime, Purpose::Runtime)
            .await
            .is_err(),
        "column-level proof mutation must refuse activation"
    );
    QueryBuilder::<Postgres>::new(
        "REVOKE UPDATE (completed_at) ON public.fulfilled_account_deletions FROM ",
    )
    .push(runtime_name)
    .build()
    .execute(admin)
    .await?;
    verify_database_role(runtime, Purpose::Runtime).await?;
    QueryBuilder::<Postgres>::new(
        "GRANT SELECT (sealed_provider_session) ON public.user_sessions TO ",
    )
    .push(maintenance_name)
    .build()
    .execute(admin)
    .await?;
    anyhow::ensure!(
        verify_database_role(maintenance, Purpose::Maintenance)
            .await
            .is_err(),
        "column-level sealed session reads must refuse activation"
    );
    QueryBuilder::<Postgres>::new(
        "REVOKE SELECT (sealed_provider_session) ON public.user_sessions FROM ",
    )
    .push(maintenance_name)
    .build()
    .execute(admin)
    .await?;
    verify_database_role(maintenance, Purpose::Maintenance).await?;
    for statement in [
        "CREATE TABLE public.runtime_ddl_forbidden(id integer)",
        "ALTER TABLE public.admin_audit_events DISABLE TRIGGER ALL",
        "TRUNCATE public.admin_audit_events",
        "DELETE FROM public.admin_audit_events",
        "UPDATE public.admin_audit_events SET metadata='{}'::jsonb",
        "SET ROLE deepref_migration_owner",
        "SET ROLE deepref_maintenance",
        "SET SESSION AUTHORIZATION postgres",
        "SET session_replication_role=replica",
        "CREATE FUNCTION public.purge_retention_records(text) RETURNS int LANGUAGE SQL AS 'SELECT 1'",
        "CREATE TEMP TABLE forged_audit(id integer)",
        "UPDATE public.database_role_contract SET activated_at=NULL",
        "DELETE FROM public._sqlx_migrations",
        "UPDATE public.fulfilled_account_deletions SET completed_at=now()",
        "SELECT public.purge_retention_records(1)",
        "SELECT public.configure_database_roles('a','b','c')",
    ] {
        deny(runtime, statement).await?;
    }
    for statement in [
        "DELETE FROM public.admin_audit_events",
        "SET ROLE deepref_migration_owner",
        "ALTER FUNCTION public.purge_retention_records(integer) SECURITY INVOKER",
        "SELECT sealed_provider_session FROM public.user_sessions",
        "SELECT session_token_hash FROM public.user_sessions",
        "SELECT metadata FROM public.admin_audit_events",
        "SELECT email_hmac_sha256 FROM public.fulfilled_account_deletions",
        "SELECT id FROM public.users",
    ] {
        deny(maintenance, statement).await?;
    }
    let user = Uuid::new_v4();
    sqlx::query("INSERT INTO public.users(id,workos_user_id,primary_email,status) VALUES($1,$2,'fixture@example.invalid','active')")
            .bind(user).bind(format!("role-fixture-{user}")).execute(admin).await?;
    sqlx::query("INSERT INTO public.admin_audit_events(id,event_type,principal_kind,target_user_id,occurred_at,metadata) VALUES(gen_random_uuid(),'auth.login','system',$1,now()-interval '7 months','{}'),(gen_random_uuid(),'auth.login','system',$1,now(),'{}'),(gen_random_uuid(),'account.deletion_completed','system',$1,now()-interval '7 months','{}'),(gen_random_uuid(),'account.deletion_completed','system',$1,now()-interval '6 years','{}')")
            .bind(user).execute(admin).await?;
    sqlx::query("INSERT INTO public.fulfilled_account_deletions(user_id,completed_at) VALUES($1,now()-interval '6 years')").bind(user).execute(admin).await?;
    // Even an accidental direct DELETE grant plus forged GUC cannot pass
    // the independent trigger's trusted-owner check.
    QueryBuilder::<Postgres>::new("GRANT DELETE ON public.admin_audit_events TO ")
        .push(runtime_name)
        .build()
        .execute(admin)
        .await?;
    let mut forged = runtime.begin().await?;
    sqlx::query("SET LOCAL deepref.audit_purge='on'")
        .execute(&mut *forged)
        .await?;
    let refusal = sqlx::query(
        "DELETE FROM public.admin_audit_events WHERE occurred_at<now()-interval '6 months'",
    )
    .execute(&mut *forged)
    .await
    .err()
    .context("forged purge GUC bypassed trusted-owner gate")?;
    anyhow::ensure!(
        refusal
            .as_database_error()
            .and_then(|error| error.code())
            .is_some_and(|code| code == "23000")
    );
    forged.rollback().await?;
    anyhow::ensure!(
        verify_database_role(runtime, Purpose::Runtime)
            .await
            .is_err(),
        "activation must detect privilege drift"
    );
    QueryBuilder::<Postgres>::new("REVOKE DELETE ON public.admin_audit_events FROM ")
        .push(runtime_name)
        .build()
        .execute(admin)
        .await?;
    verify_database_role(runtime, Purpose::Runtime).await?;
    let function_safe:bool=sqlx::query_scalar("SELECT prosecdef AND proowner='deepref_migration_owner'::regrole AND proconfig @> ARRAY['search_path=pg_catalog'] FROM pg_proc WHERE oid='public.purge_retention_records(integer)'::regprocedure").fetch_one(admin).await?;
    anyhow::ensure!(function_safe);
    for invalid in [-1, 0, 1001] {
        let error = sqlx::query("SELECT public.purge_retention_records($1)")
            .bind(invalid)
            .execute(maintenance)
            .await
            .err()
            .context("maintenance input bound was bypassed")?;
        anyhow::ensure!(
            error
                .as_database_error()
                .and_then(|error| error.code())
                .is_some_and(|code| code == "22023")
        );
    }
    let null_batch = sqlx::query("SELECT public.purge_retention_records($1)")
        .bind(None::<i32>)
        .execute(maintenance)
        .await
        .err()
        .context("NULL maintenance input bound was bypassed")?;
    anyhow::ensure!(
        null_batch
            .as_database_error()
            .and_then(|error| error.code())
            .is_some_and(|code| code == "22023")
    );
    let mut blocker = admin.begin().await?;
    sqlx::query("LOCK TABLE public.admin_audit_events IN ACCESS EXCLUSIVE MODE")
        .execute(&mut *blocker)
        .await?;
    let bounded_read = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        deepref_app::audit_admin::purge_admin_audit(maintenance, true, Some(1)),
    )
    .await
    .context("blocked dry-run retention query did not obey its budget")?;
    let bounded_error = bounded_read
        .err()
        .context("blocked retention read unexpectedly succeeded")?;
    let timeout_refusal = match &bounded_error {
        sqlx::Error::Io(error) => error.kind() == std::io::ErrorKind::TimedOut,
        sqlx::Error::Database(error) => error.code().is_some_and(|code| code == "57014"),
        _ => false,
    };
    // A client deadline alone must not leave its backend waiting forever on
    // the still-held lock. PostgreSQL's statement deadline must clear it too.
    let backend_stopped = tokio::time::timeout(std::time::Duration::from_secs(1),async {
        loop {
            let waiting: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE usename=$1 AND wait_event_type='Lock')")
                .bind(maintenance_name.as_str()).fetch_one(admin).await?;
            if !waiting { return Ok::<(),anyhow::Error>(()); }
            tokio::task::yield_now().await;
        }
    }).await;
    blocker.rollback().await?;
    anyhow::ensure!(
        timeout_refusal,
        "unexpected retention refusal: {bounded_error}"
    );
    backend_stopped.context("retention backend query outlived its statement budget")??;
    let report = deepref_app::audit_admin::purge_admin_audit(maintenance, false, Some(1)).await?;
    anyhow::ensure!(
        report.audit_rows_purged == 2
            && report.fulfilled_deletion_rows_purged == 1
            && report.batches == 3
    );
    let retained:i64=sqlx::query_scalar("SELECT count(*) FROM public.admin_audit_events WHERE event_type='account.deletion_completed'").fetch_one(admin).await?;
    anyhow::ensure!(
        retained == 1,
        "five-year proof cannot use ordinary six-month cutoff"
    );
    let young: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM public.admin_audit_events WHERE event_type='auth.login'",
    )
    .fetch_one(admin)
    .await?;
    anyhow::ensure!(young == 1);
    let audit_count:i64=sqlx::query_scalar("SELECT count(*) FROM public.admin_audit_events WHERE event_type='audit.purged' AND metadata=jsonb_build_object('rows_purged',2)").fetch_one(admin).await?;
    anyhow::ensure!(audit_count == 1, "purge summary commits with deletions");
    sqlx::query("INSERT INTO public.admin_audit_events(id,event_type,principal_kind,occurred_at,metadata) SELECT gen_random_uuid(),'auth.login','system',now()-interval '7 months','{}' FROM generate_series(1,40)").execute(admin).await?;
    let bounded = deepref_app::audit_admin::purge_admin_audit(maintenance, false, Some(1)).await?;
    anyhow::ensure!(bounded.batches == 32 && bounded.audit_rows_purged == 32);
    let remaining:i64=sqlx::query_scalar("SELECT count(*) FROM public.admin_audit_events WHERE event_type='auth.login' AND occurred_at<now()-interval '6 months'").fetch_one(admin).await?;
    anyhow::ensure!(
        remaining == 8,
        "one operator call must not run an unbounded purge loop"
    );
    Ok(())
}

// These are trusted immutable migration fragments, never SQL obtained from the
// live catalog or operator input. Display is a closed fixture-only DDL seam.
#[derive(Clone, Copy)]
enum ProtectedFixtureFunction {
    Guard,
    Purge,
    Redaction,
}
impl std::fmt::Display for ProtectedFixtureFunction {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let (source, marker) = match self {
            Self::Guard => (
                include_str!("../migrations/0188_database_roles_and_bounded_audit_retention.sql"),
                "CREATE OR REPLACE FUNCTION reject_admin_audit_event_changes()",
            ),
            Self::Purge => (
                include_str!("../migrations/0188_database_roles_and_bounded_audit_retention.sql"),
                "CREATE FUNCTION public.purge_retention_records(",
            ),
            Self::Redaction => (
                include_str!(
                    "../migrations/0117_authorized_account_redaction_and_address_digests.sql"
                ),
                "CREATE FUNCTION redact_deleted_user_audit(",
            ),
        };
        let definition = source.split_once(marker).ok_or(std::fmt::Error)?.1;
        let (definition, _) = definition.split_once("$$;").ok_or(std::fmt::Error)?;
        write!(
            f,
            "CREATE OR REPLACE FUNCTION{}{}$$;",
            marker.split_once("FUNCTION").ok_or(std::fmt::Error)?.1,
            definition
        )
    }
}

async fn backend_has_stopped(admin: &PgPool, name: &FixtureLogin) -> Result<()> {
    tokio::time::timeout(std::time::Duration::from_secs(1), async {
        loop {
            let waiting: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE usename=$1 AND wait_event_type='Lock')")
                .bind(name.as_str()).fetch_one(admin).await?;
            if !waiting { return Ok::<(), anyhow::Error>(()); }
            tokio::task::yield_now().await;
        }
    }).await.context("backend still waiting while blocker remains held")??;
    Ok(())
}

async fn prove_review_corrections(
    admin: &PgPool,
    runtime: &PgPool,
    maintenance: &PgPool,
    names: [&FixtureLogin; 3],
) -> Result<()> {
    let mut failures = Vec::new();
    prove_sequence_repairs(admin, runtime, maintenance, names, &mut failures).await?;
    prove_protected_definitions(admin, runtime, &mut failures).await?;
    prove_bounded_operational_reads(admin, runtime, maintenance, names, &mut failures).await?;
    anyhow::ensure!(failures.is_empty(), "review regressions: {failures:#?}");
    Ok(())
}

async fn prove_sequence_repairs(
    admin: &PgPool,
    runtime: &PgPool,
    maintenance: &PgPool,
    names: [&FixtureLogin; 3],
    failures: &mut Vec<String>,
) -> Result<()> {
    let [runtime_name, migration_name, maintenance_name] = names;
    // Exact relation in this UUID-isolated database, never an existing service.
    sqlx::query("CREATE SEQUENCE public.role_sequence_fixture")
        .execute(admin)
        .await?;
    // Every injected drift is restored before recording failure, so one old
    // omission does not hide independent baseline regressions or poison cleanup.
    for (grant, revoke, purpose, pool) in [
        (
            "GRANT USAGE ON ALL SEQUENCES IN SCHEMA public TO PUBLIC",
            "REVOKE USAGE ON ALL SEQUENCES IN SCHEMA public FROM PUBLIC",
            Purpose::Runtime,
            runtime,
        ),
        (
            "GRANT UPDATE ON ALL SEQUENCES IN SCHEMA public TO PUBLIC",
            "REVOKE UPDATE ON ALL SEQUENCES IN SCHEMA public FROM PUBLIC",
            Purpose::Runtime,
            runtime,
        ),
        (
            "GRANT USAGE ON ALL SEQUENCES IN SCHEMA public TO deepref_maintenance",
            "REVOKE USAGE ON ALL SEQUENCES IN SCHEMA public FROM deepref_maintenance",
            Purpose::Maintenance,
            maintenance,
        ),
        (
            "GRANT UPDATE ON ALL SEQUENCES IN SCHEMA public TO deepref_runtime",
            "REVOKE UPDATE ON ALL SEQUENCES IN SCHEMA public FROM deepref_runtime",
            Purpose::Runtime,
            runtime,
        ),
    ] {
        // There need not be a serial column in today's schema; an actual
        // owner-created sequence exercises the separate ACL family explicitly.
        sqlx::query(grant).execute(admin).await?;
        let refused = verify_database_role(pool, purpose).await.is_err();
        sqlx::query(revoke).execute(admin).await?;
        if !refused {
            failures.push(format!("sequence drift accepted: {grant}"));
        }
    }
    sqlx::query("ALTER SEQUENCE public.role_sequence_fixture OWNER TO deepref_migration_owner")
        .execute(admin)
        .await?;
    sqlx::query("GRANT SELECT ON SEQUENCE public.role_sequence_fixture TO deepref_runtime")
        .execute(admin)
        .await?;
    sqlx::query(
        "REVOKE USAGE,UPDATE ON SEQUENCE public.role_sequence_fixture FROM deepref_runtime",
    )
    .execute(admin)
    .await?;
    sqlx::query(
        "GRANT USAGE,UPDATE ON SEQUENCE public.role_sequence_fixture TO PUBLIC,deepref_maintenance",
    )
    .execute(admin)
    .await?;
    sqlx::query("ALTER DEFAULT PRIVILEGES FOR ROLE deepref_migration_owner IN SCHEMA public REVOKE USAGE ON SEQUENCES FROM deepref_runtime").execute(admin).await?;
    sqlx::query("ALTER DEFAULT PRIVILEGES FOR ROLE deepref_migration_owner GRANT UPDATE ON SEQUENCES TO PUBLIC").execute(admin).await?;
    let defaults_refused = verify_database_role(runtime, Purpose::Runtime)
        .await
        .is_err();
    if !defaults_refused {
        failures.push("unsafe future sequence default accepted".into());
    }
    // Protected-family repair must apply after activation, not just initially.
    sqlx::query("GRANT INSERT,UPDATE,DELETE ON public.user_sessions,public.database_role_contract,public._sqlx_migrations TO deepref_maintenance").execute(admin).await?;
    provision_database_roles(
        admin,
        runtime_name.as_str(),
        migration_name.as_str(),
        maintenance_name.as_str(),
    )
    .await?;
    for (pool, statement) in [
        (runtime, "SELECT nextval('public.role_sequence_fixture')"),
        (runtime, "SELECT setval('public.role_sequence_fixture',99)"),
        (
            maintenance,
            "SELECT nextval('public.role_sequence_fixture')",
        ),
        (
            maintenance,
            "SELECT setval('public.role_sequence_fixture',99)",
        ),
        (maintenance, "DELETE FROM public.user_sessions WHERE false"),
        (
            maintenance,
            "UPDATE public.database_role_contract SET activated_at=NULL WHERE false",
        ),
        (
            maintenance,
            "DELETE FROM public._sqlx_migrations WHERE false",
        ),
    ] {
        if let Err(error) = deny(pool, statement).await {
            failures.push(format!("repair {statement}: {error}"));
        }
    }
    // Create through the actual migration identity, including SET ROLE, so the
    // default ACL subject is the canonical owner rather than the administrator.
    let migration_options = (*admin.connect_options())
        .clone()
        .username(migration_name.as_str())
        .password("deepref-role-fixture-only");
    let future_migration = PgPoolOptions::new()
        .max_connections(1)
        .connect_with(migration_options)
        .await?;
    let mut owner = future_migration.begin().await?;
    sqlx::query("SET LOCAL ROLE deepref_migration_owner")
        .execute(&mut *owner)
        .await?;
    sqlx::query("CREATE SEQUENCE public.role_owner_future_sequence_fixture")
        .execute(&mut *owner)
        .await?;
    owner.commit().await?;
    future_migration.close().await;
    if let Err(error) = deny(
        runtime,
        "SELECT nextval('public.role_owner_future_sequence_fixture')",
    )
    .await
    {
        failures.push(format!("future narrowed runtime grant: {error}"));
    }
    if let Err(error) = deny(
        maintenance,
        "SELECT setval('public.role_owner_future_sequence_fixture',99)",
    )
    .await
    {
        failures.push(format!("future unsafe default: {error}"));
    }
    let narrowed: bool=sqlx::query_scalar("SELECT has_sequence_privilege(session_user,'public.role_sequence_fixture','SELECT') AND NOT has_sequence_privilege(session_user,'public.role_sequence_fixture','USAGE,UPDATE')").fetch_one(runtime).await?;
    if !narrowed {
        failures.push("reactivation widened later SELECT-only sequence grant".into());
    }
    sqlx::query("ALTER DEFAULT PRIVILEGES FOR ROLE deepref_migration_owner REVOKE UPDATE ON SEQUENCES FROM PUBLIC").execute(admin).await?;
    sqlx::query("REVOKE ALL ON SEQUENCE public.role_owner_future_sequence_fixture FROM PUBLIC,deepref_maintenance").execute(admin).await?;
    sqlx::query(
        "REVOKE UPDATE ON SEQUENCE public.role_owner_future_sequence_fixture FROM deepref_runtime",
    )
    .execute(admin)
    .await?;
    // Reset baseline omissions before independent function/lock checks.
    sqlx::query(
        "REVOKE ALL ON SEQUENCE public.role_sequence_fixture FROM PUBLIC,deepref_maintenance",
    )
    .execute(admin)
    .await?;
    sqlx::query(
        "REVOKE USAGE,UPDATE ON SEQUENCE public.role_sequence_fixture FROM deepref_runtime",
    )
    .execute(admin)
    .await?;
    sqlx::query("REVOKE INSERT,UPDATE,DELETE ON public.user_sessions,public.database_role_contract,public._sqlx_migrations FROM deepref_maintenance").execute(admin).await?;
    Ok(())
}

async fn prove_protected_definitions(
    admin: &PgPool,
    runtime: &PgPool,
    failures: &mut Vec<String>,
) -> Result<()> {
    for (function, drift) in [
        (
            ProtectedFixtureFunction::Guard,
            "CREATE OR REPLACE FUNCTION public.reject_admin_audit_event_changes() RETURNS trigger LANGUAGE plpgsql SET search_path=pg_catalog AS $$ BEGIN RETURN NEW; END; $$",
        ),
        (
            ProtectedFixtureFunction::Purge,
            "CREATE OR REPLACE FUNCTION public.purge_retention_records(p_batch integer) RETURNS TABLE(batches integer,audit_rows bigint,fulfilled_rows bigint,session_rows bigint) LANGUAGE plpgsql SECURITY DEFINER SET search_path=pg_catalog SET lock_timeout='250ms' AS $$ BEGIN RETURN; END; $$",
        ),
        (
            ProtectedFixtureFunction::Redaction,
            "CREATE OR REPLACE FUNCTION public.redact_deleted_user_audit(p_user_id uuid) RETURNS bigint LANGUAGE plpgsql SECURITY DEFINER SET search_path=pg_catalog AS $$ BEGIN RETURN 0; END; $$",
        ),
        (
            ProtectedFixtureFunction::Redaction,
            "CREATE OR REPLACE FUNCTION public.redact_deleted_user_audit(p_user_id uuid) RETURNS bigint LANGUAGE SQL SECURITY DEFINER SET search_path=pg_catalog AS 'SELECT 0::bigint'",
        ),
    ] {
        let before: serde_json::Value = sqlx::query_scalar("SELECT jsonb_agg(jsonb_build_array(oid,proowner,prosecdef,proconfig,proacl) ORDER BY oid) FROM pg_proc WHERE oid IN('public.reject_admin_audit_event_changes()'::regprocedure,'public.purge_retention_records(integer)'::regprocedure,'public.redact_deleted_user_audit(uuid)'::regprocedure)").fetch_one(admin).await?;
        sqlx::query(drift).execute(admin).await?;
        let after: serde_json::Value = sqlx::query_scalar("SELECT jsonb_agg(jsonb_build_array(oid,proowner,prosecdef,proconfig,proacl) ORDER BY oid) FROM pg_proc WHERE oid IN('public.reject_admin_audit_event_changes()'::regprocedure,'public.purge_retention_records(integer)'::regprocedure,'public.redact_deleted_user_audit(uuid)'::regprocedure)").fetch_one(admin).await?;
        let refused = verify_database_role(runtime, Purpose::Runtime)
            .await
            .is_err();
        QueryBuilder::<Postgres>::new("")
            .push(function)
            .build()
            .execute(admin)
            .await?;
        sqlx::query(
            "ALTER FUNCTION public.redact_deleted_user_audit(uuid) SET search_path=pg_catalog",
        )
        .execute(admin)
        .await?;
        anyhow::ensure!(
            before == after,
            "drift test changed previously checked attributes"
        );
        if !refused {
            failures.push(format!("protected source/language drift accepted: {drift}"));
        }
    }
    sqlx::query("CREATE FUNCTION public.role_wrong_trigger_fixture() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN RETURN NEW; END; $$").execute(admin).await?;
    for (drift, restore) in [
        (
            "ALTER TABLE public.admin_audit_events DISABLE TRIGGER admin_audit_events_append_only_trigger",
            "ALTER TABLE public.admin_audit_events ENABLE TRIGGER admin_audit_events_append_only_trigger",
        ),
        (
            "CREATE OR REPLACE TRIGGER admin_audit_events_append_only_trigger BEFORE UPDATE ON public.admin_audit_events FOR EACH ROW EXECUTE FUNCTION public.reject_admin_audit_event_changes()",
            "CREATE OR REPLACE TRIGGER admin_audit_events_append_only_trigger BEFORE UPDATE OR DELETE ON public.admin_audit_events FOR EACH ROW EXECUTE FUNCTION public.reject_admin_audit_event_changes()",
        ),
        (
            "CREATE OR REPLACE TRIGGER admin_audit_events_no_truncate_trigger BEFORE TRUNCATE ON public.admin_audit_events FOR EACH STATEMENT EXECUTE FUNCTION public.role_wrong_trigger_fixture()",
            "CREATE OR REPLACE TRIGGER admin_audit_events_no_truncate_trigger BEFORE TRUNCATE ON public.admin_audit_events FOR EACH STATEMENT EXECUTE FUNCTION public.reject_admin_audit_event_changes()",
        ),
    ] {
        sqlx::query(drift).execute(admin).await?;
        let refused = verify_database_role(runtime, Purpose::Runtime)
            .await
            .is_err();
        sqlx::query(restore).execute(admin).await?;
        if !refused {
            failures.push(format!("trigger drift accepted: {drift}"));
        }
    }
    Ok(())
}

async fn prove_bounded_operational_reads(
    admin: &PgPool,
    runtime: &PgPool,
    maintenance: &PgPool,
    names: [&FixtureLogin; 3],
    failures: &mut Vec<String>,
) -> Result<()> {
    let [runtime_name, _, maintenance_name] = names;
    let mut blocker = admin.begin().await?;
    sqlx::query("LOCK TABLE public.database_role_contract IN ACCESS EXCLUSIVE MODE")
        .execute(&mut *blocker)
        .await?;
    let refusal = verify_database_role(runtime, Purpose::Runtime).await;
    let stopped = backend_has_stopped(admin, runtime_name).await;
    blocker.rollback().await?;
    let backend_refusal = refusal.as_ref().err().is_some_and(|error| {
        error.chain().any(|cause| {
            cause
                .downcast_ref::<sqlx::Error>()
                .and_then(sqlx::Error::as_database_error)
                .and_then(|error| error.code())
                .is_some_and(|code| code == "55P03" || code == "57014")
        })
    });
    if !backend_refusal || stopped.is_err() {
        failures.push(format!(
            "role verification backend not bounded: {refusal:?}, {stopped:?}"
        ));
    }
    verify_database_role(runtime, Purpose::Runtime).await?;
    // This new app entry point has no old public counterpart: this is green
    // server-cancellation coverage, not a claimed baseline behavioral red.
    let mut blocker = admin.begin().await?;
    sqlx::query("LOCK TABLE public.admin_audit_events IN ACCESS EXCLUSIVE MODE")
        .execute(&mut *blocker)
        .await?;
    let remaining = deepref_app::audit_admin::remaining_retention_eligible(maintenance).await;
    let stopped = backend_has_stopped(admin, maintenance_name).await;
    blocker.rollback().await?;
    let backend_refusal = remaining
        .as_ref()
        .err()
        .and_then(sqlx::Error::as_database_error)
        .and_then(|error| error.code())
        .is_some_and(|code| code == "55P03" || code == "57014");
    if !backend_refusal || stopped.is_err() {
        failures.push(format!(
            "remaining-query backend not bounded: {remaining:?}, {stopped:?}"
        ));
    }
    // The max-one maintenance pool is immediately reusable after cancellation.
    deepref_app::audit_admin::remaining_retention_eligible(maintenance).await?;
    Ok(())
}
