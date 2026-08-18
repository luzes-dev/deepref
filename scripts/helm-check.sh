#!/usr/bin/env bash
set -euo pipefail

chart=${1:-charts/deepref}
command -v helm >/dev/null 2>&1 || { echo "helm is required" >&2; exit 127; }

tmp_dir=$(mktemp -d)
trap 'rm -rf "$tmp_dir"' EXIT

helm lint "$chart" -f "$chart/tests/values/development.yaml"

for environment in development staging production; do
  rendered="$tmp_dir/deepref-${environment}.yaml"
  helm template deepref "$chart" \
    --namespace "deepref-${environment}" \
    -f "$chart/tests/values/${environment}.yaml" >"$rendered"

  if command -v kubeconform >/dev/null 2>&1; then
    kubeconform -strict -summary -ignore-missing-schemas "$rendered"
  else
    echo "skip kubeconform: not installed" >&2
  fi
done

if helm plugin list 2>/dev/null | awk 'NR > 1 {print $1}' | grep -qx unittest; then
  helm unittest "$chart"
else
  echo "skip helm-unittest: plugin not installed" >&2
fi

if command -v conftest >/dev/null 2>&1; then
  conftest test "$tmp_dir"/*.yaml --policy policy/helm
else
  echo "skip conftest: not installed" >&2
fi

if command -v kyverno >/dev/null 2>&1; then
  kyverno test "$chart/tests/kyverno"
else
  echo "skip kyverno: not installed" >&2
fi
