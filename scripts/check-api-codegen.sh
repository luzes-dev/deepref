#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

cp "$root/docs/openapi.json" "$tmp/openapi.json"
cp -R "$root/apps/web/src/lib/api/generated" "$tmp/generated"

(
	cd "$root"
	pnpm generate:api
)

status=0
diff -u "$tmp/openapi.json" "$root/docs/openapi.json" || status=1
diff -ru "$tmp/generated" "$root/apps/web/src/lib/api/generated" || status=1

if [[ "$status" -ne 0 ]]; then
	echo "OpenAPI or Orval output is stale. Run: pnpm generate:api" >&2
	exit "$status"
fi
