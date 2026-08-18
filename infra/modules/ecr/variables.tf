variable "name_prefix" {
  description = "Prefix used for the optional cross-account promotion role."
  type        = string
}

variable "repositories" {
  description = "Immutable ECR repositories keyed by logical artifact name."
  type = map(object({
    name                  = string
    retain_tagged_images  = optional(number, 100)
    expire_untagged_after = optional(number, 7)
  }))

  validation {
    condition = alltrue([
      for repository in values(var.repositories) : repository.retain_tagged_images > 0
    ])
    error_message = "retain_tagged_images must be greater than zero."
  }

  validation {
    condition     = length(distinct([for repository in values(var.repositories) : repository.name])) == length(var.repositories)
    error_message = "Every repository name must be unique."
  }
}

variable "kms_key_arn" {
  description = "KMS key ARN used for repository encryption."
  type        = string
}

variable "repository_pull_principal_arns" {
  description = "Principals allowed to pull from every repository, including downstream promotion roles."
  type        = set(string)
  default     = []
}

variable "promotion_trusted_principal_arns" {
  description = "AWS principals allowed to assume the destination promotion role."
  type        = set(string)
  default     = []
}

variable "promotion_oidc_provider_arn" {
  description = "Optional IAM OIDC provider ARN allowed to assume the destination promotion role."
  type        = string
  default     = null
  nullable    = true

  validation {
    condition = (
      var.promotion_oidc_provider_arn == null ||
      can(regex("^arn:[^:]+:iam::[0-9]{12}:oidc-provider/.+$", var.promotion_oidc_provider_arn))
    )
    error_message = "promotion_oidc_provider_arn must be a valid IAM OIDC provider ARN."
  }
}

variable "promotion_oidc_subjects" {
  description = "Exact or wildcard OIDC subject claims allowed to assume the promotion role."
  type        = set(string)
  default     = []
}

variable "promotion_source_repository_arns" {
  description = "Upstream ECR repository ARNs the promotion role may read."
  type        = set(string)
  default     = []
}

variable "tags" {
  description = "Additional tags applied to all resources."
  type        = map(string)
  default     = {}
}
