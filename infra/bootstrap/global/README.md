# Global state bootstrap

This isolated root creates or adopts the encrypted, versioned S3 backend for global Cloudflare, GitHub, and Argo bootstrap state. It runs only in the explicitly selected AWS account, uses the implicit local backend for the one privileged bootstrap/adoption, rejects non-default workspaces, and creates native S3 lockfile permissions instead of a DynamoDB table.

Decide which existing account is the global-state anchor before apply. Sign in with the approved SSO administrator role and verify `aws sts get-caller-identity` returns that exact account. Put the populated variable file outside the repository, run `tofu init`, `tofu plan -var-file=/secure/path/global-bootstrap.tfvars`, and apply only the reviewed plan. Until migration, protect the resulting local state as a sensitive credential-bearing artifact.

Copy `backend.tf.remote.example` to an uncommitted local `backend.tf`, put a populated `backend.hcl.example` outside the repository, and run:

```sh
tofu init -migrate-state -backend-config=/secure/path/global-bootstrap-backend.hcl
```

Confirm the remote object and `.tflock` can be written, then securely remove every local state/backup file. The global environment uses a distinct `ambient-scribes/global/terraform.tfstate` key in the same bucket. Never commit state, plans, populated tfvars, populated backend configuration, credentials, generated secrets, or orphan-branch contents.

To adopt existing resources, initialize locally and import the KMS key/alias plus the bucket's ownership, public-access, versioning, encryption, lifecycle, and policy resources at their `module.state_backend` addresses before planning. Review the full plan before applying; import does not make a mismatched policy safe.
