# Contributing

## Branch Workflow

The repository uses a single-branch workflow targeting `main`.

Create feature work from `main`:

```bash
git fetch origin
git checkout main
git pull --ff-only
git checkout -b feature/<short-description>
```

Open pull requests into `main`.

## Setup

```bash
mise trust
mise install
mise exec -- just bootstrap
```

## Checks

```bash
mise exec -- just verify
mise exec -- just test-unit
```

Run Fallow before larger TypeScript changes:

```bash
mise exec -- pnpm run quality:ts
```

## CI/CD

GitHub Actions runs Rust, web, E2E, TypeScript quality, Docker build, and security checks on pull requests to `main`.

`ci.yml` is the pipeline map; reusable `ci-{rust,web,codegen,integration,e2e,containers}.yml` workflows own lane execution. Composite `setup-rust` and `setup-node` actions share environment mechanics, while `scripts/ci` owns cross-stack policy. The required check names remain `Rust`, `Web`, `Fallow`, `OpenAPI and Orval drift`, `E2E`, `Docker`, and `Security`.

UI-only changes run Web checks and `@deepref/ui` tests. Container PR checks remain limited to build-definition and shared toolchain/lockfile changes; push-to-main builds both native architectures with cache-only output.

`security.yml` uses Semgrep CE 1.181.0 (digest-pinned image), OSV-Scanner 2.7.0, Gitleaks CLI 8.30.1, and Trivy 0.76.0 (checksum-verified binaries), without CodeQL, SARIF upload, or paid services. Semgrep anonymously fetches the mutable `p/rust`, `p/typescript`, and `p/javascript` community packs. OSV recursively scans source lockfiles, including root/fuzz Cargo and pnpm; Gitleaks scans all Git history with redaction; Trivy scans Dockerfiles and supported IaC without rebuilding images. Compose and Bake retain their separate configuration validation.

Accepted dependency risks are recorded in `osv-scanner.toml` and `deny.toml`: unmaintained `paste 1.0.15` and the `rsa 0.9.10` private-key timing issue (production only verifies public-key signatures). Gitleaks exceptions in `.gitleaks.toml` match verified fixture values and paths; rule-specific Semgrep comments explain false positives at the affected call sites.

Weekly scheduled lanes run Rust and TypeScript mutation testing, hostile-input fuzzing, Criterion benchmarks, and security checks. Failures remain failures (mutation commands do not suppress their exit status), reports are uploaded where available, and failed scheduled runs open an issue linking to the run. Manual dispatches do not create scheduled-failure issues.

The fuzz package is a standalone workspace with its own lockfile. Criterion uses `cargo bench --workspace --bench '*' --locked -- --save-baseline main`: the named-target glob avoids library/binary libtest harnesses, which `--benches` would also select and which reject Criterion flags.

Container publishing requires the latest push run of `ci.yml` for the exact checked-out commit to have completed successfully. A tag whose commit has no successful push CI run is rejected before registry login or image building. Dispatch image tags are passed as environment variables and validated as Docker tags, not evaluated as shell code.

Stable `vMAJOR.MINOR.PATCH` releases publish the version tag, tree-hash tag, and `latest`. Pre-release tags, custom dispatch tags, and SHA-based publications do not advance `latest`; the dispatch tag `latest` is reserved and rejected. Local Bake builds can still request `TAG=latest` explicitly, or opt into a rolling tag with `PUBLISH_LATEST=true`.

Local nested worktrees and agent metadata (`.worktrees/` and `.claude/`) are excluded from Docker build contexts.
