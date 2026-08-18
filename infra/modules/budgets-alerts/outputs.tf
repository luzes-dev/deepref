output "topic_arn" {
  description = "Encrypted operations SNS topic ARN."
  value       = aws_sns_topic.operations.arn
}

output "budget_name" {
  description = "Monthly AWS Budget name."
  value       = aws_budgets_budget.monthly.name
}

output "alarm_arns" {
  description = "CloudWatch alarm ARNs keyed by alarm name."
  value       = { for name, alarm in aws_cloudwatch_metric_alarm.this : name => alarm.arn }
}

output "email_subscription_arns" {
  description = "Email subscription ARNs; pending subscriptions remain unconfirmed until recipients act."
  value       = { for address, subscription in aws_sns_topic_subscription.email : address => subscription.arn }
}
