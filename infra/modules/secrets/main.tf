locals {
  common_tags = merge(var.tags, { ManagedBy = "OpenTofu" })
  secrets_with_readers = {
    for name, config in var.secrets : name => config
    if length(config.reader_principal_arns) > 0
  }
}

resource "aws_secretsmanager_secret" "this" {
  for_each = var.secrets

  name                    = each.value.name
  description             = each.value.description
  kms_key_id              = var.kms_key_arn
  recovery_window_in_days = each.value.recovery_window_days

  dynamic "replica" {
    for_each = each.value.replica_regions
    content {
      region     = replica.value
      kms_key_id = lookup(var.replica_kms_key_arns, replica.value, null)
    }
  }

  tags = merge(local.common_tags, { Name = each.value.name })
}

resource "aws_secretsmanager_secret_policy" "reader" {
  for_each = local.secrets_with_readers

  secret_arn          = aws_secretsmanager_secret.this[each.key].arn
  block_public_policy = true
  policy = jsonencode({
    Version = "2012-10-17"
    Statement = [{
      Sid       = "ReadSecret"
      Effect    = "Allow"
      Principal = { AWS = sort(tolist(each.value.reader_principal_arns)) }
      Action = [
        "secretsmanager:DescribeSecret",
        "secretsmanager:GetSecretValue",
      ]
      Resource = aws_secretsmanager_secret.this[each.key].arn
    }]
  })
}
