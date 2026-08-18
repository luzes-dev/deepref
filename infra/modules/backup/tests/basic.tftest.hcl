mock_provider "aws" {}

run "encrypted_locked_continuous_backup" {
  command = plan

  variables {
    name        = "ambient-scribes-test"
    kms_key_arn = "arn:aws:kms:sa-east-1:111111111111:key/00000000-0000-0000-0000-000000000000"
    resource_arns = [
      "arn:aws:rds:sa-east-1:111111111111:db:ambient-scribes-test",
    ]
    vault_lock_changeable_for_days = 7
  }

  assert {
    condition     = aws_backup_vault.this.kms_key_arn != null
    error_message = "The backup vault must use a customer-managed KMS key."
  }

  assert {
    condition     = aws_backup_plan.this.rule[0].enable_continuous_backup
    error_message = "Continuous/PITR backup must be enabled by default."
  }

  assert {
    condition     = aws_backup_vault_lock_configuration.this.min_retention_days == 7
    error_message = "Vault Lock must enforce the declared minimum retention."
  }

  assert {
    condition     = length(aws_iam_role.backup) == 1
    error_message = "A service role must be created when no existing role is supplied."
  }
}
