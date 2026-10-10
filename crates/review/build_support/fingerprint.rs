//! Pure logic behind the two semantic fingerprints of `deepref-review`.
//!
//! `crates/review/build.rs` includes this file to emit
//! `DEEPREF_SEMANTIC_IMPLEMENTATION_SHA` and `DEEPREF_SEMANTIC_DEPENDENCY_SHA`.
//! `crates/review/src/lib.rs` includes the same file under `#[cfg(test)]`, so
//! the logic is unit-tested with the crate's own harness.
//!
//! Nothing here reads environment variables, the clock or the current
//! directory. Every input is addressed relative to the workspace root, so the
//! digests are a function of the workspace contents alone: identical contents
//! give identical digests on any machine, at any checkout path, whatever order
//! the filesystem lists directories in.
//!
//! # Implementation boundary
//!
//! `DEEPREF_SEMANTIC_IMPLEMENTATION_SHA` covers the checked-in assets and the
//! Rust code that decides what a screening request contains, how the answer is
//! read, and how a verdict is routed. [`IMPLEMENTATION_BOUNDARY`] is the list.
//! [`EXCLUDED_PATHS`] and [`OUTSIDE_BOUNDARY`] say what is left out and why.
//!
//! The boundary is conservative and screening-centred. It is the interim
//! boundary until the screening semantic engine lives behind its own crate.
//! Three rules keep it honest:
//!
//! * A required path that is missing fails the build, so the table cannot
//!   drift silently. An optional path is recorded as absent in the audit listing.
//! * Directories are walked recursively and watched, so a new file in a
//!   covered directory changes the digest. Files and directories listed as
//!   excluded are never hashed.
//! * Rust files have their `#[cfg(test)]` items removed before hashing, so
//!   adding a unit test does not invalidate calibration. The removal is
//!   conservative: when the item boundaries are not clear, the text is kept.
//!
//! Coupling the table cannot express, recorded for reviewers:
//!
//! * The subject-to-input mapping (`review_preparation.rs`) and the full-text
//!   retrieval SQL (`ai.rs`) decide which passages the model sees. The document
//!   text comes from `crates/documents`, which is outside the boundary. A parser
//!   change is therefore safe only when it bumps the document parser version
//!   that retrieval filters on (`active_parser_version`).
//! * `processor.rs` and `nodes.rs` are included whole, so unrelated edits in
//!   those files also change the digest. That over-invalidates, by design.
//!
//! # Dependency boundary
//!
//! `DEEPREF_SEMANTIC_DEPENDENCY_SHA` covers the third-party crates that the
//! boundary uses for semantics ([`SEMANTIC_DEPENDENCY_ROOTS`]), their transitive
//! closure from `Cargo.lock` with transport, runtime and macro crates pruned
//! ([`EXCLUDED_THIRD_PARTY`]), and the manifest lines that declare the roots.
//! The lockfile does not record features, and some features change behaviour
//! without adding a dependency (`serde_json/arbitrary_precision` is an
//! example), so the declarations are part of the digest as well.

use std::{
    collections::{BTreeMap, BTreeSet},
    error::Error,
    fmt::{self, Write as _},
    fs,
    path::Path,
};

use sha2::{Digest, Sha256};

/// Error raised while computing a fingerprint. Build scripts print it verbatim.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FingerprintError(String);

impl FingerprintError {
    fn new(message: impl Into<String>) -> Self {
        Self(message.into())
    }
}

impl fmt::Display for FingerprintError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl Error for FingerprintError {}

type Result<T, E = FingerprintError> = std::result::Result<T, E>;

/// Whether a boundary entry is one file or a directory walked recursively.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Kind {
    File,
    Directory,
}

/// What happens when a boundary path is absent from the workspace.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Presence {
    /// The build fails: the table names a path that must exist.
    Required,
    /// Recorded as absent in the audit listing. Its parent is watched, so that
    /// creating the path reruns the build script.
    Optional,
}

/// One entry of the semantic implementation boundary.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct BoundaryEntry {
    /// Workspace-relative path with `/` separators and no trailing slash.
    pub(crate) path: &'static str,
    pub(crate) kind: Kind,
    pub(crate) presence: Presence,
    /// Why the entry is in the boundary.
    pub(crate) reason: &'static str,
}

/// A path inside an included directory that is deliberately not hashed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Exclusion {
    pub(crate) path: &'static str,
    pub(crate) kind: Kind,
    /// Why the path is outside the boundary even though a parent is inside it.
    pub(crate) reason: &'static str,
}

/// The semantic implementation boundary. Each entry says why it is included.
pub(crate) const IMPLEMENTATION_BOUNDARY: &[BoundaryEntry] = &[
    BoundaryEntry {
        path: "review-definitions",
        kind: Kind::Directory,
        presence: Presence::Required,
        reason: "checked-in prompts, output schemas, workflow graphs, and the shared policy and parser bundles compiled into every definition",
    },
    BoundaryEntry {
        path: "crates/review/src",
        kind: Kind::Directory,
        presence: Presence::Required,
        reason: "compiled review semantics: definitions, task binding, node execution, subject and manifest identity (test-only files excluded below)",
    },
    BoundaryEntry {
        path: "crates/review/src/golden.rs",
        kind: Kind::File,
        presence: Presence::Optional,
        reason: "golden render and parse behaviour of screening (also covered by the directory entry once present)",
    },
    BoundaryEntry {
        path: "crates/review/golden",
        kind: Kind::Directory,
        presence: Presence::Optional,
        reason: "golden render and parse fixtures that pin screening behaviour",
    },
    BoundaryEntry {
        path: "crates/ai/src",
        kind: Kind::Directory,
        presence: Presence::Required,
        reason: "the AI request and response path: runner, screening task, normalisation, grounding, gateways, provider adapters, shared types and admission policy; the modules in EXCLUDED_PATHS are the rest of the crate",
    },
    BoundaryEntry {
        path: "crates/ai/src/endpoint.rs",
        kind: Kind::File,
        presence: Presence::Optional,
        reason: "provider endpoint identity, added by the provider-endpoint work (also covered by the directory entry once present)",
    },
    BoundaryEntry {
        path: "crates/domain/src",
        kind: Kind::Directory,
        presence: Presence::Required,
        reason: "eligibility criteria, screening stages and decisions, document evidence identifiers, and the bibliography normalisation that shares these types",
    },
    BoundaryEntry {
        path: "crates/application/src/workflows/gating.rs",
        kind: Kind::File,
        presence: Presence::Required,
        reason: "routes records by the AI verdict (included, excluded or unsure)",
    },
    BoundaryEntry {
        path: "crates/application/src/workflows/autonomy.rs",
        kind: Kind::File,
        presence: Presence::Required,
        reason: "autonomy levels that decide whether an AI verdict is only suggested, recorded as a second opinion, or applied",
    },
    BoundaryEntry {
        path: "crates/postgres/src/review_preparation.rs",
        kind: Kind::File,
        presence: Presence::Required,
        reason: "maps a screening subject to the ScreeningInput the model sees: criteria prompts, metadata evidence and the full-text retrieval query",
    },
    BoundaryEntry {
        path: "crates/postgres/src/ai.rs",
        kind: Kind::File,
        presence: Presence::Required,
        reason: "screening target, model route resolution, and the full-text evidence retrieval SQL that decides which passages the model sees",
    },
    BoundaryEntry {
        path: "crates/postgres/src/ai/acceptance.rs",
        kind: Kind::File,
        presence: Presence::Required,
        reason: "maps an accepted AI suggestion to a screening decision, stage and exclusion reason",
    },
    BoundaryEntry {
        path: "crates/postgres/src/ai/proposals.rs",
        kind: Kind::File,
        presence: Presence::Required,
        reason: "accepting a screening suggestion routes it through the screening state machine",
    },
    BoundaryEntry {
        path: "crates/postgres/src/screening.rs",
        kind: Kind::File,
        presence: Presence::Required,
        reason: "the state machine that records every screening decision, including accepted AI suggestions, and checks exclusion reasons and revisions",
    },
    BoundaryEntry {
        path: "crates/postgres/src/ai_autonomy_apply.rs",
        kind: Kind::File,
        presence: Presence::Required,
        reason: "applies the autonomy level to a persisted AI proposal, including the second-reviewer opinion",
    },
    BoundaryEntry {
        path: "crates/postgres/src/autonomy.rs",
        kind: Kind::File,
        presence: Presence::Required,
        reason: "resolves the project autonomy level that gates each AI verdict",
    },
    BoundaryEntry {
        path: "crates/postgres/src/ai_first",
        kind: Kind::Directory,
        presence: Presence::Required,
        reason: "AI-first eligibility, authority, inference and blinded routing policy",
    },
    BoundaryEntry {
        path: "crates/application/src/ai_first.rs",
        kind: Kind::File,
        presence: Presence::Required,
        reason: "AI-first eligibility, authority, inference and blinded routing policy",
    },
    BoundaryEntry {
        path: "crates/postgres/migrations/0054_ai_first_cohorts.sql",
        kind: Kind::File,
        presence: Presence::Required,
        reason: "AI-first eligibility, authority, inference and blinded routing policy",
    },
    BoundaryEntry {
        path: "crates/postgres/migrations/0055_ai_first_blind_controls.sql",
        kind: Kind::File,
        presence: Presence::Required,
        reason: "AI-first blind audit read boundary",
    },
    BoundaryEntry {
        path: "crates/postgres/src/graph.rs",
        kind: Kind::File,
        presence: Presence::Required,
        reason: "AI-first blind audit read boundary",
    },
    BoundaryEntry {
        path: "crates/postgres/src/agent_tools.rs",
        kind: Kind::File,
        presence: Presence::Required,
        reason: "AI-first blind audit read boundary",
    },
    BoundaryEntry {
        path: "crates/postgres/src/documents.rs",
        kind: Kind::File,
        presence: Presence::Required,
        reason: "AI-first blind audit read boundary",
    },
    BoundaryEntry {
        path: "crates/postgres/src/study.rs",
        kind: Kind::File,
        presence: Presence::Required,
        reason: "AI-first blind audit read boundary",
    },
    BoundaryEntry {
        path: "crates/postgres/src/ai_exposure.rs",
        kind: Kind::File,
        presence: Presence::Required,
        reason: "availability provenance and independent human/AI evidence selection",
    },
    BoundaryEntry {
        path: "crates/postgres/src/audit_export.rs",
        kind: Kind::File,
        presence: Presence::Required,
        reason: "audit export blinding prevents conditional workflow topology from revealing exclusions",
    },
    BoundaryEntry {
        path: "crates/postgres/src/automations.rs",
        kind: Kind::File,
        presence: Presence::Required,
        reason: "public automation inspection withholds active AI-first topology",
    },
    BoundaryEntry {
        path: "crates/postgres/src/activity.rs",
        kind: Kind::File,
        presence: Presence::Required,
        reason: "second-reviewer activity blinding and exposure recording",
    },
    BoundaryEntry {
        path: "crates/postgres/migrations/0052_ai_opinion_availability.sql",
        kind: Kind::File,
        presence: Presence::Required,
        reason: "database availability policy and waiting-proposal blinding",
    },
    BoundaryEntry {
        path: "crates/postgres/src/ai_reviewer.rs",
        kind: Kind::File,
        presence: Presence::Required,
        reason: "compares the AI second-reviewer verdict with the human decision and resolves the conflict",
    },
    BoundaryEntry {
        path: "services/worker/src/processor.rs",
        kind: Kind::File,
        presence: Presence::Required,
        reason: "compiled screening orchestration: primary screen, independent screen, reconciliation, candidate audit and bounded repair (other job code in the file is included too)",
    },
    BoundaryEntry {
        path: "services/worker/src/workflows/nodes.rs",
        kind: Kind::File,
        presence: Presence::Required,
        reason: "the Run an AI review workflow node: schedules screening, waits for verdicts, routes records and suggests decisions (other node code in the file is included too)",
    },
];

