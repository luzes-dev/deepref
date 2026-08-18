locals {
  common_tags = merge(var.tags, { ManagedBy = "OpenTofu" })
}

resource "aws_cloudwatch_log_group" "this" {
  for_each = var.log_groups

  name              = each.value.name
  retention_in_days = each.value.retention_in_days
  kms_key_id        = var.logs_kms_key_arn
  tags              = merge(local.common_tags, { Name = each.value.name })
}

resource "aws_prometheus_workspace" "this" {
  alias       = var.name
  kms_key_arn = var.amp_kms_key_arn

  logging_configuration {
    log_group_arn = "${aws_cloudwatch_log_group.this["amp"].arn}:*"
  }

  tags = local.common_tags
}

resource "aws_iam_role" "grafana" {
  name = "${var.name}-grafana"
  assume_role_policy = jsonencode({
    Version = "2012-10-17"
    Statement = [{
      Effect    = "Allow"
      Principal = { Service = "grafana.amazonaws.com" }
      Action    = "sts:AssumeRole"
    }]
  })
  tags = local.common_tags
}

resource "aws_iam_role_policy" "grafana" {
  name = "observability-read"
  role = aws_iam_role.grafana.id
  policy = jsonencode({
    Version = "2012-10-17"
    Statement = [
      {
        Sid    = "PrometheusQuery"
        Effect = "Allow"
        Action = [
          "aps:GetLabels",
          "aps:GetMetricMetadata",
          "aps:GetSeries",
          "aps:QueryMetrics",
        ]
        Resource = aws_prometheus_workspace.this.arn
      },
      {
        Sid    = "CloudWatchRead"
        Effect = "Allow"
        Action = [
          "cloudwatch:DescribeAlarms",
          "cloudwatch:GetMetricData",
          "cloudwatch:GetMetricStatistics",
          "cloudwatch:ListMetrics",
          "ec2:DescribeTags",
          "logs:DescribeLogGroups",
          "logs:GetLogEvents",
          "logs:GetLogGroupFields",
          "logs:GetLogRecord",
          "logs:GetQueryResults",
          "logs:StartQuery",
          "logs:StopQuery",
        ]
        Resource = "*"
      },
      {
        Sid    = "XRayRead"
        Effect = "Allow"
        Action = [
          "xray:BatchGetTraces",
          "xray:GetGroups",
          "xray:GetInsight",
          "xray:GetInsightEvents",
          "xray:GetInsightImpactGraph",
          "xray:GetInsightSummaries",
          "xray:GetSamplingRules",
          "xray:GetSamplingStatisticSummaries",
          "xray:GetServiceGraph",
          "xray:GetTimeSeriesServiceStatistics",
          "xray:GetTraceGraph",
          "xray:GetTraceSummaries",
        ]
        Resource = "*"
      },
    ]
  })
}

resource "aws_grafana_workspace" "this" {
  name                     = var.name
  account_access_type      = "CURRENT_ACCOUNT"
  authentication_providers = ["AWS_SSO"]
  data_sources             = ["CLOUDWATCH", "PROMETHEUS", "XRAY"]
  kms_key_id               = var.grafana_kms_key_arn
  permission_type          = "CUSTOMER_MANAGED"
  role_arn                 = aws_iam_role.grafana.arn
  tags                     = local.common_tags
}

resource "aws_grafana_role_association" "admin" {
  count = length(var.grafana_admin_user_ids) > 0 ? 1 : 0

  role         = "ADMIN"
  user_ids     = sort(tolist(var.grafana_admin_user_ids))
  workspace_id = aws_grafana_workspace.this.id
}

resource "aws_grafana_role_association" "editor" {
  count = length(var.grafana_editor_user_ids) > 0 ? 1 : 0

  role         = "EDITOR"
  user_ids     = sort(tolist(var.grafana_editor_user_ids))
  workspace_id = aws_grafana_workspace.this.id
}

resource "aws_grafana_role_association" "viewer" {
  count = length(var.grafana_viewer_user_ids) > 0 ? 1 : 0

  role         = "VIEWER"
  user_ids     = sort(tolist(var.grafana_viewer_user_ids))
  workspace_id = aws_grafana_workspace.this.id
}

resource "aws_iam_policy" "adot" {
  name        = "${var.name}-adot"
  description = "ADOT remote-write, trace, and telemetry permissions"
  policy = jsonencode({
    Version = "2012-10-17"
    Statement = [
      {
        Sid      = "PrometheusRemoteWrite"
        Effect   = "Allow"
        Action   = "aps:RemoteWrite"
        Resource = aws_prometheus_workspace.this.arn
      },
      {
        Sid    = "XRayWriteAndSampling"
        Effect = "Allow"
        Action = [
          "xray:GetSamplingRules",
          "xray:GetSamplingStatisticSummaries",
          "xray:GetSamplingTargets",
          "xray:PutTelemetryRecords",
          "xray:PutTraceSegments",
        ]
        Resource = "*"
      },
      {
        Sid      = "CloudWatchMetricsWrite"
        Effect   = "Allow"
        Action   = "cloudwatch:PutMetricData"
        Resource = "*"
        Condition = {
          StringEquals = { "cloudwatch:namespace" = "${var.name}/ADOT" }
        }
      },
    ]
  })
  tags = local.common_tags
}

resource "aws_xray_encryption_config" "this" {
  type   = "KMS"
  key_id = var.xray_kms_key_arn
}

resource "aws_xray_group" "this" {
  group_name        = var.name
  filter_expression = "service(\"${var.name}\")"

  insights_configuration {
    insights_enabled      = true
    notifications_enabled = false
  }

  tags = local.common_tags

  depends_on = [aws_xray_encryption_config.this]
}

resource "aws_xray_sampling_rule" "this" {
  for_each = var.xray_sampling_rules

  rule_name      = "${var.name}-${each.key}"
  priority       = each.value.priority
  version        = 1
  reservoir_size = each.value.reservoir_size
  fixed_rate     = each.value.fixed_rate
  host           = each.value.host
  http_method    = each.value.http_method
  url_path       = each.value.url_path
  service_name   = each.value.service_name
  service_type   = each.value.service_type
  resource_arn   = each.value.resource_arn
  attributes     = each.value.attributes
  tags           = local.common_tags

  depends_on = [aws_xray_encryption_config.this]
}
