# Global platform root

This isolated root owns only global Cloudflare/GitHub policy and the initial Argo CD installation/bootstrap in the three private EKS clusters. Its S3 backend uses the separately bootstrapped global key and native `use_lockfile = true` locking. It rejects non-default workspaces, execution from an unexpected state-anchor account, duplicate environment accounts, cross-account role ARNs whose account does not match their declaration, and any assumed role that resolves to the wrong account.

The default AWS credentials must be for the selected state-anchor account. Each existing cluster-access role must be assumable from that identity, authorized by EKS access entries, and able to reach the private EKS endpoint from the runner. Cloudflare and GitHub providers read their API tokens from standard provider environment variables; do not add token variables or committed credentials.

Apply order:

1. Bootstrap and migrate `infra/bootstrap/global`; verify the remote `.tflock` path.
2. Apply the three per-environment AWS roots so the private clusters, access roles, secret containers, and network path exist.
3. Create/install the GitHub OAuth App, deployment GitHub App, reviewer team, Cloudflare zone/Zero Trust team, and protected orphan `gitops` branch through their reviewed out-of-band procedures. Populate no orphan-branch content from this root.
4. Put only non-secret populated inputs in a protected external variable file. Inject the OAuth client secret and provider tokens from the approved secret broker, initialize this root, review the no-save plan, and apply from the private-cluster-capable protected runner.
5. Deliver the Argo read-only repository credential (if the repository is private) and each Cloudflare tunnel token directly to its approved secret container. Confirm Argo sync and Cloudflare Access denial before enabling users.

The root never creates application workloads, workload namespaces, tunnel Deployments, release locks, GitOps branch contents, repository/provider credentials, tunnel tokens, or generated secrets. Argo owns all children below each supplied GitOps path. Run `tofu test` here for the mocked account/root contract and `infra/tests/static-contracts.sh` for cross-root ownership and policy checks.
