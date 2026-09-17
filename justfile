set dotenv-load
set export
set shell := ["bash", "-euo", "pipefail", "-c"]

export SQLX_OFFLINE := env_var_or_default("SQLX_OFFLINE", "true")

# Internal shared flags
compose := "docker compose -f compose.yaml"
process_compose_port := env_var_or_default("PROCESS_COMPOSE_PORT", "8090")
process_compose := "process-compose --port " + process_compose_port + " -f process-compose.yaml"

[private]
default:
    @just --list

# ==============================================================================
# Setup & Lifecycle
# ==============================================================================

# First-time machine bootstrap: creates .env, installs dependencies & browsers
[group('Setup & Lifecycle')]
bootstrap:
    test -f .env || cp .env.example .env
    pnpm install --frozen-lockfile
    cargo fetch --locked
    pnpm --filter @deepref/web exec playwright install chromium
    {{ compose }} pull

# Fast diagnosis of local toolchain, metadata, and repository health
[group('Setup & Lifecycle')]
doctor:
    cargo xtask doctor

# Start full stack: boots PostgreSQL, applies migrations, and launches process-compose
[group('Setup & Lifecycle')]
up *ARGS:
    {{ compose }} up -d --wait
    cargo run -q -p deepref-server -- migrate
    {{ process_compose }} up {{ ARGS }}

alias dev := up

# Stop application processes and background containers
[group('Setup & Lifecycle')]
down:
    {{ process_compose }} down >/dev/null 2>&1 || true
    {{ compose }} down

alias dev-down := down

# Stop local stack and permanently remove PostgreSQL volumes
[group('Setup & Lifecycle')]
dev-reset:
    {{ process_compose }} down >/dev/null 2>&1 || true
    {{ compose }} down -v --remove-orphans

# ==============================================================================
# Development
# ==============================================================================

# Start only the web frontend dev server (Vite/SvelteKit)
[group('Development')]
dev-web:
    pnpm --filter @deepref/web dev

# Start Storybook component explorer
[group('Development')]
storybook:
    pnpm --filter @deepref/ui storybook

# Build static Storybook documentation
[group('Development')]
storybook-build:
    pnpm --filter @deepref/ui build-storybook

# Start background infrastructure (PostgreSQL) only
[group('Development')]
infra-up:
    {{ compose }} up -d --wait

# Stop background infrastructure (PostgreSQL)
[group('Development')]
infra-down:
    {{ compose }} down

# ==============================================================================
# Quality & Formatting
# ==============================================================================

