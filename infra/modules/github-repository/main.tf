locals {
  source_rulesets = {
    development = {
      approvals     = 1
      merge_methods = ["squash"]
      linear        = true
    }
    staging = {
      approvals     = 1
      merge_methods = ["merge"]
      linear        = false
    }
    main = {
      approvals     = 2
      merge_methods = ["merge"]
      linear        = false
    }
  }

  workflow_environments = {
    "development-release" = {
      branch          = "development"
      requires_review = false
      restrict_branch = true
    }
    "staging-promotion" = {
      branch          = "development"
      requires_review = true
      restrict_branch = true
    }
    "production-promotion" = {
      branch          = "main"
      requires_review = true
      restrict_branch = true
    }
    "rollback-development" = {
      branch          = "development"
      requires_review = true
      restrict_branch = true
    }
    "rollback-staging" = {
      branch          = "staging"
      requires_review = true
      restrict_branch = true
    }
    "rollback-production" = {
      branch          = "main"
      requires_review = true
      restrict_branch = true
    }
    "infra-development-plan" = {
      branch          = null
      requires_review = false
      restrict_branch = false
    }
    "infra-staging-plan" = {
      branch          = null
      requires_review = false
      restrict_branch = false
    }
    "infra-production-plan" = {
      branch          = null
      requires_review = false
      restrict_branch = false
    }
    "infra-global-plan" = {
      branch          = null
      requires_review = false
      restrict_branch = false
    }
    "infra-development-apply" = {
      branch          = "development"
      requires_review = true
      restrict_branch = true
    }
    "infra-staging-apply" = {
      branch          = "staging"
      requires_review = true
      restrict_branch = true
    }
    "infra-production-apply" = {
      branch          = "main"
      requires_review = true
      restrict_branch = true
    }
    "infra-global-apply" = {
      branch          = "main"
      requires_review = true
      restrict_branch = true
    }
  }
}

data "github_team" "reviewers" {
  slug = var.reviewer_team_slug
}

resource "github_repository_ruleset" "source" {
  for_each = local.source_rulesets

  name        = "protected-${each.key}"
  repository  = var.repository
  target      = "branch"
  enforcement = "active"

  conditions {
    ref_name {
      include = ["refs/heads/${each.key}"]
      exclude = []
    }
  }

  rules {
    deletion                = true
    non_fast_forward        = true
    required_linear_history = each.value.linear

    pull_request {
      allowed_merge_methods            = each.value.merge_methods
      dismiss_stale_reviews_on_push     = true
      require_code_owner_review         = true
      require_last_push_approval        = true
      required_approving_review_count   = each.value.approvals
      required_review_thread_resolution = true
    }

    required_status_checks {
      strict_required_status_checks_policy = true

      dynamic "required_check" {
        for_each = var.source_required_status_checks
        content {
          context = required_check.value
        }
      }
    }
  }
}

resource "github_repository_ruleset" "gitops" {
  name        = "deployment-app-only-gitops"
  repository  = var.repository
  target      = "branch"
  enforcement = "active"

  conditions {
    ref_name {
      include = ["refs/heads/gitops"]
      exclude = []
    }
  }

  bypass_actors {
    actor_id    = var.deployment_app_id
    actor_type  = "Integration"
    bypass_mode = "pull_request"
  }

  rules {
    deletion                = true
    non_fast_forward        = true
    required_linear_history = true
    update                  = true

    pull_request {
      allowed_merge_methods            = ["squash"]
      dismiss_stale_reviews_on_push     = true
      require_code_owner_review         = false
      require_last_push_approval        = false
      required_approving_review_count   = 0
      required_review_thread_resolution = true
    }

    required_status_checks {
      strict_required_status_checks_policy = true

      dynamic "required_check" {
        for_each = var.gitops_required_status_checks
        content {
          context = required_check.value
        }
      }
    }

    required_reviewers {
      file_patterns     = ["environments/staging/**"]
      minimum_approvals = 1

      reviewer {
        id   = data.github_team.reviewers.id
        type = "Team"
      }
    }

    required_reviewers {
      file_patterns     = ["environments/production/**"]
      minimum_approvals = 2

      reviewer {
        id   = data.github_team.reviewers.id
        type = "Team"
      }
    }
  }
}

resource "github_repository_environment" "workflow" {
  for_each = local.workflow_environments

  repository          = var.repository
  environment         = each.key
  can_admins_bypass   = false
  prevent_self_review = true

  dynamic "reviewers" {
    for_each = each.value.requires_review ? [1] : []
    content {
      teams = [data.github_team.reviewers.id]
    }
  }

  deployment_branch_policy {
    protected_branches     = false
    custom_branch_policies = each.value.restrict_branch
  }
}

resource "github_repository_environment_deployment_policy" "workflow" {
  for_each = {
    for name, environment in local.workflow_environments : name => environment
    if environment.restrict_branch
  }

  repository     = var.repository
  environment    = github_repository_environment.workflow[each.key].environment
  branch_pattern = each.value.branch
}
