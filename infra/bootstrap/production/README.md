# Production state bootstrap

This isolated root creates or adopts the production account's encrypted, versioned OpenTofu state backend. It uses the implicit local backend only for the one privileged bootstrap/adoption and forbids non-default workspaces. Never commit state, plans, populated variable files, backend configuration, credentials, or generated secrets.

## New backend

1. Sign in with the approved AWS SSO administrator role and verify `aws sts get-caller-identity` returns the exact expected account. Ensure every ARN in `state_access_principal_arns` already exists and includes the role performing migration.
2. Put a populated copy of `terraform.tfvars.example` outside the repository. Run `tofu init`, `tofu plan -var-file=/secure/path/production.tfvars`, and `tofu apply -var-file=/secure/path/production.tfvars`. Until migration, guard the resulting local `terraform.tfstate` as a credential-bearing sensitive artifact.
3. Copy `backend.tf.remote.example` to local `backend.tf` and place a populated copy of `backend.hcl.example` outside the repository. Run `tofu init -migrate-state -backend-config=/secure/path/production-bootstrap-backend.hcl` and answer the state-copy prompt.
4. Run `tofu state pull` only into a protected temporary location if verification is required; confirm the remote object and `.tflock` can be written, then securely remove every local state/backup file. Keep the generated `backend.tf` (or deliver that empty partial block through the approved deployment mechanism) for every later run; never fall back to local state and never place credentials in backend configuration.
5. Configure `infra/environments/production` with a different key such as `ambient-scribes/production/terraform.tfstate`, the same bucket/key ARN, and `use_lockfile = true`. Do not use workspaces.

## Adopt an existing backend

Initialize locally and import before planning. Use the real bucket, key ID, and alias, plus the same external variable file:

```sh
tofu init
tofu import -var-file=/secure/path/production.tfvars module.state_backend.aws_kms_key.state REPLACE_WITH_KEY_ID
tofu import -var-file=/secure/path/production.tfvars module.state_backend.aws_kms_alias.state alias/ambient-scribes-production-state
tofu import -var-file=/secure/path/production.tfvars module.state_backend.aws_s3_bucket.state REPLACE_WITH_BUCKET
tofu import -var-file=/secure/path/production.tfvars module.state_backend.aws_s3_bucket_ownership_controls.state REPLACE_WITH_BUCKET
tofu import -var-file=/secure/path/production.tfvars module.state_backend.aws_s3_bucket_public_access_block.state REPLACE_WITH_BUCKET
tofu import -var-file=/secure/path/production.tfvars module.state_backend.aws_s3_bucket_versioning.state REPLACE_WITH_BUCKET
tofu import -var-file=/secure/path/production.tfvars module.state_backend.aws_s3_bucket_server_side_encryption_configuration.state REPLACE_WITH_BUCKET
tofu import -var-file=/secure/path/production.tfvars module.state_backend.aws_s3_bucket_lifecycle_configuration.state REPLACE_WITH_BUCKET
tofu import -var-file=/secure/path/production.tfvars module.state_backend.aws_s3_bucket_policy.state REPLACE_WITH_BUCKET
```

Review the complete plan before applying; an import does not make a mismatched policy safe. Then perform the remote-state migration above. If migrating legacy DynamoDB locking, first run S3 and DynamoDB locks together across every writer, wait until all old clients are gone, and only then remove `dynamodb_table`.