/// Paths inside included directories that are deliberately not hashed.
/// Test-only files (`tests.rs`, `*_tests.rs`) are excluded by name everywhere.
pub(crate) const EXCLUDED_PATHS: &[Exclusion] = &[
    Exclusion {
        path: "crates/review/src/memory.rs",
        kind: Kind::File,
        reason: "in-memory scheduler adapter, used only by its own unit test (verified by search)",
    },
    Exclusion {
        path: "crates/ai/src/agent_loop.rs",
        kind: Kind::File,
        reason: "assistant tool-calling loop; screening never runs through it",
    },
    Exclusion {
        path: "crates/ai/src/agents.rs",
        kind: Kind::File,
        reason: "assistant agent tools and their authorisation; not on the screening path",
    },
    Exclusion {
        path: "crates/ai/src/assistant.rs",
        kind: Kind::File,
        reason: "assistant chat answers; not on the screening path",
    },
    Exclusion {
        path: "crates/ai/src/chat.rs",
        kind: Kind::File,
        reason: "multi-turn chat types; screening calls only AiGateway::complete",
    },
    Exclusion {
        path: "crates/ai/src/classification.rs",
        kind: Kind::File,
        reason: "study-design classification task of another review definition",
    },
    Exclusion {
        path: "crates/ai/src/dedupe.rs",
        kind: Kind::File,
        reason: "duplicate-detection task of another review definition",
    },
    Exclusion {
        path: "crates/ai/src/review_assistance.rs",
        kind: Kind::File,
        reason: "study grouping, appraisal prefill and data extraction tasks of other review definitions; those definitions keep the broad source hash (see identity.rs)",
    },
    Exclusion {
        path: "crates/ai/src/pricing.rs",
        kind: Kind::File,
        reason: "price book: changes cost, never the content of a request or an answer",
    },
    Exclusion {
        path: "crates/ai/src/usage.rs",
        kind: Kind::File,
        reason: "metering wrapper: records usage and may refuse a call over budget; it does not alter request or answer content",
    },
    Exclusion {
        path: "crates/ai/src/evals.rs",
        kind: Kind::File,
        reason: "offline evaluation harness and release gates; does not run inside a request",
    },
    Exclusion {
        path: "crates/ai/src/evals",
        kind: Kind::Directory,
        reason: "offline evaluation harness and release gates; does not run inside a request",
    },
];

/// Documentation of what is outside the boundary, written to the audit
/// listing. Paths here are not hashed, and nothing enforces this list.
pub(crate) const OUTSIDE_BOUNDARY: &[(&str, &str)] = &[
    (
        "crates/http-api",
        "HTTP scheduling adapter: it selects a typed subject and calls review_preparation; it interprets no semantics",
    ),
    (
        "crates/graph",
        "graph utilities for the workflow and reporting views; not on the screening path",
    ),
    (
        "crates/documents",
        "PDF parser and block extraction; document text enters the boundary only through the stored blocks and must change with a parser version bump (see the module docs)",
    ),
    (
        "crates/postgres/src/review_runs.rs",
        "run state machine and persistence of manifests and attempts; no interpretation of answers",
    ),
    (
        "crates/postgres/src/review_run_setup.rs",
        "builds manifest inputs; the identity-relevant values are hashed in deepref-review and runtime provenance is recorded for audit only",
    ),
    (
        "crates/postgres/src/ai/persistence.rs",
        "stores runs, route rows and embeddings",
    ),
    (
        "crates/postgres/src/ai_usage.rs",
        "usage ledger persistence and budgets",
    ),
    (
        "crates/postgres/src/appraisal.rs",
        "appraisal persistence for another review definition",
    ),
    (
        "crates/application/src/appraisal",
        "appraisal suggestion logic of another review definition; it keeps the broad source hash",
    ),
    (
        "crates/application/src/workflows (other files)",
        "workflow graph model, catalog, validation, engine and templates: they shape the graph, which the definition workflow hash covers",
    ),
    (
        "services/worker/src/lib.rs",
        "wiring of gateways and providers; the endpoint and resolved model routes are identity components of their own",
    ),
    (
        "services/worker/src/reconciler.rs",
        "recovers stuck work; does not change answers",
    ),
    (
        "apps/, tests/, benches/, fuzz/, docs/",
        "applications, test code, benchmarks, fuzz targets and documentation",
    ),
    (
        "crates/ai/src test modules (tests.rs, *_tests.rs)",
        "compiled only under cfg(test)",
    ),
];

/// Third-party crates that the boundary uses for semantics. Each root is the
/// direct dependency of at least one boundary crate, and each is resolved to the
/// exact lockfile version that crate uses.
pub(crate) const SEMANTIC_DEPENDENCY_ROOTS: &[(&str, &str)] = &[
    ("num-bigint", "exact fixed-cohort authority test arithmetic"),
    (
        "serde",
        "derives (de)serialisation of every boundary type, including manifests and identity snapshots",
    ),
    (
        "serde_json",
        "builds prompts and canonical JSON, and computes reuse and manifest hashes",
    ),
    (
        "schemars",
        "generates the output JSON Schema that the model is asked to satisfy",
    ),
    (
        "jsonschema",
        "validates model output against that schema before it is accepted",
    ),
    (
        "sha2",
        "content hashes, evidence keys, reuse keys, and manifest and fingerprint digests",
    ),
    (
        "uuid",
        "identifiers in requests, evidence, proposals and the citation keys the model copies",
    ),
    (
        "chrono",
        "timestamps in run records and the screening wait window",
    ),
    (
        "unicode-normalization",
        "normalises titles and identifiers in the domain bibliography model",
    ),
    (
        "unicode_categories",
        "character classes used by the same normalisation",
    ),
    (
        "rapidfuzz",
        "title similarity in the domain bibliography model",
    ),
];

/// Transport, runtime and macro crates pruned from the closure: they are
/// neither roots nor traversed, and neither is their dependency subtree.
/// Without the pruning, `jsonschema`'s default features would pull the HTTP and
/// TLS stack (`reqwest`, `rustls`, `aws-lc-rs`) into the screening identity.
/// Schema validation does not depend on any of them.
pub(crate) const EXCLUDED_THIRD_PARTY: &[(&str, &str)] = &[
    (
        "thiserror",
        "derive-only: the Display strings are literals in boundary source",
    ),
    ("tracing", "logging and spans only"),
    (
        "tokio",
        "async runtime, timers and retry sleeps; no semantic values",
    ),
    (
        "reqwest",
        "HTTP transport; request bodies are built with serde_json inside the boundary",
    ),
    (
        "rustls",
        "TLS transport that jsonschema reaches through its reqwest feature; the screening schemas have no remote references",
    ),
    (
        "sqlx",
        "database transport; the SQL text that decides results is in the boundary files",
    ),
    (
        "pgvector",
        "lossless binary encoding of embeddings; similarity is computed by PostgreSQL",
    ),
    (
        "anyhow",
        "error plumbing in the worker; messages are not verdicts",
    ),
    (
        "rig-core",
        "RigGateway and RigEmbeddingGateway have no callers in the workspace (verified by search)",
    ),
    ("proptest", "dev-only property tests"),
];

/// Workspace packages whose direct dependencies define the roots.
pub(crate) const BOUNDARY_PACKAGES: &[&str] = &[
    "deepref-review",
    "deepref-ai",
    "deepref-domain",
    "deepref-application",
    "deepref-postgres",
    "deepref-worker",
];

