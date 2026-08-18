# State backend module

Creates one protected, versioned S3 bucket and a dedicated rotating KMS key for OpenTofu state. Public access and ACLs are blocked, TLS and explicit KMS encryption are enforced, old versions are retained, and the bucket and key have `prevent_destroy` safeguards.

Pass only the CI and operator role ARNs that must access state. Backend callers must set `encrypt = true`, `kms_key_id` to the output key ARN, and `use_lockfile = true`. The module deliberately creates no DynamoDB table: native S3 lockfiles are the current locking mechanism.

For a legacy backend, stop all writers, upgrade every runner to OpenTofu 1.12 or later, add `use_lockfile = true` while retaining the DynamoDB setting for one transition, verify a lock object is created, then remove `dynamodb_table`. Do not delete the old table until no older runner can apply.

Existing buckets and keys are adopted with `tofu import` before the first plan. Import the bucket plus its versioning, encryption, lifecycle, access-block, ownership, and policy resources separately; import the key by key ID and alias by alias name. Confirm the existing encryption and access policy match this module before applying.
