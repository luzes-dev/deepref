use std::fs;
use std::process::Command;

use anyhow::{Context, Result, bail};

use crate::workspace::{cargo_bin, find_root};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GenerateMode {
    Write,
    Check,
}

pub fn run(mode: GenerateMode) -> Result<()> {
    let root = find_root()?;

    match mode {
        GenerateMode::Write => {
            println!("Generating OpenAPI schema from deepref-server...");
            let sqlx_offline = std::env::var("SQLX_OFFLINE").unwrap_or_else(|_| "true".into());
            let openapi_output = Command::new(cargo_bin())
                .args(["run", "-q", "-p", "deepref-server", "--", "--print-openapi"])
                .current_dir(&root)
                .env("SQLX_OFFLINE", &sqlx_offline)
                .output()
                .context("failed to invoke 'cargo run -p deepref-server -- --print-openapi'")?;

            if !openapi_output.status.success() {
                let stderr = String::from_utf8_lossy(&openapi_output.stderr);
                bail!("failed to generate OpenAPI document: {stderr}");
            }

            let openapi_path = root.join("docs/openapi.json");
            fs::write(&openapi_path, &openapi_output.stdout)
                .with_context(|| format!("failed to write {}", openapi_path.display()))?;
            println!("Updated {}", openapi_path.display());

            println!("Generating web API client with Orval...");
            let pnpm_status = Command::new("pnpm")
                .args(["--filter", "@deepref/web", "generate:client"])
                .current_dir(&root)
                .status()
                .context("failed to invoke 'pnpm --filter @deepref/web generate:client'")?;

            if !pnpm_status.success() {
                bail!(
                    "'pnpm --filter @deepref/web generate:client' failed with exit code: {:?}",
                    pnpm_status.code()
                );
            }

            println!("Generated contracts updated successfully.");
        }
        GenerateMode::Check => {
            println!("Checking generated API contracts against committed state...");
            let script = root.join("scripts/check-api-codegen.sh");
            let status = Command::new("bash")
                .arg(&script)
                .current_dir(&root)
                .env(
                    "SQLX_OFFLINE",
                    std::env::var("SQLX_OFFLINE").unwrap_or_else(|_| "true".into()),
                )
                .status()
                .with_context(|| format!("failed to invoke {}", script.display()))?;

            if !status.success() {
                bail!(
                    "Generated contracts check failed: committed outputs are stale or out of sync.\n\
                     To fix: run 'cargo xtask generate' and commit the updated contracts."
                );
            }

            println!("Committed generated contracts are current.");
        }
    }

    Ok(())
}
