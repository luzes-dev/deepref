mock_provider "cloudflare" {}

run "github_access_tunnel_only_perimeter" {
  command = plan

  variables {
    account_id                = "11111111111111111111111111111111"
    zone_id                   = "22222222222222222222222222222222"
    base_domain               = "example.com"
    access_team_name          = "deepref"
    github_oauth_client_id    = "Iv1.placeholder"
    github_oauth_client_secret = "placeholder-not-a-real-secret"
    github_organization        = "example-org"
  }

  assert {
    condition = output.hostnames == {
      development = "deepref-dev.example.com"
      staging     = "deepref-staging.example.com"
      production  = "deepref.example.com"
    }
    error_message = "The perimeter must expose only the three planned hostnames."
  }

  assert {
    condition = alltrue([
      for record in cloudflare_dns_record.environment :
      record.proxied && record.type == "CNAME" && endswith(record.content, ".cfargotunnel.com")
    ])
    error_message = "Every public record must be a proxied Cloudflare Tunnel CNAME."
  }

  assert {
    condition = alltrue([
      for configuration in cloudflare_zero_trust_tunnel_cloudflared_config.environment :
      endswith(configuration.config.ingress[0].service, ".svc.cluster.local:8080") &&
      configuration.config.ingress[0].origin_request.access.required &&
      configuration.config.ingress[1].service == "http_status:404"
    ])
    error_message = "Tunnels must use cluster-local origins, require Access JWTs, and fail closed."
  }

  assert {
    condition     = cloudflare_zero_trust_access_policy.github_organization.include[0].github_organization.name == "example-org"
    error_message = "Access must require membership in the configured GitHub organization."
  }
}