/// Both digests and the audit listings that build.rs writes to `OUT_DIR`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Fingerprints {
    /// Hex SHA-256 for `DEEPREF_SEMANTIC_IMPLEMENTATION_SHA`.
    pub(crate) implementation_sha: String,
    /// Hex SHA-256 for `DEEPREF_SEMANTIC_DEPENDENCY_SHA`.
    pub(crate) dependency_sha: String,
    /// Audit listing of the implementation boundary.
    pub(crate) implementation_report: String,
    /// Audit listing of the dependency closure, exposed as `SEMANTIC_DEPENDENCIES`.
    pub(crate) dependency_report: String,
    /// Workspace-relative paths whose change can change an output.
    pub(crate) watched: BTreeSet<String>,
}

/// Computes both fingerprints for the workspace at `workspace`.
pub(crate) fn compute(workspace: &Path) -> Result<Fingerprints> {
    let implementation =
        implementation_boundary(workspace, IMPLEMENTATION_BOUNDARY, EXCLUDED_PATHS)?;
    let dependency = dependency_boundary(workspace)?;
    let mut watched = implementation.watched;
    watched.extend(dependency.watched);
    Ok(Fingerprints {
        implementation_sha: implementation.sha,
        dependency_sha: dependency.sha,
        implementation_report: implementation.report,
        dependency_report: dependency.report,
        watched,
    })
}

#[derive(Debug)]
struct Digested {
    sha: String,
    report: String,
    watched: BTreeSet<String>,
}

/// Hashes the implementation boundary. The table and exclusions are parameters
/// so that tests can run the real logic against a small synthetic workspace.
fn implementation_boundary(
    workspace: &Path,
    entries: &[BoundaryEntry],
    exclusions: &[Exclusion],
) -> Result<Digested> {
    let mut files: BTreeMap<String, Vec<u8>> = BTreeMap::new();
    let mut watched = BTreeSet::new();
    let mut absent = Vec::new();

    for entry in entries.iter().filter(|entry| entry.kind == Kind::Directory) {
        let directory = workspace.join(entry.path);
        if !directory.is_dir() {
            absent_or_fail(entry, &mut absent, &mut watched)?;
            continue;
        }
        watched.insert(entry.path.to_owned());
        collect_directory(workspace, entry.path, exclusions, &mut files)?;
    }

    for entry in entries.iter().filter(|entry| entry.kind == Kind::File) {
        if files.contains_key(entry.path) {
            // Already covered by a directory entry; nothing more to do.
            watched.insert(entry.path.to_owned());
            continue;
        }
        if !workspace.join(entry.path).is_file() {
            absent_or_fail(entry, &mut absent, &mut watched)?;
            continue;
        }
        watched.insert(entry.path.to_owned());
        let bytes = read_boundary_file(workspace, entry.path)?;
        files.insert(entry.path.to_owned(), bytes);
    }

    let sha = file_set_digest(&files);
    let report = implementation_report(&sha, entries, exclusions, &files, &absent);
    Ok(Digested {
        sha,
        report,
        watched,
    })
}

fn absent_or_fail(
    entry: &BoundaryEntry,
    absent: &mut Vec<&'static str>,
    watched: &mut BTreeSet<String>,
) -> Result<()> {
    match entry.presence {
        Presence::Required => Err(FingerprintError::new(format!(
            "semantic boundary path `{}` is missing. The build fails so the boundary table \
             cannot drift silently. Restore the path, or update IMPLEMENTATION_BOUNDARY in \
             crates/review/build_support/fingerprint.rs. Purpose: {}",
            entry.path, entry.reason
        ))),
        Presence::Optional => {
            absent.push(entry.path);
            watched.insert(parent_of(entry.path).to_owned());
            Ok(())
        }
    }
}

fn parent_of(path: &str) -> &str {
    path.rsplit_once('/').map_or(path, |(parent, _)| parent)
}

/// Walks `relative` (a workspace-relative directory) and records every hashed
/// file under it. Symbolic links are refused: they would make the boundary
/// depend on paths outside the workspace.
fn collect_directory(
    workspace: &Path,
    relative: &str,
    exclusions: &[Exclusion],
    files: &mut BTreeMap<String, Vec<u8>>,
) -> Result<()> {
    let directory = workspace.join(relative);
    let mut names = Vec::new();
    for entry in fs::read_dir(&directory)
        .map_err(|error| FingerprintError::new(format!("cannot read {relative}: {error}")))?
    {
        let entry = entry
            .map_err(|error| FingerprintError::new(format!("cannot read {relative}: {error}")))?;
        let name = entry
            .file_name()
            .into_string()
            .map_err(|_| FingerprintError::new(format!("non-UTF-8 file name under {relative}")))?;
        let file_type = entry.file_type().map_err(|error| {
            FingerprintError::new(format!("cannot stat {relative}/{name}: {error}"))
        })?;
        if file_type.is_symlink() {
            return Err(FingerprintError::new(format!(
                "symbolic link `{relative}/{name}` is not allowed inside the semantic boundary"
            )));
        }
        names.push((name, file_type.is_dir()));
    }
    names.sort();

    for (name, is_directory) in names {
        let child = format!("{relative}/{name}");
        if is_test_only_name(&name, is_directory) || is_excluded(&child, is_directory, exclusions) {
            continue;
        }
        if is_directory {
            collect_directory(workspace, &child, exclusions, files)?;
        } else {
            let bytes = read_boundary_file(workspace, &child)?;
            files.insert(child, bytes);
        }
    }
    Ok(())
}

fn is_test_only_name(name: &str, is_directory: bool) -> bool {
    if is_directory {
        name == "tests"
    } else {
        name == "tests.rs" || name.ends_with("_tests.rs")
    }
}

fn is_excluded(path: &str, is_directory: bool, exclusions: &[Exclusion]) -> bool {
    exclusions.iter().any(|exclusion| {
        exclusion.path == path && (exclusion.kind == Kind::Directory) == is_directory
    })
}

/// Reads one boundary file and applies the test-only removal for Rust sources.
fn read_boundary_file(workspace: &Path, relative: &str) -> Result<Vec<u8>> {
    let bytes = fs::read(workspace.join(relative))
        .map_err(|error| FingerprintError::new(format!("cannot read {relative}: {error}")))?;
    if relative.ends_with(".rs")
        && let Ok(text) = std::str::from_utf8(&bytes)
        && let Some(stripped) = remove_test_only_items(text)
    {
        return Ok(stripped.into_bytes());
    }
    Ok(bytes)
}

/// SHA-256 over `path NUL contents NUL` for each file, in path order.
fn file_set_digest(files: &BTreeMap<String, Vec<u8>>) -> String {
    let mut digest = Sha256::new();
    for (path, contents) in files {
        digest.update(path.as_bytes());
        digest.update([0]);
        digest.update(contents);
        digest.update([0]);
    }
    hex(&digest.finalize())
}

fn implementation_report(
    sha: &str,
    entries: &[BoundaryEntry],
    exclusions: &[Exclusion],
    files: &BTreeMap<String, Vec<u8>>,
    absent: &[&str],
) -> String {
    let mut report = String::new();
    let _ = writeln!(report, "# DeepRef semantic implementation boundary.");
    let _ = writeln!(
        report,
        "# Generated by crates/review/build.rs; boundary table: crates/review/build_support/fingerprint.rs."
    );
    let _ = writeln!(
        report,
        "# DEEPREF_SEMANTIC_IMPLEMENTATION_SHA = SHA-256 over `path NUL contents NUL` for each file below, in path order."
    );
    let _ = writeln!(
        report,
        "# Rust contents have #[cfg(test)] items removed first. Paths are workspace-relative."
    );
    let _ = writeln!(report, "digest {sha}");
    let _ = writeln!(report, "[entries]");
    for entry in entries {
        let presence = match entry.presence {
            Presence::Required => "required",
            Presence::Optional => "optional",
        };
        let kind = match entry.kind {
            Kind::File => "file",
            Kind::Directory => "dir",
        };
        let _ = writeln!(
            report,
            "{kind} {presence} {} | {}",
            entry.path, entry.reason
        );
    }
    let _ = writeln!(report, "[excluded]");
    for exclusion in exclusions {
        let kind = match exclusion.kind {
            Kind::File => "file",
            Kind::Directory => "dir",
        };
        let _ = writeln!(report, "{kind} {} | {}", exclusion.path, exclusion.reason);
    }
    for (path, reason) in OUTSIDE_BOUNDARY {
        let _ = writeln!(report, "outside {path} | {reason}");
    }
    let _ = writeln!(report, "[absent]");
    for path in absent {
        let _ = writeln!(report, "{path}");
    }
    let _ = writeln!(report, "[files]");
    for (path, contents) in files {
        let _ = writeln!(report, "{} {path}", hex(&Sha256::digest(contents)));
    }
    report
}

/// The dependency closure, its declaring manifest lines and the digest over them.
struct DependencyDigest {
    sha: String,
    report: String,
    watched: BTreeSet<String>,
}

fn dependency_boundary(workspace: &Path) -> Result<DependencyDigest> {
    dependency_digest(workspace, BOUNDARY_PACKAGES, SEMANTIC_DEPENDENCY_ROOTS)
}

