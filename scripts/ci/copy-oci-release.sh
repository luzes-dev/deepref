#!/usr/bin/env bash
set -euo pipefail

source_manifest=
environment=
destination_registry=
repository_prefix=
output=
while (($#)); do
  case "$1" in
    --source-manifest) source_manifest=${2:?missing source manifest}; shift 2 ;;
    --environment) environment=${2:?missing environment}; shift 2 ;;
    --destination-registry) destination_registry=${2:?missing destination registry}; shift 2 ;;
    --repository-prefix) repository_prefix=${2:?missing repository prefix}; shift 2 ;;
    --output) output=${2:?missing output}; shift 2 ;;
    *) printf 'unknown argument: %s\n' "$1" >&2; exit 64 ;;
  esac
done
: "${source_manifest:?--source-manifest is required}"
: "${environment:?--environment is required}"
: "${destination_registry:?--destination-registry is required}"
: "${repository_prefix:?--repository-prefix is required}"
: "${output:?--output is required}"
[[ $environment =~ ^(staging|production)$ ]] || { echo 'promotion target must be staging or production' >&2; exit 64; }
command -v regctl >/dev/null || { echo 'regctl is required' >&2; exit 69; }

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
python3 - "$source_manifest" "$work/source.json" <<'PY'
import json, pathlib, sys, yaml
source = pathlib.Path(sys.argv[1])
document = json.loads(source.read_text()) if source.suffix == ".json" else yaml.safe_load(source.read_text())
document.pop("environment", None)
pathlib.Path(sys.argv[2]).write_text(json.dumps(document, indent=2) + "\n")
PY

python3 - "$work/source.json" "$destination_registry" "$repository_prefix" <<'PY' |
import json, pathlib, sys
doc = json.loads(pathlib.Path(sys.argv[1]).read_text())
registry, prefix = sys.argv[2].rstrip("/"), sys.argv[3].strip("/")
for name, item in [("chart", doc["chart"]), *doc["images"].items()]:
    leaf = item["repository"].rsplit("/", 1)[-1]
    print(name, item["repository"], item["digest"], f"{registry}/{prefix}/{leaf}", sep="\t")
PY
while IFS=$'\t' read -r name source_repository digest destination_repository; do
  regctl image copy --digest-tags --referrers --force-recursive \
    "$source_repository@$digest" "$destination_repository@$digest"
  [[ $(regctl image digest "$destination_repository@$digest") == "$digest" ]] || {
    echo "$name destination digest changed during copy" >&2
    exit 1
  }
  printf '%s\t%s\n' "$name" "$destination_repository" >>"$work/repositories"
done

python3 - "$work/source.json" "$work/repositories" "$environment" "$output" <<'PY'
import json, pathlib, sys, yaml
doc = json.loads(pathlib.Path(sys.argv[1]).read_text())
repositories = dict(line.split("\t", 1) for line in pathlib.Path(sys.argv[2]).read_text().splitlines())
doc["environment"] = sys.argv[3]
doc["chart"]["repository"] = repositories["chart"]
for name in ("api", "worker", "projector", "web"):
    doc["images"][name]["repository"] = repositories[name]
pathlib.Path(sys.argv[4]).write_text(yaml.safe_dump(doc, sort_keys=False))
PY
scripts/ci/validate-release-lock.sh --environment "$environment" "$output"
scripts/ci/verify-release-digests.sh --remote --source-manifest "$work/source.json" "$output"
