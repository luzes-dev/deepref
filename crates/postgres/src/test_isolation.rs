#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
//! Per-test PostgreSQL isolation for integration tests.
//!
//! Every helper here hands out a **private database** cloned from a migrated
//! template, so test binaries can run in parallel without claiming each
//! other's jobs or reading each other's fixtures. This is the preferred fix
//! for the shared-database interference described in Packet 1 of the
//! 2026-10-10 fix handoff: `claim_job` takes the next job of *any* project,
//! and an in-process mutex cannot serialize nextest's per-process test
//! runners.
//!
//! Usage from an integration test's `database()` helper:
//! ```ignore
//! async fn database() -> Option<PgPool> {
//!     deepref_postgres::test_isolation::isolated_pool_from_env("DATABASE_URL", 8).await
//! }
//! ```
//! The helpers return `None` when no URL is set (local skip) and `panic!`
//! when `DEEPREF_REQUIRE_DATABASE=1` is set without a URL (CI must fail
//! closed instead of passing vacuously).
//!
//! Requirements on the server: the connecting role needs `CREATEDB` (the
//! disposable CI/local superuser has it). Databases are named `dt_<time>_<rand>`;
//! each call also drops same-prefix databases older than an hour, so repeated
//! local runs do not accumulate garbage. CI runners start from a fresh server.

use std::time::{SystemTime, UNIX_EPOCH};

use sqlx::{PgPool, postgres::PgPoolOptions};
use uuid::Uuid;

/// Template database holding the fully migrated schema. Cloned per call.
const TEMPLATE_DB: &str = "deepref_test_template";
/// Prefix for per-call test databases. Must stay a valid unquoted identifier
/// prefix and match the janitor's `LIKE` filter below.
const TEST_DB_PREFIX: &str = "dt_";
/// Advisory-lock key serializing template setup across test processes.
const SETUP_LOCK_KEY: i64 = 810311;
/// Test databases older than this are dropped by the janitor pass.
const JANITOR_MAX_AGE_SECS: u64 = 3600;

fn require_url(var: &str) -> Option<String> {
    match std::env::var(var) {
        Ok(url) => Some(url),
        Err(_) if std::env::var("DEEPREF_REQUIRE_DATABASE").as_deref() == Ok("1") => {
            panic!("{var} must be set when DEEPREF_REQUIRE_DATABASE=1")
        }
        Err(_) => None,
    }
}

fn resolve_first(vars: &[&str]) -> Option<String> {
    for var in vars {
        if let Ok(url) = std::env::var(var) {
            return Some(url);
        }
    }
    if std::env::var("DEEPREF_REQUIRE_DATABASE").as_deref() == Ok("1") {
        panic!(
            "{} must be set when DEEPREF_REQUIRE_DATABASE=1",
            vars.join(" or ")
        )
    }
    None
}

/// The same server and credentials, pointed at the maintenance database.
fn maintenance_url(base_url: &str) -> String {
    let (base, query) = match base_url.split_once('?') {
        Some((base, query)) => (base, Some(query)),
        None => (base_url, None),
    };
    let (server, _current) = base
        .rsplit_once('/')
        .expect("database URL names a database");
    match query {
        Some(query) => format!("{server}/postgres?{query}"),
        None => format!("{server}/postgres"),
    }
}

fn duplicate_database(error: &sqlx::Error) -> bool {
    error
        .as_database_error()
        .is_some_and(|db| db.code().as_deref() == Some("42P04"))
}

