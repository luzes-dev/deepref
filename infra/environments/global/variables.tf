variable "expected_global_state_account_id" {
  description = "Existing environment account selected to anchor global state and execution."
  type        = string

  validation {
    condition     = can(regex("^[0-9]{12}$", var.expected_global_state_account_id))
    error_message = "expected_global_state_account_id must be a 12-digit AWS account ID."
  }
}

variable "aws_region" {
  description = "Region containing all three EKS clusters and the global state backend."
  type        = string
  default     = "sa-east-1"

  validation {
    condition     = var.aws_region == "sa-east-1"
    error_message = "The production platform plan fixes every managed AWS resource in sa-east-1."
  }
}

variable "project_name" {
  description = "Stable project slug."
  type        = string
  default     = "ambient-scribes"
}

variable "aws_environments" {
  description = "Strict per-environment account, private EKS cluster, and existing read/bootstrap role mapping."
  type = object({
    development = object({
      account_id              = string
      cluster_access_role_arn = string
      eks_cluster_name        = string
    })
    staging = object({
      account_id              = string
      cluster_access_role_arn = string
      eks_cluster_name        = string
    })
    production = object({
      account_id              = string
      cluster_access_role_arn = string
      eks_cluster_name        = string
    })
  })

  validation {
    condition = alltrue([
      for environment in values(var.aws_environments) :
      can(regex("^[0-9]{12}$", environment.account_id)) &&
      can(regex("^arn:[^:]+:iam::${environment.account_id}:role/.+$", environment.cluster_access_role_arn)) &&
      length(trimspace(environment.eks_cluster_name)) > 0
    ])
    error_message = "Every environment needs a 12-digit account, a role ARN in that exact account, and an EKS cluster name."
  }

  validation {
    condition     = length(distinct([for environment in values(var.aws_environments) : environment.account_id])) == 3
    error_message = "Development, staging, and production account IDs must be distinct."
  }
}

variable "argo_chart_version" {
  description = "Exact reviewed argo-cd Helm chart version installed in every cluster."
  type        = string
}

variable "gitops_repository_url" {
  description = "HTTPS URL of this repository, read from its protected orphan gitops branch."
  type        = string
}

variable "gitops_paths" {
  description = "Existing root-definition paths on the GitOps branch."
  type = object({
    development = string
    staging     = string
    production  = string
  })
  default = {
    development = "clusters/development"
    staging     = "clusters/staging"
    production  = "clusters/production"
  }
}

variable "cloudflare_account_id" {
  description = "Cloudflare account containing Zero Trust."
  type        = string
}

variable "cloudflare_zone_id" {
  description = "Cloudflare zone containing base_domain."
  type        = string
}

variable "base_domain" {
  description = "Cloudflare-managed base domain."
  type        = string
}

variable "cloudflare_access_team_name" {
  description = "Cloudflare Zero Trust team-domain slug."
  type        = string
}

variable "github_oauth_client_id" {
  description = "GitHub OAuth App client ID for Cloudflare Access."
  type        = string
}

variable "github_oauth_client_secret" {
  description = "GitHub OAuth App client secret for Cloudflare Access; inject through an approved environment, never a committed file."
  type        = string
  sensitive   = true
}

variable "github_owner" {
  description = "GitHub organization owning the repository."
  type        = string
}

variable "github_repository" {
  description = "Existing GitHub repository name."
  type        = string
}

variable "github_reviewer_team_slug" {
  description = "Existing GitHub organization team for protected reviews."
  type        = string
}

variable "deployment_github_app_id" {
  description = "Numeric ID of the installed deployment GitHub App."
  type        = number
}

variable "source_required_status_checks" {
  description = "Optional override for exact source-branch check-run names."
  type        = set(string)
  default     = null
  nullable    = true
}

variable "gitops_required_status_checks" {
  description = "Optional override for exact GitOps check-run names."
  type        = set(string)
  default     = null
  nullable    = true
}

variable "tags" {
  description = "Additional non-sensitive AWS tags."
  type        = map(string)
  default     = {}
}
