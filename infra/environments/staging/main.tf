locals {
  environment = "staging"
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
      error_message = "Refusing to manage staging from an unexpected AWS account."
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
    amp = {
      alias              = "alias/${local.name}-amp"
      description        = "Amazon Managed Service for Prometheus encryption for ${local.name}"
      service_principals = ["aps.amazonaws.com"]
    }
    backup = {
      alias              = "alias/${local.name}-backup"
      description        = "AWS Backup vault encryption for ${local.name}"
      service_principals = ["backup.amazonaws.com"]
    }
    grafana = {
      alias              = "alias/${local.name}-grafana"
      description        = "Amazon Managed Grafana encryption for ${local.name}"
      service_principals = ["grafana.amazonaws.com"]
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
    sns = {
      alias              = "alias/${local.name}-sns"
      description        = "Operations SNS encryption for ${local.name}"
      service_principals = ["sns.amazonaws.com"]
    }
    secrets = {
      alias              = "alias/${local.name}-secrets"
      description        = "Secrets Manager encryption for ${local.name}"
      service_principals = ["secretsmanager.amazonaws.com"]
    }
    xray = {
      alias              = "alias/${local.name}-xray"
      description        = "AWS X-Ray encryption for ${local.name}"
      service_principals = ["xray.amazonaws.com"]
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
  flow_log_retention_days = 90
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
      recovery_window_days  = 14
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
    api         = { name = "${var.project_name}/api" }
    chart       = { name = "${var.project_name}/charts/platform" }
    projector   = { name = "${var.project_name}/projector" }
    third_party = { name = "${var.project_name}/third-party" }
    web         = { name = "${var.project_name}/web" }
    worker      = { name = "${var.project_name}/worker" }
  }
  repository_pull_principal_arns   = var.repository_pull_principal_arns
  promotion_trusted_principal_arns = var.promotion_trusted_principal_arns
  promotion_oidc_provider_arn      = var.promotion_oidc_provider_arn
  promotion_oidc_subjects          = var.promotion_oidc_subjects
  promotion_source_repository_arns = var.promotion_source_repository_arns
  tags                             = local.common_tags

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
  control_plane_log_retention_days = 90
  access_entries                   = var.eks_access_entries
  stateful_node_count              = 3
  stateful_instance_types          = ["m7g.large"]
  stateless_instance_types         = ["m7g.large", "m7g.xlarge"]
  stateless_min_size               = 3
  stateless_desired_size           = 3
  stateless_max_size               = 9
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
  multi_az                       = false
  deletion_protection            = false
  backup_retention_days          = 14
  preferred_availability_zone    = var.availability_zones[0]
  kms_key_arn                    = module.kms.key_arns["rds"]
  master_secret_kms_key_arn      = module.kms.key_arns["secrets"]
  tags                           = local.common_tags

  depends_on = [terraform_data.account_guard]
}

module "backup" {
  source = "../../modules/backup"

  name              = "${local.name}-backup"
  kms_key_arn       = module.kms.key_arns["backup"]
  resource_arns     = [module.rds.instance_arn]
  delete_after_days = 14
  tags              = local.common_tags

  depends_on = [terraform_data.account_guard]
}

module "observability" {
  source = "../../modules/observability"

  name                = local.name
  amp_kms_key_arn     = module.kms.key_arns["amp"]
  logs_kms_key_arn    = module.kms.key_arns["logs"]
  grafana_kms_key_arn = module.kms.key_arns["grafana"]
  xray_kms_key_arn    = module.kms.key_arns["xray"]
  log_groups = {
    amp         = { name = "/aws/aps/${local.name}", retention_in_days = 30 }
    application = { name = "/deepref/${local.environment}/application", retention_in_days = 30 }
    adot        = { name = "/deepref/${local.environment}/adot", retention_in_days = 30 }
  }
  grafana_admin_user_ids  = var.grafana_admin_user_ids
  grafana_editor_user_ids = var.grafana_editor_user_ids
  grafana_viewer_user_ids = var.grafana_viewer_user_ids
  tags                    = local.common_tags

  depends_on = [terraform_data.account_guard]
}

module "budgets_alerts" {
  source = "../../modules/budgets-alerts"

  account_id            = data.aws_caller_identity.current.account_id
  name                  = "${local.name}-operations"
  sns_kms_key_arn       = module.kms.key_arns["sns"]
  monthly_budget_amount = var.monthly_budget_amount
  email_subscribers     = var.operations_email_addresses
  tags                  = local.common_tags

  depends_on = [terraform_data.account_guard]
}

module "admin_runner" {
  source = "../../modules/admin-runner"

  name                 = "${local.name}-admin"
  vpc_id               = module.network.vpc_id
  subnet_ids           = toset(module.network.private_subnet_ids)
  eks_cluster_name     = module.eks.cluster_name
  eks_cluster_arn      = module.eks.cluster_arn
  log_kms_key_arn      = module.kms.key_arns["logs"]
  assumable_role_arns  = var.admin_runner_assumable_role_arns
  kms_decrypt_key_arns = var.admin_runner_kms_decrypt_key_arns
  egress_cidr_blocks   = var.admin_runner_egress_cidr_blocks
  tags                 = local.common_tags

  depends_on = [terraform_data.account_guard]
}

module "pod_identity" {
  source = "../../modules/pod-identity"

  cluster_name = module.eks.cluster_name
  name_prefix  = local.name
  associations = {
    adot = {
      namespace           = "deepref-${local.environment}"
      service_account     = "deepref-adot"
      managed_policy_arns = [module.observability.adot_policy_arn]
    }
  }
  tags = local.common_tags

  depends_on = [terraform_data.account_guard]
}
