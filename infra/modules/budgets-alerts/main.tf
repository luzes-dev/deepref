data "aws_partition" "current" {}
data "aws_region" "current" {}

locals {
  common_tags = merge(var.tags, { ManagedBy = "OpenTofu" })
  root_arn    = "arn:${data.aws_partition.current.partition}:iam::${var.account_id}:root"
}

resource "aws_sns_topic" "operations" {
  name              = var.name
  kms_master_key_id = var.sns_kms_key_arn
  tags              = local.common_tags
}

resource "aws_sns_topic_policy" "operations" {
  arn = aws_sns_topic.operations.arn
  policy = jsonencode({
    Version = "2012-10-17"
    Statement = [
      {
        Sid       = "AccountOwnerAdministration"
        Effect    = "Allow"
        Principal = { AWS = local.root_arn }
        Action    = "SNS:*"
        Resource  = aws_sns_topic.operations.arn
      },
      {
        Sid       = "BudgetsPublish"
        Effect    = "Allow"
        Principal = { Service = "budgets.amazonaws.com" }
        Action    = "sns:Publish"
        Resource  = aws_sns_topic.operations.arn
        Condition = {
          StringEquals = { "aws:SourceAccount" = var.account_id }
          ArnLike      = { "aws:SourceArn" = "arn:${data.aws_partition.current.partition}:budgets::${var.account_id}:budget/*" }
        }
      },
      {
        Sid       = "CloudWatchPublish"
        Effect    = "Allow"
        Principal = { Service = "cloudwatch.amazonaws.com" }
        Action    = "sns:Publish"
        Resource  = aws_sns_topic.operations.arn
        Condition = {
          StringEquals = { "aws:SourceAccount" = var.account_id }
          ArnLike      = { "aws:SourceArn" = "arn:${data.aws_partition.current.partition}:cloudwatch:${data.aws_region.current.name}:${var.account_id}:alarm:*" }
        }
      },
    ]
  })
}

resource "aws_sns_topic_subscription" "email" {
  for_each = var.email_subscribers

  topic_arn = aws_sns_topic.operations.arn
  protocol  = "email"
  endpoint  = each.value
}

resource "aws_budgets_budget" "monthly" {
  name         = var.name
  budget_type  = "COST"
  limit_amount = tostring(var.monthly_budget_amount)
  limit_unit   = var.currency
  time_unit    = "MONTHLY"

  dynamic "notification" {
    for_each = var.budget_notifications
    content {
      comparison_operator        = "GREATER_THAN"
      threshold                  = notification.value.threshold
      threshold_type             = "PERCENTAGE"
      notification_type          = notification.value.notification_type
      subscriber_sns_topic_arns  = [aws_sns_topic.operations.arn]
    }
  }

  depends_on = [aws_sns_topic_policy.operations]
}

resource "aws_cloudwatch_metric_alarm" "this" {
  for_each = var.metric_alarms

  alarm_name                = each.key
  alarm_description         = each.value.alarm_description
  namespace                 = each.value.namespace
  metric_name               = each.value.metric_name
  statistic                 = each.value.statistic
  period                    = each.value.period_seconds
  evaluation_periods        = each.value.evaluation_periods
  datapoints_to_alarm       = each.value.datapoints_to_alarm
  threshold                 = each.value.threshold
  comparison_operator       = each.value.comparison_operator
  dimensions                = each.value.dimensions
  treat_missing_data        = each.value.treat_missing_data
  alarm_actions             = [aws_sns_topic.operations.arn]
  ok_actions                = [aws_sns_topic.operations.arn]
  insufficient_data_actions = each.value.insufficient_data_actions ? [aws_sns_topic.operations.arn] : []
  tags                      = local.common_tags

  depends_on = [aws_sns_topic_policy.operations]
}
