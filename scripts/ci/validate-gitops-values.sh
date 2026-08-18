#!/usr/bin/env bash
set -euo pipefail

mode="any"
if [[ "${1:-}" == "--mode" ]]; then
  mode=${2:?usage: validate-gitops-values.sh [--mode start|stop|any] DIFF}
  shift 2
fi
diff_file=${1:?usage: validate-gitops-values.sh [--mode start|stop|any] DIFF}

case "$mode" in
  start|stop|any) ;;
  *) echo "mode must be start, stop, or any" >&2; exit 2 ;;
esac

python3 - "$mode" "$diff_file" <<'PY'
import pathlib
import re
import sys

mode, diff_path = sys.argv[1:]
text = pathlib.Path(diff_path).read_text()
headers = re.findall(r"^diff --git a/(.+?) b/(.+?)$", text, re.M)
if len(headers) != 1:
    raise SystemExit("rebuild values PR must change exactly one file")

old_path, new_path = headers[0]
expected = re.compile(r"^environments/(development|staging|production)/values\.yaml$")
if old_path != new_path or not expected.fullmatch(new_path):
    raise SystemExit("rebuild values PR may change only one environment values.yaml")
if "GIT binary patch" in text or re.search(r"^Binary files ", text, re.M):
    raise SystemExit("binary GitOps changes are forbidden")
if re.search(r"^deleted file mode ", text, re.M) or re.search(r"^\+\+\+ /dev/null$", text, re.M):
    raise SystemExit("environment values may not be deleted")

added = []
removed = []
for line in text.splitlines():
    if line.startswith(("+++", "---", "@@", "\\")):
        continue
    if line.startswith("+"):
        added.append(line[1:])
    elif line.startswith("-"):
        removed.append(line[1:])

enabled = re.compile(r"^\s{2}enabled: (true|false)$")
run_id = re.compile(r'^\s{2}runId: "([^"]*)"$')

def parse(lines):
    values = {}
    for line in lines:
        enabled_match = enabled.fullmatch(line)
        if enabled_match:
            values["enabled"] = enabled_match.group(1)
            continue
        run_match = run_id.fullmatch(line)
        if run_match:
            values["runId"] = run_match.group(1)
            continue
        raise SystemExit("values PR may change only rebuild.enabled and rebuild.runId")
    if set(values) != {"enabled", "runId"}:
        raise SystemExit("values PR must change rebuild.enabled and rebuild.runId together")
    return values

old = parse(removed)
new = parse(added)
uuid_pattern = r"[0-9a-f]{8}-[0-9a-f]{4}-[1-5][0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}"

is_start = old == {"enabled": "false", "runId": ""} and new["enabled"] == "true" and re.fullmatch(uuid_pattern, new["runId"])
is_stop = old["enabled"] == "true" and re.fullmatch(uuid_pattern, old["runId"]) and new == {"enabled": "false", "runId": ""}

if mode == "start" and not is_start:
    raise SystemExit("start rebuild must change disabled/empty values to enabled and a UUID")
if mode == "stop" and not is_stop:
    raise SystemExit("stop rebuild must change enabled/UUID values to disabled/empty")
if mode == "any" and not (is_start or is_stop):
    raise SystemExit("rebuild values change is not a valid start or stop transition")
PY
