output "namespace" {
  description = "Namespace containing the OpenTofu-owned Argo CD installation."
  value       = kubernetes_namespace_v1.argocd.metadata[0].name
}

output "release_name" {
  description = "Argo CD Helm release name."
  value       = helm_release.argocd.name
}

output "root_application_name" {
  description = "Bootstrap Application installed through the pinned Argo CD chart."
  value       = "deepref-root"
}

output "gitops_source" {
  description = "Non-secret GitOps source selected by the root Application."
  value = {
    path       = var.gitops_path
    repository = var.gitops_repository_url
    revision   = var.gitops_revision
  }
}
