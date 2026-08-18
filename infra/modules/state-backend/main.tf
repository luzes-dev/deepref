locals {
  common_tags = merge(var.tags, { ManagedBy = "OpenTofu" })
  root_arn    = "arn:${data.aws_partition.current.partition}:iam::${var.account_id}:root"
}

data "aws_partition" "current" {}

resource "aws_kms_key" "state" {
  description             = "OpenTofu state encryption for ${var.bucket_name}"
  deletion_window_in_days = var.kms_deletion_window_days
  enable_key_rotation     = true
  policy = jsonencode({
    Version = "2012-10-17"
    Statement = concat(
      [{
        Sid       = "AccountRootDelegatesThroughIAM"
        Effect    = "Allow"
        Principal = { AWS = local.root_arn }
        Action    = "kms:*"
        Resource  = "*"
      }],
      length(var.kms_administrator_principal_arns) == 0 ? [] : [{
        Sid       = "KeyAdministration"
        Effect    = "Allow"
        Principal = { AWS = sort(tolist(var.kms_administrator_principal_arns)) }
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
      [{
        Sid       = "StateCryptographicUse"
        Effect    = "Allow"
        Principal = { AWS = sort(tolist(var.state_access_principal_arns)) }
        Action = [
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

  tags = merge(local.common_tags, { Name = var.kms_alias })

  lifecycle {
    prevent_destroy = true
  }
}

resource "aws_kms_alias" "state" {
  name          = var.kms_alias
  target_key_id = aws_kms_key.state.key_id
}

resource "aws_s3_bucket" "state" {
  bucket = var.bucket_name
  tags   = merge(local.common_tags, { Name = var.bucket_name })

  lifecycle {
    prevent_destroy = true
  }
}

resource "aws_s3_bucket_ownership_controls" "state" {
  bucket = aws_s3_bucket.state.id

  rule {
    object_ownership = "BucketOwnerEnforced"
  }
}

resource "aws_s3_bucket_public_access_block" "state" {
  bucket = aws_s3_bucket.state.id

  block_public_acls       = true
  block_public_policy     = true
  ignore_public_acls      = true
  restrict_public_buckets = true
}

resource "aws_s3_bucket_versioning" "state" {
  bucket = aws_s3_bucket.state.id

  versioning_configuration {
    status = "Enabled"
  }
}

resource "aws_s3_bucket_server_side_encryption_configuration" "state" {
  bucket = aws_s3_bucket.state.id

  rule {
    bucket_key_enabled = true
    apply_server_side_encryption_by_default {
      kms_master_key_id = aws_kms_key.state.arn
      sse_algorithm     = "aws:kms"
    }
  }
}

resource "aws_s3_bucket_lifecycle_configuration" "state" {
  bucket = aws_s3_bucket.state.id

  rule {
    id     = "retain-state-history"
    status = "Enabled"

    filter {}

    abort_incomplete_multipart_upload {
      days_after_initiation = 7
    }

    noncurrent_version_expiration {
      noncurrent_days = var.noncurrent_version_retention_days
    }
  }

  depends_on = [aws_s3_bucket_versioning.state]
}

resource "aws_s3_bucket_policy" "state" {
  bucket = aws_s3_bucket.state.id
  policy = jsonencode({
    Version = "2012-10-17"
    Statement = [
      {
        Sid       = "DenyInsecureTransport"
        Effect    = "Deny"
        Principal = "*"
        Action    = "s3:*"
        Resource = [
          aws_s3_bucket.state.arn,
          "${aws_s3_bucket.state.arn}/*",
        ]
        Condition = { Bool = { "aws:SecureTransport" = "false" } }
      },
      {
        Sid       = "DenyUnencryptedObjectUploads"
        Effect    = "Deny"
        Principal = "*"
        Action    = "s3:PutObject"
        Resource  = "${aws_s3_bucket.state.arn}/*"
        Condition = {
          StringNotEquals = { "s3:x-amz-server-side-encryption" = "aws:kms" }
        }
      },
      {
        Sid       = "DenyWrongEncryptionKey"
        Effect    = "Deny"
        Principal = "*"
        Action    = "s3:PutObject"
        Resource  = "${aws_s3_bucket.state.arn}/*"
        Condition = {
          StringNotEquals = { "s3:x-amz-server-side-encryption-aws-kms-key-id" = aws_kms_key.state.arn }
        }
      },
      {
        Sid       = "StateBucketMetadata"
        Effect    = "Allow"
        Principal = { AWS = sort(tolist(var.state_access_principal_arns)) }
        Action = [
          "s3:GetBucketVersioning",
          "s3:ListBucket",
        ]
        Resource = aws_s3_bucket.state.arn
      },
      {
        Sid       = "StateAndNativeLockObjects"
        Effect    = "Allow"
        Principal = { AWS = sort(tolist(var.state_access_principal_arns)) }
        Action = [
          "s3:DeleteObject",
          "s3:GetObject",
          "s3:GetObjectTagging",
          "s3:GetObjectVersion",
          "s3:PutObject",
          "s3:PutObjectTagging",
        ]
        Resource = "${aws_s3_bucket.state.arn}/*"
      },
    ]
  })

  depends_on = [aws_s3_bucket_public_access_block.state]
}
