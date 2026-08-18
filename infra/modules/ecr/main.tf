locals {
  common_tags           = merge(var.tags, { ManagedBy = "OpenTofu" })
  create_promotion_role = length(var.promotion_trusted_principal_arns) > 0 || var.promotion_oidc_provider_arn != null
  promotion_oidc_issuer = var.promotion_oidc_provider_arn == null ? null : split("oidc-provider/", var.promotion_oidc_provider_arn)[1]
}

resource "aws_ecr_repository" "this" {
  for_each = var.repositories

  name                 = each.value.name
  image_tag_mutability = "IMMUTABLE"

  encryption_configuration {
    encryption_type = "KMS"
    kms_key         = var.kms_key_arn
  }

  image_scanning_configuration {
    scan_on_push = true
  }

  tags = merge(local.common_tags, { Name = each.value.name })
}

resource "aws_ecr_lifecycle_policy" "this" {
  for_each = var.repositories

  repository = aws_ecr_repository.this[each.key].name
  policy = jsonencode({
    rules = [
      {
        rulePriority = 1
        description  = "Expire untagged images"
        selection = {
          tagStatus   = "untagged"
          countType   = "sinceImagePushed"
          countUnit   = "days"
          countNumber = each.value.expire_untagged_after
        }
        action = { type = "expire" }
      },
      {
        rulePriority = 2
        description  = "Retain bounded immutable release history"
        selection = {
          tagStatus     = "tagged"
          tagPrefixList = ["sha-", "tree-", "v"]
          countType     = "imageCountMoreThan"
          countNumber   = each.value.retain_tagged_images
        }
        action = { type = "expire" }
      },
    ]
  })
}

resource "aws_ecr_repository_policy" "pull" {
  for_each = length(var.repository_pull_principal_arns) == 0 ? {} : var.repositories

  repository = aws_ecr_repository.this[each.key].name
  policy = jsonencode({
    Version = "2012-10-17"
    Statement = [{
      Sid       = "CrossAccountPull"
      Effect    = "Allow"
      Principal = { AWS = sort(tolist(var.repository_pull_principal_arns)) }
      Action = [
        "ecr:BatchCheckLayerAvailability",
        "ecr:BatchGetImage",
        "ecr:GetDownloadUrlForLayer",
      ]
    }]
  })
}

resource "aws_iam_role" "promotion" {
  count = local.create_promotion_role ? 1 : 0

  name = "${var.name_prefix}-ecr-promotion"
  assume_role_policy = jsonencode({
    Version = "2012-10-17"
    Statement = concat(
      length(var.promotion_trusted_principal_arns) == 0 ? [] : [{
        Sid       = "TrustedAWSPrincipals"
        Effect    = "Allow"
        Principal = { AWS = sort(tolist(var.promotion_trusted_principal_arns)) }
        Action    = "sts:AssumeRole"
      }],
      var.promotion_oidc_provider_arn == null ? [] : [{
        Sid       = "TrustedOIDCSubjects"
        Effect    = "Allow"
        Principal = { Federated = var.promotion_oidc_provider_arn }
        Action    = "sts:AssumeRoleWithWebIdentity"
        Condition = {
          StringEquals = {
            "${local.promotion_oidc_issuer}:aud" = "sts.amazonaws.com"
          }
          StringLike = {
            "${local.promotion_oidc_issuer}:sub" = sort(tolist(var.promotion_oidc_subjects))
          }
        }
      }],
    )
  })
  tags = local.common_tags

  lifecycle {
    precondition {
      condition     = var.promotion_oidc_provider_arn == null || length(var.promotion_oidc_subjects) > 0
      error_message = "promotion_oidc_subjects must be explicit when an OIDC provider is configured."
    }
  }
}

resource "aws_iam_role_policy" "promotion" {
  count = local.create_promotion_role ? 1 : 0

  name = "copy-exact-oci-manifests"
  role = aws_iam_role.promotion[0].id
  policy = jsonencode({
    Version = "2012-10-17"
    Statement = [
      {
        Sid      = "RegistryLogin"
        Effect   = "Allow"
        Action   = "ecr:GetAuthorizationToken"
        Resource = "*"
      },
      {
        Sid    = "ReadSourceArtifacts"
        Effect = "Allow"
        Action = [
          "ecr:BatchCheckLayerAvailability",
          "ecr:BatchGetImage",
          "ecr:GetDownloadUrlForLayer",
        ]
        Resource = sort(tolist(var.promotion_source_repository_arns))
      },
      {
        Sid    = "WriteDestinationArtifacts"
        Effect = "Allow"
        Action = [
          "ecr:BatchCheckLayerAvailability",
          "ecr:CompleteLayerUpload",
          "ecr:InitiateLayerUpload",
          "ecr:PutImage",
          "ecr:UploadLayerPart",
        ]
        Resource = [for repository in aws_ecr_repository.this : repository.arn]
      },
    ]
  })

  lifecycle {
    precondition {
      condition     = length(var.promotion_source_repository_arns) > 0
      error_message = "A promotion role requires at least one explicit source repository ARN."
    }
  }
}
