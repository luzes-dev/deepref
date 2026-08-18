mock_provider "github" {
  mock_data "github_team" {
    defaults = {
      id = 12345
    }
  }
}

run "branch_ladder_app_only_gitops_and_environments" {
  command = plan

  variables {
    repository         = "ambient-scribes"
    deployment_app_id  = 67890
    reviewer_team_slug = "platform"
  }

  assert {
    condition = (
      github_repository_ruleset.source["development"].rules[0].pull_request[0].required_approving_review_count == 1 &&
      github_repository_ruleset.source["development"].rules[0].pull_request[0].allowed_merge_methods == tolist(["squash"]) &&
      github_repository_ruleset.source["staging"].rules[0].pull_request[0].required_approving_review_count == 1 &&
      github_repository_ruleset.source["main"].rules[0].pull_request[0].required_approving_review_count == 2
    )
    error_message = "Source rulesets must encode the planned approval ladder and merge methods."
  }

  assert {
    condition = alltrue([
      for ruleset in github_repository_ruleset.source :
      ruleset.rules[0].deletion && ruleset.rules[0].non_fast_forward && length(ruleset.bypass_actors) == 0
    ])
    error_message = "Admins must not bypass source deletion and force-push protection."
  }

  assert {
    condition = (
      github_repository_ruleset.gitops.rules[0].update &&
      github_repository_ruleset.gitops.bypass_actors[0].actor_id == 67890 &&
      github_repository_ruleset.gitops.bypass_actors[0].actor_type == "Integration" &&
      github_repository_ruleset.gitops.bypass_actors[0].bypass_mode == "pull_request"
    )
    error_message = "Only the deployment App may update gitops, and only through pull requests."
  }

  assert {
    condition = (
      github_repository_ruleset.gitops.rules[0].required_reviewers[0].minimum_approvals == 1 &&
      github_repository_ruleset.gitops.rules[0].required_reviewers[1].minimum_approvals == 2
    )
    error_message = "GitOps staging and production changes must require one and two team approvals respectively."
  }

  assert {
    condition = (
      github_repository_environment.workflow["infra-production-apply"].can_admins_bypass == false &&
      github_repository_environment.workflow["infra-production-apply"].prevent_self_review
    )
    error_message = "Production infrastructure applies must use a non-bypassable protected environment."
  }
}