/// Hashes the closure of the roots in `root_table`, as declared by
/// `boundary_packages`, for the workspace at `workspace`.
fn dependency_digest(
    workspace: &Path,
    boundary_packages: &[&str],
    root_table: &[(&str, &str)],
) -> Result<DependencyDigest> {
    let lock_text = read_text(&workspace.join("Cargo.lock"))?;
    let lock = parse_lockfile(&lock_text)?;
    let roots = root_packages(&lock, boundary_packages, root_table)?;
    let closure = closure(&lock, &roots, EXCLUDED_THIRD_PARTY)?;

    let mut closure_lines = Vec::new();
    for index in &closure.packages {
        let package = lock.packages.get(*index).ok_or_else(|| {
            FingerprintError::new("internal error: closure index outside the lockfile")
        })?;
        closure_lines.push(format!(
            "{} {} {} {}",
            package.name,
            package.version,
            package.source.as_deref().unwrap_or("-"),
            package.checksum.as_deref().unwrap_or("-")
        ));
    }
    closure_lines.sort();

    let manifests = member_manifests(workspace)?;
    let root_names: BTreeSet<&str> = root_table.iter().map(|(name, _)| *name).collect();
    let mut declaration_lines = Vec::new();
    for manifest in &manifests {
        let text = read_text(&workspace.join(manifest))?;
        declaration_lines.extend(declaration_lines_of(manifest, &text, &root_names));
    }
    declaration_lines.sort();

    let mut digest = Sha256::new();
    digest.update(b"closure\n");
    for line in &closure_lines {
        digest.update(line.as_bytes());
        digest.update(b"\n");
    }
    digest.update(b"declarations\n");
    for line in &declaration_lines {
        digest.update(line.as_bytes());
        digest.update(b"\n");
    }
    let sha = hex(&digest.finalize());

    let report = dependency_report(&sha, &closure_lines, &closure.pruned, &declaration_lines);
    let mut watched: BTreeSet<String> = manifests.into_iter().collect();
    watched.insert("Cargo.lock".to_owned());
    Ok(DependencyDigest {
        sha,
        report,
        watched,
    })
}

fn dependency_report(
    sha: &str,
    closure: &[String],
    pruned: &BTreeSet<String>,
    declarations: &[String],
) -> String {
    let mut report = String::new();
    let _ = writeln!(report, "# DeepRef semantic dependency closure.");
    let _ = writeln!(
        report,
        "# Generated by crates/review/build.rs; roots and rules: crates/review/build_support/fingerprint.rs."
    );
    let _ = writeln!(
        report,
        "# DEEPREF_SEMANTIC_DEPENDENCY_SHA covers the [closure] lines, then the [declarations] lines, each sorted."
    );
    let _ = writeln!(report, "digest {sha}");
    let _ = writeln!(report, "[roots]");
    for (name, reason) in SEMANTIC_DEPENDENCY_ROOTS {
        let _ = writeln!(report, "{name} | {reason}");
    }
    let _ = writeln!(report, "[excluded]");
    for (name, reason) in EXCLUDED_THIRD_PARTY {
        let _ = writeln!(report, "{name} | {reason}");
    }
    let _ = writeln!(report, "[pruned] reached from a root, not hashed");
    for package in pruned {
        let _ = writeln!(report, "{package}");
    }
    let _ = writeln!(report, "[closure] name version source checksum");
    for line in closure {
        let _ = writeln!(report, "{line}");
    }
    let _ = writeln!(report, "[declarations] manifest | declaration");
    for line in declarations {
        let _ = writeln!(report, "{line}");
    }
    report
}

/// The root manifest and every workspace member manifest, in path order.
fn member_manifests(workspace: &Path) -> Result<Vec<String>> {
    let mut manifests = vec!["Cargo.toml".to_owned()];
    for group in ["crates", "services", "apps", "tools"] {
        let directory = workspace.join(group);
        if !directory.is_dir() {
            continue;
        }
        let mut names = Vec::new();
        for entry in fs::read_dir(&directory)
            .map_err(|error| FingerprintError::new(format!("cannot read {group}: {error}")))?
        {
            let entry = entry
                .map_err(|error| FingerprintError::new(format!("cannot read {group}: {error}")))?;
            let file_type = entry
                .file_type()
                .map_err(|error| FingerprintError::new(format!("cannot read {group}: {error}")))?;
            if file_type.is_dir() {
                let name = entry.file_name().into_string().map_err(|_| {
                    FingerprintError::new(format!("non-UTF-8 directory name under {group}"))
                })?;
                names.push(name);
            }
        }
        names.sort();
        for name in names {
            let candidate = format!("{group}/{name}/Cargo.toml");
            if workspace.join(&candidate).is_file() {
                manifests.push(candidate);
            }
        }
    }
    Ok(manifests)
}

/// The lines of one manifest that declare a semantic root, plus every line of
/// its `[features]` table. Everything else (members, lints, profiles, package
/// metadata, unrelated dependencies) is left out, so unrelated manifest edits do
/// not change the digest.
fn declaration_lines_of(manifest: &str, text: &str, roots: &BTreeSet<&str>) -> Vec<String> {
    #[derive(Clone, Copy, PartialEq, Eq)]
    enum Section {
        Other,
        Features,
        /// A dependency table such as `[dependencies]`: its keys are names.
        DependencyKeys,
        /// A table named for one dependency, such as `[dependencies.serde]`.
        DependencyTable {
            relevant: bool,
        },
    }

    let mut section = Section::Other;
    let mut open_depth: i64 = 0;
    let mut lines = Vec::new();
    for raw in text.lines() {
        let trimmed = raw.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        if open_depth > 0 {
            // Continuation of a root declaration that spans several lines.
            lines.push(format!("{manifest} | {trimmed}"));
            open_depth += bracket_balance(trimmed);
            continue;
        }
        if trimmed.starts_with('[') {
            let inner = trimmed.trim_start_matches('[').trim_end_matches(']').trim();
            section = if inner == "features" {
                Section::Features
            } else if inner.ends_with("dependencies") {
                Section::DependencyKeys
            } else if let Some((_, name)) = inner.rsplit_once('.')
                && inner.contains("dependencies.")
            {
                let name = name.trim().trim_matches('"').trim_matches('\'');
                let relevant = roots.contains(name);
                if relevant {
                    lines.push(format!("{manifest} | {trimmed}"));
                }
                Section::DependencyTable { relevant }
            } else {
                Section::Other
            };
            continue;
        }
        match section {
            Section::Features => lines.push(format!("{manifest} | {trimmed}")),
            Section::DependencyTable { relevant: true } => {
                lines.push(format!("{manifest} | {trimmed}"));
            }
            Section::DependencyKeys => {
                if let Some(key) = declared_key(trimmed)
                    && roots.contains(key)
                {
                    lines.push(format!("{manifest} | {trimmed}"));
                    open_depth = bracket_balance(trimmed);
                }
            }
            Section::DependencyTable { relevant: false } | Section::Other => {}
        }
    }
    lines
}

/// The dependency name a key-value line declares: `serde = ...`, `serde.workspace = true`.
fn declared_key(line: &str) -> Option<&str> {
    let (left, _) = line.split_once('=')?;
    let key = left.split('.').next()?.trim().trim_matches('"');
    (!key.is_empty()).then_some(key)
}

/// Brackets a line opens minus the brackets it closes.
fn bracket_balance(line: &str) -> i64 {
    line.chars().fold(0, |depth, character| match character {
        '[' | '{' => depth + 1,
        ']' | '}' => depth - 1,
        _ => depth,
    })
}

/// One resolved package from `Cargo.lock`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LockPackage {
    pub(crate) name: String,
    pub(crate) version: String,
    pub(crate) source: Option<String>,
    pub(crate) checksum: Option<String>,
    pub(crate) dependencies: Vec<LockDependency>,
}

/// One entry of a package's `dependencies` list. The lockfile writes a version
/// only when the name is ambiguous, and a source only when name and version are
/// both ambiguous.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LockDependency {
    pub(crate) name: String,
    pub(crate) version: Option<String>,
    pub(crate) source: Option<String>,
}

/// A parsed `Cargo.lock` in the version 3 or 4 format.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Lockfile {
    pub(crate) packages: Vec<LockPackage>,
}

#[derive(Default)]
struct PartialPackage {
    name: Option<String>,
    version: Option<String>,
    source: Option<String>,
    checksum: Option<String>,
    dependencies: Vec<LockDependency>,
}

/// Parses the subset of TOML that `Cargo.lock` versions 3 and 4 use: a top-level
/// `version`, `[[package]]` tables with string keys, and string arrays. Anything
/// else is an error, so an unfamiliar lockfile fails the build instead of
/// producing a partial closure.
pub(crate) fn parse_lockfile(text: &str) -> Result<Lockfile> {
    let lines: Vec<&str> = text.lines().collect();
    let mut format = None;
    let mut packages = Vec::new();
    let mut current: Option<(usize, PartialPackage)> = None;
    let mut index = 0;
    while index < lines.len() {
        let number = index + 1;
        let line = lines.get(index).map_or("", |line| line.trim());
        index += 1;
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if line == "[[package]]" {
            if let Some((start, partial)) = current.take() {
                packages.push(finish_package(start, partial)?);
            }
            current = Some((number, PartialPackage::default()));
            continue;
        }
        if line.starts_with('[') {
            if let Some((start, partial)) = current.take() {
                packages.push(finish_package(start, partial)?);
            }
            continue;
        }
        let (key, value) = line.split_once('=').ok_or_else(|| {
            FingerprintError::new(format!("Cargo.lock line {number} is not `key = value`"))
        })?;
        let key = key.trim();
        let mut value = value.trim().to_owned();
        if value.starts_with('[') {
            while !array_is_closed(&value) {
                let next = lines.get(index).ok_or_else(|| {
                    FingerprintError::new(format!(
                        "Cargo.lock array starting at line {number} is not closed"
                    ))
                })?;
                value.push('\n');
                value.push_str(next);
                index += 1;
            }
            let items = parse_string_list(&value).map_err(|reason| {
                FingerprintError::new(format!("Cargo.lock line {number}: {reason}"))
            })?;
            if let Some((_, partial)) = current.as_mut()
                && key == "dependencies"
            {
                partial.dependencies = items
                    .iter()
                    .map(|item| parse_dependency(item))
                    .collect::<Result<Vec<_>>>()?;
            }
            continue;
        }
        match current.as_mut() {
            Some((_, partial)) => {
                let text = parse_string_value(&value).map_err(|reason| {
                    FingerprintError::new(format!("Cargo.lock line {number}: {reason}"))
                })?;
                match key {
                    "name" => partial.name = Some(text),
                    "version" => partial.version = Some(text),
                    "source" => partial.source = Some(text),
                    "checksum" => partial.checksum = Some(text),
                    _ => {}
                }
            }
            None if key == "version" => {
                format = Some(value.parse::<u32>().map_err(|_| {
                    FingerprintError::new(format!("Cargo.lock version `{value}` is not a number"))
                })?);
            }
            None => {}
        }
    }
    if let Some((start, partial)) = current.take() {
        packages.push(finish_package(start, partial)?);
    }
    let format =
        format.ok_or_else(|| FingerprintError::new("Cargo.lock has no top-level version line"))?;
    if format != 3 && format != 4 {
        return Err(FingerprintError::new(format!(
            "Cargo.lock version {format} is not supported by the semantic dependency fingerprint \
             (versions 3 and 4 are). Update fingerprint.rs before relying on it."
        )));
    }
    Ok(Lockfile { packages })
}

