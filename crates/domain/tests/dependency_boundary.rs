use std::{fs, path::PathBuf};

#[test]
fn domain_dependencies_remain_pure() {
    let manifest_path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml");
    let manifest = fs::read_to_string(&manifest_path)
        .unwrap_or_else(|error| panic!("failed to read {}: {error}", manifest_path.display()));

    let allowed = ["serde", "thiserror", "uuid"];
    let mut section = "";
    for raw_line in manifest.lines() {
        let line = raw_line.trim();
        if line.starts_with('[') && line.ends_with(']') {
            section = line;
            if section.starts_with("[dependencies.") {
                panic!("domain must not declare dependency tables: {section}");
            }
            continue;
        }
        if section != "[dependencies]" || line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((name, _)) = line.split_once('=') else {
            continue;
        };
        let name = name.trim().split('.').next().unwrap_or(name.trim());
        assert!(
            allowed.contains(&name),
            "deepref-domain gained a non-domain dependency {name:?}"
        );
    }
}
