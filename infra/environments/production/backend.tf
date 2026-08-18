terraform {
  backend "s3" {
    bucket       = "REPLACE_WITH_PRODUCTION_STATE_BUCKET"
    key          = "ambient-scribes/production/terraform.tfstate"
    region       = "sa-east-1"
    encrypt      = true
    kms_key_id   = "REPLACE_WITH_PRODUCTION_STATE_KMS_KEY_ARN"
    use_lockfile = true

    state_tags = {
      Environment = "production"
      ManagedBy   = "OpenTofu"
      ObjectType  = "state"
    }
    lock_tags = {
      Environment = "production"
      ManagedBy   = "OpenTofu"
      ObjectType  = "lock"
    }
  }
}
