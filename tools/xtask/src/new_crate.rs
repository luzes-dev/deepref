use std::fs;
use std::path::Path;
use std::process::Command;

use anyhow::{Context, Result, bail};

use crate::workspace::{cargo_bin, find_root};

const VALID_LAYERS: &[&str] = &[
    "domain",
    "application",
    "core",
    "graph",
    "ai",
    "review",
    "configuration",
    "adapter",
    "persistence",
    "http",
    "worker",
    "composition",
    "tooling",
];

pub fn run(layer: &str, raw_name: &str) -> Result<()> {
    let layer = layer.trim().to_lowercase();
    if !VALID_LAYERS.contains(&layer.as_str()) {
        bail!(
            "invalid architectural layer '{layer}'. Valid layers are:\n  {}",
            VALID_LAYERS.join(", ")
        );
    }

    validate_crate_name(raw_name)?;

    let (pkg_name, folder_name) = if let Some(stripped) = raw_name.strip_prefix("deepref-") {
        (raw_name.to_string(), stripped.to_string())
    } else {
        (format!("deepref-{raw_name}"), raw_name.to_string())
    };

    let root = find_root()?;
    let relative_path = if layer == "tooling" {
        format!("tools/{folder_name}")
    } else if layer == "worker" && folder_name == "worker" {
        "services/worker".to_string()
    } else {
        format!("crates/{folder_name}")
    };

    let crate_dir = root.join(&relative_path);
    if crate_dir.exists() {
        bail!("directory already exists at {}", crate_dir.display());
    }

    let src_dir = crate_dir.join("src");
    fs::create_dir_all(&src_dir)
        .with_context(|| format!("failed to create directory {}", src_dir.display()))?;

    let cargo_toml_content = format!(
        r#"[package]
name = "{pkg_name}"
version.workspace = true
edition.workspace = true
license.workspace = true

[dependencies]

[lints]
workspace = true

[package.metadata.deepref]
layer = "{layer}"
"#
    );

    let manifest_path = crate_dir.join("Cargo.toml");
    fs::write(&manifest_path, cargo_toml_content)
        .with_context(|| format!("failed to write {}", manifest_path.display()))?;

    let lib_rs_content = format!(
        r#"//! {pkg_name} ({layer} layer).

#![forbid(unsafe_code)]
"#
    );

    let lib_rs_path = src_dir.join("lib.rs");
    fs::write(&lib_rs_path, lib_rs_content)
        .with_context(|| format!("failed to write {}", lib_rs_path.display()))?;

    register_workspace_member(&root, &relative_path)?;

    let _ = Command::new(cargo_bin())
        .args(["fmt", "--all"])
        .current_dir(&root)
        .status();

    println!("Validating architectural boundaries for new crate...");
    crate::boundaries::run().context("new crate failed architecture boundary validation")?;

    println!(
        "Successfully created crate '{pkg_name}' at {} with layer '{layer}'.",
        relative_path
    );

    Ok(())
}

fn validate_crate_name(name: &str) -> Result<()> {
    if name.is_empty() {
        bail!("crate name cannot be empty");
    }
    let valid = name
        .chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_');
    if !valid || !name.starts_with(|c: char| c.is_ascii_alphanumeric()) {
        bail!(
            "invalid crate name '{name}'. Crate names must start with an alphanumeric character \
             and contain only lowercase ASCII letters, digits, hyphens, and underscores."
        );
    }
    Ok(())
}

fn register_workspace_member(root: &Path, relative_path: &str) -> Result<()> {
    let root_cargo_path = root.join("Cargo.toml");
    let content = fs::read_to_string(&root_cargo_path)
        .with_context(|| format!("failed to read {}", root_cargo_path.display()))?;

    let member_entry = format!("    \"{relative_path}\",\n");
    if content.contains(&format!("\"{relative_path}\"")) {
        return Ok(());
    }

    if let Some((before, after)) = content.split_once("members = [\n") {
        let mut new_content = String::with_capacity(content.len() + member_entry.len());
        new_content.push_str(before);
        new_content.push_str("members = [\n");
        new_content.push_str(&member_entry);
        new_content.push_str(after);

        fs::write(&root_cargo_path, new_content)
            .with_context(|| format!("failed to update {}", root_cargo_path.display()))?;
    } else {
        bail!("could not find 'members = [' in root Cargo.toml");
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_valid_crate_names() {
        assert!(validate_crate_name("test-crate").is_ok());
        assert!(validate_crate_name("deepref-analytics").is_ok());
        assert!(validate_crate_name("crate_123").is_ok());
        assert!(validate_crate_name("").is_err());
        assert!(validate_crate_name("-invalid").is_err());
        assert!(validate_crate_name("Invalid_Name").is_err());
    }
}
