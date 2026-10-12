#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
//! Per-test PostgreSQL isolation for integration tests (feature `test-support`).
//!
//! Every helper here hands out a **private database** cloned from a migrated
//! template, so test binaries can run in parallel without claiming each
//! other's jobs or reading each other's fixtures (`claim_job` takes the next
//! job of *any* project, and an in-process mutex cannot serialize nextest's
//! per-process test runners).
//!
//! Usage from an integration test's `database()` helper:
//! ```ignore
//! async fn database() -> Option<PgPool> {
//!     deepref_postgres::test_isolation::isolated_pool_from_env("DATABASE_URL", 8).await
//! }
//! ```
//! The clone is already fully migrated; tests need not call `migrate` again.
//! The helpers return `None` when no URL is set (local skip) and `panic!`
//! when `DEEPREF_REQUIRE_DATABASE=1` is set without a URL (CI must fail
//! closed instead of passing vacuously).
//!
//! How the databases are managed (the connecting role needs `CREATEDB`):
//! - **Template.** Named `dtpl_<hash>` after the versions and checksums of the
//!   embedded migrations, so checkouts with different migration sets sharing a
//!   server never see each other's schema. It is built once, under a temporary
//!   `dt_` name, and renamed into place only after every migration applied; a
//!   crash mid-setup leaves a temporary database for the reaper, never a
//!   half-migrated template. When the template exists, setup is skipped.
//! - **Clones.** Named `dt_<unix secs hex>_<rand>`. Each one is held by a lease
//!   connection that this process keeps open until it exits, so a clone with
//!   no connections belongs to a finished test.
//! - **Reaper.** Each setup starts a background task that drops a few clones
//!   left by earlier runs: no connections and older than ten minutes. It never
//!   uses `FORCE`, and tests never wait for it. `DROP DATABASE` forces an
//!   immediate checkpoint, which takes 15-90 s under a parallel suite, so
//!   dropping a run's own clones while it runs stalls tests into timeouts.
//!   Clones of the current run (about 14 MB each) stay until a later run.

use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use sha2::{Digest, Sha256};
use sqlx::{AssertSqlSafe, Connection, PgConnection, PgPool, postgres::PgPoolOptions};
use uuid::Uuid;

/// Prefix for per-test and in-construction databases. Must stay a valid
/// unquoted identifier prefix and match the reaper's `LIKE` filter.
const TEST_DB_PREFIX: &str = "dt_";
/// Prefix for migrated templates, followed by the migration-set hash.
const TEMPLATE_PREFIX: &str = "dtpl_";
/// Advisory-lock key making template construction and reaping exclusive.
const SETUP_LOCK_KEY: i64 = 810_311;
/// Clones younger than this are never reaped (see the module docs).
const REAP_AFTER_SECS: u64 = 600;
/// Most clones one background reaper drops.
const REAP_BATCH: usize = 4;

/// One connection per database this process handed out. Never closed: the
/// operating system closes them when the process exits, which is what tells
/// the reaper that the database's test is over.
static LEASES: Mutex<Vec<PgConnection>> = Mutex::new(Vec::new());

/// The first set variable among `vars`, `None` to skip locally, or a panic
/// under `DEEPREF_REQUIRE_DATABASE=1`.
fn base_url(vars: &[&str]) -> Option<String> {
    if let Some(url) = vars.iter().find_map(|var| std::env::var(var).ok()) {
        return Some(url);
    }
    if std::env::var("DEEPREF_REQUIRE_DATABASE").as_deref() == Ok("1") {
        panic!(
            "{} must be set when DEEPREF_REQUIRE_DATABASE=1",
            vars.join(" or ")
        )
    }
    None
}

/// `base_url` on the same server and credentials, naming `database` instead.
/// Any query string (`sslmode`, …) is kept.
fn with_database(base_url: &str, database: &str) -> String {
    let (base, query) = match base_url.split_once('?') {
        Some((base, query)) => (base, Some(query)),
        None => (base_url, None),
    };
    let (server, _) = base
        .rsplit_once('/')
        .expect("database URL names a database");
    match query {
        Some(query) => format!("{server}/{database}?{query}"),
        None => format!("{server}/{database}"),
    }
}