fn finish_package(start: usize, partial: PartialPackage) -> Result<LockPackage> {
    let (Some(name), Some(version)) = (partial.name, partial.version) else {
        return Err(FingerprintError::new(format!(
            "Cargo.lock package starting at line {start} has no name or version"
        )));
    };
    Ok(LockPackage {
        name,
        version,
        source: partial.source,
        checksum: partial.checksum,
        dependencies: partial.dependencies,
    })
}

/// Parses one `dependencies` entry: `name`, `name version`, `name version (source)`
/// or `name (source)`.
pub(crate) fn parse_dependency(entry: &str) -> Result<LockDependency> {
    let (head, source) = match entry.split_once(" (") {
        Some((head, rest)) => {
            let source = rest.strip_suffix(')').ok_or_else(|| {
                FingerprintError::new(format!("dependency entry `{entry}` has an unclosed source"))
            })?;
            (head, Some(source.to_owned()))
        }
        None => (entry, None),
    };
    let mut parts = head.split_whitespace();
    let name = parts
        .next()
        .ok_or_else(|| FingerprintError::new(format!("dependency entry `{entry}` is empty")))?;
    let version = parts.next().map(str::to_owned);
    if parts.next().is_some() || name.contains('(') || name.contains(')') {
        return Err(FingerprintError::new(format!(
            "dependency entry `{entry}` is not `name`, `name version` or `name version (source)`"
        )));
    }
    Ok(LockDependency {
        name: name.to_owned(),
        version,
        source,
    })
}

/// Whether a string array that starts with `[` has its closing `]` outside any string.
fn array_is_closed(text: &str) -> bool {
    let mut in_string = false;
    let mut escaped = false;
    for character in text.chars() {
        if in_string {
            if escaped {
                escaped = false;
            } else if character == '\\' {
                escaped = true;
            } else if character == '"' {
                in_string = false;
            }
        } else if character == '"' {
            in_string = true;
        } else if character == ']' {
            return true;
        }
    }
    false
}

/// Reads the strings of a single-line or multi-line array. Only quoted strings,
/// commas, whitespace and the brackets may appear.
fn parse_string_list(text: &str) -> std::result::Result<Vec<String>, String> {
    let mut items = Vec::new();
    let mut chars = text.chars().peekable();
    let mut opened = false;
    let mut closed = false;
    while let Some(&character) = chars.peek() {
        match character {
            _ if character.is_whitespace() || character == ',' => {
                chars.next();
            }
            '[' if !opened => {
                chars.next();
                opened = true;
            }
            ']' if opened && !closed => {
                chars.next();
                closed = true;
            }
            '"' if opened && !closed => {
                chars.next();
                items.push(read_string_body(&mut chars)?);
            }
            other => return Err(format!("unexpected character {other:?} in an array")),
        }
    }
    if !closed {
        return Err("array is not closed".to_owned());
    }
    Ok(items)
}

/// Reads exactly one quoted string with nothing after it.
fn parse_string_value(value: &str) -> std::result::Result<String, String> {
    let mut chars = value.chars();
    if chars.next() != Some('"') {
        return Err(format!("expected a quoted string, found `{value}`"));
    }
    let mut body = chars.peekable();
    let text = read_string_body(&mut body)?;
    if body.next().is_some() {
        return Err(format!("unexpected text after the string in `{value}`"));
    }
    Ok(text)
}

/// Reads a TOML basic string whose opening quote is already consumed.
fn read_string_body(
    chars: &mut std::iter::Peekable<std::str::Chars<'_>>,
) -> std::result::Result<String, String> {
    let mut text = String::new();
    loop {
        match chars.next() {
            None => return Err("unterminated string".to_owned()),
            Some('"') => return Ok(text),
            Some('\\') => match chars.next() {
                Some('"') => text.push('"'),
                Some('\\') => text.push('\\'),
                other => return Err(format!("unsupported escape {other:?}")),
            },
            Some(character) => text.push(character),
        }
    }
}

/// Resolves a dependency entry to every package it can name. A name without a
/// version matches every version of that name, which is the conservative choice.
pub(crate) fn resolve_entry(lock: &Lockfile, dependency: &LockDependency) -> Result<Vec<usize>> {
    let matches: Vec<usize> = lock
        .packages
        .iter()
        .enumerate()
        .filter(|(_, package)| {
            package.name == dependency.name
                && dependency
                    .version
                    .as_ref()
                    .is_none_or(|version| &package.version == version)
                && dependency
                    .source
                    .as_ref()
                    .is_none_or(|source| package.source.as_ref() == Some(source))
        })
        .map(|(index, _)| index)
        .collect();
    if matches.is_empty() {
        return Err(FingerprintError::new(format!(
            "Cargo.lock has no package for the dependency `{}`{}{}",
            dependency.name,
            dependency
                .version
                .as_ref()
                .map_or_else(String::new, |version| format!(" {version}")),
            dependency
                .source
                .as_ref()
                .map_or_else(String::new, |source| format!(" ({source})")),
        )));
    }
    Ok(matches)
}

/// The lockfile packages that the roots depend on, each resolved exactly as the
/// boundary package that declares it resolves it.
pub(crate) fn root_packages(
    lock: &Lockfile,
    boundary_packages: &[&str],
    roots: &[(&str, &str)],
) -> Result<Vec<usize>> {
    let mut found = BTreeSet::new();
    let mut declared = BTreeSet::new();
    for boundary in boundary_packages {
        let package = lock
            .packages
            .iter()
            .find(|package| package.name == *boundary && package.source.is_none())
            .ok_or_else(|| {
                FingerprintError::new(format!(
                    "workspace package `{boundary}` is not in Cargo.lock"
                ))
            })?;
        for dependency in &package.dependencies {
            if roots.iter().any(|(name, _)| *name == dependency.name) {
                declared.insert(dependency.name.as_str());
                found.extend(resolve_entry(lock, dependency)?);
            }
        }
    }
    for (name, _) in roots {
        if !declared.contains(name) {
            return Err(FingerprintError::new(format!(
                "semantic dependency root `{name}` is not a direct dependency of any boundary \
                 crate. Remove it from SEMANTIC_DEPENDENCY_ROOTS or restore the dependency."
            )));
        }
    }
    Ok(found.into_iter().collect())
}

/// The packages a closure hashes, and the pruned packages it reached.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Closure {
    /// Indices into the lockfile's packages.
    pub(crate) packages: BTreeSet<usize>,
    /// `name version` of pruned packages that the traversal reached. Listed for
    /// audit, never hashed.
    pub(crate) pruned: BTreeSet<String>,
}

/// The transitive closure of `roots` through the lockfile `dependencies` lists,
/// not entering any package named in `pruned`.
pub(crate) fn closure(
    lock: &Lockfile,
    roots: &[usize],
    pruned: &[(&str, &str)],
) -> Result<Closure> {
    let mut seen = BTreeSet::new();
    let mut reached_pruned = BTreeSet::new();
    let mut pending = roots.to_vec();
    while let Some(index) = pending.pop() {
        let package = lock.packages.get(index).ok_or_else(|| {
            FingerprintError::new("internal error: closure index outside the lockfile")
        })?;
        if pruned.iter().any(|(name, _)| *name == package.name) {
            reached_pruned.insert(format!("{} {}", package.name, package.version));
            continue;
        }
        if !seen.insert(index) {
            continue;
        }
        for dependency in &package.dependencies {
            for next in resolve_entry(lock, dependency)? {
                if !seen.contains(&next) {
                    pending.push(next);
                }
            }
        }
    }
    Ok(Closure {
        packages: seen,
        pruned: reached_pruned,
    })
}

/// Reads a UTF-8 text file, naming the path in the error.
fn read_text(path: &Path) -> Result<String> {
    fs::read_to_string(path)
        .map_err(|error| FingerprintError::new(format!("cannot read {}: {error}", path.display())))
}

fn hex(bytes: &[u8]) -> String {
    let mut encoded = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        let _ = write!(encoded, "{byte:02x}");
    }
    encoded
}

/// Removes items that are compiled only under `#[cfg(test)]`, leaving everything
/// else byte-for-byte. A `#[cfg(test)]` line is removed together with the item
/// it gates, when that item is a single `;`-terminated line or a block whose
/// closing `}` sits at the same indentation as the attribute. Anything that
/// does not fit that shape is kept, so the removal can only over-include. A
/// block that dedents before it closes is also kept.
///
/// Returns `None` when nothing was removed, so the caller keeps the original bytes.
pub(crate) fn remove_test_only_items(source: &str) -> Option<String> {
    let lines: Vec<&str> = source.split_inclusive('\n').collect();
    let mut kept = String::with_capacity(source.len());
    let mut removed = false;
    let mut index = 0;
    while index < lines.len() {
        let line = lines.get(index).copied().unwrap_or_default();
        if line.trim() != "#[cfg(test)]" {
            kept.push_str(line);
            index += 1;
            continue;
        }
        let indent = indent_of(line);
        let mut item = index + 1;
        while lines
            .get(item)
            .is_some_and(|candidate| candidate.trim_start().starts_with("#["))
        {
            item += 1;
        }
        let Some(item_line) = lines.get(item).copied() else {
            kept.push_str(line);
            index += 1;
            continue;
        };
        if indent_of(item_line) != indent {
            kept.push_str(line);
            index += 1;
            continue;
        }
        let item_text = item_line.trim();
        if item_text.ends_with(';') && !item_text.contains('{') {
            index = item + 1;
            removed = true;
            continue;
        }
        if item_text.contains('{') && !item_text.contains('}') {
            match closing_line(&lines, item + 1, indent) {
                Some(end) => {
                    index = end + 1;
                    removed = true;
                }
                None => {
                    kept.push_str(line);
                    index += 1;
                }
            }
            continue;
        }
        kept.push_str(line);
        index += 1;
    }
    removed.then_some(kept)
}

