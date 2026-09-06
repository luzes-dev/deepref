# Default target: print available recipes.
default:
    just --list

# Tooling versions and common flags.
compose := "docker compose -f infra/local/compose.yaml"
dev_env := "set -a; [ -f .env ] && . ./.env; set +a"

# Start the full local environment with process-compose.
up:
    process-compose up

# Start background infrastructure dependencies (Postgres, NATS, Neo4j, MinIO).
infra-up:
    {{compose}} up -d --wait

# Stop background infrastructure dependencies and remove containers.
infra-down:
    {{compose}} down

# Run all code quality and architecture verification checks.
verify:
    cargo xtask boundaries
    cargo fmt --all -- --check
    cargo clippy --workspace --all-targets --locked -- -D warnings
    cargo shear --deny-warnings
    pnpm run lint
    pnpm run check
    taplo fmt --check .mise.toml
    actionlint
    mapfile -t shell_scripts < <(git ls-files '*.sh'); if ((${#shell_scripts[@]})); then shellcheck "${shell_scripts[@]}"; fi
    docker compose -f infra/local/compose.yaml config --quiet
    process-compose -f process-compose.yaml --dry-run
    bash infra/tests/static-contracts.sh
    bash scripts/check-docs.sh

# Run the full unit test suite for Rust and TypeScript.
test-unit:
    cargo nextest run --workspace --lib --bins --locked
    cargo test --doc --workspace --locked
    pnpm --filter @deepref/web test:unit -- --run

# Run Criterion microbenchmarks across algorithmic workspace crates.
bench *ARGS:
    SQLX_OFFLINE=true cargo bench --workspace {{ARGS}}

# Run property tests for Rust and TypeScript with high iteration counts.
test-property:
    cargo nextest run --workspace --lib --bins --locked -E 'test(/::property_tests::|::tests::.*_property|::tests::.*_invariants)/'
    pnpm --filter @deepref/web test:unit -- --run --testNamePattern='property'

# Run Rust mutation testing (cargo-mutants) across core domain and algorithmic crates.
test-mutants *ARGS:
    SQLX_OFFLINE=true cargo mutants {{ARGS}}

# Run time-bounded cargo-fuzz hostile-input fuzzing on nightly toolchain.
test-fuzz TARGET DURATION="30":
    cargo +nightly fuzz run {{TARGET}} -- -max_total_time={{DURATION}}

# Run TypeScript tests with coverage reporting.
test-coverage-ts:
    pnpm --filter @deepref/web test:unit:coverage

# Run Rust tests with source-based coverage and generate LCOV and summary reports.
test-coverage-rust:
    cargo llvm-cov nextest --workspace --lib --bins --locked --lcov --output-path target/llvm-cov/lcov.info
    cargo llvm-cov report --workspace --summary-only

# Run both TypeScript and Rust coverage suites and output reports.
test-coverage: test-coverage-ts test-coverage-rust

# Report uncovered complexity from an existing LCOV file without rerunning tests.
risk-rust LCOV="target/llvm-cov/lcov.info":
    cargo crap --workspace --lcov {{quote(LCOV)}} --exclude '**/tests/**' --exclude '**/benches/**' --exclude '**/generated/**' --exclude '**/tools/xtask/**'

# Start dependencies and run integration test targets.
test-integration:
    {{compose}} up -d --wait
    cargo run -q -p deepref-server -- migrate
    cargo nextest run --workspace --tests --locked

# Run browser tests; Playwright builds and starts its own preview server.
test-e2e:
    pnpm --filter @deepref/web test:e2e

# Regenerate the OpenAPI document and Orval client.
codegen:
    cargo xtask generate

# Prove committed generated output is current without modifying the worktree.
codegen-check:
    cargo xtask generate --check

# Run Rust dependency/source hygiene check with cargo-shear.
shear:
    cargo shear --deny-warnings

# Run Rust code duplication analysis with jscpd (report-only).
duplication-rust:
    pnpm run quality:rust:duplication

# Scaffold a new crate with workspace inheritance and architectural layer classification.
new-crate LAYER NAME:
    cargo xtask new-crate --layer {{LAYER}} {{NAME}}

# Fast diagnosis of repository setup, metadata, and invariant health.
doctor:
    cargo xtask doctor

# Build the three immutable application image targets locally.
docker-build:
    docker buildx bake api worker web

# Lint, render, schema-check, and policy-check the Helm chart.
helm-check:
    bash scripts/helm-check.sh

# Format every OpenTofu root and module.
infra-fmt:
    tofu fmt -recursive infra

# Validate formatting, lint, initialization, configuration, and native tests for every root.
infra-validate:
    tofu fmt -check -recursive infra
    tflint --recursive --chdir=infra
    for kind in bootstrap environments; do for root in development staging production global; do directory="infra/$kind/$root"; tofu -chdir="$directory" init -backend=false -lockfile=readonly; tofu -chdir="$directory" validate; tofu -chdir="$directory" test; done; done

# Create a speculative plan for development, staging, production, or global.
infra-plan ENV:
    env_name="{{ENV}}"; case "$env_name" in development|staging|production|global) ;; *) echo "ENV must be development, staging, production, or global" >&2; exit 2 ;; esac; tofu -chdir="infra/environments/$env_name" init -lockfile=readonly; tofu -chdir="infra/environments/$env_name" plan -input=false -lock-timeout=5m -no-color

# Update development with a fast-forward-only pull and create feature/SLUG.
feature SLUG:
    slug="{{SLUG}}"; [[ "$slug" =~ ^[a-z0-9][a-z0-9._-]*$ ]] || { echo "SLUG must contain lowercase letters, digits, dots, underscores, or hyphens" >&2; exit 2; }; [[ -z "$(git status --porcelain)" ]] || { echo "feature requires a clean worktree" >&2; exit 1; }; git fetch origin development; git switch development; git pull --ff-only origin development; git switch -c "feature/$slug"

# Validate every workspace dependency against the architecture contract.
architecture:
    cargo xtask boundaries

# Regenerate SQLx offline query cache (.sqlx/) against a running database.
sqlx-prepare:
    cargo xtask sqlx prepare

# Verify committed SQLx offline metadata matches database and queries.
sqlx-check:
    cargo xtask sqlx check