/// Template name derived from every migration's version, kind and checksum.
fn template_name<'a>(migrations: impl IntoIterator<Item = &'a sqlx::migrate::Migration>) -> String {
    let mut hasher = Sha256::new();
    for migration in migrations {
        hasher.update(migration.version.to_le_bytes());
        hasher.update(format!("{:?}", migration.migration_type).as_bytes());
        hasher.update(migration.checksum.as_ref());
    }
    let hex: String = hasher
        .finalize()
        .iter()
        .take(8)
        .map(|byte| format!("{byte:02x}"))
        .collect();
    format!("{TEMPLATE_PREFIX}{hex}")
}

fn new_test_db_name() -> String {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system time is after the epoch")
        .as_secs();
    let rand = Uuid::new_v4().simple().to_string();
    format!(
        "{TEST_DB_PREFIX}{secs:x}_{}",
        rand.get(..8).unwrap_or(&rand)
    )
}

fn db_age_secs(name: &str) -> Option<u64> {
    let rest = name.strip_prefix(TEST_DB_PREFIX)?;
    let (hex, _) = rest.split_once('_')?;
    let created = u64::from_str_radix(hex, 16).ok()?;
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .map(|now| now.as_secs().saturating_sub(created))
}

async fn database_exists(maintenance: &mut PgConnection, name: &str) -> bool {
    sqlx::query_scalar::<_, bool>("SELECT EXISTS (SELECT 1 FROM pg_database WHERE datname = $1)")
        .bind(name)
        .fetch_one(maintenance)
        .await
        .expect("pg_database is readable")
}

async fn create_database(maintenance: &mut PgConnection, name: &str, template: Option<&str>) {
    let sql = match template {
        Some(template) => format!("CREATE DATABASE \"{name}\" TEMPLATE \"{template}\""),
        None => format!("CREATE DATABASE \"{name}\""),
    };
    sqlx::query(AssertSqlSafe(sql))
        .execute(maintenance)
        .await
        .unwrap_or_else(|error| panic!("test database {name} cannot be created: {error}"));
}

/// Builds the template for the current migration set unless it exists.
async fn ensure_template(maintenance: &mut PgConnection, base_url: &str, template: &str) {
    if database_exists(maintenance, template).await {
        return;
    }
    // Session-scoped: if setup panics, dropping the connection releases it.
    sqlx::query("SELECT pg_advisory_lock($1)")
        .bind(SETUP_LOCK_KEY)
        .execute(&mut *maintenance)
        .await
        .expect("template setup lock");
    if !database_exists(maintenance, template).await {
        let building = new_test_db_name();
        create_database(maintenance, &building, None).await;
        let mut connection = PgConnection::connect(&with_database(base_url, &building))
            .await
            .expect("template under construction is reachable");
        crate::MIGRATOR
            .run(&mut connection)
            .await
            .expect("test template migrations must apply");
        connection
            .close()
            .await
            .expect("template connection closes");
        let rename = format!("ALTER DATABASE \"{building}\" RENAME TO \"{template}\"");
        sqlx::query(AssertSqlSafe(rename))
            .execute(&mut *maintenance)
            .await
            .unwrap_or_else(|error| panic!("test template cannot be renamed into place: {error}"));
    }
    sqlx::query("SELECT pg_advisory_unlock($1)")
        .bind(SETUP_LOCK_KEY)
        .execute(maintenance)
        .await
        .expect("template setup unlock");
}

/// Drops up to [`REAP_BATCH`] clones of finished earlier runs. Skipped while
/// another process builds a template or reaps (the lock covers both). Runs
/// detached, so errors are ignored and it never panics.
async fn reap(mut maintenance: PgConnection) {
    let locked: Result<bool, _> = sqlx::query_scalar("SELECT pg_try_advisory_lock($1)")
        .bind(SETUP_LOCK_KEY)
        .fetch_one(&mut maintenance)
        .await;
    if locked.is_ok_and(|locked| locked) {
        let idle: Vec<String> = sqlx::query_scalar(
            "SELECT d.datname FROM pg_database d
             WHERE d.datname LIKE 'dt\\_%'
               AND NOT EXISTS (SELECT 1 FROM pg_stat_activity a WHERE a.datid = d.oid)",
        )
        .fetch_all(&mut maintenance)
        .await
        .unwrap_or_default();
        let finished = idle
            .into_iter()
            .filter(|name| db_age_secs(name).is_some_and(|age| age > REAP_AFTER_SECS))
            .take(REAP_BATCH);
        for name in finished {
            // Without FORCE: a database somebody connected to meanwhile stays.
            let _ = sqlx::query(AssertSqlSafe(format!("DROP DATABASE IF EXISTS \"{name}\"")))
                .execute(&mut maintenance)
                .await;
        }
    }
    // Closing ends the session, which also releases the lock. If the test
    // process exits first, the server still finishes the DROP in flight.
    let _ = maintenance.close().await;
}

