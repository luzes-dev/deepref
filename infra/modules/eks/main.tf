locals {
  common_tags = merge(var.tags, { ManagedBy = "OpenTofu" })
  ordered_azs = sort(keys(var.private_subnets_by_az))
  stateful_azs = slice(
    local.ordered_azs,
    0,
    var.stateful_node_count,
  )
  stateful_subnets = {
    for zone in local.stateful_azs : zone => var.private_subnets_by_az[zone]
  }
  access_policy_associations = {
    for association in flatten([
      for entry_name, entry in var.access_entries : [
        for policy_arn in entry.access_policy_arns : {
          key           = "${entry_name}:${policy_arn}"
          entry_name    = entry_name
          principal_arn = entry.principal_arn
          policy_arn    = policy_arn
        }
      ]
    ]) : association.key => association
  }
}

resource "aws_iam_role" "cluster" {
  name = "${var.name}-cluster"
  assume_role_policy = jsonencode({
    Version = "2012-10-17"
    Statement = [{
      Effect    = "Allow"
      Principal = { Service = "eks.amazonaws.com" }
      Action = [
        "sts:AssumeRole",
        "sts:TagSession",
      ]
    }]
  })
  tags = local.common_tags
}

resource "aws_iam_role_policy_attachment" "cluster" {
  role       = aws_iam_role.cluster.name
  policy_arn = "arn:aws:iam::aws:policy/AmazonEKSClusterPolicy"
}

resource "aws_iam_role_policy" "cluster_encryption" {
  name = "cluster-envelope-encryption"
  role = aws_iam_role.cluster.id
  policy = jsonencode({
    Version = "2012-10-17"
    Statement = [{
      Effect = "Allow"
      Action = [
        "kms:CreateGrant",
        "kms:DescribeKey",
      ]
      Resource = var.cluster_kms_key_arn
    }]
  })
}

resource "aws_cloudwatch_log_group" "cluster" {
  name              = "/aws/eks/${var.name}/cluster"
  retention_in_days = var.control_plane_log_retention_days
  kms_key_id        = var.control_plane_log_kms_key_arn
  tags              = local.common_tags
}

resource "aws_eks_cluster" "this" {
  name     = var.name
  role_arn = aws_iam_role.cluster.arn
  version  = var.kubernetes_version

  enabled_cluster_log_types = [
    "api",
    "audit",
    "authenticator",
    "controllerManager",
    "scheduler",
  ]

  access_config {
    authentication_mode                         = "API"
    bootstrap_cluster_creator_admin_permissions = false
  }

  encryption_config {
    provider {
      key_arn = var.cluster_kms_key_arn
    }
    resources = ["secrets"]
  }

  vpc_config {
    endpoint_private_access = true
    endpoint_public_access  = false
    subnet_ids              = values(var.private_subnets_by_az)
  }

  tags = merge(local.common_tags, { Name = var.name })

  lifecycle {
    precondition {
      condition     = length(var.access_entries) > 0
      error_message = "At least one explicit EKS access entry is required because bootstrap creator access is disabled."
    }
  }

  depends_on = [
    aws_cloudwatch_log_group.cluster,
    aws_iam_role_policy.cluster_encryption,
    aws_iam_role_policy_attachment.cluster,
  ]
}

resource "aws_eks_access_entry" "this" {
  for_each = var.access_entries

  cluster_name      = aws_eks_cluster.this.name
  principal_arn     = each.value.principal_arn
  type              = each.value.type
  kubernetes_groups = length(each.value.kubernetes_groups) == 0 ? null : sort(tolist(each.value.kubernetes_groups))
}

resource "aws_eks_access_policy_association" "this" {
  for_each = local.access_policy_associations

  cluster_name  = aws_eks_cluster.this.name
  principal_arn = each.value.principal_arn
  policy_arn    = each.value.policy_arn

  access_scope {
    type = "cluster"
  }

  depends_on = [aws_eks_access_entry.this]
}

resource "aws_iam_role" "node" {
  name = "${var.name}-node"
  assume_role_policy = jsonencode({
    Version = "2012-10-17"
    Statement = [{
      Effect    = "Allow"
      Principal = { Service = "ec2.amazonaws.com" }
      Action    = "sts:AssumeRole"
    }]
  })
  tags = local.common_tags
}

resource "aws_iam_role_policy_attachment" "node" {
  for_each = toset([
    "arn:aws:iam::aws:policy/AmazonEC2ContainerRegistryPullOnly",
    "arn:aws:iam::aws:policy/AmazonEKS_CNI_Policy",
    "arn:aws:iam::aws:policy/AmazonEKSWorkerNodePolicy",
  ])

  role       = aws_iam_role.node.name
  policy_arn = each.value
}

