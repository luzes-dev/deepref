# ECR module

Creates KMS-encrypted, immutable, scan-on-push repositories with bounded tagged and untagged image retention. Repository policies may grant downstream accounts read access without granting writes.

When trusted AWS principals or an OIDC provider are supplied, the module creates a destination promotion role that can read exact OCI manifests from explicitly listed upstream repositories and write them into the current account. OIDC trust requires explicit audience and subject conditions. Upstream repositories must independently list that role in `repository_pull_principal_arns`; this two-sided contract prevents accidental broad cross-account reads.
