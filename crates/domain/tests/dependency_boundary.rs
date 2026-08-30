use std::{collections::BTreeSet, fs, path::Path};

fn dependencies(manifest: &Path) -> BTreeSet<String> {
    let source = fs::read_to_string(manifest).expect("crate manifest must be readable");
    let mut section = "";
    let mut names = BTreeSet::new();

    for line in source.lines() {
        let line = line.split('#').next().unwrap_or_default().trim();
        if line.starts_with('[') && line.ends_with(']') {
            section = line.trim_matches(['[', ']'].as_ref());
            assert!(
                !section.starts_with("dependencies."),
                "manifest dependency tables must remain explicit direct dependencies"
            );
            continue;
        }
        if section != "dependencies" || line.is_empty() {
            continue;
        }
        if let Some((name, _)) = line.split_once('=') {
            names.insert(name.trim().trim_end_matches(".workspace").to_owned());
        }
    }

    names
}

fn manifest(crate_name: &str) -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join(crate_name)
        .join("Cargo.toml")
}

#[test]
fn domain_dependencies_remain_pure() {
    assert_eq!(
        dependencies(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("Cargo.toml")
                .as_path()
        ),
        [
            "serde",
            "thiserror",
            "unicode-normalization",
            "unicode_categories",
            "uuid",
        ]
        .into_iter()
        .map(str::to_owned)
        .collect()
    );
}

#[test]
fn application_dependencies_are_domain_and_pure_data_or_error_crates() {
    let actual = dependencies(manifest("application").as_path());
    let allowed = [
        "deepref-domain",
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

    assert!(actual.contains("deepref-domain"));
    for dependency in actual {
        assert!(
            allowed.contains(&dependency.as_str()),
            "application dependency {dependency:?} is not a pure data/error dependency"
        );
    }
}

#[test]
fn infrastructure_and_adapter_dependencies_cannot_point_outward() {
    let domain = dependencies(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("Cargo.toml")
            .as_path(),
    );
    let application = dependencies(manifest("application").as_path());
    let review = dependencies(manifest("review").as_path());
    let postgres = dependencies(manifest("postgres").as_path());
    let http_api = dependencies(manifest("http-api").as_path());
    let providers = dependencies(manifest("providers").as_path());

    assert!(http_api.contains("deepref-application"));
    assert!(http_api.contains("deepref-postgres"));
    assert!(review.contains("deepref-domain"));
    assert!(review.contains("deepref-application"));
    assert!(review.contains("deepref-ai"));
    assert!(postgres.contains("sqlx"));

    for dependency in [
        "deepref-application",
        "deepref-postgres",
        "deepref-http-api",
    ] {
        assert!(
            !domain.contains(dependency),
            "domain must not depend on adapter/application crate {dependency}"
        );
    }
    for dependency in ["deepref-postgres", "deepref-http-api"] {
        assert!(
            !application.contains(dependency),
            "application must not depend on adapter crate {dependency}"
        );
    }
    assert!(!postgres.contains("deepref-http-api"));
    assert!(!review.contains("deepref-postgres"));
    assert!(!review.contains("deepref-http-api"));
    assert!(providers.contains("deepref-application"));

    for dependency in [
        "axum",
        "sqlx",
        "reqwest",
        "async-nats",
        "neo4rs",
        "object_store",
        "object-store",
        "aws-sdk-s3",
        "s3",
    ] {
        assert!(
            !application.contains(dependency),
            "application must not depend on infrastructure crate {dependency}"
        );
        assert!(
            !review.contains(dependency),
            "review must not depend on infrastructure crate {dependency}"
        );
    }
    for dependency in ["biblio", "biblatex", "csv", "reqwest", "sqlx"] {
        assert!(
            !application.contains(dependency),
            "application must not depend on provider/parser crate {dependency}"
        );
    }
}
