locals {
  environment = "production"
  name        = "${var.project_name}-${local.environment}"

  public_subnet_cidrs  = [for index in range(3) : cidrsubnet(var.vpc_cidr, 4, index)]
  private_subnet_cidrs = [for index in range(3) : cidrsubnet(var.vpc_cidr, 4, index + 3)]
  data_subnet_cidrs    = [for index in range(3) : cidrsubnet(var.vpc_cidr, 4, index + 6)]

  secret_definitions = {
    application = "Runtime application configuration"
    cloudflare  = "Cloudflare tunnel credentials"
    github_app  = "Deployment GitHub App credentials"
    nats        = "NATS application credentials"
    neo4j       = "Neo4j application credentials"
  }

  common_tags = merge(var.tags, {
    Environment = local.environment
    Project     = var.project_name
  })
}

resource "terraform_data" "account_guard" {
  input = data.aws_caller_identity.current.account_id

  lifecycle {
    precondition {
      condition     = data.aws_caller_identity.current.account_id == var.expected_account_id
      error_message = "Refusing to manage production from an unexpected AWS account."
    }

    precondition {
      condition     = terraform.workspace == "default"
      error_message = "This repository uses isolated roots; OpenTofu workspaces are forbidden."
    }
  }
}

module "kms" {
  source = "../../modules/kms"

  account_id                   = var.expected_account_id
  administrator_principal_arns = var.kms_administrator_principal_arns
  keys = {
    ecr = {
      alias              = "alias/${local.name}-ecr"
      description        = "ECR encryption for ${local.name}"
      service_principals = ["ecr.amazonaws.com"]
    }
    eks = {
      alias              = "alias/${local.name}-eks"
      description        = "EKS envelope and node volume encryption for ${local.name}"
      service_principals = ["ec2.amazonaws.com", "eks.amazonaws.com"]
      user_principal_arns = [
        "arn:aws:iam::${var.expected_account_id}:role/aws-service-role/autoscaling.amazonaws.com/AWSServiceRoleForAutoScaling",
      ]
    }
    logs = {
      alias       = "alias/${local.name}-logs"
      description = "CloudWatch Logs encryption for ${local.name}"
      service_principals = [
        "delivery.logs.amazonaws.com",
        "logs.${var.aws_region}.amazonaws.com",
      ]
    }
    rds = {
      alias              = "alias/${local.name}-rds"
      description        = "RDS encryption for ${local.name}"
      service_principals = ["rds.amazonaws.com"]
    }
    secrets = {
      alias              = "alias/${local.name}-secrets"
      description        = "Secrets Manager encryption for ${local.name}"
      service_principals = ["secretsmanager.amazonaws.com"]
    }
  }
  tags = local.common_tags

  depends_on = [terraform_data.account_guard]
}

module "network" {
  source = "../../modules/network"

  name                    = local.name
  vpc_cidr                = var.vpc_cidr
  availability_zones      = var.availability_zones
  public_subnet_cidrs     = local.public_subnet_cidrs
  private_subnet_cidrs    = local.private_subnet_cidrs
  data_subnet_cidrs       = local.data_subnet_cidrs
  nat_gateway_mode        = "one_per_az"
  flow_log_kms_key_arn    = module.kms.key_arns["logs"]
  flow_log_retention_days = 365
  tags                    = local.common_tags

  depends_on = [terraform_data.account_guard]
}

module "secrets" {
  source = "../../modules/secrets"

  kms_key_arn = module.kms.key_arns["secrets"]
  secrets = {
    for name, description in local.secret_definitions : name => {
      name                  = "${local.name}/${replace(name, "_", "-")}"
      description           = description
      reader_principal_arns = lookup(var.secret_reader_principal_arns, name, [])
      recovery_window_days  = 30
    }
  }
  tags = local.common_tags

  depends_on = [terraform_data.account_guard]
}

module "ecr" {
  source = "../../modules/ecr"

  name_prefix = local.name
  kms_key_arn = module.kms.key_arns["ecr"]
  repositories = {
    api         = { name = "${var.project_name}/api", retain_tagged_images = 200 }
    chart       = { name = "${var.project_name}/charts/platform", retain_tagged_images = 200 }
    projector   = { name = "${var.project_name}/projector", retain_tagged_images = 200 }
    third_party = { name = "${var.project_name}/third-party", retain_tagged_images = 200 }
    web         = { name = "${var.project_name}/web", retain_tagged_images = 200 }
    worker      = { name = "${var.project_name}/worker", retain_tagged_images = 200 }
  }
  repository_pull_principal_arns   = var.repository_pull_principal_arns
  promotion_trusted_principal_arns = var.promotion_trusted_principal_arns
  promotion_oidc_provider_arn       = var.promotion_oidc_provider_arn
  promotion_oidc_subjects           = var.promotion_oidc_subjects
  promotion_source_repository_arns = var.promotion_source_repository_arns
  tags                              = local.common_tags

  depends_on = [terraform_data.account_guard]
}

module "eks" {
  source = "../../modules/eks"

  name                             = local.name
  kubernetes_version               = "1.36"
  private_subnets_by_az            = zipmap(var.availability_zones, module.network.private_subnet_ids)
  cluster_kms_key_arn              = module.kms.key_arns["eks"]
  control_plane_log_kms_key_arn    = module.kms.key_arns["logs"]
  node_volume_kms_key_arn          = module.kms.key_arns["eks"]
  control_plane_log_retention_days = 365
  access_entries                   = var.eks_access_entries
  stateful_node_count              = 3
  stateful_instance_types          = ["m7g.xlarge"]
  stateless_instance_types         = ["m7g.xlarge", "m7g.2xlarge"]
  stateless_min_size               = 3
  stateless_desired_size           = 6
  stateless_max_size               = 18
  tags                             = local.common_tags

  depends_on = [terraform_data.account_guard]
}

module "rds" {
  source = "../../modules/rds"

  name                           = local.name
  deployment_tier                = local.environment
  vpc_id                         = module.network.vpc_id
  subnet_ids                     = module.network.data_subnet_ids
  application_security_group_ids = [module.eks.cluster_primary_security_group_id]
  allowed_cidr_blocks            = var.database_allowed_cidrs
  instance_class                 = var.database_instance_class
  allocated_storage_gib          = var.database_allocated_storage_gib
  max_allocated_storage_gib      = var.database_max_allocated_storage_gib
  multi_az                       = true
  deletion_protection            = true
  backup_retention_days          = 35
  kms_key_arn                    = module.kms.key_arns["rds"]
  master_secret_kms_key_arn      = module.kms.key_arns["secrets"]
  performance_insights_retention_days = 731
  tags                                = local.common_tags

  depends_on = [terraform_data.account_guard]
}
