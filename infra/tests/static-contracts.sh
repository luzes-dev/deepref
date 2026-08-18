#!/usr/bin/env bash
set -euo pipefail

repository_root="$(git rev-parse --show-toplevel)"
cd "$repository_root"

fail() {
  printf 'infra static contract failed: %s\n' "$1" >&2
  exit 1
}

require_pattern() {
  local pattern=$1
  local path=$2
  local message=$3
  rg --quiet --glob '*.tf' "$pattern" "$path" || fail "$message"
}

for environment in development staging production global; do
  require_pattern 'terraform\.workspace == "default"' "infra/environments/$environment" \
    "$environment environment root must reject non-default workspaces"
  require_pattern 'use_lockfile[[:space:]]*=[[:space:]]*true' "infra/environments/$environment/backend.tf" \
    "$environment environment backend must use native S3 lockfiles"
done

for environment in development staging production; do
  for module in backup observability budgets_alerts admin_runner pod_identity; do
    require_pattern "module \"$module\"" "infra/environments/$environment" \
      "$environment root must wire the $module module"
  done
done

for bootstrap in development staging production global; do
  require_pattern 'terraform\.workspace == "default"' "infra/bootstrap/$bootstrap" \
    "$bootstrap bootstrap root must reject non-default workspaces"
  require_pattern 'expected_account_id' "infra/bootstrap/$bootstrap" \
    "$bootstrap bootstrap root must bind to an expected account"
done

require_pattern 'data\.aws_caller_identity\.development\.account_id == var\.aws_environments\.development\.account_id' \
  infra/environments/global 'global root must validate the development assumed account'
require_pattern 'data\.aws_caller_identity\.staging\.account_id == var\.aws_environments\.staging\.account_id' \
  infra/environments/global 'global root must validate the staging assumed account'
require_pattern 'data\.aws_caller_identity\.production\.account_id == var\.aws_environments\.production\.account_id' \
  infra/environments/global 'global root must validate the production assumed account'

argo_resources="$(rg --no-filename --only-matching 'resource "[^"]+"' infra/modules/argo-bootstrap/main.tf | LC_ALL=C sort -u)"
expected_argo_resources=$'resource "helm_release"\nresource "kubernetes_namespace_v1"'
[[ "$argo_resources" == "$expected_argo_resources" ]] || fail 'Argo module may own only its namespace and Helm bootstrap release'
require_pattern 'type[[:space:]]*=[[:space:]]*"ClusterIP"' infra/modules/argo-bootstrap \
  'Argo server must remain ClusterIP-only'
require_pattern 'CreateNamespace=false' infra/modules/argo-bootstrap \
  'Argo bootstrap must not take ownership of workload namespaces'

require_pattern 'svc.*cluster.*local' infra/modules/cloudflare-perimeter \
  'Cloudflare origins must be constrained to Kubernetes cluster-local services'
require_pattern 'http_status:404' infra/modules/cloudflare-perimeter \
  'Cloudflare Tunnel must have a fail-closed catch-all'
require_pattern 'required[[:space:]]*=[[:space:]]*true' infra/modules/cloudflare-perimeter \
  'cloudflared must require a valid Access JWT at the origin hop'
require_pattern 'cfargotunnel\.com' infra/modules/cloudflare-perimeter \
  'Public DNS must target only Cloudflare Tunnel'

if rg --quiet --glob '*.tf' 'tunnel_cloudflared_token|tunnel_secret|aws_(lb|alb|elb)|load_balancer' \
  infra/modules/cloudflare-perimeter infra/environments/global; then
  fail 'global perimeter must not read tunnel credentials or create a public origin'
fi

[[ -f .github/workflows/rebuild.yml ]] || fail 'constrained projector rebuild workflow must exist'
rg --quiet 'deploy/rebuild/\*' .github/workflows/branch-policy.yml || fail 'rebuild branches must be admitted only through the GitOps App policy'
[[ -x scripts/ci/validate-gitops-values.sh ]] || fail 'GitOps rebuild values validator must be executable'

require_pattern 'actor_type[[:space:]]*=[[:space:]]*"Integration"' infra/modules/github-repository \
  'GitOps bypass actor must be a GitHub App integration'
require_pattern 'bypass_mode[[:space:]]*=[[:space:]]*"pull_request"' infra/modules/github-repository \
  'deployment App bypass must be pull-request scoped'
require_pattern 'update[[:space:]]*=[[:space:]]*true' infra/modules/github-repository \
  'GitOps updates must be restricted to bypass actors'
require_pattern 'minimum_approvals[[:space:]]*=[[:space:]]*2' infra/modules/github-repository \
  'production GitOps lock changes must require two approvals'
require_pattern 'required_approving_review_count[[:space:]]*=[[:space:]]*each\.value\.approvals' infra/modules/github-repository \
  'source rulesets must use the branch-specific approval ladder'

if rg --quiet 'dynamodb_table' infra/environments infra/bootstrap/global; then
  fail 'new roots must use native locks without DynamoDB locking'
fi

[[ ! -e infra/neo4j/constraints.cypher ]] || fail 'legacy Neo4j constraints file must be removed'
[[ -e crates/graph/migrations/0001_constraints.cypher ]] || fail 'versioned graph constraint migration must exist before legacy removal'

mapfile -t forbidden_artifacts < <(
  find infra -type f \( -name '*.tfstate' -o -name '*.tfstate.*' -o -name '*.tfplan' -o -name 'terraform.tfvars' \) -print
)
[[ ${#forbidden_artifacts[@]} -eq 0 ]] || fail "forbidden generated artifacts found: ${forbidden_artifacts[*]}"

printf 'infra static contracts: ok\n'
