terraform {
  backend "s3" {
    bucket       = "REPLACE_WITH_STAGING_STATE_BUCKET"
    key          = "ambient-scribes/staging/terraform.tfstate"
    region       = "sa-east-1"
    encrypt      = true
    kms_key_id   = "REPLACE_WITH_STAGING_STATE_KMS_KEY_ARN"
    use_lockfile = true

    state_tags = {
      Environment = "staging"
      ManagedBy   = "OpenTofu"
      ObjectType  = "state"
    }
    lock_tags = {
      Environment = "staging"
      ManagedBy   = "OpenTofu"
      ObjectType  = "lock"
    }
  }
}
