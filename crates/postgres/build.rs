use std::{
    env,
    error::Error,
    fmt::Write,
    fs,
    path::{Path, PathBuf},
};

use sha2::{Digest, Sha256};

fn main() -> Result<(), Box<dyn Error>> {
    let manifest_dir =
        PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").ok_or("CARGO_MANIFEST_DIR must be set")?);
    let workspace = manifest_dir
        .parent()
        .and_then(Path::parent)
        .ok_or("postgres crate must be inside the workspace")?;
    let roots = [
        workspace.join("review-definitions"),
        workspace.join("crates"),
        workspace.join("services"),
    ];
    let mut files = Vec::new();
    for root in roots {
        collect_files(&root, &mut files)?;
    }
    files.extend([workspace.join("Cargo.lock"), workspace.join("Cargo.toml")]);
    files.sort();
    files.dedup();

    let mut digest = Sha256::new();
    for path in files {
        let relative = path.strip_prefix(workspace).map_err(|_| {
            format!(
                "semantic source {} must remain inside workspace",
                path.display()
            )
        })?;
        println!("cargo:rerun-if-changed={}", path.display());
        digest.update(relative.as_os_str().as_encoded_bytes());
        digest.update([0]);
        digest.update(
            fs::read(&path)
                .map_err(|error| format!("failed to hash {}: {error}", path.display()))?,
        );
        digest.update([0]);
    }
    if let Some(build_id) = env::var_os("DEEPREF_BUILD_SHA") {
        println!("cargo:rerun-if-env-changed=DEEPREF_BUILD_SHA");
        digest.update(b"build-id\0");
        digest.update(build_id.as_encoded_bytes());
    }
    let mut encoded = String::with_capacity(64);
    for byte in digest.finalize() {
        write!(&mut encoded, "{byte:02x}")
            .map_err(|error| format!("writing to a string cannot fail: {error}"))?;
    }
    println!("cargo:rustc-env=DEEPREF_SEMANTIC_BUILD_SHA={encoded}");
    Ok(())
}

fn collect_files(root: &Path, files: &mut Vec<PathBuf>) -> Result<(), Box<dyn Error>> {
    let entries = fs::read_dir(root)
        .map_err(|error| format!("failed to scan {}: {error}", root.display()))?;
    for entry in entries {
        let entry =
            entry.map_err(|error| format!("failed to read {} entry: {error}", root.display()))?;
        let path = entry.path();
        if path.is_dir() {
            collect_files(&path, files)?;
        } else if path.is_file() {
            files.push(path);
        }
    }
    Ok(())
}
