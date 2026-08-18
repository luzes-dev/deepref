output "hostnames" {
  description = "Access-protected public hostnames keyed by environment."
  value       = { for environment, endpoint in local.endpoints : environment => endpoint.hostname }
}

output "tunnel_ids" {
  description = "Tunnel IDs consumed by the out-of-band credential delivery process; no tunnel token is read or output."
  value       = { for environment, tunnel in cloudflare_zero_trust_tunnel_cloudflared.environment : environment => tunnel.id }
}

output "access_application_ids" {
  description = "Cloudflare Access application IDs keyed by environment."
  value       = { for environment, application in cloudflare_zero_trust_access_application.environment : environment => application.id }
}

output "github_identity_provider_id" {
  description = "Cloudflare GitHub identity provider ID."
  value       = cloudflare_zero_trust_access_identity_provider.github.id
}