# Run all code quality, linting, and architectural verification checks
[group('Quality & Formatting')]
verify:
    cargo xtask boundaries
    sentrux check .
    sentrux gate .
    cargo fmt --all -- --check
    cargo clippy --workspace --all-targets --locked -- -D warnings
    cargo shear --deny-warnings
    pnpm run lint
    pnpm run check
    taplo fmt --check .mise.toml
    actionlint
    mapfile -t shell_scripts < <(git ls-files '*.sh'); if ((${#shell_scripts[@]})); then shellcheck "${shell_scripts[@]}"; fi
    docker compose -f compose.yaml config --quiet
    process-compose -f process-compose.yaml --dry-run
    bash scripts/check-docs.sh

alias check := verify

# Auto-format Rust, TypeScript, Svelte, and Mise configs across repository
[group('Quality & Formatting')]
fmt:
    cargo fmt --all
    pnpm run format
    taplo fmt .mise.toml

# Run Clippy and ESLint across the repository
[group('Quality & Formatting')]
lint:
    cargo clippy --workspace --all-targets --locked -- -D warnings
    pnpm run lint

# Validate workspace package boundary integrity and Sentrux structural rules
[group('Quality & Formatting')]
architecture:
    cargo xtask boundaries
    sentrux check .
    sentrux gate .

# Run Sentrux architectural rules and regression checks
[group('Quality & Formatting')]
sentrux:
    sentrux check .
    sentrux gate .

# Save the current structural metrics as the new Sentrux baseline
[group('Quality & Formatting')]
sentrux-save:
    sentrux gate --save .

# Check for unused Rust workspace dependencies
[group('Quality & Formatting')]
shear:
    cargo shear --deny-warnings

# Run Rust code duplication analysis (report only)
[group('Quality & Formatting')]
duplication-rust:
    pnpm run quality:rust:duplication

# ==============================================================================
# Testing
# ==============================================================================

# Run the primary unit test suite for Rust and TypeScript
[group('Testing')]
test-unit:
    cargo nextest run --workspace --lib --bins --locked
    cargo test --doc --workspace --locked
    pnpm --filter @deepref/ui test
    pnpm --filter @deepref/web test:unit -- --run

alias test := test-unit

# Run database integration tests with disposable fixtures
[group('Testing')]
test-integration:
    {{ compose }} up -d --wait
    cargo run -q -p deepref-server -- migrate
    cargo nextest run --workspace --tests --locked

# Run Playwright end-to-end browser tests
[group('Testing')]
test-e2e:
    pnpm --filter @deepref/web test:e2e

# Run both TypeScript and Rust coverage suites and print summary
[group('Testing')]
test-coverage: test-coverage-ts test-coverage-rust

# Run TypeScript tests with Vitest coverage
[group('Testing')]
test-coverage-ts:
    pnpm --filter @deepref/web test:unit:coverage

# Run Rust tests with source-based coverage (LCOV + summary)
[group('Testing')]
test-coverage-rust:
    cargo llvm-cov nextest --workspace --lib --bins --locked --lcov --output-path target/llvm-cov/lcov.info
    cargo llvm-cov report --workspace --summary-only

# Run high-iteration property tests for Rust and TypeScript
[group('Testing')]
test-property:
    cargo nextest run --workspace --lib --bins --locked -E 'test(/::property_tests::|::tests::.*_property|::tests::.*_invariants)/'
    pnpm --filter @deepref/web test:unit -- --run --testNamePattern='property'

# Run Rust mutation tests with cargo-mutants
[group('Testing')]
test-mutants *ARGS:
    cargo mutants {{ ARGS }}

# Run time-bounded hostile-input fuzzing with cargo-fuzz (nightly)
[group('Testing')]
test-fuzz TARGET DURATION="30":
    cargo +nightly fuzz run {{ TARGET }} -- -max_total_time={{ DURATION }}

# Run Criterion microbenchmarks across algorithmic crates
[group('Testing')]
bench *ARGS:
    cargo bench --workspace {{ ARGS }}

# Report uncovered complexity from existing LCOV without rerunning tests
[group('Testing')]
risk-rust LCOV="target/llvm-cov/lcov.info":
    cargo crap --workspace --lcov {{ quote(LCOV) }} --exclude '**/tests/**' --exclude '**/benches/**' --exclude '**/generated/**' --exclude '**/tools/xtask/**'

# ==============================================================================
# Database & Codegen
# ==============================================================================

# Apply all pending PostgreSQL migrations
[group('Database & Codegen')]
migrate:
    {{ compose }} up -d --wait
    cargo run -q -p deepref-server -- migrate

# Seed database with deterministic local fixtures
[group('Database & Codegen')]
seed: migrate
    {{ compose }} exec -T postgres psql --username postgres --dbname deepref --set ON_ERROR_STOP=1 < scripts/seed.sql

# Seed fictional assistant chat threads for exploring the /assistant UI
[group('Database & Codegen')]
seed-assistant:
    {{ compose }} exec -T postgres psql --username postgres --dbname deepref --set ON_ERROR_STOP=1 < scripts/seed-assistant-chat.sql

# Regenerate OpenAPI specification and Orval TypeScript client
[group('Database & Codegen')]
codegen:
    cargo xtask generate

# Verify that generated OpenAPI & Orval code is up to date
[group('Database & Codegen')]
codegen-check:
    cargo xtask generate --check

# Regenerate SQLx offline query cache (.sqlx/) against running database
[group('Database & Codegen')]
sqlx-prepare:
    cargo xtask sqlx prepare

# Verify committed SQLx offline metadata matches database and queries
[group('Database & Codegen')]
sqlx-check:
    cargo xtask sqlx check

# ==============================================================================
# Workflow & Build
# ==============================================================================

# Scaffold a new crate with workspace inheritance and layer classification
[group('Workflow & Build')]
new-crate LAYER NAME:
    cargo xtask new-crate --layer {{ LAYER }} {{ NAME }}

# Build application container images locally (optional TAG="latest")
[group('Workflow & Build')]
docker-build TAG="latest":
    TAG={{ TAG }} docker buildx bake api worker web

# Fast-forward main and switch to a new feature/<SLUG> branch
[group('Workflow & Build')]
feature SLUG:
    slug="{{ SLUG }}"; [[ "$slug" =~ ^[a-z0-9][a-z0-9._-]*$ ]] || { echo "SLUG must contain lowercase letters, digits, dots, underscores, or hyphens" >&2; exit 2; }; [[ -z "$(git status --porcelain)" ]] || { echo "feature requires a clean worktree" >&2; exit 1; }; git fetch origin main; git switch main; git pull --ff-only origin main; git switch -c "feature/$slug"
