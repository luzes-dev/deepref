mock_provider "aws" {}

run "encrypted_budget_and_alarm_topic" {
  command = plan

  variables {
    account_id            = "111111111111"
    name                  = "ambient-scribes-test-operations"
    sns_kms_key_arn       = "arn:aws:kms:sa-east-1:111111111111:key/00000000-0000-0000-0000-000000000000"
    monthly_budget_amount = 250
    email_subscribers     = ["operations@example.invalid"]
    metric_alarms = {
      rds_cpu = {
        namespace           = "AWS/RDS"
        metric_name         = "CPUUtilization"
        threshold           = 80
        comparison_operator = "GreaterThanThreshold"
        dimensions          = { DBInstanceIdentifier = "ambient-scribes-test" }
      }
    }
  }

  assert {
    condition     = aws_sns_topic.operations.kms_master_key_id != null
    error_message = "The operations topic must be KMS encrypted."
  }

  assert {
    condition     = length(aws_budgets_budget.monthly.notification) == 3
    error_message = "Default actual and forecast budget thresholds must be present."
  }

  assert {
    condition     = aws_cloudwatch_metric_alarm.this["rds_cpu"].alarm_actions[0] == aws_sns_topic.operations.arn
    error_message = "Metric alarms must publish to the operations topic."
  }
}
