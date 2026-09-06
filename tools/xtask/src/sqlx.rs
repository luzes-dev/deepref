use std::path::Path;
use std::process::Command;

use anyhow::{Context, Result, bail};

use crate::workspace::find_root;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SqlxSubcommand {
    Prepare,
    Check,
}

pub fn run(subcommand: SqlxSubcommand) -> Result<()> {
    let root = find_root()?;
    let migrations_dir = root.join("crates/postgres/migrations");
    if !migrations_dir.is_dir() {
        bail!(
            "migrations directory not found at {}",
            migrations_dir.display()
        );
    }

    let database_url = resolve_database_url(&root)?;
    ensure_sqlx_cli()?;

    match subcommand {
        SqlxSubcommand::Prepare => {
            println!("Applying migrations before prepare...");
            run_migrations(&root, &database_url)?;

            println!("Running cargo sqlx prepare --workspace...");
            let status = Command::new("cargo")
                .args(["sqlx", "prepare", "--workspace"])
                .current_dir(&root)
                .env("DATABASE_URL", &database_url)
                .status()
                .context("failed to invoke 'cargo sqlx prepare --workspace'")?;
            if !status.success() {
                bail!(
                    "'cargo sqlx prepare --workspace' failed with exit code: {:?}",
                    status.code()
                );
            }
            println!("SQLx offline metadata successfully written to .sqlx/");
        }
        SqlxSubcommand::Check => {
            println!("Running cargo sqlx prepare --check --workspace...");
            let output = Command::new("cargo")
                .args(["sqlx", "prepare", "--check", "--workspace"])
                .current_dir(&root)
                .env("DATABASE_URL", &database_url)
                .output()
                .context("failed to invoke 'cargo sqlx prepare --check --workspace'")?;
            if !output.status.success() {
                let stdout = String::from_utf8_lossy(&output.stdout);
                let stderr = String::from_utf8_lossy(&output.stderr);
                if !stdout.is_empty() {
                    eprintln!("{stdout}");
                }
                if !stderr.is_empty() {
                    eprintln!("{stderr}");
                }
                bail!(
                    "SQLx offline metadata in .sqlx/ is stale or out of sync with current queries/schema.\n\
                     To fix: ensure PostgreSQL is running and migrated, then run:\n  \
                     cargo xtask sqlx prepare\n\
                     and commit the updated .sqlx/ metadata directory."
                );
            }
            println!("SQLx offline metadata in .sqlx/ is up to date.");
        }
    }
    Ok(())
}

pub fn resolve_database_url(root: &Path) -> Result<String> {
    if let Ok(url) = std::env::var("DATABASE_URL")
        && !url.trim().is_empty()
    {
        return Ok(url.trim().to_owned());
    }

    let env_file = root.join(".env");
    if env_file.is_file() {
        let content = std::fs::read_to_string(&env_file)
            .with_context(|| format!("failed to read {}", env_file.display()))?;
        if let Some(url) = parse_database_url_from_env_str(&content) {
            return Ok(url);
        }
    }

    bail!(
        "DATABASE_URL is not set. Please export DATABASE_URL or define it in .env to run sqlx operations."
    )
}

pub fn parse_database_url_from_env_str(content: &str) -> Option<String> {
    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('#') || trimmed.is_empty() {
            continue;
        }
        if let Some(rest) = trimmed.strip_prefix("DATABASE_URL=") {
            let unquoted = rest.trim().trim_matches('"').trim_matches('\'').trim();
            if !unquoted.is_empty() {
                return Some(unquoted.to_owned());
            }
        }
    }
    None
}

pub fn ensure_sqlx_cli() -> Result<()> {
    let output = Command::new("cargo").args(["sqlx", "--version"]).output();
    match output {
        Ok(out) if out.status.success() => Ok(()),
        _ => bail!(
            "sqlx-cli is required for sqlx operations.\n\
             Install it via:\n  \
             cargo binstall sqlx-cli@0.9.0\n\
             or\n  \
             cargo install sqlx-cli --version 0.9.0 --no-default-features --features postgres,native-tls"
        ),
    }
}

fn run_migrations(root: &Path, database_url: &str) -> Result<()> {
    let source = root.join("crates/postgres/migrations");
    let status = Command::new("cargo")
        .args(migration_arguments(&source, database_url))
        .current_dir(root)
        .status()
        .context("failed to invoke 'cargo sqlx migrate run'")?;
    if !status.success() {
        bail!(
            "database migration failed with exit code: {:?}",
            status.code()
        );
    }
    Ok(())
}

fn migration_arguments(source: &Path, database_url: &str) -> Vec<String> {
    vec![
        "sqlx".to_owned(),
        "migrate".to_owned(),
        "run".to_owned(),
        "--source".to_owned(),
        source.display().to_string(),
        "--database-url".to_owned(),
        database_url.to_owned(),
        "--no-dotenv".to_owned(),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_find_workspace_root() {
        let root = find_root().expect("should find workspace root");
        assert!(root.join("Cargo.toml").is_file());
    }

    #[test]
    fn test_parse_database_url_from_env_str() {
        let env_content = r#"
# Comments
POSTGRES_HOST_PORT=5433
DATABASE_URL="postgres://postgres:postgres@127.0.0.1:5433/deepref"
API_BIND_ADDR=127.0.0.1:8080
"#;
        assert_eq!(
            parse_database_url_from_env_str(env_content),
            Some("postgres://postgres:postgres@127.0.0.1:5433/deepref".to_owned())
        );

        let unquoted = "DATABASE_URL=postgres://user:pass@localhost:5432/db";
        assert_eq!(
            parse_database_url_from_env_str(unquoted),
            Some("postgres://user:pass@localhost:5432/db".to_owned())
        );

        let empty = "# only comments\nVAR=123\n";
        assert_eq!(parse_database_url_from_env_str(empty), None);
    }

    #[test]
    fn migration_arguments_run_the_shared_migration_set_without_project_compilation() {
        let args = migration_arguments(
            Path::new("crates/postgres/migrations"),
            "postgres://localhost/deepref",
        );
        assert_eq!(
            args,
            vec![
                "sqlx",
                "migrate",
                "run",
                "--source",
                "crates/postgres/migrations",
                "--database-url",
                "postgres://localhost/deepref",
                "--no-dotenv",
            ]
        );
    }
}
