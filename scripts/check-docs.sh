#!/usr/bin/env bash
set -euo pipefail

fail() {
  printf 'documentation check failed: %s\n' "$*" >&2
  exit 1
}

guide_count=$(find docs/operations -maxdepth 1 -type f -name '*.md' | wc -l)
[[ $guide_count -eq 8 ]] || fail "expected exactly 8 top-level operations guides, found $guide_count"

runbook_count=$(find docs/operations/runbooks -maxdepth 1 -type f -name '*.md' | wc -l)
[[ $runbook_count -eq 12 ]] || fail "expected exactly 12 runbooks, found $runbook_count"

expected_runbooks=(
  bootstrap.md
  promotion.md
  rollback.md
  migration-failure.md
  rds-failover-pitr.md
  nats-quorum-dlq-recovery.md
  neo4j-rebuild.md
  projection-lag.md
  cloudflare-or-idp-outage.md
  credential-rotation.md
  node-az-drain.md
  break-glass-access.md
)

for runbook in "${expected_runbooks[@]}"; do
  file="docs/operations/runbooks/$runbook"
  [[ -s $file ]] || fail "missing runbook $file"
  for heading in \
    'Purpose and scope' \
    'Safety warnings' \
    'Prerequisites and authorization' \
    'Triggers and symptoms' \
    'Ordered steps' \
    'Verification' \
    'Rollback or safe stop' \
    'Escalation' \
    'Evidence and audit capture'; do
    rg --quiet --fixed-strings "## $heading" "$file" || fail "$file is missing heading: $heading"
  done
done

acceptance=docs/acceptance/production-platform.md
[[ -s $acceptance ]] || fail "missing acceptance register"
for number in $(seq -w 1 16); do
  count=$(rg --count "^## AC-${number} — " "$acceptance" || true)
  [[ $count -eq 1 ]] || fail "AC-${number} must have exactly one acceptance heading"
done

mapfile -d '' markdown_files < <(
  find README.md CONTRIBUTING.md SECURITY.md docs apps/web/README.md infra/README.md charts/deepref/README.md \
    -type f -name '*.md' -print0
)

repository_root=$(pwd -P)
for file in "${markdown_files[@]}"; do
  while IFS= read -r destination; do
    destination=${destination#<}
    destination=${destination%>}
    destination=${destination%% *}
    case "$destination" in
      ''|\#*|http://*|https://*|mailto:*) continue ;;
    esac
    path=${destination%%#*}
    path=${path%%\?*}
    [[ -n $path ]] || continue
    resolved=$(realpath -m "$(dirname "$file")/$path")
    [[ $resolved == "$repository_root"/* ]] || fail "$file links outside the repository: $destination"
    [[ -e $resolved ]] || fail "$file has a broken internal link: $destination"
  done < <(perl -ne 'while (/\[[^\]]+\]\(([^)]+)\)/g) { print "$1\n" }' "$file")
done

while IFS= read -r runbook_path; do
  [[ -f $runbook_path ]] || fail "observability alert has a broken runbook path: $runbook_path"
done < <(
  rg --no-filename --only-matching 'runbook: "[^"]+"' observability/alerts |
    cut -d'"' -f2
)

if rg --ignore-case --line-number \
  'docker-compose\.selfhost|Self-hosting Example|examples/docker-compose|infra/docker-compose\.yml|ghcr\.io|compose:config|docs/runbooks/' \
  README.md CONTRIBUTING.md SECURITY.md docs apps/web/README.md package.json; then
  fail "obsolete deployment or runbook guidance remains"
fi

if rg --line-number 'https://REPLACE_WITH_[A-Z_]*HOST/(health|projects)' docs/operations; then
  fail "hosted same-origin API commands must use the /api prefix"
fi

printf 'documentation checks passed: 8 guides, 12 runbooks, AC-01..AC-16, links, and obsolete-path scan\n'
