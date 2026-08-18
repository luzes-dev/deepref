locals {
  namespace_labels = merge(var.labels, {
    "app.kubernetes.io/part-of"    = "argocd"
    "app.kubernetes.io/managed-by" = "OpenTofu"
    "deepref.io/environment"       = var.environment
    "deepref.io/owner"             = "opentofu"
  })

  chart_values = {
    configs = {
      cm = {
        "admin.enabled" = "false"
      }
      params = {
        "server.insecure" = "false"
      }
    }
    server = {
      ingress = {
        enabled = false
      }
      service = {
        type = "ClusterIP"
      }
    }
    extraObjects = [{
      apiVersion = "argoproj.io/v1alpha1"
      kind       = "Application"
      metadata = {
        name      = "deepref-root"
        namespace = var.namespace
        labels = {
          "app.kubernetes.io/managed-by" = "OpenTofu"
          "deepref.io/environment"       = var.environment
          "deepref.io/owner"             = "opentofu-bootstrap"
        }
      }
      spec = {
        project = "default"
        source = {
          repoURL        = var.gitops_repository_url
          targetRevision = var.gitops_revision
          path           = var.gitops_path
        }
        destination = {
          server    = "https://kubernetes.default.svc"
          namespace = var.namespace
        }
        syncPolicy = {
          automated = {
            allowEmpty = false
            prune      = true
            selfHeal   = true
          }
          syncOptions = [
            "CreateNamespace=false",
            "PrunePropagationPolicy=foreground",
          ]
        }
      }
    }]
  }
}

resource "kubernetes_namespace_v1" "argocd" {
  metadata {
    name   = var.namespace
    labels = local.namespace_labels
  }
}

resource "helm_release" "argocd" {
  name       = var.release_name
  namespace  = kubernetes_namespace_v1.argocd.metadata[0].name
  repository = "https://argoproj.github.io/argo-helm"
  chart      = "argo-cd"
  version    = var.chart_version

  atomic            = true
  cleanup_on_fail   = true
  create_namespace  = false
  dependency_update = false
  lint              = true
  max_history       = 10
  timeout           = var.timeout_seconds
  wait              = true
  wait_for_jobs     = true

  values = [yamlencode(local.chart_values)]
}