/// The first `}` at `indent` after `start`, provided every non-blank line in
/// between is indented deeper than `indent`.
fn closing_line(lines: &[&str], start: usize, indent: usize) -> Option<usize> {
    for (offset, line) in lines.iter().enumerate().skip(start) {
        let text = line.trim();
        if text.is_empty() {
            continue;
        }
        if text == "}" && indent_of(line) == indent {
            return Some(offset);
        }
        if indent_of(line) <= indent {
            return None;
        }
    }
    None
}

/// The width of the leading spaces and tabs, counted in characters.
fn indent_of(line: &str) -> usize {
    line.chars()
        .take_while(|character| *character == ' ' || *character == '\t')
        .count()
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]
mod tests {
    //! Unit tests for the build-support logic. They exercise the same functions
    //! that build.rs runs, against fixtures and a temporary synthetic workspace.

    use std::{
        path::PathBuf,
        sync::atomic::{AtomicUsize, Ordering},
    };

    use super::*;

    const REGISTRY: &str = "registry+https://github.com/rust-lang/crates.io-index";

    const LOCK: &str = r#"# This file is automatically @generated by Cargo.
# It is not intended for manual editing.
version = 4

[[package]]
name = "deepref-ai"
version = "0.1.0"
dependencies = [
 "serde",
 "sha2 0.11.0",
 "uuid",
]

[[package]]
name = "serde"
version = "1.0.228"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "1111"
dependencies = [
 "serde_core",
 "serde_derive",
]

[[package]]
name = "serde_core"
version = "1.0.228"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "2222"

[[package]]
name = "serde_derive"
version = "1.0.228"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "3333"
dependencies = [
 "syn",
]

[[package]]
name = "syn"
version = "2.0.1"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "4444"

[[package]]
name = "sha2"
version = "0.10.9"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "5555"

[[package]]
name = "sha2"
version = "0.11.0"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "6666"
dependencies = [
 "digest",
]

[[package]]
name = "digest"
version = "0.11.0"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "7777"

[[package]]
name = "uuid"
version = "1.26.0"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "8888"
dependencies = ["getrandom 0.4.3", "serde_core"]

[[package]]
name = "getrandom"
version = "0.4.3"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "9999"

[[package]]
name = "unrelated"
version = "9.9.9"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "0000"
dependencies = ["syn"]

[[package]]
name = "local-path"
version = "0.2.0"
dependencies = ["serde 1.0.228"]
"#;

    fn lock() -> Lockfile {
        parse_lockfile(LOCK).expect("fixture lockfile should parse")
    }

    fn names_and_versions(lock: &Lockfile, indices: &BTreeSet<usize>) -> Vec<String> {
        indices
            .iter()
            .map(|index| {
                let package = &lock.packages[*index];
                format!("{} {}", package.name, package.version)
            })
            .collect()
    }

    #[test]
    fn lockfile_parses_packages_with_and_without_source_and_checksum() {
        let lock = lock();
        assert_eq!(lock.packages.len(), 12);
        let serde = &lock.packages[1];
        assert_eq!(serde.name, "serde");
        assert_eq!(serde.version, "1.0.228");
        assert_eq!(serde.source.as_deref(), Some(REGISTRY));
        assert_eq!(serde.checksum.as_deref(), Some("1111"));
        assert_eq!(serde.dependencies.len(), 2);

        let local = lock.packages.last().expect("local package");
        assert_eq!(local.name, "local-path");
        assert_eq!(local.source, None);
        assert_eq!(local.checksum, None);
    }

    #[test]
    fn dependency_entries_parse_name_version_and_source_forms() {
        assert_eq!(
            parse_dependency("serde").expect("name"),
            LockDependency {
                name: "serde".to_owned(),
                version: None,
                source: None,
            }
        );
        assert_eq!(
            parse_dependency("sha2 0.11.0").expect("name version"),
            LockDependency {
                name: "sha2".to_owned(),
                version: Some("0.11.0".to_owned()),
                source: None,
            }
        );
        assert_eq!(
            parse_dependency("getrandom 0.4.3 (registry+https://x/y)")
                .expect("name version source"),
            LockDependency {
                name: "getrandom".to_owned(),
                version: Some("0.4.3".to_owned()),
                source: Some("registry+https://x/y".to_owned()),
            }
        );
        assert_eq!(
            parse_dependency("serde (git+https://x/y?rev=1#abc)").expect("name source"),
            LockDependency {
                name: "serde".to_owned(),
                version: None,
                source: Some("git+https://x/y?rev=1#abc".to_owned()),
            }
        );
    }

    #[test]
    fn malformed_lockfiles_and_entries_are_rejected() {
        assert!(parse_lockfile("[[package]]\nname = \"a\"\nversion = \"1\"\n").is_err());
        assert!(parse_lockfile("version = 2\n").is_err());
        assert!(parse_lockfile("version = 5\n").is_err());
        assert!(parse_lockfile("version = 4\n[[package]]\nname = \"a\"\n").is_err());
        assert!(
            parse_lockfile("version = 4\n[[package]]\nname = \"a\"\nversion = \"1\"\ndependencies = [\n \"b\"\n").is_err(),
            "an unclosed dependency array must fail"
        );
        assert!(parse_dependency("a b c").is_err());
        assert!(parse_dependency("a (unclosed").is_err());
        assert!(parse_dependency("").is_err());
    }

    #[test]
    fn closure_follows_dependencies_transitively_and_shares_nodes() {
        let lock = lock();
        let roots = root_packages(&lock, &["deepref-ai"], &[("serde", ""), ("uuid", "")])
            .expect("roots resolve");
        let closure = closure(&lock, &roots, &[]).expect("closure");
        let names = names_and_versions(&lock, &closure.packages);
        assert!(names.contains(&"serde 1.0.228".to_owned()));
        assert!(names.contains(&"serde_core 1.0.228".to_owned()));
        assert!(names.contains(&"serde_derive 1.0.228".to_owned()));
        assert!(
            names.contains(&"syn 2.0.1".to_owned()),
            "transitive dependency"
        );
        assert!(
            names.contains(&"getrandom 0.4.3".to_owned()),
            "dependency through a versioned entry"
        );
        assert!(
            !names.contains(&"unrelated 9.9.9".to_owned()),
            "packages outside the closure stay out"
        );
    }

    #[test]
    fn unversioned_dependency_entries_include_every_version_of_the_name() {
        let lock = lock();
        // `sha2` has two versions. The boundary package names `sha2 0.11.0`, so
        // only that version is a root; 0.10.9 is not pulled in by the root.
        let roots = root_packages(&lock, &["deepref-ai"], &[("sha2", "")]).expect("roots");
        let names = names_and_versions(
            &lock,
            &closure(&lock, &roots, &[]).expect("closure").packages,
        );
        assert!(names.contains(&"sha2 0.11.0".to_owned()));
        assert!(names.contains(&"digest 0.11.0".to_owned()));
        assert!(!names.contains(&"sha2 0.10.9".to_owned()));

        // A dependency entry with no version matches every package of that name.
        let every_sha2 = LockDependency {
            name: "sha2".to_owned(),
            version: None,
            source: None,
        };
        assert_eq!(resolve_entry(&lock, &every_sha2).expect("matches").len(), 2);
    }

    #[test]
    fn pruned_packages_are_reached_but_their_subtrees_are_not_traversed() {
        // deepref-ai -> serde -> serde_derive -> syn. Pruning serde_derive keeps
        // syn out, and the pruned package is recorded for audit.
        let lock = lock();
        let roots = root_packages(&lock, &["deepref-ai"], &[("serde", "")]).expect("roots");
        let closure = closure(&lock, &roots, &[("serde_derive", "test")]).expect("closure");
        let names = names_and_versions(&lock, &closure.packages);
        assert!(names.contains(&"serde 1.0.228".to_owned()));
        assert!(!names.contains(&"serde_derive 1.0.228".to_owned()));
        assert!(!names.contains(&"syn 2.0.1".to_owned()));
        assert!(closure.pruned.contains("serde_derive 1.0.228"));
    }

    #[test]
    fn a_root_no_boundary_package_declares_is_an_error() {
        let lock = lock();
        let error = root_packages(&lock, &["deepref-ai"], &[("syn", "")])
            .expect_err("syn is not a direct dependency of deepref-ai");
        assert!(error.to_string().contains("not a direct dependency"));
    }

    #[test]
    fn a_dependency_with_no_matching_package_is_an_error() {
        let lock = lock();
        let missing = LockDependency {
            name: "missing".to_owned(),
            version: None,
            source: None,
        };
        assert!(resolve_entry(&lock, &missing).is_err());
    }

