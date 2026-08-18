variable "expected_account_id" {
  description = "AWS account ID dedicated to development. Plans fail in every other account."
  type        = string

  validation {
    condition     = can(regex("^[0-9]{12}$", var.expected_account_id))
    error_message = "expected_account_id must be a 12-digit AWS account ID."
  }
}

variable "aws_region" {
  description = "AWS region for the development platform."
  type        = string
  default     = "sa-east-1"
}

variable "project_name" {
  description = "Stable project slug."
  type        = string
  default     = "ambient-scribes"
}

variable "vpc_cidr" {
  description = "Development VPC CIDR."
  type        = string
  default     = "10.10.0.0/16"
}

variable "availability_zones" {
  description = "Three development availability zones."
  type        = list(string)
  default     = ["sa-east-1a", "sa-east-1b", "sa-east-1c"]

  validation {
    condition     = length(var.availability_zones) == 3 && length(distinct(var.availability_zones)) == 3
    error_message = "availability_zones must contain exactly three distinct zones."
  }
}

variable "kms_administrator_principal_arns" {
  description = "Approved KMS administrator role ARNs."
  type        = set(string)
  default     = []
}

variable "eks_access_entries" {
  description = "Approved private-cluster access entries."
  type = map(object({
    principal_arn      = string
    type               = optional(string, "STANDARD")
    kubernetes_groups  = optional(set(string), [])
    access_policy_arns = optional(set(string), [])
  }))
  default = {}
}

variable "secret_reader_principal_arns" {
  description = "Reader role ARNs keyed by secret logical name; typically populated by the IAM support slice."
  type        = map(set(string))
  default     = {}
}

variable "repository_pull_principal_arns" {
  description = "Cross-account principals allowed to pull development artifacts."
  type        = set(string)
  default     = []
}

variable "promotion_trusted_principal_arns" {
  description = "Principals allowed to assume a destination promotion role. Development normally leaves this empty."
  type        = set(string)
  default     = []
}

variable "promotion_oidc_provider_arn" {
  description = "Optional GitHub OIDC provider ARN for a destination promotion role."
  type        = string
  default     = null
  nullable    = true
}

variable "promotion_oidc_subjects" {
  description = "Explicit GitHub OIDC subjects allowed to assume the promotion role."
  type        = set(string)
  default     = []
}

variable "promotion_source_repository_arns" {
  description = "Upstream repositories readable by the optional promotion role."
  type        = set(string)
  default     = []
}

variable "database_allowed_cidrs" {
  description = "Optional private administration CIDRs allowed to connect to PostgreSQL."
  type        = set(string)
  default     = []
}

variable "database_instance_class" {
  description = "Development RDS instance class."
  type        = string
  default     = "db.t4g.medium"
}

variable "database_allocated_storage_gib" {
  description = "Initial development RDS storage."
  type        = number
  default     = 30
}

variable "database_max_allocated_storage_gib" {
  description = "Development RDS storage autoscaling ceiling."
  type        = number
  default     = 100
}

variable "tags" {
  description = "Additional non-sensitive resource tags."
  type        = map(string)
  default     = {}
}
