data "aws_caller_identity" "current" {}
data "aws_partition" "current" {}
data "aws_region" "current" {}

locals {
  common_tags = merge(var.tags, { ManagedBy = "OpenTofu" })
}

resource "aws_cloudwatch_log_group" "this" {
  name              = "/aws/codebuild/${var.name}"
  retention_in_days = var.log_retention_days
  kms_key_id        = var.log_kms_key_arn
  tags              = local.common_tags
}

resource "aws_security_group" "this" {
  name_prefix = "${var.name}-"
  description = "Egress-only security group for the private administration runner"
  vpc_id      = var.vpc_id

  dynamic "egress" {
    for_each = var.egress_cidr_blocks
    content {
      description = "Approved administration egress"
      from_port   = 0
      to_port     = 0
      protocol    = "-1"
      cidr_blocks = [egress.value]
    }
  }

  tags = merge(local.common_tags, { Name = var.name })
}

resource "aws_iam_role" "this" {
  name = var.name
  assume_role_policy = jsonencode({
    Version = "2012-10-17"
    Statement = [{
      Effect    = "Allow"
      Principal = { Service = "codebuild.amazonaws.com" }
      Action    = "sts:AssumeRole"
    }]
  })
  tags = local.common_tags
}

resource "aws_iam_role_policy" "this" {
  name = "private-cluster-administration"
  role = aws_iam_role.this.id
  policy = jsonencode({
    Version = "2012-10-17"
    Statement = concat(
      [
        {
          Sid    = "RunnerLogs"
          Effect = "Allow"
          Action = [
            "logs:CreateLogStream",
            "logs:PutLogEvents",
          ]
          Resource = "${aws_cloudwatch_log_group.this.arn}:*"
        },
        {
          Sid    = "VpcNetworkInterfaces"
          Effect = "Allow"
          Action = [
            "ec2:CreateNetworkInterface",
            "ec2:DeleteNetworkInterface",
            "ec2:DescribeDhcpOptions",
            "ec2:DescribeNetworkInterfaces",
            "ec2:DescribeSecurityGroups",
            "ec2:DescribeSubnets",
            "ec2:DescribeVpcs",
          ]
          Resource = "*"
        },
        {
          Sid      = "VpcNetworkInterfacePermission"
          Effect   = "Allow"
          Action   = "ec2:CreateNetworkInterfacePermission"
          Resource = "arn:${data.aws_partition.current.partition}:ec2:${data.aws_region.current.name}:${data.aws_caller_identity.current.account_id}:network-interface/*"
          Condition = {
            StringEquals = {
              "ec2:AuthorizedService" = "codebuild.amazonaws.com"
            }
            ArnEquals = {
              "ec2:Subnet" = [
                for subnet_id in sort(tolist(var.subnet_ids)) :
                "arn:${data.aws_partition.current.partition}:ec2:${data.aws_region.current.name}:${data.aws_caller_identity.current.account_id}:subnet/${subnet_id}"
              ]
            }
          }
        },
        {
          Sid      = "PrivateClusterDiscovery"
          Effect   = "Allow"
          Action   = ["eks:AccessKubernetesApi", "eks:DescribeCluster"]
          Resource = var.eks_cluster_arn
        },
      ],
      length(var.assumable_role_arns) == 0 ? [] : [{
        Sid      = "ApprovedBreakGlassRoles"
        Effect   = "Allow"
        Action   = "sts:AssumeRole"
        Resource = sort(tolist(var.assumable_role_arns))
      }],
      length(var.kms_decrypt_key_arns) == 0 ? [] : [{
        Sid      = "ApprovedDecryptKeys"
        Effect   = "Allow"
        Action   = ["kms:Decrypt", "kms:DescribeKey"]
        Resource = sort(tolist(var.kms_decrypt_key_arns))
      }],
    )
  })
}

resource "aws_codebuild_project" "this" {
  name          = var.name
  description   = "VPC-connected runner for approved private-cluster bootstrap and break-glass tasks"
  service_role  = aws_iam_role.this.arn
  build_timeout = 60

  artifacts {
    type = "NO_ARTIFACTS"
  }

  source {
    type      = "NO_SOURCE"
    buildspec = var.buildspec
  }

  environment {
    compute_type                = "BUILD_GENERAL1_SMALL"
    image                       = var.build_image
    type                        = "LINUX_CONTAINER"
    image_pull_credentials_type = "CODEBUILD"
    privileged_mode             = false

    environment_variable {
      name  = "AWS_REGION"
      value = data.aws_region.current.name
      type  = "PLAINTEXT"
    }

    environment_variable {
      name  = "EKS_CLUSTER_NAME"
      value = var.eks_cluster_name
      type  = "PLAINTEXT"
    }
  }

  vpc_config {
    vpc_id             = var.vpc_id
    subnets            = sort(tolist(var.subnet_ids))
    security_group_ids = [aws_security_group.this.id]
  }

  logs_config {
    cloudwatch_logs {
      group_name  = aws_cloudwatch_log_group.this.name
      stream_name = "administration"
      status      = "ENABLED"
    }
  }

  tags = local.common_tags
}
