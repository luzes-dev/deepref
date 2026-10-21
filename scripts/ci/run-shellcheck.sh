#!/usr/bin/env bash
# Run ShellCheck over every tracked shell script, including this helper.
set -euo pipefail

mapfile -t scripts < <(git ls-files '*.sh')
if ((${#scripts[@]} > 0)); then
  exec shellcheck "${scripts[@]}"
fi
