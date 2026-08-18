locals {
  account_ids = {
    global      = data.aws_caller_identity.global.account_id
    development = data.aws_caller_identity.development.account_id
    staging     = data.aws_caller_identity.staging.account_id
    production  = data.aws_caller_identity.production.account_id
  }
}

resource "terraform_data" "account_and_workspace_guard" {
  input = local.account_ids

  lifecycle {
    precondition {
      condition     = data.aws_caller_identity.global.account_id == var.expected_global_state_account_id
      error_message = "Refusing to manage global resources from an unexpected state-anchor account."
    }

    precondition {
      condition     = contains([for environment in values(var.aws_environments) : environment.account_id], var.expected_global_state_account_id)
      error_message = "The global state anchor must be one of the three explicitly isolated environment accounts."
    }

    precondition {
      condition = (
        data.aws_caller_identity.development.account_id == var.aws_environments.development.account_id &&
        data.aws_caller_identity.staging.account_id == var.aws_environments.staging.account_id &&
        data.aws_caller_identity.production.account_id == var.aws_environments.production.account_id
      )
      error_message = "At least one EKS provider assumed into an unexpected AWS account."
    }

    precondition {
      condition     = terraform.workspace == "default"
      error_message = "This repository uses isolated roots; OpenTofu workspaces are forbidden."
    }
  }
}

module "cloudflare_perimeter" {
  source = "../../modules/cloudflare-perimeter"

  account_id                 = var.cloudflare_account_id
  zone_id                    = var.cloudflare_zone_id
  base_domain                = var.base_domain
  name_prefix                = var.project_name
  access_team_name           = var.cloudflare_access_team_name
  github_oauth_client_id     = var.github_oauth_client_id
  github_oauth_client_secret = var.github_oauth_client_secret
  github_organization        = var.github_owner

  depends_on = [terraform_data.account_and_workspace_guard]
}

module "github_repository" {
  source = "../../modules/github-repository"

  repository                    = var.github_repository
  deployment_app_id             = var.deployment_github_app_id
  reviewer_team_slug            = var.github_reviewer_team_slug
  source_required_status_checks = var.source_required_status_checks == null ? [
    "CodeQL / javascript-typescript",
    "CodeQL / rust",
    "Dependency policy",
    "Full-stack integration and E2E",
    "IaC and container configuration scan",
    "OpenAPI and Orval drift",
    "Reproducible container build",
    "Rust / clippy",
    "Rust / fmt",
    "Rust / test",
    "Validate trusted branch ladder",
    "Web",
    "Workflow and shell policy",
  ] : var.source_required_status_checks
  gitops_required_status_checks = var.gitops_required_status_checks == null ? [
    "Exact orphan tree and deployment diff",
    "Validate trusted branch ladder",
  ] : var.gitops_required_status_checks

  depends_on = [terraform_data.account_and_workspace_guard]
}

module "argo_development" {
  source = "../../modules/argo-bootstrap"

  providers = {
    helm       = helm.development
    kubernetes = kubernetes.development
  }

  environment           = "development"
  chart_version         = var.argo_chart_version
  gitops_repository_url = var.gitops_repository_url
  gitops_path           = var.gitops_paths.development

  depends_on = [terraform_data.account_and_workspace_guard]
}

module "argo_staging" {
  source = "../../modules/argo-bootstrap"

  providers = {
    helm       = helm.staging
    kubernetes = kubernetes.staging
  }

  environment           = "staging"
  chart_version         = var.argo_chart_version
  gitops_repository_url = var.gitops_repository_url
  gitops_path           = var.gitops_paths.staging

  depends_on = [terraform_data.account_and_workspace_guard]
}

module "argo_production" {
  source = "../../modules/argo-bootstrap"

  providers = {
    helm       = helm.production
    kubernetes = kubernetes.production
  }

  environment           = "production"
  chart_version         = var.argo_chart_version
  gitops_repository_url = var.gitops_repository_url
  gitops_path           = var.gitops_paths.production

  depends_on = [terraform_data.account_and_workspace_guard]
}
