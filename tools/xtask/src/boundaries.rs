use std::{
    collections::BTreeMap,
    fmt,
    path::{Path, PathBuf},
    process::Command,
};

use anyhow::{Context, Result, bail};
use serde::Deserialize;

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
enum Layer {
    Domain,
    Application,
    Ai,
    Review,
    Configuration,
    Adapter,
    Persistence,
    Http,
    Worker,
    Composition,
    Tooling,
}

impl fmt::Display for Layer {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{self:?}")
    }
}

impl Layer {
    fn permits(self, target: Self) -> bool {
        use Layer::{
            Adapter, Ai, Application, Composition, Configuration, Domain, Http, Persistence,
            Review, Tooling, Worker,
        };
        match self {
            Domain => false,
            Application | Ai | Configuration => target == Domain,
            Review => matches!(target, Ai | Application | Domain),
            Adapter => matches!(target, Adapter | Application | Configuration | Domain),
            Persistence => matches!(target, Ai | Application | Adapter | Domain | Review),
            Http | Worker => !matches!(target, Http | Worker | Composition | Tooling),
            Composition => !matches!(target, Composition | Tooling),
            Tooling => true,
        }
    }
}

#[derive(Deserialize)]
struct Metadata {
    packages: Vec<Package>,
    workspace_members: Vec<String>,
}

#[derive(Deserialize)]
struct Package {
    id: String,
    name: String,
    manifest_path: PathBuf,
    dependencies: Vec<Dependency>,
    #[serde(default)]
    metadata: PackageMetadata,
}

#[derive(Default, Deserialize)]
struct PackageMetadata {
    #[serde(default)]
    deepref: Classification,
}

#[derive(Default, Deserialize)]
struct Classification {
    layer: Option<Layer>,
}

#[derive(Deserialize)]
struct Dependency {
    name: String,
    path: Option<PathBuf>,
    kind: Option<String>,
}

const INFRASTRUCTURE: &[&str] = &[
    "sqlx",
    "axum",
    "reqwest",
    "async-nats",
    "neo4rs",
    "object_store",
    "object-store",
    "aws-sdk-s3",
    "s3",
    "pdfium-render",
    "rig-core",
    "opentelemetry",
    "opentelemetry-otlp",
    "opentelemetry_sdk",
    "tracing-opentelemetry",
    "biblio",
    "biblatex",
    "csv",
];
const DOMAIN_EXTERNAL: &[&str] = &[
    // Domain-owned ingestion, event envelopes and graph algorithms remain pure.
    "chrono",
    "petgraph",
    "serde_json",
    "sha2",
    "serde",
    "thiserror",
    "unicode-normalization",
    "unicode_categories",
    "uuid",
];
const APPLICATION_EXTERNAL: &[&str] = &[
    "anyhow",
    "bytes",
    "chrono",
    // Pure computation, no I/O: workflow schedules, webhook signing and
    // address checks.
    "chrono-tz",
    "futures",
    "hmac",
    "jsonschema",
    // Pure integer arithmetic for exact finite-population audit authorization.
    "num-bigint",
    "regex",
    "serde",
    "serde_json",
    "schemars",
    "sha2",
    "thiserror",
    "time",
    "rapidfuzz",
    "url",
    "uuid",
];

