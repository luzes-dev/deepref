#!/usr/bin/env bash
set -euo pipefail

remote=false
source_manifest=
while (($#)); do
  case "$1" in
    --remote) remote=true; shift ;;
    --source-manifest) source_manifest=${2:?missing source manifest}; shift 2 ;;
    --) shift; break ;;
    -*) printf 'unknown option: %s\n' "$1" >&2; exit 64 ;;
    *) break ;;
  esac
done
lock=${1:?usage: verify-release-digests.sh [--remote] [--source-manifest FILE] LOCK}
scripts/ci/validate-release-lock.sh "$lock"

python3 - "$lock" "$source_manifest" <<'PY'
import json, pathlib, sys, yaml

lock = yaml.safe_load(pathlib.Path(sys.argv[1]).read_text())
source_path = sys.argv[2]
if source_path:
    source = json.loads(pathlib.Path(source_path).read_text())
    for field in ("schema_version", "source", "migration_version", "created_at"):
        if lock[field] != source[field]:
            raise SystemExit(f"promoted lock changed immutable field: {field}")
    if lock["chart"]["version"] != source["chart"]["version"]:
        raise SystemExit("promoted lock changed chart version")
    for name in ("chart", "api", "worker", "projector", "web"):
        left = lock["chart"] if name == "chart" else lock["images"][name]
        right = source["chart"] if name == "chart" else source["images"][name]
        for field in ("digest", "referrers"):
            if left[field] != right[field]:
                raise SystemExit(f"promoted {name} changed exact {field} inventory")
PY

if [[ $remote == false ]]; then
  exit 0
fi
command -v regctl >/dev/null || { echo 'regctl is required for --remote' >&2; exit 69; }

python3 - "$lock" <<'PY' | while IFS=$'\t' read -r name repository digest expected; do
import json, pathlib, sys, yaml
doc = yaml.safe_load(pathlib.Path(sys.argv[1]).read_text())
items = [("chart", doc["chart"]), *doc["images"].items()]
for name, artifact in items:
    inventory = sorted(artifact["referrers"], key=lambda value: (value["digest"], value["mediaType"], value.get("artifactType") or ""))
    print(name, artifact["repository"], artifact["digest"], json.dumps(inventory, separators=(",", ":")), sep="\t")
PY
  reference="$repository@$digest"
  actual_digest=$(regctl image digest "$reference")
  [[ $actual_digest == "$digest" ]] || { echo "$name digest mismatch: $actual_digest != $digest" >&2; exit 1; }
  actual=$(regctl artifact list --digest-tags --format '{{json .Manifest}}' "$reference" |
    jq -c '[.manifests[]? | {digest, mediaType, artifactType: (.artifactType // null)}] | sort_by(.digest, .mediaType, (.artifactType // ""))')
  [[ $actual == "$expected" ]] || { echo "$name referrer inventory mismatch" >&2; exit 1; }
done
