variable "account_id" {
  description = "Cloudflare account containing Zero Trust and the three tunnels."
  type        = string

  validation {
    condition     = can(regex("^[0-9a-f]{32}$", var.account_id))
    error_message = "account_id must be a 32-character lowercase Cloudflare account ID."
  }
}

variable "zone_id" {
  description = "Cloudflare zone containing base_domain."
  type        = string

  validation {
    condition     = can(regex("^[0-9a-f]{32}$", var.zone_id))
    error_message = "zone_id must be a 32-character lowercase Cloudflare zone ID."
  }
}

variable "base_domain" {
  description = "Existing Cloudflare-managed DNS zone, without a scheme or trailing dot."
  type        = string

  validation {
    condition     = can(regex("^[a-z0-9](?:[a-z0-9-]{0,61}[a-z0-9])?(?:\\.[a-z0-9](?:[a-z0-9-]{0,61}[a-z0-9])?)+$", var.base_domain))
    error_message = "base_domain must be a lowercase DNS name with no scheme or trailing dot."
  }
}

variable "name_prefix" {
  description = "Stable non-secret prefix for Cloudflare resources."
  type        = string
  default     = "ambient-scribes"
}

variable "access_team_name" {
  description = "Cloudflare Zero Trust team-domain slug used for origin-side Access JWT validation."
  type        = string

  validation {
    condition     = can(regex("^[a-z0-9-]+$", var.access_team_name))
    error_message = "access_team_name must be the lowercase Zero Trust team-domain slug."
  }
}

variable "github_oauth_client_id" {
  description = "GitHub OAuth App client ID used only by the Cloudflare GitHub identity provider."
  type        = string

  validation {
    condition     = length(trimspace(var.github_oauth_client_id)) >= 8
    error_message = "github_oauth_client_id must be supplied from the approved GitHub OAuth App."
  }
}

variable "github_oauth_client_secret" {
  description = "GitHub OAuth App client secret. Supply through an ephemeral approved channel; it is never output."
  type        = string
  sensitive   = true

  validation {
    condition     = length(var.github_oauth_client_secret) >= 16
    error_message = "github_oauth_client_secret must be supplied from the approved GitHub OAuth App."
  }
}

variable "github_organization" {
  description = "GitHub organization whose members may pass Cloudflare Access."
  type        = string

  validation {
    condition     = can(regex("^[A-Za-z0-9](?:[A-Za-z0-9-]{0,37}[A-Za-z0-9])?$", var.github_organization))
    error_message = "github_organization must be a valid GitHub organization login."
  }
}

variable "origin_services" {
  description = "Cluster-local web Service URLs keyed by environment; public, IP, localhost, and AWS load-balancer origins are rejected."
  type        = map(string)
  default = {
    development = "http://deepref-web.deepref.svc.cluster.local:8080"
    staging     = "http://deepref-web.deepref.svc.cluster.local:8080"
    production  = "http://deepref-web.deepref.svc.cluster.local:8080"
  }

  validation {
    condition = (
      setequals(toset(keys(var.origin_services)), toset(["development", "staging", "production"])) &&
      alltrue([
        for service in values(var.origin_services) :
        can(regex("^https?://[a-z0-9]([-a-z0-9.]*[a-z0-9])?\\.svc\\.cluster\\.local(:[0-9]{2,5})?$", service))
      ])
    )
    error_message = "origin_services must define only development, staging, and production cluster-local .svc.cluster.local URLs."
  }
}

variable "access_session_duration" {
  description = "Cloudflare Access session duration for all three applications."
  type        = string
  default     = "12h"

  validation {
    condition     = can(regex("^[1-9][0-9]*(m|h)$", var.access_session_duration))
    error_message = "access_session_duration must be a positive duration in minutes or hours."
  }
}
