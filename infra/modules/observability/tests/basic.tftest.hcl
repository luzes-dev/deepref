mock_provider "aws" {}

run "managed_encrypted_observability" {
  command = plan

  variables {
    name                = "ambient-scribes-test"
    amp_kms_key_arn     = "arn:aws:kms:sa-east-1:111111111111:key/amp"
    grafana_kms_key_arn = "arn:aws:kms:sa-east-1:111111111111:key/grafana"
    logs_kms_key_arn    = "arn:aws:kms:sa-east-1:111111111111:key/logs"
    xray_kms_key_arn    = "arn:aws:kms:sa-east-1:111111111111:key/xray"
    log_groups = {
      amp         = { name = "/ambient-scribes/test/amp", retention_in_days = 30 }
      application = { name = "/ambient-scribes/test/application", retention_in_days = 30 }
      adot        = { name = "/ambient-scribes/test/adot", retention_in_days = 30 }
    }
  }

  assert {
    condition     = aws_prometheus_workspace.this.kms_key_arn != null
    error_message = "AMP must use a customer-managed KMS key."
  }

  assert {
    condition     = aws_grafana_workspace.this.authentication_providers[0] == "AWS_SSO"
    error_message = "Managed Grafana must authenticate through IAM Identity Center."
  }

  assert {
    condition     = aws_grafana_workspace.this.kms_key_id != null
    error_message = "Managed Grafana workspace data must use a customer-managed KMS key."
  }

  assert {
    condition     = alltrue([for group in aws_cloudwatch_log_group.this : group.retention_in_days == 30 && group.kms_key_id != null])
    error_message = "Every test log group must be encrypted and retained for 30 days."
  }

  assert {
    condition     = aws_xray_encryption_config.this.type == "KMS"
    error_message = "X-Ray traces must use the declared customer-managed key."
  }

  assert {
    condition     = can(regex("aps:RemoteWrite", aws_iam_policy.adot.policy))
    error_message = "The ADOT policy must permit remote write to AMP."
  }
}
