use std::{collections::BTreeMap, fmt, path::PathBuf, process::Command};

use anyhow::{Context, Result, bail};
use serde::Deserialize;

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
enum Layer {
    Domain,
    Application,
    Core,
    Graph,
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
            Adapter, Ai, Application, Composition, Configuration, Core, Domain, Graph, Http,
            Persistence, Review, Tooling, Worker,
        };
        match self {
            Domain | Configuration => false,
            Application | Core | Ai => target == Domain,
            Graph => matches!(target, Core | Domain),
            Review => matches!(target, Ai | Application | Domain),
            Adapter => matches!(
                target,
                Adapter | Application | Configuration | Core | Domain
            ),
            Persistence => matches!(
                target,
                Ai | Application | Adapter | Core | Domain | Graph | Review
            ),
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
    "futures",
    "jsonschema",
    "serde",
    "serde_json",
    "schemars",
    "thiserror",
    "time",
    "rapidfuzz",
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
    let errors = validate(&metadata);
    if !errors.is_empty() {
        bail!("architecture violations:\n\n{}", errors.join("\n\n"));
    }
    println!(
        "Architecture validated: {} workspace members (all dependency kinds and targets).",
        metadata.workspace_members.len()
    );
    Ok(())
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
                if let Some(target_layer) = target.metadata.deepref.layer {
                    // Existing HTTP integration fixtures exercise the worker. This is not a runtime edge.
                    let fixture_edge = source.name == "deepref-http-api"
                        && target.name == "deepref-worker"
                        && dependency.kind.as_deref() == Some("dev");
                    if !layer.permits(target_layer) && !fixture_edge {
                        errors.push(format!("{} -> {}\nsource layer: {layer}\ntarget layer: {target_layer}\n{layer} -> {target_layer} is forbidden ({})",
                            source.name, target.name, dependency.kind.as_deref().unwrap_or("normal")));
                    }
                }
            } else {
                let normal = dependency.kind.is_none();
                let forbidden = dependency.name == "async-nats"
                    || (matches!(layer, Layer::Domain | Layer::Application | Layer::Review)
                        && INFRASTRUCTURE.contains(&dependency.name.as_str()))
                    || (normal
                        && layer == Layer::Domain
                        && !DOMAIN_EXTERNAL.contains(&dependency.name.as_str()))
                    || (normal
                        && layer == Layer::Application
                        && !APPLICATION_EXTERNAL.contains(&dependency.name.as_str()));
                if forbidden {
                    errors.push(format!("{} -> {}\nsource layer: {layer}\ntarget layer: external\ndependency violates the pure dependency contract or removed NATS guard", source.name, dependency.name));
                }
            }
        }
    }
    // Preserve the former contract's required ownership seams as well as forbidden edges.
    for (source_name, dependencies) in [
        ("deepref-application", &["deepref-domain"][..]),
        (
            "deepref-http-api",
            &["deepref-application", "deepref-postgres"][..],
        ),
        (
            "deepref-review",
            &["deepref-domain", "deepref-application", "deepref-ai"][..],
        ),
        ("deepref-postgres", &["sqlx"][..]),
        ("deepref-providers", &["deepref-application"][..]),
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
    errors.sort();
    errors
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};

    fn package(name: &str, layer: &str, dependencies: Value) -> Value {
        json!({"id": name, "name": name, "manifest_path": format!("/workspace/{name}/Cargo.toml"),
            "metadata": {"deepref": {"layer": layer}}, "dependencies": dependencies})
    }

    fn errors(packages: Vec<Value>) -> Vec<String> {
        let members: Vec<_> = packages.iter().map(|p| p["id"].clone()).collect();
        let metadata =
            serde_json::from_value(json!({"packages": packages, "workspace_members": members}))
                .unwrap();
        validate(&metadata)
    }

    #[test]
    fn rejects_unknown_member() {
        let mut unknown = package("new-crate", "domain", json!([]));
        unknown["metadata"] = json!({});
        assert!(errors(vec![unknown])[0].contains("missing"));
    }

    #[test]
    fn checks_renamed_optional_target_and_build_dependencies() {
        let source = package(
            "deepref-domain",
            "domain",
            json!([{
                "name": "deepref-postgres", "rename": "innocent", "path": "/workspace/deepref-postgres",
                "optional": true, "target": "cfg(windows)", "kind": "build"
            }]),
        );
        let actual = errors(vec![
            source,
            package("deepref-postgres", "persistence", json!([])),
        ]);
        assert!(actual[0].contains("deepref-domain -> deepref-postgres"));
        assert!(actual[0].contains("(build)"));
    }

    #[test]
    fn production_cannot_depend_on_tooling_even_for_tests() {
        for kind in [Value::Null, json!("dev"), json!("build")] {
            assert!(
                !errors(vec![
                    package(
                        "server",
                        "composition",
                        json!([{
                            "name": "xtask", "path": "/workspace/xtask", "kind": kind
                        }])
                    ),
                    package("xtask", "tooling", json!([]))
                ])
                .is_empty()
            );
        }
    }

    #[test]
    fn worker_fixture_exception_is_dev_only() {
        for (kind, allowed) in [
            (json!("dev"), true),
            (Value::Null, false),
            (json!("build"), false),
        ] {
            assert_eq!(errors(vec![package("deepref-http-api", "http", json!([{
                "name": "deepref-worker", "path": "/workspace/deepref-worker", "kind": kind
            }, {"name": "deepref-application", "kind": null}, {"name": "deepref-postgres", "kind": null}])), package("deepref-worker", "worker", json!([]))]).is_empty(), allowed);
        }
    }

    #[test]
    fn retains_external_guards_including_renames() {
        for name in ["async-nats", "sqlx", "reqwest", "biblatex"] {
            assert!(
                !errors(vec![package(
                    "application",
                    "application",
                    json!([{
                        "name": name, "rename": "alias", "kind": null
                    }])
                )])
                .is_empty()
            );
        }
    }

    #[test]
    fn inward_edges_are_allowed_but_persistence_cannot_orchestrate_workers() {
        assert!(Layer::Application.permits(Layer::Domain));
        assert!(Layer::Review.permits(Layer::Ai));
        assert!(Layer::Http.permits(Layer::Persistence));
        assert!(!Layer::Persistence.permits(Layer::Worker));
        assert!(!Layer::Review.permits(Layer::Persistence));
        assert!(!Layer::Application.permits(Layer::Adapter));
    }
}
