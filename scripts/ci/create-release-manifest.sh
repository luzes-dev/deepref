#!/usr/bin/env bash
set -euo pipefail

output=${1:-release-manifest.json}

required=(
  SOURCE_COMMIT GIT_TREE_HASH CREATED_AT MIGRATION_VERSION
  CHART_REPOSITORY CHART_VERSION CHART_DIGEST CHART_REFERRERS_JSON
  API_REPOSITORY API_DIGEST API_REFERRERS_JSON
  WORKER_REPOSITORY WORKER_DIGEST WORKER_REFERRERS_JSON
  WEB_REPOSITORY WEB_DIGEST WEB_REFERRERS_JSON
)
for name in "${required[@]}"; do
  if [[ -z ${!name:-} ]]; then
    printf 'required environment variable is empty: %s\n' "$name" >&2
    exit 64
  fi
done

normalize_referrers() {
  jq -ce '
    if type != "array" or length == 0 then error("referrer inventory must be a non-empty array") else . end
    | map({digest, mediaType, artifactType: (.artifactType // null)})
    | sort_by(.digest, .mediaType, .artifactType)
    | unique_by([.digest, .mediaType, .artifactType])
  ' <<<"$1"
}

chart_referrers=$(normalize_referrers "$CHART_REFERRERS_JSON")
api_referrers=$(normalize_referrers "$API_REFERRERS_JSON")
worker_referrers=$(normalize_referrers "$WORKER_REFERRERS_JSON")
web_referrers=$(normalize_referrers "$WEB_REFERRERS_JSON")

jq -n \
  --arg source_commit "$SOURCE_COMMIT" \
  --arg source_tree "$GIT_TREE_HASH" \
  --arg chart_repository "$CHART_REPOSITORY" \
  --arg chart_version "$CHART_VERSION" \
  --arg chart_digest "$CHART_DIGEST" \
  --argjson chart_referrers "$chart_referrers" \
  --arg api_repository "$API_REPOSITORY" \
  --arg api_digest "$API_DIGEST" \
  --argjson api_referrers "$api_referrers" \
  --arg worker_repository "$WORKER_REPOSITORY" \
  --arg worker_digest "$WORKER_DIGEST" \
  --argjson worker_referrers "$worker_referrers" \
  --arg web_repository "$WEB_REPOSITORY" \
  --arg web_digest "$WEB_DIGEST" \
  --argjson web_referrers "$web_referrers" \
  --arg migration_version "$MIGRATION_VERSION" \
  --arg created_at "$CREATED_AT" '
  {
    schema_version: 1,
    source: {commit: $source_commit, tree: $source_tree},
    chart: {
      repository: $chart_repository,
      version: $chart_version,
      digest: $chart_digest,
      referrers: $chart_referrers
    },
    images: {
      api: {repository: $api_repository, digest: $api_digest, referrers: $api_referrers},
      worker: {repository: $worker_repository, digest: $worker_digest, referrers: $worker_referrers},
      web: {repository: $web_repository, digest: $web_digest, referrers: $web_referrers}
    },
    migration_version: $migration_version,
    created_at: $created_at
  }' >"$output"

python3 - "$output" <<'PY'
import json, pathlib, sys
from jsonschema import Draft202012Validator, FormatChecker

document = json.loads(pathlib.Path(sys.argv[1]).read_text())
schema = json.loads(pathlib.Path("schemas/release-manifest.schema.json").read_text())
Draft202012Validator(schema, format_checker=FormatChecker()).validate(document)
PY
