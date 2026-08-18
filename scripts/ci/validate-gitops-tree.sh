#!/usr/bin/env bash
set -euo pipefail

tree=${1:?usage: validate-gitops-tree.sh TREE_LIST}
expected=$(mktemp)
actual=$(mktemp)
trap 'rm -f "$expected" "$actual"' EXIT

printf '%s\n' \
  argocd/appproject.yaml \
  argocd/applicationset.yaml \
  argocd/root-application.yaml \
  environments/development/release-lock.yaml \
  environments/development/values.yaml \
  environments/production/release-lock.yaml \
  environments/production/values.yaml \
  environments/staging/release-lock.yaml \
  environments/staging/values.yaml | LC_ALL=C sort >"$expected"

sed -e '/^[[:space:]]*$/d' -e 's#^\./##' "$tree" | LC_ALL=C sort -u >"$actual"
if ! diff -u "$expected" "$actual"; then
  echo 'GitOps orphan branch must contain exactly the nine allowlisted files' >&2
  exit 1
fi