fn new_test_db_name() -> String {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system time is after the epoch")
        .as_secs();
    let rand: String = Uuid::new_v4()
        .simple()
        .to_string()
        .chars()
        .take(8)
        .collect();
    format!("{TEST_DB_PREFIX}{secs:x}_{rand}")
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

fn template_url(base_url: &str) -> String {
    let (server, _) = base_url
        .split_once('?')
        .map_or(base_url, |(base, _)| base)
        .rsplit_once('/')
        .expect("database URL names a database");
    match base_url.split_once('?') {
        Some((_, query)) => format!("{server}/{TEMPLATE_DB}?{query}"),
        None => format!("{server}/{TEMPLATE_DB}"),
    }
}

async fn ensure_template(maintenance: &PgPool, base_url: &str) {
    sqlx::query("SELECT pg_advisory_lock($1)")
        .bind(SETUP_LOCK_KEY)
        .execute(maintenance)
        .await
        .expect("template setup lock");
    // The lock is session-scoped: a panic here drops the maintenance
    // connection and releases it, so setup cannot deadlock.
    let created = sqlx::query(sqlx::AssertSqlSafe(format!(
        "CREATE DATABASE \"{TEMPLATE_DB}\""
    )))
    .execute(maintenance)
    .await;
    match created {
        Ok(_) => {}
        Err(error) if duplicate_database(&error) => {}
        Err(error) => panic!("test template database cannot be created: {error}"),
    }
    let template = PgPoolOptions::new()
        .max_connections(2)
        .connect(&template_url(base_url))
        .await
        .expect("test template database must be reachable");
    crate::migrate(&template)
        .await
        .expect("test template migrations must apply");
    template.close().await;
    sqlx::query("SELECT pg_advisory_unlock($1)")
        .bind(SETUP_LOCK_KEY)
        .execute(maintenance)
        .await
        .expect("template setup unlock");
}

async fn janitor(maintenance: &PgPool) {
    let names: Vec<String> =
        sqlx::query_scalar("SELECT datname FROM pg_database WHERE datname LIKE 'dt\\_%'")
            .fetch_all(maintenance)
            .await
            .unwrap_or_default();
    for name in names {
        let old = db_age_secs(&name).is_some_and(|age| age > JANITOR_MAX_AGE_SECS);
        if !old {
            continue;
        }
        let _ = sqlx::query(sqlx::AssertSqlSafe(format!(
            "DROP DATABASE \"{name}\" WITH (FORCE)"
        )))
        .execute(maintenance)
        .await;
    }
}

async fn create_test_db(maintenance: &PgPool, base_url: &str) -> String {
    let name = new_test_db_name();
    sqlx::query(sqlx::AssertSqlSafe(format!(
        "CREATE DATABASE \"{name}\" TEMPLATE \"{TEMPLATE_DB}\""
    )))
    .execute(maintenance)
    .await
    .unwrap_or_else(|error| panic!("test database {name} cannot be cloned: {error}"));
    let (server, _) = base_url
        .split_once('?')
        .map_or(base_url, |(base, _)| base)
        .rsplit_once('/')
        .expect("database URL names a database");
    match base_url.split_once('?') {
        Some((_, query)) => format!("{server}/{name}?{query}"),
        None => format!("{server}/{name}"),
    }
}

async fn setup(base_url: &str) -> Option<String> {
    let maintenance = PgPoolOptions::new()
        .max_connections(2)
        .connect(&maintenance_url(base_url))
        .await
        .expect("test maintenance database must be reachable (role needs CREATEDB)");
    ensure_template(&maintenance, base_url).await;
    janitor(&maintenance).await;
    let url = create_test_db(&maintenance, base_url).await;
    maintenance.close().await;
    Some(url)
}

/// A private, migrated database URL for the calling test, or `None` when no
/// URL is configured (local skip). Schema-scoped migration tests that manage
/// their own schemas connect to this URL exactly like the shared one.
pub async fn isolated_url_from_env(var: &str) -> Option<String> {
    let base = require_url(var)?;
    setup(&base).await
}

/// A pool on a private, migrated database for the calling test.
pub async fn isolated_pool_from_env(var: &str, max_connections: u32) -> Option<PgPool> {
    let base = require_url(var)?;
    let url = setup(&base).await?;
    Some(
        PgPoolOptions::new()
            .max_connections(max_connections)
            .connect(&url)
            .await
            .expect("isolated test database must be reachable"),
    )
}

/// Like [`isolated_pool_from_env`] with `TEST_DATABASE_URL`-then-`DATABASE_URL`
/// fallback for suites that historically preferred a separate test database.
pub async fn isolated_pool_from_env_first(vars: &[&str], max_connections: u32) -> Option<PgPool> {
    let base = resolve_first(vars)?;
    let url = setup(&base).await?;
    Some(
        PgPoolOptions::new()
            .max_connections(max_connections)
            .connect(&url)
            .await
            .expect("isolated test database must be reachable"),
    )
}
