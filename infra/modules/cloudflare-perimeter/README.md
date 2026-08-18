# Cloudflare perimeter module

Creates one GitHub identity provider, one reusable organization-membership Access policy, and an Access application, remotely managed Tunnel, fail-closed tunnel configuration, and proxied CNAME for each environment. The only hostnames are `deepref-dev.<base_domain>`, `deepref-staging.<base_domain>`, and `deepref.<base_domain>`.

Origins are validated as Kubernetes `.svc.cluster.local` URLs. Every named ingress performs origin-side Access JWT validation, the final ingress always returns 404, and DNS points only at `cfargotunnel.com`. The module cannot express a public AWS load balancer, IP address, localhost, or bypass hostname. GitOps still owns the `cloudflared` Deployments and must run one replica in development and two fixed replicas in staging and production.

The Cloudflare provider token must arrive through its standard environment variable and needs only DNS, Tunnel, Access application/policy, and identity-provider permissions for the selected account and zone. The GitHub OAuth client secret is a sensitive input required by the Cloudflare API and will be present in encrypted OpenTofu state; never put it in a committed tfvars file or command history. Decide and document the approved secret-injection mechanism before apply.

This module intentionally does not read or output a tunnel token. After apply, an authorized operator or one-shot credential broker retrieves each remotely managed tunnel token directly from Cloudflare and writes it to the pre-created environment secret container without logging it. External Secrets and the `cloudflared` workload remain GitOps-owned.

Run `tofu test` in this directory. The mock test verifies the exact hostname set, proxied tunnel-only DNS, GitHub organization rule, cluster-local origins, required Access JWT validation, and fail-closed catch-all.
