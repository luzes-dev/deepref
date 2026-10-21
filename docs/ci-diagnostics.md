# CI diagnostics

DeepRef CI uses a **probe → gate** model so one run exposes every failure that is
still meaningful from the current repository state.

- **Probe** — one independently useful command from
  [`scripts/ci/diagnostics.toml`](../scripts/ci/diagnostics.toml): lint,
  type-check, Clippy, SQLx validation, tests, build, scanner, and so on.
- **Gate** — the final lane decision. It runs after all reachable probes, writes
  the report, then exits non-zero if any probe failed or was unexpectedly
  unavailable.
- **Blocked** — a probe that cannot produce useful output because a prerequisite
  did not pass. Blocked is not another failure and does not run the command.

The runner is [`scripts/ci/diagnose.py`](../scripts/ci/diagnose.py). It uses only
the Python standard library (Python 3.11+ for `tomllib`), does not import Cargo
metadata or `xtask`, and can run when the Rust workspace does not compile.

## Commands

```bash
# One probe (dependencies are checked against earlier results)
python3 scripts/ci/diagnose.py run-check web.lint

# All probes in one or more groups
python3 scripts/ci/diagnose.py run web
python3 scripts/ci/diagnose.py run rust integration

# Final lane gate: mark probes that never ran as blocked and fail if not green
python3 scripts/ci/diagnose.py gate integration

# Rerun failed probes plus blocked descendants that are now reachable
python3 scripts/ci/diagnose.py rerun-failed target/ci-diagnostics/report.json

# Merge lane reports/artifact directories (aggregate observability)
python3 scripts/ci/diagnose.py merge diagnostics-artifacts
```

Groups are `rust`, `web`, `fallow`, `codegen`, `integration`, `e2e`,
`security-dependencies`, `security-policy`, `security-sast`,
`security-secrets`, and `security-configuration`. Running the runner without a
group selects all of them.

Local shortcuts use the same runner:

```bash
just diagnose rust web
just diagnose integration
just diagnose-failed
just diagnose-full
```

`diagnose-failed` keeps passed probes untouched. A blocked probe is selected only
when every dependency is already passed or is itself being rerun. `diagnose-full`
is the final confidence pass before handoff.

## Report layout

Everything is written under the ignored working directory
`target/ci-diagnostics/` unless `--output` says otherwise:

```text
target/ci-diagnostics/
├── checks/<probe>.json   # small public result
├── logs/<probe>.log      # complete stdout/stderr, never truncated
├── report.json
└── report.md
```

A result has at least `id`, `group`, `status`, `exit_code`, `duration_ms`, and
`log`. Status is one of `passed`, `failed`, or `blocked`. Infrastructure errors
(such as a missing executable) are reported as `failed` with extra `error`
metadata; they are never silently skipped.

`report.md` contains a compact probe table, bounded excerpts for failed probes,
the full-log path beside each excerpt, and short blocked-probe reasons. ANSI
sequences and obvious nondeterministic noise are removed from excerpts only.
Identical location-bearing diagnostics are deduplicated conservatively; the
runner does not guess root causes.

## Blocked semantics

When a prerequisite fails, dependent commands are not executed:

```text
PASS     integration.migrations
FAIL     integration.sqlx
FAIL     integration.tests

FAIL     integration.migrations
BLOCKED  integration.sqlx
BLOCKED  integration.tests
```

The same rule applies to `codegen.clean-tree` after `codegen.api` and to
`e2e.browser` after `e2e.build`. Setup failures (checkout, toolchain, dependencies,
database, browsers) remain fail-fast; the gate marks probes that never ran as
blocked.

## GitHub Actions

Each lane runs independent probes with `continue-on-error: true`, then a gate
step with `if: always()` restores strict semantics. Lanes upload
`ci-diagnostics-*` artifacts. The non-required `CI diagnostics` job in
[`ci.yml`](../.github/workflows/ci.yml) downloads them, merges all lane results,
and writes the combined table to the workflow summary.

The Security workflow follows the same model inside its dependency, repository
policy, scanner, and configuration jobs. The external required checks remain
authoritative and unchanged: `Rust`, `Web`, `Fallow`, `OpenAPI and Orval drift`,
`E2E`, `Docker`, and `Security`.

## Adding or changing a probe

Edit `scripts/ci/diagnostics.toml`, then call the probe from a workflow with
`run-check <id>`. Commands are argv arrays; use a shell only when the command
genuinely needs shell semantics. Environment bootstrap belongs in composite
actions or lane setup steps, not in the manifest.

The manifest also owns dependency edges (`depends_on`), so a new blocked
relationship does not need separate logic in local tooling and CI.