    #[test]
    fn declaration_lines_keep_roots_features_and_root_tables_only() {
        let manifest = r#"[package]
name = "deepref-ai"
version.workspace = true

[dependencies]
serde = { workspace = true, features = ["derive"] }
tokio.workspace = true
serde_json.workspace = true

[dev-dependencies.schemars]
version = "1"
features = ["uuid1"]

[dependencies.uuid]
workspace = true

[features]
default = ["serde/std"]

[lints]
workspace = true
"#;
        let roots: BTreeSet<&str> = ["serde", "serde_json", "schemars", "uuid"]
            .into_iter()
            .collect();
        let lines = declaration_lines_of("crates/ai/Cargo.toml", manifest, &roots);
        let expected = [
            "crates/ai/Cargo.toml | serde = { workspace = true, features = [\"derive\"] }",
            "crates/ai/Cargo.toml | serde_json.workspace = true",
            "crates/ai/Cargo.toml | [dev-dependencies.schemars]",
            "crates/ai/Cargo.toml | version = \"1\"",
            "crates/ai/Cargo.toml | features = [\"uuid1\"]",
            "crates/ai/Cargo.toml | [dependencies.uuid]",
            "crates/ai/Cargo.toml | workspace = true",
            "crates/ai/Cargo.toml | default = [\"serde/std\"]",
        ];
        assert_eq!(lines, expected);
    }

    #[test]
    fn multi_line_root_declarations_keep_their_continuation_lines() {
        let manifest = "[workspace.dependencies]\nserde = { version = \"1\", features = [\n  \"derive\",\n] }\nother = \"1\"\n";
        let roots: BTreeSet<&str> = ["serde"].into_iter().collect();
        let lines = declaration_lines_of("Cargo.toml", manifest, &roots);
        assert_eq!(
            lines,
            vec![
                "Cargo.toml | serde = { version = \"1\", features = [".to_owned(),
                "Cargo.toml | \"derive\",".to_owned(),
                "Cargo.toml | ] }".to_owned(),
            ]
        );
    }

    #[test]
    fn the_closure_listing_and_dependency_digest_are_deterministic() {
        let workspace = TempWorkspace::new("deps");
        workspace.write("Cargo.lock", LOCK);
        workspace.write(
            "Cargo.toml",
            "[workspace.dependencies]\nserde = \"1\"\nunrelated = \"1\"\n",
        );
        workspace.write(
            "crates/ai/Cargo.toml",
            "[dependencies]\nserde.workspace = true\n",
        );
        let packages = ["deepref-ai"];
        let roots = [("serde", "test root"), ("uuid", "test root")];
        let first = dependency_digest(&workspace.path, &packages, &roots).expect("digest");
        let second = dependency_digest(&workspace.path, &packages, &roots).expect("digest");
        assert_eq!(first.sha, second.sha);
        assert!(
            first
                .report
                .contains("[closure] name version source checksum")
        );
        assert!(first.report.contains("serde_derive 1.0.228"));
        assert!(!first.report.contains("unrelated 9.9.9"));
        assert!(first.report.contains("Cargo.toml | serde = \"1\""));
    }

    #[test]
    fn a_manifest_change_that_touches_no_root_leaves_the_dependency_digest_alone() {
        let workspace = TempWorkspace::new("unrelated-manifest");
        workspace.write("Cargo.lock", LOCK);
        workspace.write("Cargo.toml", "[workspace.dependencies]\nserde = \"1\"\n");
        let packages = ["deepref-ai"];
        let roots = [("serde", "test root")];
        let digest = |workspace: &TempWorkspace| {
            dependency_digest(&workspace.path, &packages, &roots)
                .expect("digest")
                .sha
        };
        let before = digest(&workspace);
        workspace.write(
            "Cargo.toml",
            "[workspace.dependencies]\nserde = \"1\"\n\n[workspace.lints.rust]\nunsafe_code = \"forbid\"\n",
        );
        assert_eq!(before, digest(&workspace));

        workspace.write(
            "Cargo.toml",
            "[workspace.dependencies]\nserde = { version = \"1\", features = [\"rc\"] }\n",
        );
        assert_ne!(
            before,
            digest(&workspace),
            "a feature on a root is a semantic change"
        );
    }

    #[test]
    fn test_only_items_are_removed_and_everything_else_is_kept() {
        let source = "pub fn real() -> u8 {\n    1\n}\n\n#[cfg(test)]\nmod tests {\n    #[test]\n    fn works() {\n        assert_eq!(super::real(), 1);\n    }\n}\n\npub fn after() {}\n";
        let stripped = remove_test_only_items(source).expect("a test module is removed");
        assert_eq!(
            stripped,
            "pub fn real() -> u8 {\n    1\n}\n\n\npub fn after() {}\n"
        );
    }

    #[test]
    fn single_line_test_declarations_are_removed_even_with_attributes_between() {
        let source =
            "mod real;\n#[cfg(test)]\n#[path = \"real_tests.rs\"]\nmod tests;\npub fn after() {}\n";
        assert_eq!(
            remove_test_only_items(source).expect("removed"),
            "mod real;\npub fn after() {}\n"
        );
    }

    #[test]
    fn a_block_that_dedents_before_it_closes_is_kept_unchanged() {
        // The closing brace is not at the attribute's indentation, so the item
        // boundary is unclear and the text must be kept.
        let source = "    #[cfg(test)]\n    impl Odd {\n    fn leak() {}\n}\n";
        assert_eq!(remove_test_only_items(source), None);
    }

    #[test]
    fn unrelated_source_is_returned_unchanged_so_its_bytes_are_hashed_as_written() {
        assert_eq!(remove_test_only_items("fn a() {}\n// comment\n"), None);
        // A cfg(test) on a field or a non-block item we cannot bound is kept.
        assert_eq!(
            remove_test_only_items("#[cfg(test)]\nfn bounded_no_block()\n"),
            None
        );
    }

    #[test]
    fn set_digest_is_independent_of_insertion_order() {
        let mut forward = BTreeMap::new();
        forward.insert("a.rs".to_owned(), b"one".to_vec());
        forward.insert("b.rs".to_owned(), b"two".to_vec());
        let mut reverse = BTreeMap::new();
        reverse.insert("b.rs".to_owned(), b"two".to_vec());
        reverse.insert("a.rs".to_owned(), b"one".to_vec());
        assert_eq!(file_set_digest(&forward), file_set_digest(&reverse));
    }

    #[test]
    fn set_digest_is_sensitive_to_content_and_path() {
        let mut base = BTreeMap::new();
        base.insert("crates/review/src/a.rs".to_owned(), b"fn a() {}".to_vec());
        let original = file_set_digest(&base);

        let mut content = base.clone();
        content.insert(
            "crates/review/src/a.rs".to_owned(),
            b"fn a() { 1; }".to_vec(),
        );
        assert_ne!(original, file_set_digest(&content));

        let mut renamed = BTreeMap::new();
        renamed.insert("crates/review/src/b.rs".to_owned(), b"fn a() {}".to_vec());
        assert_ne!(original, file_set_digest(&renamed));

        // The NUL separators keep a path/content split from colliding with another split.
        let mut shifted = BTreeMap::new();
        shifted.insert("crates/review/src/a.rsfn".to_owned(), b" a() {}".to_vec());
        assert_ne!(original, file_set_digest(&shifted));
    }

    /// A synthetic workspace with one placeholder file for every required path
    /// of the real table. Tests add, change or remove files on top of it.
    struct TempWorkspace {
        path: PathBuf,
    }

    impl TempWorkspace {
        fn new(label: &str) -> Self {
            static COUNTER: AtomicUsize = AtomicUsize::new(0);
            let path = std::env::temp_dir().join(format!(
                "deepref-fingerprint-{label}-{}-{}",
                std::process::id(),
                COUNTER.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir_all(&path).expect("temp workspace");
            Self { path }
        }

        fn write(&self, relative: &str, contents: &str) {
            let target = self.path.join(relative);
            fs::create_dir_all(target.parent().expect("parent")).expect("parent dir");
            fs::write(target, contents).expect("write fixture file");
        }

        fn remove(&self, relative: &str) {
            fs::remove_file(self.path.join(relative)).expect("remove fixture file");
        }

        /// Writes a placeholder for every required entry of the real boundary table.
        fn with_required_boundary(label: &str) -> Self {
            let workspace = Self::new(label);
            for entry in IMPLEMENTATION_BOUNDARY {
                if entry.presence == Presence::Required && entry.kind == Kind::File {
                    workspace.write(entry.path, &format!("// placeholder for {}\n", entry.path));
                }
            }
            for entry in IMPLEMENTATION_BOUNDARY {
                if entry.presence == Presence::Required && entry.kind == Kind::Directory {
                    workspace.write(
                        &format!("{}/placeholder.rs", entry.path),
                        "pub fn placeholder() {}\n",
                    );
                }
            }
            workspace.write("review-definitions/screening/v1/prompt.txt", "prompt\n");
            workspace.write("crates/review/src/lib.rs", "pub fn lib() {}\n");
            workspace.write("crates/review/src/memory.rs", "pub fn memory() {}\n");
            workspace.write("crates/review/src/definition_tests.rs", "fn t() {}\n");
            workspace.write("crates/ai/src/lib.rs", "pub fn ai() {}\n");
            workspace.write("crates/ai/src/chat.rs", "pub fn chat() {}\n");
            workspace.write("crates/ai/src/tests.rs", "fn t() {}\n");
            workspace.write("crates/ai/src/evals/gate.rs", "pub fn gate() {}\n");
            workspace.write("crates/domain/src/lib.rs", "pub fn domain() {}\n");
            workspace
        }
    }

    impl Drop for TempWorkspace {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.path);
        }
    }

    fn implementation_digest(workspace: &TempWorkspace) -> String {
        implementation_boundary(&workspace.path, IMPLEMENTATION_BOUNDARY, EXCLUDED_PATHS)
            .expect("boundary digest")
            .sha
    }

