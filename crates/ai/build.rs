//! Emits the exact Rig versions this crate builds against, for cassette
//! provenance (`DEEPREF_RIG_VERSIONS`, e.g. `rig-agent 0.44.0, rig-core
//! 0.44.0, rig-cassette 0.44.0`).
//!
//! Lockfile semantic (shared with `crates/review/build.rs`): several
//! resolved copies of one crate are joined with `+` rather than hidden or
//! fatal. Provenance must record ambiguity, never break the build over it;
//! a single resolved copy — the current tree — emits the bare version,
//! exactly as before.

use std::{env, error::Error, fs, path::PathBuf};

fn main() -> Result<(), Box<dyn Error>> {
    let manifest_dir =
        PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").ok_or("CARGO_MANIFEST_DIR must be set")?);
    let workspace = manifest_dir
        .parent()
        .and_then(std::path::Path::parent)
        .ok_or("deepref-ai must live at crates/ai inside the workspace")?;
    println!(
        "cargo:rerun-if-changed={}",
        workspace.join("Cargo.lock").display()
    );
    let lock = fs::read_to_string(workspace.join("Cargo.lock"))?;
    let mut versions = Vec::new();
    for name in ["rig-agent", "rig-core", "rig-cassette"] {
        versions.push(format!("{name} {}", lock_version(&lock, name)?));
    }
    println!(
        "cargo:rustc-env=DEEPREF_RIG_VERSIONS={}",
        versions.join(", ")
    );
    Ok(())
}

/// The exact `Cargo.lock` version(s) of one crate, for provenance.
/// Several resolved copies are joined with `+` rather than hidden: the same
/// semantic as `crates/review/build.rs::lockfile_version`.
fn lock_version(lock: &str, name: &str) -> Result<String, Box<dyn Error>> {
    let mut versions = Vec::new();
    let mut lines = lock.lines().peekable();
    while let Some(line) = lines.next() {
        if line.trim() == "[[package]]" {
            let mut package_name = None;
            let mut package_version = None;
            while let Some(field) = lines.peek() {
                let field = field.trim();
                if field == "[[package]]" || field.starts_with('[') || field.is_empty() {
                    break;
                }
                if let Some(value) = field.strip_prefix("name = ") {
                    package_name = Some(value.trim_matches('"').to_owned());
                } else if let Some(value) = field.strip_prefix("version = ") {
                    package_version = Some(value.trim_matches('"').to_owned());
                }
                lines.next();
            }
            if package_name.as_deref() == Some(name)
                && let Some(version) = package_version
            {
                versions.push(version);
            }
        }
    }
    versions.sort();
    versions.dedup();
    if versions.is_empty() {
        return Err(format!("Cargo.lock has no package `{name}`").into());
    }
    Ok(versions.join("+"))
}
