locals {
  endpoints = {
    development = {
      hostname = "deepref-dev.${var.base_domain}"
      origin   = var.origin_services["development"]
    }
    staging = {
      hostname = "deepref-staging.${var.base_domain}"
      origin   = var.origin_services["staging"]
    }
    production = {
      hostname = "deepref.${var.base_domain}"
      origin   = var.origin_services["production"]
    }
  }
}

resource "cloudflare_zero_trust_access_identity_provider" "github" {
  account_id = var.account_id
  name       = "${var.name_prefix} GitHub"
  type       = "github"
  config = {
    client_id     = var.github_oauth_client_id
    client_secret = var.github_oauth_client_secret
  }
}

resource "cloudflare_zero_trust_access_policy" "github_organization" {
  account_id       = var.account_id
  name             = "Allow ${var.github_organization} organization members"
  decision         = "allow"
  session_duration = var.access_session_duration

  include = [{
    github_organization = {
      identity_provider_id = cloudflare_zero_trust_access_identity_provider.github.id
      name                 = var.github_organization
    }
  }]
}

resource "cloudflare_zero_trust_access_application" "environment" {
  for_each = local.endpoints

  account_id                 = var.account_id
  name                       = "${var.name_prefix}-${each.key}"
  type                       = "self_hosted"
  domain                     = each.value.hostname
  allowed_idps               = [cloudflare_zero_trust_access_identity_provider.github.id]
  auto_redirect_to_identity  = true
  http_only_cookie_attribute = true
  same_site_cookie_attribute = "strict"
  session_duration           = var.access_session_duration

  destinations = [{
    type = "public"
    uri  = each.value.hostname
  }]

  policies = [{
    id         = cloudflare_zero_trust_access_policy.github_organization.id
    precedence = 1
  }]
}

resource "cloudflare_zero_trust_tunnel_cloudflared" "environment" {
  for_each = local.endpoints

  account_id = var.account_id
  name       = "${var.name_prefix}-${each.key}"
  config_src = "cloudflare"
}

resource "cloudflare_zero_trust_tunnel_cloudflared_config" "environment" {
  for_each = local.endpoints

  account_id = var.account_id
  tunnel_id  = cloudflare_zero_trust_tunnel_cloudflared.environment[each.key].id
  source     = "cloudflare"

  config = {
    ingress = [
      {
        hostname = each.value.hostname
        service  = each.value.origin
        origin_request = {
          access = {
            aud_tag   = [cloudflare_zero_trust_access_application.environment[each.key].aud]
            team_name = var.access_team_name
            required  = true
          }
          connect_timeout       = 10
          http_host_header      = each.value.hostname
          keep_alive_timeout    = 90
          no_happy_eyeballs     = false
          no_tls_verify         = false
          tcp_keep_alive        = 30
          tls_timeout           = 10
        }
      },
      {
        service = "http_status:404"
      },
    ]
  }
}

resource "cloudflare_dns_record" "environment" {
  for_each = local.endpoints

  zone_id = var.zone_id
  name    = each.value.hostname
  type    = "CNAME"
  content = "${cloudflare_zero_trust_tunnel_cloudflared.environment[each.key].id}.cfargotunnel.com"
  proxied = true
  ttl     = 1
  comment = "OpenTofu-managed ${each.key} Cloudflare Tunnel route; no public origin exists"
  tags    = ["owner:opentofu", "environment:${each.key}"]
}
