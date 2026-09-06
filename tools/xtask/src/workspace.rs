use std::ffi::OsString;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

pub fn find_root() -> Result<PathBuf> {
    let current = std::env::current_dir().context("failed to get current working directory")?;
    find_root_from(&current)
}

pub fn find_root_from(start: &Path) -> Result<PathBuf> {
    let mut current = start.to_path_buf();
    loop {
        let manifest = current.join("Cargo.toml");
        if manifest.is_file() {
            let content = std::fs::read_to_string(&manifest)
                .with_context(|| format!("failed to read {}", manifest.display()))?;
            if content.contains("[workspace]") {
                return Ok(current);
            }
        }
        if !current.pop() {
            bail!("could not locate workspace root containing Cargo.toml with [workspace]");
        }
    }
}

pub fn cargo_bin() -> OsString {
    std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into())
}
