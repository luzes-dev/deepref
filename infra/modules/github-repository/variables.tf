variable "repository" {
  description = "Existing GitHub repository name; this module manages policy but never creates repository contents or branches."
  type        = string

  validation {
    condition     = can(regex("^[A-Za-z0-9_.-]+$", var.repository))
    error_message = "repository must be a GitHub repository name, not an owner/name path."
  }
}

variable "deployment_app_id" {
  description = "Numeric GitHub App ID that is the only bypass actor allowed to update gitops."
  type        = number

  validation {
    condition     = var.deployment_app_id > 0 && floor(var.deployment_app_id) == var.deployment_app_id
    error_message = "deployment_app_id must be a positive integer GitHub App ID."
  }
}

variable "reviewer_team_slug" {
  description = "Existing organization team used for environment gates and path-specific GitOps approvals."
  type        = string

  validation {
    condition     = can(regex("^[a-z0-9](?:[a-z0-9-]*[a-z0-9])?$", var.reviewer_team_slug))
    error_message = "reviewer_team_slug must be a lowercase GitHub team slug."
  }
}

variable "source_required_status_checks" {
  description = "Exact GitHub check-run names required on development, staging, and main."
  type        = set(string)
  default = [
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
  ]

  validation {
    condition     = length(var.source_required_status_checks) > 0 && alltrue([for check in var.source_required_status_checks : length(trimspace(check)) > 0])
    error_message = "source_required_status_checks must contain non-empty exact check-run names."
  }
}

variable "gitops_required_status_checks" {
  description = "Exact GitHub check-run names required on GitOps deployment pull requests."
  type        = set(string)
  default = [
    "Exact orphan tree and deployment diff",
    "Validate trusted branch ladder",
  ]

  validation {
    condition     = length(var.gitops_required_status_checks) > 0 && alltrue([for check in var.gitops_required_status_checks : length(trimspace(check)) > 0])
    error_message = "gitops_required_status_checks must contain non-empty exact check-run names."
  }
}