    #[test]
    fn excluded_paths_do_not_affect_the_implementation_digest() {
        let workspace = TempWorkspace::with_required_boundary("excluded");
        let before = implementation_digest(&workspace);

        workspace.write("crates/http-api/src/routes/ai.rs", "pub fn route() {}\n");
        workspace.write("crates/graph/src/lib.rs", "pub fn graph() {}\n");
        workspace.write("apps/server/src/main.rs", "fn main() {}\n");
        workspace.write(
            "crates/review/src/memory.rs",
            "pub fn memory_changed() {}\n",
        );
        workspace.write(
            "crates/review/src/definition_tests.rs",
            "fn t_changed() {}\n",
        );
        workspace.write("crates/ai/src/chat.rs", "pub fn chat_changed() {}\n");
        workspace.write("crates/ai/src/tests.rs", "fn t_changed() {}\n");
        workspace.write("crates/ai/src/evals/gate.rs", "pub fn gate_changed() {}\n");
        workspace.write("crates/ai/src/usage.rs", "pub fn usage() {}\n");

        assert_eq!(before, implementation_digest(&workspace));
    }

    #[test]
    fn included_paths_change_the_implementation_digest() {
        let workspace = TempWorkspace::with_required_boundary("included");
        let before = implementation_digest(&workspace);

        workspace.write("crates/ai/src/lib.rs", "pub fn ai_changed() {}\n");
        let after_content = implementation_digest(&workspace);
        assert_ne!(before, after_content);

        // A new file in an included directory is part of the boundary.
        workspace.write("crates/ai/src/endpoint.rs", "pub fn endpoint() {}\n");
        let after_new_file = implementation_digest(&workspace);
        assert_ne!(after_content, after_new_file);

        workspace.write("review-definitions/screening/v1/schema.json", "{}\n");
        assert_ne!(after_new_file, implementation_digest(&workspace));
    }

    #[test]
    fn test_only_module_edits_do_not_change_the_implementation_digest() {
        let workspace = TempWorkspace::with_required_boundary("cfg-test");
        workspace.write(
            "crates/ai/src/lib.rs",
            "pub fn ai() -> u8 {\n    1\n}\n\n#[cfg(test)]\nmod tests {\n    #[test]\n    fn a() {}\n}\n",
        );
        let before = implementation_digest(&workspace);

        workspace.write(
            "crates/ai/src/lib.rs",
            "pub fn ai() -> u8 {\n    1\n}\n\n#[cfg(test)]\nmod tests {\n    #[test]\n    fn a() {}\n\n    #[test]\n    fn b() {}\n}\n",
        );
        assert_eq!(
            before,
            implementation_digest(&workspace),
            "a new test is not semantic"
        );

        workspace.write(
            "crates/ai/src/lib.rs",
            "pub fn ai() -> u8 {\n    2\n}\n\n#[cfg(test)]\nmod tests {\n    #[test]\n    fn a() {}\n}\n",
        );
        assert_ne!(
            before,
            implementation_digest(&workspace),
            "production code is semantic"
        );
    }

    #[test]
    fn the_digest_does_not_depend_on_where_the_workspace_lives_or_creation_order() {
        let first = TempWorkspace::with_required_boundary("location-a");
        let second = TempWorkspace::new("location-b");
        // Create the same files in the reverse order.
        second.write(
            "services/worker/src/workflows/nodes.rs",
            "// placeholder for services/worker/src/workflows/nodes.rs\n",
        );
        for entry in IMPLEMENTATION_BOUNDARY.iter().rev() {
            if entry.presence == Presence::Required && entry.kind == Kind::File {
                second.write(entry.path, &format!("// placeholder for {}\n", entry.path));
            }
        }
        for entry in IMPLEMENTATION_BOUNDARY.iter().rev() {
            if entry.presence == Presence::Required && entry.kind == Kind::Directory {
                second.write(
                    &format!("{}/placeholder.rs", entry.path),
                    "pub fn placeholder() {}\n",
                );
            }
        }
        second.write("crates/domain/src/lib.rs", "pub fn domain() {}\n");
        second.write("crates/ai/src/evals/gate.rs", "pub fn gate() {}\n");
        second.write("crates/ai/src/tests.rs", "fn t() {}\n");
        second.write("crates/ai/src/chat.rs", "pub fn chat() {}\n");
        second.write("crates/ai/src/lib.rs", "pub fn ai() {}\n");
        second.write("crates/review/src/definition_tests.rs", "fn t() {}\n");
        second.write("crates/review/src/memory.rs", "pub fn memory() {}\n");
        second.write("crates/review/src/lib.rs", "pub fn lib() {}\n");
        second.write("review-definitions/screening/v1/prompt.txt", "prompt\n");
        assert_eq!(
            implementation_digest(&first),
            implementation_digest(&second)
        );
    }

    #[test]
    fn a_missing_required_path_fails_with_a_clear_message() {
        let workspace = TempWorkspace::with_required_boundary("missing-required");
        workspace.remove("services/worker/src/processor.rs");
        let error =
            implementation_boundary(&workspace.path, IMPLEMENTATION_BOUNDARY, EXCLUDED_PATHS)
                .expect_err("a missing required file must fail");
        let message = error.to_string();
        assert!(
            message.contains("services/worker/src/processor.rs"),
            "{message}"
        );
        assert!(message.contains("boundary table"), "{message}");
    }

    #[test]
    fn a_missing_optional_path_is_recorded_as_absent_and_its_parent_is_watched() {
        let workspace = TempWorkspace::with_required_boundary("optional-absent");
        let digest =
            implementation_boundary(&workspace.path, IMPLEMENTATION_BOUNDARY, EXCLUDED_PATHS)
                .expect("optional absence is not an error");
        let absent_section = digest
            .report
            .split_once("[absent]\n")
            .and_then(|(_, rest)| rest.split_once("[files]\n"))
            .map(|(absent, _)| absent)
            .expect("the listing has an absent section");
        assert!(
            absent_section.contains("crates/review/golden\n"),
            "{absent_section}"
        );
        assert!(
            absent_section.contains("crates/review/src/golden.rs\n"),
            "{absent_section}"
        );
        assert!(
            absent_section.contains("crates/ai/src/endpoint.rs\n"),
            "{absent_section}"
        );
        // Creating an absent path must rerun the build script: its parent is watched.
        assert!(digest.watched.contains("crates/review"));
        assert!(digest.watched.contains("crates/ai/src"));
    }

    #[test]
    fn boundary_table_entries_are_well_formed() {
        // Every entry must be shaped as a workspace-relative path with no
        // leading slash, no trailing slash and no parent-directory segments.
        for entry in IMPLEMENTATION_BOUNDARY {
            assert!(!entry.path.starts_with('/'), "{}", entry.path);
            assert!(!entry.path.ends_with('/'), "{}", entry.path);
            assert!(!entry.path.contains(".."), "{}", entry.path);
            assert!(!entry.reason.is_empty(), "{} needs a reason", entry.path);
        }
        for exclusion in EXCLUDED_PATHS {
            assert!(
                !exclusion.reason.is_empty(),
                "{} needs a reason",
                exclusion.path
            );
        }
    }

    #[test]
    fn the_file_listing_hashes_normalised_contents_and_marks_absent_paths() {
        let workspace = TempWorkspace::with_required_boundary("listing");
        workspace.write(
            "crates/ai/src/types.rs",
            "pub fn t() {}\n#[cfg(test)]\nmod tests {\n    fn x() {}\n}\n",
        );
        let digest =
            implementation_boundary(&workspace.path, IMPLEMENTATION_BOUNDARY, EXCLUDED_PATHS)
                .expect("digest");
        let expected_hash = hex(&Sha256::digest(b"pub fn t() {}\n"));
        assert!(
            digest
                .report
                .contains(&format!("{expected_hash} crates/ai/src/types.rs")),
            "the test module must be absent from the hashed contents:\n{}",
            digest.report
        );
    }

    /// A lockfile in which every boundary package depends on every root, and
    /// every root is a registry package. It is complete for the real tables.
    fn synthetic_lock() -> String {
        let roots: Vec<&str> = SEMANTIC_DEPENDENCY_ROOTS
            .iter()
            .map(|(name, _)| *name)
            .collect();
        let quoted = roots
            .iter()
            .map(|name| format!("\"{name}\""))
            .collect::<Vec<_>>()
            .join(", ");
        let mut text = String::from("version = 4\n\n");
        for package in BOUNDARY_PACKAGES {
            text.push_str(&format!(
                "[[package]]\nname = \"{package}\"\nversion = \"0.1.0\"\ndependencies = [{quoted}]\n\n"
            ));
        }
        for root in roots {
            text.push_str(&format!(
                "[[package]]\nname = \"{root}\"\nversion = \"1.0.0\"\nsource = \"{REGISTRY}\"\nchecksum = \"abc\"\n\n"
            ));
        }
        text
    }

    #[test]
    fn compute_writes_both_fingerprints_from_a_complete_synthetic_workspace() {
        let workspace = TempWorkspace::with_required_boundary("compute");
        workspace.write("Cargo.lock", &synthetic_lock());
        workspace.write("Cargo.toml", "[workspace.dependencies]\nserde = \"1\"\n");
        let fingerprints = compute(&workspace.path).expect("a complete workspace fingerprints");
        assert_eq!(
            fingerprints.implementation_sha,
            implementation_digest(&workspace)
        );
        assert_eq!(fingerprints.dependency_sha.len(), 64);
        assert!(fingerprints.dependency_report.contains("serde 1.0.0"));
        assert!(fingerprints.watched.contains("Cargo.lock"));
        assert!(fingerprints.watched.contains("review-definitions"));
    }

    #[test]
    fn a_lockfile_missing_a_boundary_package_fails_the_dependency_half() {
        let workspace = TempWorkspace::with_required_boundary("compute-missing");
        workspace.write("Cargo.lock", LOCK);
        workspace.write("Cargo.toml", "[workspace]\n");
        let error = compute(&workspace.path).expect_err("LOCK has no deepref-review");
        assert!(error.to_string().contains("deepref-review"), "{error}");
    }
}
