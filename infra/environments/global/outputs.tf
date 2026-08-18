output "cloudflare_hostnames" {
  description = "Access-protected hostnames keyed by environment."
  value       = module.cloudflare_perimeter.hostnames
}

output "cloudflare_tunnel_ids" {
  description = "Non-secret tunnel IDs keyed by environment."
  value       = module.cloudflare_perimeter.tunnel_ids
}

output "github_source_ruleset_ids" {
  description = "Source branch ruleset IDs."
  value       = module.github_repository.source_ruleset_ids
}

output "github_gitops_ruleset_id" {
  description = "App-only GitOps ruleset ID."
  value       = module.github_repository.gitops_ruleset_id
}

output "github_workflow_environments" {
  description = "Protected workflow environments."
  value       = module.github_repository.workflow_environments
}

output "argo_bootstraps" {
  description = "Initial Argo release and root Application identity per cluster."
  value = {
    development = {
      namespace        = module.argo_development.namespace
      release          = module.argo_development.release_name
      root_application = module.argo_development.root_application_name
    }
    staging = {
      namespace        = module.argo_staging.namespace
      release          = module.argo_staging.release_name
      root_application = module.argo_staging.root_application_name
    }
    production = {
      namespace        = module.argo_production.namespace
      release          = module.argo_production.release_name
      root_application = module.argo_production.root_application_name
    }
  }
}
