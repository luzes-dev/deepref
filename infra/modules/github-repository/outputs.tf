output "source_ruleset_ids" {
  description = "Active ruleset IDs keyed by long-lived source branch."
  value       = { for branch, ruleset in github_repository_ruleset.source : branch => ruleset.ruleset_id }
}

output "gitops_ruleset_id" {
  description = "Ruleset ID restricting the GitOps branch to deployment-App pull requests."
  value       = github_repository_ruleset.gitops.ruleset_id
}

output "workflow_environments" {
  description = "Protected GitHub workflow environments managed by this module."
  value       = sort(keys(github_repository_environment.workflow))
}

output "reviewer_team_id" {
  description = "Resolved numeric ID of the configured reviewer team."
  value       = data.github_team.reviewers.id
}
