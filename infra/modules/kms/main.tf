locals {
  common_tags = merge(var.tags, { ManagedBy = "OpenTofu" })
  account_root_arn = format(
    "arn:aws:iam::%s:root",
    var.account_id,
  )
}

resource "aws_kms_key" "this" {
  for_each = var.keys

  description             = each.value.description
  deletion_window_in_days = each.value.deletion_window_days
  enable_key_rotation     = true
  multi_region            = each.value.multi_region

  policy = jsonencode({
    Version = "2012-10-17"
    Statement = concat(
      [{
        Sid       = "EnableAccountPermissions"
        Effect    = "Allow"
        Principal = { AWS = local.account_root_arn }
        Action    = "kms:*"
        Resource  = "*"
      }],
      length(var.administrator_principal_arns) == 0 ? [] : [{
        Sid       = "KeyAdministration"
        Effect    = "Allow"
        Principal = { AWS = sort(tolist(var.administrator_principal_arns)) }
        Action = [
          "kms:CancelKeyDeletion",
          "kms:CreateAlias",
          "kms:CreateGrant",
          "kms:DeleteAlias",
          "kms:DescribeKey",
          "kms:DisableKey",
          "kms:EnableKey",
          "kms:EnableKeyRotation",
          "kms:GetKeyPolicy",
          "kms:GetKeyRotationStatus",
          "kms:ListGrants",
          "kms:ListResourceTags",
          "kms:PutKeyPolicy",
          "kms:RevokeGrant",
          "kms:ScheduleKeyDeletion",
          "kms:TagResource",
          "kms:UntagResource",
          "kms:UpdateKeyDescription",
        ]
        Resource = "*"
      }],
      length(each.value.service_principals) == 0 ? [] : [{
        Sid       = "ServiceCryptographicUse"
        Effect    = "Allow"
        Principal = { Service = sort(tolist(each.value.service_principals)) }
        Action = [
          "kms:Decrypt",
          "kms:DescribeKey",
          "kms:Encrypt",
          "kms:GenerateDataKey*",
          "kms:ReEncrypt*",
        ]
        Resource = "*"
      }, {
        Sid       = "ServiceGrantManagement"
        Effect    = "Allow"
        Principal = { Service = sort(tolist(each.value.service_principals)) }
        Action    = "kms:CreateGrant"
        Resource  = "*"
        Condition = {
          Bool = { "kms:GrantIsForAWSResource" = "true" }
        }
      }],
      length(each.value.user_principal_arns) == 0 ? [] : [{
        Sid       = "PrincipalUse"
        Effect    = "Allow"
        Principal = { AWS = sort(tolist(each.value.user_principal_arns)) }
        Action = [
          "kms:CreateGrant",
          "kms:Decrypt",
          "kms:DescribeKey",
          "kms:Encrypt",
          "kms:GenerateDataKey*",
          "kms:ReEncrypt*",
        ]
        Resource = "*"
      }],
    )
  })

  tags = merge(local.common_tags, { Name = each.value.alias })
}

resource "aws_kms_alias" "this" {
  for_each = var.keys

  name          = each.value.alias
  target_key_id = aws_kms_key.this[each.key].key_id
}
