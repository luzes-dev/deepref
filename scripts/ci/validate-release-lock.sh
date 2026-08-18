#!/usr/bin/env bash
set -euo pipefail

environment=
manifest=
while (($#)); do
  case "$1" in
    --environment) environment=${2:?missing environment}; shift 2 ;;
    --manifest) manifest=${2:?missing manifest}; shift 2 ;;
    --) shift; break ;;
    -*) printf 'unknown option: %s\n' "$1" >&2; exit 64 ;;
    *) break ;;
  esac
done
lock=${1:?usage: validate-release-lock.sh [--environment ENV] [--manifest FILE] LOCK}
[[ $# -eq 1 ]] || { echo 'exactly one lock file is required' >&2; exit 64; }

python3 - "$lock" "$environment" "$manifest" <<'PY'
import copy, json, pathlib, sys
import yaml
from jsonschema import Draft202012Validator, FormatChecker
from referencing import Registry, Resource

lock_path, expected_environment, manifest_path = sys.argv[1:]
manifest_schema = json.loads(pathlib.Path("schemas/release-manifest.schema.json").read_text())
lock_schema = json.loads(pathlib.Path("schemas/release-lock.schema.json").read_text())
registry = Registry().with_resource(manifest_schema["$id"], Resource.from_contents(manifest_schema))
lock = yaml.safe_load(pathlib.Path(lock_path).read_text())
Draft202012Validator(lock_schema, registry=registry, format_checker=FormatChecker()).validate(lock)

if expected_environment and lock["environment"] != expected_environment:
    raise SystemExit(f"release lock environment {lock['environment']!r} does not match {expected_environment!r}")
if manifest_path:
    manifest = json.loads(pathlib.Path(manifest_path).read_text())
    Draft202012Validator(manifest_schema, format_checker=FormatChecker()).validate(manifest)
    comparable = copy.deepcopy(lock)
    comparable.pop("environment")
    if comparable != manifest:
        raise SystemExit("release lock does not exactly equal the supplied release manifest")
PY
