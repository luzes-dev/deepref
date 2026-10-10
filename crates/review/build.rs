//! Emits build provenance for [`BuildProvenance`][crate::BuildProvenance].
//!
//! * `DEEPREF_PROVENANCE_IMPLEMENTATION_SHA`: the audited implementation boundary.
//! * `DEEPREF_PROVENANCE_DEPENDENCY_SHA`: the closure of the audited third-party
//!   dependencies, with the manifest lines that declare them.
//! * `DEEPREF_RIG_VERSION`, `DEEPREF_SERDE_JSON_VERSION`: exact lockfile versions.
//!
//! These digests are forensic audit evidence ("what exact software produced
//! this run"). They never gate calibration compatibility: no production
//! calibration decision depends on repository path hashing.
//!
//! The logic, the boundary table and its rules live in
//! `build_support/fingerprint.rs`, which is also unit-tested under `cfg(test)`.
//! The audit listings are written to `OUT_DIR` for forensic inspection.

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
        "cargo:rustc-env=DEEPREF_PROVENANCE_IMPLEMENTATION_SHA={}",
        fingerprints.implementation_sha
    );
    println!(
        "cargo:rustc-env=DEEPREF_PROVENANCE_DEPENDENCY_SHA={}",
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

/// The exact `Cargo.lock` version(s) of one third-party crate, for provenance.
/// Several copies of a crate are joined with `+` rather than hidden: the same
/// semantic as `crates/ai/build.rs::lock_version`. Provenance must record
/// ambiguity, never break the build over it; a single resolved copy — the
/// current tree — emits the bare version, exactly as before.
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
