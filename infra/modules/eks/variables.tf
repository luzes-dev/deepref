variable "name" {
  description = "EKS cluster name."
  type        = string
}

variable "kubernetes_version" {
  description = "EKS Kubernetes control-plane version."
  type        = string
  default     = "1.36"

  validation {
    condition     = var.kubernetes_version == "1.36"
    error_message = "This platform contract requires EKS 1.36."
  }
}

variable "private_subnets_by_az" {
  description = "Private workload subnet IDs keyed by three availability zones."
  type        = map(string)

  validation {
    condition     = length(var.private_subnets_by_az) == 3
    error_message = "private_subnets_by_az must contain exactly three AZs."
  }
}

variable "cluster_kms_key_arn" {
  description = "KMS key ARN used for Kubernetes secret envelope encryption."
  type        = string
}

variable "control_plane_log_kms_key_arn" {
  description = "KMS key ARN used for EKS control-plane logs."
  type        = string
}

variable "control_plane_log_retention_days" {
  description = "CloudWatch retention for control-plane logs."
  type        = number
  default     = 90
}

variable "access_entries" {
  description = "EKS API access entries and cluster-scoped access policies."
  type = map(object({
    principal_arn      = string
    type               = optional(string, "STANDARD")
    kubernetes_groups  = optional(set(string), [])
    access_policy_arns = optional(set(string), [])
  }))
  default = {}
}

variable "stateful_node_count" {
  description = "Fixed stateful node count. Use one for development and three for staging/production."
  type        = number

  validation {
    condition     = contains([1, 3], var.stateful_node_count)
    error_message = "stateful_node_count must be one or three."
  }
}

variable "stateful_instance_types" {
  description = "Allowed instance types for fixed stateful nodes."
  type        = list(string)
  default     = ["m7g.large"]
}

variable "node_ami_type" {
  description = "EKS managed-node AMI type; defaults to the ARM64 AL2023 image used by Graviton instances."
  type        = string
  default     = "AL2023_ARM_64_STANDARD"
}

variable "stateful_root_volume_gib" {
  description = "Encrypted root volume size for stateful nodes."
  type        = number
  default     = 80
}

variable "stateless_instance_types" {
  description = "Allowed instance types for autoscaled stateless nodes."
  type        = list(string)
  default     = ["m7g.large", "m7g.xlarge"]
}

variable "stateless_min_size" {
  description = "Minimum stateless node count."
  type        = number
}

variable "stateless_desired_size" {
  description = "Initial stateless node count."
  type        = number
}

variable "stateless_max_size" {
  description = "Maximum stateless node count."
  type        = number
}

variable "stateless_root_volume_gib" {
  description = "Encrypted root volume size for stateless nodes."
  type        = number
  default     = 50
}

variable "node_volume_kms_key_arn" {
  description = "KMS key ARN used for EKS node root volumes."
  type        = string
}

variable "addon_versions" {
  description = "Optional explicit EKS add-on versions keyed by add-on name. Null selects the AWS default compatible version."
  type        = map(string)
  default = {
    aws-ebs-csi-driver     = null
    coredns                = null
    eks-pod-identity-agent = null
    kube-proxy             = null
    vpc-cni                = null
  }
}

variable "tags" {
  description = "Additional tags applied to all resources."
  type        = map(string)
  default     = {}
}
