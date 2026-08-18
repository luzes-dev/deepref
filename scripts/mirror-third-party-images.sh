#!/usr/bin/env bash
set -euo pipefail

usage() {
  echo "usage: $0 [--dry-run] [--lock PATH] DESTINATION_REGISTRY" >&2
}

dry_run=false
lock_file="deploy/third-party-images.lock.yaml"
while (($#)); do
  case "$1" in
    --dry-run) dry_run=true; shift ;;
    --lock) lock_file=${2:?missing lock path}; shift 2 ;;
    -h|--help) usage; exit 0 ;;
    --*) usage; exit 64 ;;
    *) break ;;
  esac
done

destination_registry=${1:-}
[[ -n "$destination_registry" && $# -eq 1 ]] || { usage; exit 64; }
[[ "$destination_registry" != *@* && "$destination_registry" != *:* ]] || {
  echo "destination registry must be a registry hostname without a tag or digest" >&2
  exit 64
}

for command in jq crane oras; do
  if ! command -v "$command" >/dev/null 2>&1; then
    if [[ "$dry_run" == true && ("$command" == crane || "$command" == oras) ]]; then
      continue
    fi
    echo "$command is required" >&2
    exit 127
  fi
done

[[ -f "$lock_file" ]] || { echo "lock file not found: $lock_file" >&2; exit 66; }
if command -v yq >/dev/null 2>&1; then
  yq -e '
    (.apiVersion == "deepref.dev/v1alpha1") and
    (.kind == "ThirdPartyImageLock") and
    ((keys | sort) == ["apiVersion", "kind", "metadata", "spec"]) and
    ((.metadata | keys | sort) == ["generatedAt"]) and
    ((.spec | keys | sort) == ["images", "requiredPlatforms"]) and
    (.spec.requiredPlatforms == ["linux/amd64", "linux/arm64"])
  ' "$lock_file" >/dev/null || {
    echo "invalid third-party lock structure" >&2
    exit 65
  }
  mapfile -t entries < <(yq -o=json -I=0 '.spec.images[]' "$lock_file")
else
  command -v python3 >/dev/null 2>&1 || { echo "yq or python3 with PyYAML is required" >&2; exit 127; }
  mapfile -t entries < <(python3 - "$lock_file" <<'PY'
import json
import sys

try:
    import yaml
except ImportError as error:
    raise SystemExit(f"PyYAML is required when yq is unavailable: {error}")

with open(sys.argv[1], encoding="utf-8") as stream:
    lock = yaml.safe_load(stream)
if set(lock) != {"apiVersion", "kind", "metadata", "spec"}:
    raise SystemExit("invalid third-party lock root keys")
if lock.get("apiVersion") != "deepref.dev/v1alpha1" or lock.get("kind") != "ThirdPartyImageLock":
    raise SystemExit("unsupported third-party lock kind")
if set(lock.get("metadata", {})) != {"generatedAt"}:
    raise SystemExit("invalid third-party lock metadata")
if set(lock.get("spec", {})) != {"images", "requiredPlatforms"}:
    raise SystemExit("invalid third-party lock spec")
if lock["spec"]["requiredPlatforms"] != ["linux/amd64", "linux/arm64"]:
    raise SystemExit("third-party lock must require amd64 and arm64")
for image in lock.get("spec", {}).get("images", []):
    print(json.dumps(image, separators=(",", ":")))
PY
  )
fi
((${#entries[@]} > 0)) || { echo "image lock is empty" >&2; exit 65; }

declare -A seen=()
for entry in "${entries[@]}"; do
  [[ $(jq -c 'keys | sort' <<<"$entry") == '["destinationRepository","name","source"]' ]] || {
    echo "image lock entries may contain only name, source, and destinationRepository" >&2
    exit 65
  }
  name=$(jq -er '.name' <<<"$entry")
  source=$(jq -er '.source' <<<"$entry")
  repository=$(jq -er '.destinationRepository' <<<"$entry")
  [[ "$source" =~ ^[^[:space:]@]+@sha256:[0-9a-f]{64}$ ]] || {
    echo "$name has a mutable or invalid source reference" >&2
    exit 65
  }
  [[ "$repository" =~ ^[a-z0-9]+([._/-][a-z0-9]+)*$ ]] || {
    echo "$name has an invalid destination repository" >&2
    exit 65
  }
  [[ -z ${seen[$repository]+x} ]] || { echo "duplicate destination: $repository" >&2; exit 65; }
  seen[$repository]=1

  digest=${source##*@}
  destination="${destination_registry}/${repository}@${digest}"
  if [[ "$dry_run" == true ]]; then
    printf 'mirror %s -> %s\n' "$source" "$destination"
    continue
  fi

  crane copy "$source" "$destination"
  oras cp --recursive "$source" "$destination"
  copied_digest=$(crane digest "$destination")
  [[ "$copied_digest" == "$digest" ]] || {
    echo "$name digest mismatch: expected $digest, got $copied_digest" >&2
    exit 1
  }

  source_platforms=$(crane manifest "$source" | jq -cS '[.manifests[]?.platform | {os,architecture}]')
  destination_platforms=$(crane manifest "$destination" | jq -cS '[.manifests[]?.platform | {os,architecture}]')
  [[ "$source_platforms" == "$destination_platforms" ]] || {
    echo "$name platform index changed during mirroring" >&2
    exit 1
  }
done

echo "verified ${#entries[@]} mirrored image indexes" >&2
