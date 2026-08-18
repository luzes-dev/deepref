locals {
  common_tags = merge(var.tags, { ManagedBy = "OpenTofu" })

  managed_policy_attachments = {
    for attachment in flatten([
      for workload, association in var.associations : [
        for policy_arn in association.managed_policy_arns : {
          key        = "${workload}:${policy_arn}"
          workload   = workload
          policy_arn = policy_arn
        }
      ]
    ]) : attachment.key => attachment
  }
}

resource "aws_iam_role" "this" {
  for_each = var.associations

  name                 = coalesce(each.value.role_name, "${var.name_prefix}-${each.key}")
  description          = coalesce(each.value.description, "EKS Pod Identity role for ${each.value.namespace}/${each.value.service_account}")
  permissions_boundary = var.permissions_boundary_arn
  assume_role_policy = jsonencode({
    Version = "2012-10-17"
    Statement = [{
      Sid       = "EksPodIdentity"
      Effect    = "Allow"
      Principal = { Service = "pods.eks.amazonaws.com" }
      Action = [
        "sts:AssumeRole",
        "sts:TagSession",
      ]
    }]
  })

  tags = merge(local.common_tags, each.value.tags, {
    KubernetesNamespace      = each.value.namespace
    KubernetesServiceAccount = each.value.service_account
  })
}

resource "aws_iam_role_policy" "inline" {
  for_each = {
    for workload, association in var.associations : workload => association
    if association.inline_policy_json != null
  }

  name   = "workload-permissions"
  role   = aws_iam_role.this[each.key].id
  policy = each.value.inline_policy_json
}

resource "aws_iam_role_policy_attachment" "managed" {
  for_each = local.managed_policy_attachments

  role       = aws_iam_role.this[each.value.workload].name
  policy_arn = each.value.policy_arn
}

resource "aws_eks_pod_identity_association" "this" {
  for_each = var.associations

  cluster_name    = var.cluster_name
  namespace       = each.value.namespace
  service_account = each.value.service_account
  role_arn        = aws_iam_role.this[each.key].arn
  tags            = merge(local.common_tags, each.value.tags)

  depends_on = [
    aws_iam_role_policy.inline,
    aws_iam_role_policy_attachment.managed,
  ]
}
