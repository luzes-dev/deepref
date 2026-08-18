#!/usr/bin/env bash
set -euo pipefail

diff_file=${1:-/dev/stdin}
python3 - "$diff_file" <<'PY'
import pathlib, re, sys

text = pathlib.Path(sys.argv[1]).read_text() if sys.argv[1] != "/dev/stdin" else sys.stdin.read()
if not text.strip():
    raise SystemExit("GitOps diff is empty")
if "GIT binary patch" in text or re.search(r"^Binary files ", text, re.M):
    raise SystemExit("binary GitOps changes are forbidden")

headers = re.findall(r"^diff --git a/(.+?) b/(.+?)$", text, re.M)
if not headers:
    raise SystemExit("input is not a unified Git diff")
allowed = re.compile(r"^environments/(development|staging|production)/release-lock\.yaml$")
environments = set()
for old, new in headers:
    if old != new:
        raise SystemExit(f"renames are forbidden: {old} -> {new}")
    match = allowed.fullmatch(new)
    if not match:
        raise SystemExit(f"GitOps deployment PR may change only one release lock: {new}")
    environments.add(match.group(1))
if len(headers) != 1 or len(environments) != 1:
    raise SystemExit("GitOps deployment PR must change exactly one environment release lock")
if re.search(r"^deleted file mode ", text, re.M) or re.search(r"^\+\+\+ /dev/null$", text, re.M):
    raise SystemExit("release locks may not be deleted")
PY
