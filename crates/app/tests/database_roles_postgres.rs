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
                .max_connections(2)
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
