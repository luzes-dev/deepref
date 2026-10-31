//! `cargo xtask alerts check`: every metric referenced by the alerting
//! configuration in `operations/alerting` must be an instrument the code
//! actually emits, under the standard OTLP -> Prometheus name translation.
//! Rules must also name a catalog row and an existing runbook.
// Every slice boundary comes from `find` of an ASCII needle or an ASCII byte
// scan, so it always lands on a character boundary.
#![allow(clippy::string_slice)]
use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
};

use anyhow::{Context, Result, bail};

use crate::workspace::find_root;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Counter,
    Gauge,
    Histogram,
}

/// Instruments declared as `meter.<type>_<kind>("name")` with an optional
/// `.with_unit("unit")` before `.build()`.
fn instruments(source: &str) -> Vec<(String, Kind, Option<String>)> {
    let mut found = Vec::new();
    for (marker, kind) in [
        ("_counter(\"", Kind::Counter),
        ("_gauge(\"", Kind::Gauge),
        ("_histogram(\"", Kind::Histogram),
    ] {
        let mut rest = source;
        while let Some(start) = rest.find(marker) {
            let after = &rest[start + marker.len()..];
            let Some(end) = after.find('"') else { break };
            let name = &after[..end];
            let tail = &after[end..];
            let chain = &tail[..tail.find(".build()").unwrap_or(0)];
            let unit = chain.find(".with_unit(\"").map(|at| {
                let unit = &chain[at + ".with_unit(\"".len()..];
                unit[..unit.find('"').unwrap_or(0)].to_owned()
            });
            if name.starts_with("deepref.") {
                found.push((name.to_owned(), kind, unit));
            }
            rest = after;
        }
    }
    found
}

/// Prometheus names a backend exposes for one instrument.
fn prometheus_names(name: &str, kind: Kind, unit: Option<&str>) -> Vec<String> {
    let mut base = name.replace('.', "_");
    match unit {
        Some("s") => base.push_str("_seconds"),
        Some("By") => base.push_str("_bytes"),
        _ => {}
    }
    match kind {
        Kind::Counter => vec![format!("{base}_total")],
        Kind::Gauge => vec![base],
        Kind::Histogram => ["_bucket", "_sum", "_count"]
            .iter()
            .map(|suffix| format!("{base}{suffix}"))
            .collect(),
    }
}

fn referenced_metrics(text: &str) -> BTreeSet<String> {
    let mut names = BTreeSet::new();
    let bytes = text.as_bytes();
    let mut index = 0;
    while let Some(offset) = text[index..].find("deepref_") {
        let start = index + offset;
        let boundary = start == 0 || {
            let previous = bytes[start - 1];
            !(previous.is_ascii_alphanumeric() || previous == b'_' || previous == b'-')
        };
        let mut end = start;
        while end < bytes.len()
            && (bytes[end].is_ascii_lowercase()
                || bytes[end].is_ascii_digit()
                || bytes[end] == b'_')
        {
            end += 1;
        }
        if boundary {
            names.insert(text[start..end].to_owned());
        }
        index = end.max(start + 1);
    }
    names
}

fn collect_rust(dir: &Path, out: &mut String) -> Result<()> {
    for entry in std::fs::read_dir(dir).with_context(|| dir.display().to_string())? {
        let path = entry?.path();
        if path.is_dir() {
            if path
                .file_name()
                .is_some_and(|name| name == "tests" || name == "target")
            {
                continue;
            }
            collect_rust(&path, out)?;
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            out.push_str(&std::fs::read_to_string(&path)?);
            out.push('\n');
        }
    }
    Ok(())
}

/// Problems in `files` (name, contents) against the emitted `source`.
fn check(
    source: &str,
    files: &[(String, String)],
    catalog_ids: &BTreeSet<String>,
    root: &Path,
) -> Vec<String> {
    let emitted: BTreeMap<String, String> = instruments(source)
        .into_iter()
        .flat_map(|(name, kind, unit)| {
            prometheus_names(&name, kind, unit.as_deref())
                .into_iter()
                .map(move |prom| (prom, name.clone()))
        })
        .collect();
    let mut problems = Vec::new();
    for (file, text) in files {
        for metric in referenced_metrics(text) {
            if !emitted.contains_key(&metric) {
                problems.push(format!(
                    "{file}: `{metric}` is not emitted by any instrument"
                ));
            }
        }
        for line in text.lines().map(str::trim) {
            if let Some(id) = line.split("catalog_id: ").nth(1) {
                let id = id.trim_end_matches([' ', '}']).trim();
                if !catalog_ids.contains(id) {
                    problems.push(format!(
                        "{file}: catalog_id `{id}` is not in the metric catalog"
                    ));
                }
            }
            if let Some(runbook) = line.strip_prefix("runbook: ")
                && !root.join(runbook.trim()).is_file()
            {
                problems.push(format!("{file}: runbook `{runbook}` does not exist"));
            }
        }
    }
    problems
}

pub fn run() -> Result<()> {
    let root = find_root()?;
    let mut source = String::new();
    for dir in ["crates", "services", "apps/server"] {
        collect_rust(&root.join(dir), &mut source)?;
    }
    let catalog: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(
        root.join("operations/metrics/catalog.json"),
    )?)?;
    let catalog_ids = catalog["rules"]
        .as_array()
        .context("catalog rules")?
        .iter()
        .filter_map(|rule| rule["id"].as_str().map(str::to_owned))
        .collect();
    let mut files = Vec::new();
    for entry in std::fs::read_dir(root.join("operations/alerting"))? {
        let path = entry?.path();
        if path
            .extension()
            .is_some_and(|ext| ext == "yml" || ext == "json")
        {
            files.push((
                path.strip_prefix(&root)?.display().to_string(),
                std::fs::read_to_string(&path)?,
            ));
        }
    }
    if files.is_empty() {
        bail!("no alerting configuration found in operations/alerting");
    }
    let problems = check(&source, &files, &catalog_ids, &root);
    if !problems.is_empty() {
        bail!(
            "alerting configuration check failed:\n  {}",
            problems.join("\n  ")
        );
    }
    println!(
        "alerting configuration references only emitted metrics ({} files)",
        files.len()
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const SOURCE: &str = r#"
        meter.u64_counter("deepref.http.requests").with_unit("{request}").build(),
        meter.f64_histogram("deepref.http.response_header.duration").with_unit("s").build(),
        meter.u64_gauge("deepref.worker.live").build(),
        meter.f64_gauge("deepref.backlog.oldest.age").with_unit("s").build(),
    "#;

    fn problems(rules: &str) -> Vec<String> {
        let ids = BTreeSet::from(["queue-age".to_owned()]);
        check(
            SOURCE,
            &[("rules.yml".into(), rules.into())],
            &ids,
            Path::new("/nonexistent"),
        )
    }

    #[test]
    fn emitted_names_follow_the_prometheus_translation() {
        assert!(problems(
            "rate(deepref_http_requests_total[5m]) deepref_http_response_header_duration_seconds_bucket \
             max(deepref_worker_live) deepref_backlog_oldest_age_seconds"
        )
        .is_empty());
    }

    #[test]
    fn a_misspelled_or_unemitted_metric_fails() {
        let found =
            problems("max(deepref_worker_alive) deepref_http_requests deepref_backlog_oldest_age");
        assert_eq!(found.len(), 3, "{found:?}");
    }

    #[test]
    fn unknown_catalog_ids_and_missing_runbooks_fail() {
        let found =
            problems("catalog_id: nope }\nrunbook: docs/missing.md\ncatalog_id: queue-age }");
        assert_eq!(found.len(), 2, "{found:?}");
    }
}