/// A private database URL leased to this process: a clone of the migrated
/// template, or an empty database when `migrated` is false.
async fn isolated_url(vars: &[&str], migrated: bool) -> Option<String> {
    let base = base_url(vars)?;
    let mut maintenance = PgConnection::connect(&with_database(&base, "postgres"))
        .await
        .expect("test maintenance database must be reachable (role needs CREATEDB)");
    let template = migrated.then(|| template_name(crate::MIGRATOR.iter()));
    if let Some(template) = &template {
        ensure_template(&mut maintenance, &base, template).await;
    }
    let name = new_test_db_name();
    create_database(&mut maintenance, &name, template.as_deref()).await;
    let url = with_database(&base, &name);
    let lease = PgConnection::connect(&url)
        .await
        .expect("isolated test database must be reachable");
    LEASES
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .push(lease);
    tokio::spawn(reap(maintenance));
    Some(url)
}

/// A private, migrated database URL for the calling test, or `None` when no
/// URL is configured (local skip). Schema-scoped migration tests that manage
/// their own schemas connect to this URL exactly like the shared one.
pub async fn isolated_url_from_env(var: &str) -> Option<String> {
    isolated_url(&[var], true).await
}

/// A private, **empty** database URL (no migrations applied), for tests that
/// replay migrations over a legacy schema. Leased and reaped like the clones.
pub async fn isolated_empty_url_from_env(var: &str) -> Option<String> {
    isolated_url(&[var], false).await
}

/// A pool on a private, migrated database for the calling test.
pub async fn isolated_pool_from_env(var: &str, max_connections: u32) -> Option<PgPool> {
    isolated_pool_from_env_first(&[var], max_connections).await
}

/// Like [`isolated_pool_from_env`], reading the first of `vars` that is set
/// (e.g. `TEST_DATABASE_URL` then `DATABASE_URL`).
pub async fn isolated_pool_from_env_first(vars: &[&str], max_connections: u32) -> Option<PgPool> {
    let url = isolated_url(vars, true).await?;
    Some(
        PgPoolOptions::new()
            .max_connections(max_connections)
            .connect(&url)
            .await
            .expect("isolated test database must be reachable"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn database_is_swapped_and_query_string_kept() {
        assert_eq!(
            with_database("postgres://u:p@h:5432/deepref?sslmode=disable", "dt_1_a"),
            "postgres://u:p@h:5432/dt_1_a?sslmode=disable"
        );
        assert_eq!(
            with_database("postgres://u:p@h/deepref", "postgres"),
            "postgres://u:p@h/postgres"
        );
    }

    #[test]
    fn template_name_tracks_the_migration_set() {
        let current: Vec<_> = crate::MIGRATOR.iter().cloned().collect();
        let name = template_name(&current);
        assert!(name.starts_with(TEMPLATE_PREFIX));
        assert_eq!(name, template_name(crate::MIGRATOR.iter()));

        let fewer = template_name(current.iter().take(current.len() - 1));
        assert_ne!(name, fewer, "a branch with one migration less");

        let mut edited = current.clone();
        let last = edited.last_mut().expect("at least one migration");
        last.checksum = std::borrow::Cow::Owned(vec![0; 48]);
        assert_ne!(name, template_name(&edited), "an edited migration");
    }

    #[test]
    fn clone_age_comes_from_its_name_and_foreign_names_have_none() {
        let name = new_test_db_name();
        assert!(db_age_secs(&name).is_some_and(|age| age <= 1));
        assert_eq!(db_age_secs("dt_zz_abc"), None);
        assert_eq!(db_age_secs("deepref"), None);
        assert!(db_age_secs("dt_1_abc").is_some_and(|age| age > REAP_AFTER_SECS));
    }
}
