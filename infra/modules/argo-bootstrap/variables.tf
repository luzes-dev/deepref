variable "environment" {
  description = "Environment whose cluster receives this Argo CD bootstrap."
  type        = string

  validation {
    condition     = contains(["development", "staging", "production"], var.environment)
    error_message = "environment must be development, staging, or production."
  }
}

variable "namespace" {
  description = "Dedicated namespace owned with the Argo CD installation."
  type        = string
  default     = "argocd"

  validation {
    condition     = can(regex("^[a-z0-9]([-a-z0-9]*[a-z0-9])?$", var.namespace))
    error_message = "namespace must be a valid Kubernetes DNS label."
  }
}

variable "chart_version" {
  description = "Reviewed, immutable argo-cd Helm chart version."
  type        = string

  validation {
    condition     = can(regex("^[0-9]+\\.[0-9]+\\.[0-9]+([+-][0-9A-Za-z.-]+)?$", var.chart_version))
    error_message = "chart_version must be an exact semantic version, never a range."
  }
}

variable "gitops_repository_url" {
  description = "HTTPS URL of the repository containing the protected orphan GitOps branch."
  type        = string

  validation {
    condition     = can(regex("^https://github\\.com/[^/]+/[^/]+(\\.git)?$", var.gitops_repository_url))
    error_message = "gitops_repository_url must be an HTTPS GitHub repository URL."
  }
}

variable "gitops_revision" {
  description = "Protected branch read by the root Application."
  type        = string
  default     = "gitops"

  validation {
    condition     = var.gitops_revision == "gitops"
    error_message = "The bootstrap revision is deliberately fixed to the protected gitops branch."
  }
}

variable "gitops_path" {
  description = "Existing path on the GitOps branch containing this environment's Argo root definitions."
  type        = string

  validation {
    condition     = can(regex("^[A-Za-z0-9._/-]+$", var.gitops_path)) && !startswith(var.gitops_path, "/") && !strcontains(var.gitops_path, "..")
    error_message = "gitops_path must be a relative repository path without parent traversal."
  }
}

variable "release_name" {
  description = "Helm release name."
  type        = string
  default     = "argocd"
}

variable "timeout_seconds" {
  description = "Maximum time to wait for the initial Argo CD installation."
  type        = number
  default     = 900

  validation {
    condition     = var.timeout_seconds >= 300 && var.timeout_seconds <= 1800
    error_message = "timeout_seconds must be between 300 and 1800."
  }
}

variable "labels" {
  description = "Non-sensitive labels attached to the Argo CD namespace."
  type        = map(string)
  default     = {}
}
