variable "cluster_name" {
  description = "Existing EKS cluster name with the Pod Identity Agent installed."
  type        = string
}

variable "name_prefix" {
  description = "Prefix for generated IAM role names."
  type        = string
}

variable "associations" {
  description = "Pod Identity roles and associations keyed by stable workload name. This module does not create Kubernetes ServiceAccounts."
  type = map(object({
    namespace           = string
    service_account     = string
    role_name           = optional(string)
    description         = optional(string)
    inline_policy_json  = optional(string)
    managed_policy_arns = optional(set(string), [])
    tags                = optional(map(string), {})
  }))

  validation {
    condition = alltrue([
      for association in values(var.associations) :
      length(trimspace(association.namespace)) > 0 && length(trimspace(association.service_account)) > 0
    ])
    error_message = "Every association requires a namespace and service_account."
  }

  validation {
    condition = alltrue([
      for association in values(var.associations) : association.inline_policy_json == null || can(jsondecode(association.inline_policy_json))
    ])
    error_message = "Every inline_policy_json value must be valid JSON."
  }

  validation {
    condition = length(distinct([
      for association in values(var.associations) : "${association.namespace}/${association.service_account}"
    ])) == length(var.associations)
    error_message = "A namespace/service_account pair may be associated only once."
  }
}

variable "permissions_boundary_arn" {
  description = "Optional IAM permissions boundary applied to every workload role."
  type        = string
  default     = null
  nullable    = true
}

variable "tags" {
  description = "Additional non-sensitive tags."
  type        = map(string)
  default     = {}
}
