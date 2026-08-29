variable "name" {
  description = "Name for the private CodeBuild administration runner."
  type        = string
}

variable "vpc_id" {
  description = "VPC containing the private EKS endpoint."
  type        = string
}

variable "subnet_ids" {
  description = "At least two private workload subnet IDs used by CodeBuild."
  type        = set(string)

  validation {
    condition     = length(var.subnet_ids) >= 2
    error_message = "subnet_ids must contain at least two private subnets."
  }
}

variable "eks_cluster_name" {
  description = "Private EKS cluster name."
  type        = string
}

variable "eks_cluster_arn" {
  description = "Private EKS cluster ARN."
  type        = string
}

variable "log_kms_key_arn" {
  description = "Customer-managed KMS key ARN for the runner log group."
  type        = string
}

variable "log_retention_days" {
  description = "CloudWatch log retention in days."
  type        = number
  default     = 90
}

variable "build_image" {
  description = "CodeBuild image containing AWS CLI and kubectl; pin to an approved immutable image where required."
  type        = string
  default     = "aws/codebuild/standard:7.0"
}

variable "buildspec" {
  description = "Non-secret CodeBuild buildspec. The default performs a private EKS readiness check and can be overridden per approved build."
  type        = string
  default     = <<-YAML
    version: 0.2
    phases:
      build:
        commands:
          - aws eks update-kubeconfig --name "$EKS_CLUSTER_NAME" --region "$AWS_REGION"
          - kubectl get --raw=/readyz
  YAML
}

variable "assumable_role_arns" {
  description = "Optional explicit break-glass roles the runner may assume."
  type        = set(string)
  default     = []
}

variable "kms_decrypt_key_arns" {
  description = "Optional KMS keys the runner may decrypt during an approved administration task."
  type        = set(string)
  default     = []
}

variable "egress_cidr_blocks" {
  description = "Explicit IPv4 egress destinations for approved administration traffic. Empty by default; callers must opt in to required destinations."
  type        = set(string)
  default     = []
}

variable "tags" {
  description = "Additional non-sensitive tags."
  type        = map(string)
  default     = {}
}
