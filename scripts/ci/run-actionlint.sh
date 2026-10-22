#!/usr/bin/env bash
# Run actionlint at the version pinned by CI when Go is available.  Local
# developers may have a standalone actionlint binary instead.
set -euo pipefail

if command -v go >/dev/null 2>&1; then
  exec go run github.com/rhysd/actionlint/cmd/actionlint@v1.7.7 "$@"
fi

if command -v actionlint >/dev/null 2>&1; then
  exec actionlint "$@"
fi

echo "actionlint is unavailable: install actionlint or Go" >&2
exit 127
