# Cross-root infrastructure contracts

`static-contracts.sh` complements provider-backed `tofu test` suites with repository-wide invariants that a single module test cannot prove. It checks account/default-workspace guards, native S3 locks, the OpenTofu/Argo ownership boundary, cluster-local Tunnel origins, absence of public-origin or tunnel-token resources, App-only GitOps updates, branch approval policy, versioned graph migration ownership, and forbidden generated artifacts.

Run from any directory inside the repository:

```sh
infra/tests/static-contracts.sh
```

The script is read-only and requires Bash, Git, `rg`, and standard POSIX utilities.
