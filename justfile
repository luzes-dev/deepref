set dotenv-load := true
set shell := ["bash", "-euo", "pipefail", "-c"]

compose := "docker compose -f infra/local/compose.yaml"

default:
    @just --list

# Install repository dependencies and prefetch disposable dependency images.
bootstrap:
    test -f .env || cp .env.example .env
    pnpm install --frozen-lockfile
    cargo fetch --locked
    pnpm --filter @deepref/web exec playwright install chromium
    {{compose}} pull

# Start disposable dependencies, prepare their schemas, and supervise every app process.
dev:
    {{compose}} up -d --wait
    cargo run -q -p deepref-api -- migrate
    scripts/bootstrap-local-nats.sh
    exec process-compose -f process-compose.yaml up

# Stop app processes and disposable dependencies while retaining local data.
dev-down:
    process-compose -f process-compose.yaml down >/dev/null 2>&1 || true
    {{compose}} down --remove-orphans

# Stop the local stack and permanently delete its disposable named volumes.
dev-reset:
    process-compose -f process-compose.yaml down >/dev/null 2>&1 || true
    {{compose}} down --volumes --remove-orphans

# Apply all PostgreSQL migrations with the one-shot API command.
migrate:
    {{compose}} up -d --wait postgres
    cargo run -q -p deepref-api -- migrate

# Load the deterministic, idempotent local fixture set.
seed: migrate
    {{compose}} exec -T postgres psql --username postgres --dbname deepref --set ON_ERROR_STOP=1 < scripts/seed.sql

# Run fast repository checks without changing generated files.
verify:
    cargo fmt --all -- --check
    cargo clippy --workspace --all-targets --locked -- -D warnings
    pnpm run lint
    pnpm run check
    taplo fmt --check .mise.toml
    actionlint
    mapfile -t shell_scripts < <(git ls-files '*.sh'); if ((${#shell_scripts[@]})); then shellcheck "${shell_scripts[@]}"; fi
    {{compose}} config --quiet
    process-compose -f process-compose.yaml --dry-run
    bash infra/tests/static-contracts.sh
    bash scripts/check-docs.sh

# Validate operations structure, acceptance IDs, links, and obsolete guidance.
docs-check:
    bash scripts/check-docs.sh

# Run Rust library/binary tests and web unit tests.
test-unit:
    cargo test --workspace --lib --bins --locked
    pnpm --filter @deepref/web test:unit -- --run

# Start dependencies and run integration test targets.
test-integration:
    {{compose}} up -d --wait
    cargo run -q -p deepref-api -- migrate
    scripts/bootstrap-local-nats.sh
    cargo test --workspace --tests --locked

# Run browser tests; Playwright builds and starts its own preview server.
test-e2e:
    pnpm --filter @deepref/web test:e2e

# Regenerate the OpenAPI document and Orval client.
codegen:
    pnpm run generate:api

# Prove committed generated output is current without modifying the worktree.
codegen-check:
    pnpm run generate:api:check

# Build the four immutable application image targets locally.
docker-build:
    docker buildx bake api worker projector web

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
