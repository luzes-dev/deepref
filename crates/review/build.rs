//! Emits the two semantic fingerprints that identify screening calibration.
//!
//! * `DEEPREF_SEMANTIC_IMPLEMENTATION_SHA`: the narrow implementation boundary.
//! * `DEEPREF_SEMANTIC_DEPENDENCY_SHA`: the closure of the semantic third-party
//!   dependencies, with the manifest lines that declare them.
//!
//! It also emits `DEEPREF_RIG_VERSION` and `DEEPREF_SERDE_JSON_VERSION`, the
//! exact lockfile versions recorded in [`BuildProvenance`][crate::BuildProvenance]
//! for audit. Neither version gates calibration.
//!
//! The logic, the boundary table and its rules live in
//! `build_support/fingerprint.rs`, which is also unit-tested under `cfg(test)`.
//! The audit listings are written to `OUT_DIR` and the dependency listing is
//! exposed as `SEMANTIC_DEPENDENCIES`.

use std::{
    env,
    error::Error,
    fs,
    path::{Path, PathBuf},
};

#[path = "build_support/fingerprint.rs"]
mod fingerprint;

fn main() -> Result<(), Box<dyn Error>> {
    let manifest_dir =
        PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").ok_or("CARGO_MANIFEST_DIR must be set")?);
    let workspace = manifest_dir
        .parent()
        .and_then(Path::parent)
        .ok_or("deepref-review must live at crates/review inside the workspace")?;
    let out_dir = PathBuf::from(env::var_os("OUT_DIR").ok_or("OUT_DIR must be set")?);

    let fingerprints = fingerprint::compute(workspace)?;
    for path in &fingerprints.watched {
        println!("cargo:rerun-if-changed={}", workspace.join(path).display());
    }
    fs::write(
        out_dir.join("semantic_implementation.txt"),
        &fingerprints.implementation_report,
    )?;
    fs::write(
        out_dir.join("semantic_dependencies.txt"),
        &fingerprints.dependency_report,
    )?;
    println!(
        "cargo:rustc-env=DEEPREF_SEMANTIC_IMPLEMENTATION_SHA={}",
        fingerprints.implementation_sha
    );
    println!(
        "cargo:rustc-env=DEEPREF_SEMANTIC_DEPENDENCY_SHA={}",
        fingerprints.dependency_sha
    );
    println!(
        "cargo:rustc-env=DEEPREF_RIG_VERSION={}",
        lockfile_version(workspace, "rig-core")?
    );
    println!(
        "cargo:rustc-env=DEEPREF_SERDE_JSON_VERSION={}",
        lockfile_version(workspace, "serde_json")?
    );
    Ok(())
}

/// The exact `Cargo.lock` version of one third-party crate, for provenance.
/// Several copies of a crate would be joined rather than hidden.
fn lockfile_version(workspace: &Path, name: &str) -> Result<String, Box<dyn Error>> {
    let text = fs::read_to_string(workspace.join("Cargo.lock"))?;
    let lock = fingerprint::parse_lockfile(&text)?;
    let mut versions = lock
        .packages
        .iter()
        .filter(|package| package.name == name)
        .map(|package| package.version.as_str())
        .collect::<Vec<_>>();
    versions.sort();
    versions.dedup();
    if versions.is_empty() {
        return Err(format!("Cargo.lock has no package `{name}`").into());
    }
    Ok(versions.join("+"))
}
