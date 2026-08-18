variable "expected_account_id" {
  description = "AWS account ID dedicated to staging. Plans fail in every other account."
  type        = string

  validation {
    condition     = can(regex("^[0-9]{12}$", var.expected_account_id))
    error_message = "expected_account_id must be a 12-digit AWS account ID."
  }
}

variable "aws_region" {
  description = "AWS region for the staging platform."
  type        = string
  default     = "sa-east-1"
}

variable "project_name" {
  description = "Stable project slug."
  type        = string
  default     = "ambient-scribes"
}

variable "vpc_cidr" {
  description = "Staging VPC CIDR."
  type        = string
  default     = "10.20.0.0/16"
}

variable "availability_zones" {
  description = "Three staging availability zones."
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
  description = "Cross-account principals allowed to pull staging artifacts."
  type        = set(string)
  default     = []
}

variable "promotion_trusted_principal_arns" {
  description = "Principals allowed to assume the staging promotion role."
  type        = set(string)
  default     = []
}

variable "promotion_oidc_provider_arn" {
  description = "GitHub Actions IAM OIDC provider ARN used by the staging promotion role."
  type        = string
}

variable "promotion_oidc_subjects" {
  description = "Explicit GitHub OIDC subjects allowed to assume the staging promotion role."
  type        = set(string)
}

variable "promotion_source_repository_arns" {
  description = "Development repositories readable by the promotion role."
  type        = set(string)
}

variable "database_allowed_cidrs" {
  description = "Optional private administration CIDRs allowed to connect to PostgreSQL."
  type        = set(string)
  default     = []
}

variable "database_instance_class" {
  description = "Staging RDS instance class."
  type        = string
  default     = "db.r7g.large"
}

variable "database_allocated_storage_gib" {
  description = "Initial staging RDS storage."
  type        = number
  default     = 100
}

variable "database_max_allocated_storage_gib" {
  description = "Staging RDS storage autoscaling ceiling."
  type        = number
  default     = 500
}

variable "monthly_budget_amount" {
  description = "Monthly AWS cost budget for staging alerts."
  type        = number
  default     = 1500
}

variable "operations_email_addresses" {
  description = "Operations recipients who must confirm encrypted SNS subscriptions."
  type        = set(string)
  default     = []
}

variable "grafana_admin_user_ids" {
  description = "Optional IAM Identity Center user IDs for Grafana administrators."
  type        = set(string)
  default     = []
}

variable "grafana_editor_user_ids" {
  description = "Optional IAM Identity Center user IDs for Grafana editors."
  type        = set(string)
  default     = []
}

variable "grafana_viewer_user_ids" {
  description = "Optional IAM Identity Center user IDs for Grafana viewers."
  type        = set(string)
  default     = []
}

variable "admin_runner_assumable_role_arns" {
  description = "Explicit break-glass roles the private administration runner may assume."
  type        = set(string)
  default     = []
}

variable "admin_runner_kms_decrypt_key_arns" {
  description = "Explicit KMS keys the private administration runner may decrypt."
  type        = set(string)
  default     = []
}

variable "admin_runner_egress_cidr_blocks" {
  description = "Additional approved IPv4 egress destinations for the private administration runner."
  type        = set(string)
  default     = []
}

variable "tags" {
  description = "Additional non-sensitive resource tags."
  type        = map(string)
  default     = {}
}
