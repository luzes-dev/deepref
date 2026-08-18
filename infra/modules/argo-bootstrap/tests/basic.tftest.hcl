mock_provider "helm" {}
mock_provider "kubernetes" {}

run "private_pinned_bootstrap_only" {
  command = plan

  variables {
    environment           = "development"
    chart_version         = "8.5.0"
    gitops_repository_url = "https://github.com/example/ambient-scribes"
    gitops_path           = "clusters/development"
  }

  assert {
    condition     = helm_release.argocd.version == "8.5.0"
    error_message = "Argo CD must use the exact reviewed chart version."
  }

  assert {
    condition     = strcontains(helm_release.argocd.values[0], "ClusterIP") && strcontains(helm_release.argocd.values[0], "ingress")
    error_message = "Argo CD must remain internal to the cluster."
  }

  assert {
    condition     = strcontains(helm_release.argocd.values[0], "admin.enabled") && strcontains(helm_release.argocd.values[0], "gitops")
    error_message = "The bootstrap must disable local admin and read only the protected GitOps revision."
  }

  assert {
    condition     = output.gitops_source.revision == "gitops"
    error_message = "The root Application must follow the protected gitops branch."
  }
}
