# DeepRef Helm chart

This chart deploys the DeepRef API, PostgreSQL-backed worker, web gateway, Cloudflare Tunnel, ADOT, External Secrets, network policies, disruption budgets, and image-verification policy.

## Release inputs

Application image repositories and digests are intentionally unset in the base values because they are outputs of the build-once release workflow. A GitOps release lock must provide all three immutable application references. Hosted overlays must also replace public third-party repositories with their ECR mirror repositories while retaining the verified lock digests.

The chart never accepts secret values. `externalSecrets.remote` contains only AWS Secrets Manager identifiers and property names. Before rendering a hosted release, provide the environment-specific identifiers, exact API CORS origin, AMP remote-write endpoint, reviewed egress CIDRs, image repositories/digests, and the keyless signing subject.

## Ordering and ownership

- OpenTofu owns infrastructure, IAM roles, EKS Pod Identity associations, secret containers, and the initial controller installations.
- Helm owns Kubernetes service accounts, application workloads, ExternalSecret objects, policies, and collectors.
- `deepref-server migrate` runs as an Argo CD `PreSync` hook. A failed migration blocks the new sync before Deployments change.
- `deepref-server serve` serves HTTP only; `deepref-server worker` claims durable PostgreSQL jobs.

## Required secrets

The configured `ClusterSecretStore` must expose the database URL and Cloudflare tunnel token.

## Validation

Run `scripts/helm-check.sh`. Helm lint/render is mandatory; kubeconform, helm-unittest, conftest, and Kyverno CLI execute when installed. The files under `tests/values` are render-only fixtures and are not environment release locks.
