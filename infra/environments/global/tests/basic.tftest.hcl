mock_provider "aws" {
  mock_data "aws_caller_identity" {
    defaults = {
      account_id = "111111111111"
    }
  }
}

mock_provider "aws" {
  alias = "development"
  mock_data "aws_caller_identity" {
    defaults = {
      account_id = "111111111111"
    }
  }
  mock_data "aws_eks_cluster" {
    defaults = {
      endpoint = "https://development.example.invalid"
      certificate_authority = [{
        data = "dGVzdA=="
      }]
    }
  }
}

mock_provider "aws" {
  alias = "staging"
  mock_data "aws_caller_identity" {
    defaults = {
      account_id = "222222222222"
    }
  }
  mock_data "aws_eks_cluster" {
    defaults = {
      endpoint = "https://staging.example.invalid"
      certificate_authority = [{
        data = "dGVzdA=="
      }]
    }
  }
}

mock_provider "aws" {
  alias = "production"
  mock_data "aws_caller_identity" {
    defaults = {
      account_id = "333333333333"
    }
  }
  mock_data "aws_eks_cluster" {
    defaults = {
      endpoint = "https://production.example.invalid"
      certificate_authority = [{
        data = "dGVzdA=="
      }]
    }
  }
}

mock_provider "cloudflare" {}
mock_provider "github" {
  mock_data "github_team" {
    defaults = {
      id = 12345
    }
  }
}
mock_provider "helm" {
  alias = "development"
}

mock_provider "helm" {
  alias = "staging"
}

mock_provider "helm" {
  alias = "production"
}

mock_provider "kubernetes" {
  alias = "development"
}

mock_provider "kubernetes" {
  alias = "staging"
}

mock_provider "kubernetes" {
  alias = "production"
}

run "guarded_global_ownership_root" {
  command = plan

  variables {
    expected_global_state_account_id = "111111111111"
    aws_environments = {
      development = {
        account_id              = "111111111111"
        cluster_access_role_arn = "arn:aws:iam::111111111111:role/global-bootstrap"
        eks_cluster_name        = "ambient-scribes-development"
      }
      staging = {
        account_id              = "222222222222"
        cluster_access_role_arn = "arn:aws:iam::222222222222:role/global-bootstrap"
        eks_cluster_name        = "ambient-scribes-staging"
      }
      production = {
        account_id              = "333333333333"
        cluster_access_role_arn = "arn:aws:iam::333333333333:role/global-bootstrap"
        eks_cluster_name        = "ambient-scribes-production"
      }
    }
    argo_chart_version          = "8.5.0"
    gitops_repository_url       = "https://github.com/example/ambient-scribes"
    cloudflare_account_id       = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
    cloudflare_zone_id          = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"
    base_domain                 = "example.com"
    cloudflare_access_team_name = "deepref"
    github_oauth_client_id      = "Iv1.placeholder"
    github_oauth_client_secret  = "placeholder-not-a-real-secret"
    github_owner                = "example"
    github_repository           = "ambient-scribes"
    github_reviewer_team_slug   = "platform"
    deployment_github_app_id    = 67890
  }

  assert {
    condition = terraform_data.account_and_workspace_guard.input == {
      global      = "111111111111"
      development = "111111111111"
      staging     = "222222222222"
      production  = "333333333333"
    }
    error_message = "The global root must bind execution to all four resolved account identities."
  }

  assert {
    condition     = output.cloudflare_hostnames.production == "deepref.example.com"
    error_message = "The root must compose the Cloudflare perimeter module."
  }

  assert {
    condition     = output.argo_bootstraps.production.root_application == "deepref-root"
    error_message = "The root must install only the initial Argo bootstrap in production."
  }
}
