#!/usr/bin/env bash
set -euo pipefail
export SQLX_OFFLINE="${SQLX_OFFLINE:-true}"

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
tmp_root="$(mktemp -d)"
trap 'rm -rf "$tmp_root"' EXIT

tmp_openapi="$tmp_root/docs/openapi.json"
tmp_generated="$tmp_root/apps/web/src/lib/api/generated"
mkdir -p "$(dirname "$tmp_openapi")" "$(dirname "$tmp_generated")" "$tmp_root/apps/web/src/routes"
mkdir -p "$tmp_root/apps/web/.svelte-kit"
cp "$root/apps/web/src/lib/api/custom-fetch.ts" "$tmp_root/apps/web/src/lib/api/custom-fetch.ts"
cp "$root/apps/web/orval.config.ts" "$tmp_root/apps/web/orval.config.ts"
cp "$root/apps/web/.prettierrc" "$tmp_root/apps/web/.prettierrc"
cp "$root/apps/web/package.json" "$tmp_root/apps/web/package.json"
cp "$root/apps/web/tsconfig.json" "$tmp_root/apps/web/tsconfig.json"
cp "$root/apps/web/.svelte-kit/tsconfig.json" "$tmp_root/apps/web/.svelte-kit/tsconfig.json"
cp "$root/apps/web/src/routes/layout.css" "$tmp_root/apps/web/src/routes/layout.css"
cp "$root/.editorconfig" "$tmp_root/.editorconfig"
ln -s "$root/apps/web/node_modules" "$tmp_root/apps/web/node_modules"

(
	cd "$root"
	cargo run -q -p deepref-server -- --print-openapi > "$tmp_openapi"
)

(
	cd "$tmp_root/apps/web"
	"$root/apps/web/node_modules/.bin/orval" --config ./orval.config.ts >/dev/null
	"$root/apps/web/node_modules/.bin/prettier" --log-level error --write src/lib/api/generated
)

status=0
diff -u "$root/docs/openapi.json" "$tmp_openapi" || status=1
diff -ru "$root/apps/web/src/lib/api/generated" "$tmp_generated" || status=1

if [[ "$status" -ne 0 ]]; then
	echo "OpenAPI or Orval output is stale. Run: pnpm generate:api" >&2
	exit "$status"
fi
