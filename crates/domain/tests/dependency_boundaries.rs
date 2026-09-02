use std::path::{Path, PathBuf};

const FORBIDDEN_INFRASTRUCTURE_DEPENDENCIES: &[&str] = &[
    "deepref-postgres",
    "deepref-http-api",
    "deepref-documents",
    "deepref-providers",
    "deepref-crossref",
    "deepref-telemetry",
    "sqlx",
    "axum",
    "reqwest",
    "object_store",
    "pdfium-render",
    "rig-core",
    "opentelemetry",
    "opentelemetry-otlp",
    "opentelemetry_sdk",
    "tracing-opentelemetry",
];

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("domain crate must live under <workspace>/crates/domain")
        .to_path_buf()
}

fn manifest(relative_path: &str) -> String {
    let path = workspace_root().join(relative_path);
    std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("failed to read {}: {error}", path.display()))
}

fn declares_dependency(manifest: &str, dependency: &str) -> bool {
    manifest.lines().any(|raw_line| {
        let line = raw_line.split('#').next().unwrap_or_default().trim();
        if line.is_empty() {
            return false;
        }
        if line.starts_with('[') {
            return line.contains("dependencies.")
                && line
                    .trim_end_matches(']')
                    .rsplit('.')
                    .next()
                    .is_some_and(|name| name == dependency);
        }
        line.strip_prefix(dependency).is_some_and(|rest| {
            rest.starts_with('.') || rest.trim_start().starts_with('=')
        })
    })
}

fn assert_no_infrastructure_dependencies(crate_name: &str, relative_manifest: &str) {
    let manifest = manifest(relative_manifest);
    for dependency in FORBIDDEN_INFRASTRUCTURE_DEPENDENCIES {
        assert!(
            !declares_dependency(&manifest, dependency),
            "{crate_name} must not depend directly on infrastructure dependency {dependency}"
        );
    }
}

#[test]
fn domain_has_no_infrastructure_dependencies() {
    assert_no_infrastructure_dependencies("deepref-domain", "crates/domain/Cargo.toml");
}

#[test]
fn application_has_no_infrastructure_dependencies() {
    assert_no_infrastructure_dependencies("deepref-application", "crates/application/Cargo.toml");
}