resource "aws_iam_role" "ebs_csi" {
  name = "${var.name}-ebs-csi"
  assume_role_policy = jsonencode({
    Version = "2012-10-17"
    Statement = [{
      Effect    = "Allow"
      Principal = { Service = "pods.eks.amazonaws.com" }
      Action = [
        "sts:AssumeRole",
        "sts:TagSession",
      ]
    }]
  })
  tags = local.common_tags
}

resource "aws_iam_role_policy_attachment" "ebs_csi" {
  role       = aws_iam_role.ebs_csi.name
  policy_arn = "arn:aws:iam::aws:policy/service-role/AmazonEBSCSIDriverPolicy"
}

resource "aws_eks_pod_identity_association" "ebs_csi" {
  cluster_name    = aws_eks_cluster.this.name
  namespace       = "kube-system"
  service_account = "ebs-csi-controller-sa"
  role_arn        = aws_iam_role.ebs_csi.arn
}

resource "aws_launch_template" "stateful" {
  name_prefix            = "${var.name}-stateful-"
  update_default_version = true

  block_device_mappings {
    device_name = "/dev/xvda"
    ebs {
      encrypted             = true
      kms_key_id            = var.node_volume_kms_key_arn
      volume_size           = var.stateful_root_volume_gib
      volume_type           = "gp3"
      delete_on_termination = true
    }
  }

  metadata_options {
    http_endpoint               = "enabled"
    http_put_response_hop_limit = 1
    http_tokens                 = "required"
  }

  tag_specifications {
    resource_type = "instance"
    tags          = merge(local.common_tags, { Workload = "stateful" })
  }

  tags = local.common_tags
}

resource "aws_launch_template" "stateless" {
  name_prefix            = "${var.name}-stateless-"
  update_default_version = true

  block_device_mappings {
    device_name = "/dev/xvda"
    ebs {
      encrypted             = true
      kms_key_id            = var.node_volume_kms_key_arn
      volume_size           = var.stateless_root_volume_gib
      volume_type           = "gp3"
      delete_on_termination = true
    }
  }

  metadata_options {
    http_endpoint               = "enabled"
    http_put_response_hop_limit = 1
    http_tokens                 = "required"
  }

  tag_specifications {
    resource_type = "instance"
    tags          = merge(local.common_tags, { Workload = "stateless" })
  }

  tags = local.common_tags
}

resource "aws_eks_node_group" "stateful" {
  for_each = local.stateful_subnets

  cluster_name    = aws_eks_cluster.this.name
  node_group_name = "stateful-${each.key}"
  node_role_arn   = aws_iam_role.node.arn
  subnet_ids      = [each.value]
  capacity_type   = "ON_DEMAND"
  ami_type        = var.node_ami_type
  instance_types  = var.stateful_instance_types

  launch_template {
    id      = aws_launch_template.stateful.id
    version = aws_launch_template.stateful.latest_version
  }

  scaling_config {
    desired_size = 1
    min_size     = 1
    max_size     = 1
  }

  labels = {
    workload = "stateful"
  }

  taint {
    key    = "dedicated"
    value  = "stateful"
    effect = "NO_SCHEDULE"
  }

  update_config {
    max_unavailable = 1
  }

  tags = merge(local.common_tags, { Workload = "stateful" })

  depends_on = [aws_iam_role_policy_attachment.node]
}

resource "aws_eks_node_group" "stateless" {
  cluster_name    = aws_eks_cluster.this.name
  node_group_name = "stateless"
  node_role_arn   = aws_iam_role.node.arn
  subnet_ids      = values(var.private_subnets_by_az)
  capacity_type   = "ON_DEMAND"
  ami_type        = var.node_ami_type
  instance_types  = var.stateless_instance_types

  launch_template {
    id      = aws_launch_template.stateless.id
    version = aws_launch_template.stateless.latest_version
  }

  scaling_config {
    desired_size = var.stateless_desired_size
    min_size     = var.stateless_min_size
    max_size     = var.stateless_max_size
  }

  labels = {
    workload = "stateless"
  }

  update_config {
    max_unavailable_percentage = 33
  }

  tags = merge(local.common_tags, { Workload = "stateless" })

  lifecycle {
    precondition {
      condition = (
        var.stateless_min_size >= 0 &&
        var.stateless_min_size <= var.stateless_desired_size &&
        var.stateless_desired_size <= var.stateless_max_size
      )
      error_message = "Stateless sizes must satisfy min <= desired <= max."
    }
  }

  depends_on = [aws_iam_role_policy_attachment.node]
}

resource "aws_eks_addon" "this" {
  for_each = var.addon_versions

  cluster_name                = aws_eks_cluster.this.name
  addon_name                  = each.key
  addon_version               = each.value
  resolve_conflicts_on_create = "OVERWRITE"
  resolve_conflicts_on_update = "PRESERVE"

  tags = local.common_tags

  depends_on = [
    aws_eks_node_group.stateful,
    aws_eks_node_group.stateless,
    aws_eks_pod_identity_association.ebs_csi,
  ]
}
