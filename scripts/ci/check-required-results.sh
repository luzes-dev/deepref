#!/usr/bin/env bash
# Verify that the results passed as NAME=RESULT all succeeded.
#
# Path-filtered CI lanes are allowed to report "skipped" on pull requests and
# merge-group runs, where the changed-path detector legitimately omitted them.
# Pass --strict to require success from every lane regardless of event (used by
# the security aggregate, whose jobs are never path filtered).
set -euo pipefail

strict=false
if [[ ${1:-} == --strict ]]; then
  strict=true
  shift
fi

label=${1:?usage: check-required-results.sh [--strict] LABEL NAME=RESULT...}
shift

allow_skipped=true
if [[ $strict == true ]]; then
  allow_skipped=false
fi
case "${GITHUB_EVENT_NAME:-push}" in
  pull_request | merge_group) ;;
  *) allow_skipped=false ;;
esac

failed=0
for entry in "$@"; do
  name=${entry%%=*}
  result=${entry#*=}

  if [[ $result == success ]]; then
    printf '%s: %s succeeded\n' "$label" "$name"
    continue
  fi

  if [[ $allow_skipped == true && $result == skipped ]]; then
    printf '%s: %s skipped (allowed for %s)\n' \
      "$label" "$name" "${GITHUB_EVENT_NAME:-local}"
    continue
  fi

  printf '::error title=%s failed::%s did not succeed (result: %s)\n' \
    "$label" "$name" "$result" >&2
  failed=1
done

exit "$failed"