pub fn run() -> Result<()> {
    // Declarations include inactive optional dependencies and every target and dependency kind.
    // Unlike resolve.nodes, this cannot hide an illegal edge behind a disabled feature.
    let output = Command::new(std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into()))
        .args(["metadata", "--no-deps", "--format-version", "1", "--locked"])
        .output()
        .context("could not run cargo metadata")?;
    if !output.status.success() {
        bail!(
            "cargo metadata failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    let metadata: Metadata = serde_json::from_slice(&output.stdout)
        .context("invalid Cargo metadata or unknown Deepref layer")?;
    let mut errors = validate(&metadata);
    validate_screening_sources(&metadata, &mut errors)?;
    validate_runtime_configuration(&crate::workspace::find_root()?, &mut errors)?;
    if !errors.is_empty() {
        bail!("architecture violations:\n\n{}", errors.join("\n\n"));
    }
    println!(
        "Architecture validated: {} workspace members (all dependency kinds and targets).",
        metadata.workspace_members.len()
    );
    Ok(())
}

fn validate_screening_sources(metadata: &Metadata, errors: &mut Vec<String>) -> Result<()> {
    let Some(http) = metadata
        .packages
        .iter()
        .find(|package| package.name == "deepref-http-api")
    else {
        bail!("screening source rule requires deepref-http-api");
    };
    let root = http
        .manifest_path
        .parent()
        .context("HTTP manifest has no parent")?;
    for relative in ["src/routes/review.rs", "src/routes/assistant/plans.rs"] {
        let path = root.join(relative);
        let source = std::fs::read_to_string(&path)
            .with_context(|| format!("could not inspect {}", path.display()))?;
        // These migrated files contain transport/DTO logic only. Unmigrated
        // contexts and separate dev-test targets may continue to use SQLx.
        for forbidden in [
            "sqlx",
            "query!",
            "query_as!",
            "query_scalar!",
            "QueryBuilder",
        ] {
            if source.contains(forbidden) {
                errors.push(format!(
                    "{}: migrated screening transport must use deepref_app::screening, not {forbidden}",
                    path.display()
                ));
            }
        }
    }
    Ok(())
}

/// These runtime contexts have completed the typed configuration cutover.
/// Build scripts, test targets, and unmigrated persistence contexts are not
/// subject to a blanket source-level infrastructure prohibition.
fn validate_runtime_configuration(root: &Path, errors: &mut Vec<String>) -> Result<()> {
    for relative in [
        "apps/server/src",
        "services/worker/src",
        "crates/http-api/src",
        "crates/ai/src",
        "crates/documents/src",
        "crates/integrations/src",
        "crates/telemetry/src",
        "crates/app/src/workflows.rs",
    ] {
        inspect_runtime_source(&root.join(relative), root, errors)?;
    }
    Ok(())
}

fn inspect_runtime_source(path: &Path, root: &Path, errors: &mut Vec<String>) -> Result<()> {
    if path.is_dir() {
        for entry in std::fs::read_dir(path)
            .with_context(|| format!("could not inspect {}", path.display()))?
        {
            inspect_runtime_source(&entry?.path(), root, errors)?;
        }
    } else if path.extension().is_some_and(|extension| extension == "rs") {
        let source = std::fs::read_to_string(path)
            .with_context(|| format!("could not read {}", path.display()))?;
        // Unit fixture modules are separate from production runtime input.
        let production = source.split("#[cfg(test)]").next().unwrap_or(&source);
        if reads_runtime_environment(production) {
            errors.push(format!(
                "{} reads runtime environment directly; use typed deepref-config configuration injected at composition",
                path.strip_prefix(root).unwrap_or(path).display(),
            ));
        }
    }
    Ok(())
}

fn reads_runtime_environment(source: &str) -> bool {
    let compact: String = source
        .chars()
        .filter(|character| !character.is_whitespace())
        .collect();
    ["env::var(", "env::var_os(", "env::vars(", "env::vars_os("]
        .iter()
        .any(|pattern| compact.contains(pattern))
}
fn validate_workspace_dependency(
    source: &Package,
    layer: Layer,
    target: &Package,
    dependency: &Dependency,
    errors: &mut Vec<String>,
) {
    if let Some(target_layer) = target.metadata.deepref.layer {
        let fixture_edge = source.name == "deepref-http-api"
            && target.name == "deepref-worker"
            && dependency.kind.as_deref() == Some("dev");
        // A package dev-depending on itself only turns on its own test-only
        // features (e.g. deepref-app `test-support`); it crosses no layer.
        let own_test_features =
            source.name == target.name && dependency.kind.as_deref() == Some("dev");
        if !layer.permits(target_layer) && !fixture_edge && !own_test_features {
            errors.push(format!(
                "{} -> {}\nsource layer: {layer}\ntarget layer: {target_layer}\n{layer} -> {target_layer} is forbidden ({})",
                source.name,
                target.name,
                dependency.kind.as_deref().unwrap_or("normal")
            ));
        }
    }
}

fn is_forbidden_external_dependency(
    source: &str,
    layer: Layer,
    name: &str,
    is_normal: bool,
) -> bool {
    if name == "async-nats" {
        return source != "deepref-integrations" || layer != Layer::Adapter;
    }
    if matches!(layer, Layer::Domain | Layer::Application | Layer::Review)
        && INFRASTRUCTURE.contains(&name)
    {
        return true;
    }
    if is_normal && layer == Layer::Domain && !DOMAIN_EXTERNAL.contains(&name) {
        return true;
    }
    if is_normal && layer == Layer::Application && !APPLICATION_EXTERNAL.contains(&name) {
        return true;
    }
    false
}

fn validate_external_dependency(
    source: &Package,
    layer: Layer,
    dependency: &Dependency,
    errors: &mut Vec<String>,
) {
    let normal = dependency.kind.is_none();
    if is_forbidden_external_dependency(&source.name, layer, &dependency.name, normal) {
        errors.push(format!(
            "{} -> {}\nsource layer: {layer}\ntarget layer: external\ndependency violates the pure dependency contract or JetStream adapter boundary",
            source.name, dependency.name
        ));
    }
}

fn validate_required_seams(packages: &[&Package], errors: &mut Vec<String>) {
    for (source_name, dependencies) in [
        ("deepref-application", &["deepref-domain"][..]),
        (
            "deepref-http-api",
            &["deepref-application", "deepref-app"][..],
        ),
        (
            "deepref-review",
            &["deepref-domain", "deepref-application", "deepref-ai"][..],
        ),
        ("deepref-app", &["sqlx"][..]),
        ("deepref-integrations", &["deepref-application"][..]),
    ] {
        if let Some(source) = packages.iter().find(|p| p.name == source_name) {
            for dependency in dependencies {
                if !source
                    .dependencies
                    .iter()
                    .any(|d| d.name == *dependency && d.kind.is_none())
                {
                    errors.push(format!(
                        "{source_name}: missing required normal dependency {dependency}"
                    ));
                }
            }
        }
    }
}

fn validate(metadata: &Metadata) -> Vec<String> {
    let packages: Vec<_> = metadata
        .packages
        .iter()
        .filter(|p| metadata.workspace_members.contains(&p.id))
        .collect();
    let by_path: BTreeMap<_, _> = packages
        .iter()
        .filter_map(|p| p.manifest_path.parent().map(|path| (path, *p)))
        .collect();
    let mut errors = Vec::new();
    for source in &packages {
        let Some(layer) = source.metadata.deepref.layer else {
            errors.push(format!(
                "{}: missing [package.metadata.deepref] layer; classify every workspace member",
                source.name
            ));
            continue;
        };
        for dependency in &source.dependencies {
            if let Some(target) = dependency
                .path
                .as_deref()
                .and_then(|path| by_path.get(path))
            {
                validate_workspace_dependency(source, layer, target, dependency, &mut errors);
            } else {
                validate_external_dependency(source, layer, dependency, &mut errors);
            }
        }
    }
    validate_required_seams(&packages, &mut errors);
    errors.sort();
    errors
}

#[cfg(test)]
mod tests {
    use super::{Layer, is_forbidden_external_dependency, reads_runtime_environment};

    #[test]
    fn nats_sdk_is_confined_to_the_transport_adapter_crate() {
        assert!(!is_forbidden_external_dependency(
            "deepref-integrations",
            Layer::Adapter,
            "async-nats",
            true
        ));
        for (name, layer) in [
            ("deepref-domain", Layer::Domain),
            ("deepref-application", Layer::Application),
            ("deepref-app", Layer::Persistence),
            ("deepref-worker", Layer::Worker),
            ("deepref-documents", Layer::Adapter),
        ] {
            assert!(
                is_forbidden_external_dependency(name, layer, "async-nats", true),
                "{name}"
            );
        }
    }

    #[test]
    fn migrated_runtime_config_guard_rejects_direct_environment_reads() {
        for source in [
            "std::env::var(\"SMTP_HOST\")",
            "env :: var_os (\"PDFIUM_LIBRARY_PATH\")",
            "std::env::vars().collect()",
            "env::vars_os()",
        ] {
            assert!(reads_runtime_environment(source));
        }
    }

    #[test]
    fn runtime_config_guard_leaves_process_and_fixture_inputs_distinct() {
        assert!(!reads_runtime_environment("std::env::args().skip(1)"));
        assert!(!reads_runtime_environment("std::env::temp_dir()"));
        assert!(!reads_runtime_environment(
            "RuntimeConfig::from_source(get)?"
        ));
    }
}
