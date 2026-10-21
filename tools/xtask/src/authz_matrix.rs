//! The authorization coverage matrix gate (handoff 03b).
//!
//! `crates/http-api/src/authz/policy.rs` is the matrix. The `authz_matrix`
//! binary compares it with the router, with `docs/openapi.json` and with the
//! rendered `docs/authorization-matrix.md`; this command runs that check, or
//! rewrites the rendered file.

use std::fs;
use std::process::Command;

use anyhow::{Context, Result, bail};

use crate::workspace::{cargo_bin, find_root};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MatrixMode {
    Check,
    Write,
}

pub fn run(mode: MatrixMode) -> Result<()> {
    let root = find_root()?;
    let sqlx_offline = std::env::var("SQLX_OFFLINE").unwrap_or_else(|_| "true".into());
    let mut command = Command::new(cargo_bin());
    command
        .args([
            "run",
            "-q",
            "-p",
            "deepref-http-api",
            "--bin",
            "authz_matrix",
        ])
        .current_dir(&root)
        .env("SQLX_OFFLINE", &sqlx_offline);
    match mode {
        MatrixMode::Check => {
            let status = command
                .args(["--", "--check"])
                .arg(&root)
                .status()
                .context("failed to invoke the authz_matrix binary")?;
            if !status.success() {
                bail!(
                    "authorization matrix check failed: every route needs exactly one row in \
                     crates/http-api/src/authz/policy.rs, and docs/authorization-matrix.md must be \
                     regenerated with 'cargo xtask authz-matrix --write'"
                );
            }
            println!("Authorization matrix covers every route.");
        }
        MatrixMode::Write => {
            let output = command
                .output()
                .context("failed to invoke the authz_matrix binary")?;
            if !output.status.success() {
                bail!(
                    "failed to render the matrix: {}",
                    String::from_utf8_lossy(&output.stderr)
                );
            }
            let path = root.join("docs/authorization-matrix.md");
            fs::write(&path, &output.stdout)
                .with_context(|| format!("failed to write {}", path.display()))?;
            println!("Updated {}", path.display());
        }
    }
    Ok(())
}
