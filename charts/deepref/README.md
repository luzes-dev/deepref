# DeepRef Helm chart

This chart deploys the DeepRef API, worker, projector, web gateway, NATS JetStream, Neo4j Community, Cloudflare Tunnel, ADOT, External Secrets, network policies, disruption budgets, and image-verification policy.

## Release inputs

Application image repositories and digests are intentionally unset in the base values because they are outputs of the build-once release workflow. A GitOps release lock must provide all four immutable application references. Hosted overlays must also replace public third-party repositories with their ECR mirror repositories while retaining the verified lock digests.

The chart never accepts secret values. `externalSecrets.remote` contains only AWS Secrets Manager identifiers and property names. Before rendering a hosted release, provide the environment-specific identifiers, exact API CORS origin, AMP remote-write endpoint, reviewed egress CIDRs, image repositories/digests, and the keyless signing subject.

## Ordering and ownership

- OpenTofu owns infrastructure, IAM roles, EKS Pod Identity associations, secret containers, and the initial controller installations.
- Helm owns Kubernetes service accounts, application workloads, NATS/Neo4j, ExternalSecret objects, policies, and collectors.
- `deepref-api migrate` runs as an Argo CD `PreSync` hook. A failed migration blocks the new sync before Deployments change.
- The NATS bootstrap Job owns the three streams and two durable consumers. Application credentials never receive administration permissions.
- The projector rebuild Job is absent unless `rebuild.enabled=true` and a unique `rebuild.runId` is supplied.
- Neo4j is deliberately single-node and has no PDB; it is rebuilt from PostgreSQL.

## Required secrets

The configured `ClusterSecretStore` must expose database URL, Neo4j username/password/auth, NATS accounts configuration, NATS TLS material, four subject-restricted NATS credentials files, and the Cloudflare tunnel token. The NATS certificate must cover client service and StatefulSet pod DNS names.

## Validation

Run `scripts/helm-check.sh`. Helm lint/render is mandatory; kubeconform, helm-unittest, conftest, and Kyverno CLI execute when installed. The files under `tests/values` are render-only fixtures and are not environment release locks.
