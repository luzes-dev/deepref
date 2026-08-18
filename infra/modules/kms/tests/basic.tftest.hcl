mock_provider "aws" {}

run "rotating_service_keys" {
  command = plan

  variables {
    account_id = "111111111111"
    keys = {
      rds = {
        alias              = "alias/test-rds"
        description        = "Test RDS key"
        service_principals = ["rds.amazonaws.com"]
      }
      secrets = {
        alias              = "alias/test-secrets"
        description        = "Test secrets key"
        service_principals = ["secretsmanager.amazonaws.com"]
      }
    }
  }

  assert {
    condition     = length(aws_kms_key.this) == 2
    error_message = "Every declared logical key must be created."
  }

  assert {
    condition     = alltrue([for key in aws_kms_key.this : key.enable_key_rotation])
    error_message = "Every KMS key must rotate."
  }
}
