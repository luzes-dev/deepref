//! Problems a block cannot get past, whatever the data: a web address that
//! points into a private network, a Slack address that is not Slack, e-mail
//! that is not set up or an address that is malformed.
//!
//! Check, test runs and real steps all call these functions, so the same
//! problem gets the same sentence everywhere. Names are not resolved here; the
//! worker checks what a name points to just before it calls out.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

use url::{Host, Url};

pub const INVALID_WEB_ADDRESS: &str = "The web address is not valid.";
pub const WEB_ADDRESS_SCHEME: &str = "Only web addresses starting with http or https can be used.";
pub const WEB_ADDRESS_CREDENTIALS: &str =
    "Web addresses with a user name or password are not allowed.";
pub const WEB_ADDRESS_NO_HOST: &str = "The web address has no host name.";
pub const PRIVATE_ADDRESS: &str =
    "That address points to a private or local network, which automations may not call.";
pub const NOT_SLACK_WEBHOOK: &str = "That does not look like a Slack webhook address.";
pub const SLACK_WEBHOOK_MISSING: &str = "The Slack webhook address has not been set.";
pub const EMAIL_NOT_CONFIGURED: &str = "E-mail is not configured on this server.";
pub const EMAIL_NOBODY: &str = "There is nobody to send the e-mail to.";

const SLACK_HOSTS: [&str; 2] = ["hooks.slack.com", "hooks.slack-gov.com"];

fn ipv4_is_public(ip: Ipv4Addr) -> bool {
    let [a, b, ..] = ip.octets();
    !(ip.is_private()
        || ip.is_loopback()
        || ip.is_link_local()
        || ip.is_unspecified()
        || ip.is_broadcast()
        || ip.is_documentation()
        || ip.is_multicast()
        || (a == 100 && (64..128).contains(&b))
        || a == 0)
}

fn ipv6_is_public(ip: Ipv6Addr) -> bool {
    if let Some(mapped) = ip.to_ipv4_mapped() {
        return ipv4_is_public(mapped);
    }
    let first = ip.segments()[0];
    !(ip.is_loopback()
        || ip.is_unspecified()
        || ip.is_multicast()
        || (first & 0xfe00) == 0xfc00
        || (first & 0xffc0) == 0xfe80)
}

/// Whether a flow may call this address: not loopback, private, link-local,
/// shared, documentation or multicast space.
pub fn ip_is_public(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => ipv4_is_public(ip),
        IpAddr::V6(ip) => ipv6_is_public(ip),
    }
}

/// `localhost` and its subdomains are local by name alone.
fn is_local_name(host: &str) -> bool {
    let host = host.trim_end_matches('.').to_ascii_lowercase();
    host == "localhost" || host.ends_with(".localhost")
}

/// The problem with a web address a block would call, if one can be seen
/// without a lookup. Host names are left for the worker to resolve.
pub fn web_address_problem(raw: &str) -> Option<&'static str> {
    let Ok(url) = Url::parse(raw.trim()) else {
        return Some(INVALID_WEB_ADDRESS);
    };
    if !matches!(url.scheme(), "http" | "https") {
        return Some(WEB_ADDRESS_SCHEME);
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Some(WEB_ADDRESS_CREDENTIALS);
    }
    match url.host() {
        None => Some(WEB_ADDRESS_NO_HOST),
        Some(Host::Ipv4(ip)) => (!ipv4_is_public(ip)).then_some(PRIVATE_ADDRESS),
        Some(Host::Ipv6(ip)) => (!ipv6_is_public(ip)).then_some(PRIVATE_ADDRESS),
        Some(Host::Domain(name)) => is_local_name(name).then_some(PRIVATE_ADDRESS),
    }
}

/// Whether an address is a Slack incoming webhook: https on Slack's own host.
pub fn slack_webhook_problem(raw: &str) -> Option<&'static str> {
    let Ok(url) = Url::parse(raw.trim()) else {
        return Some(NOT_SLACK_WEBHOOK);
    };
    let on_slack = url.scheme() == "https"
        && url.port().is_none()
        && url.username().is_empty()
        && url.password().is_none()
        && url
            .host_str()
            .is_some_and(|host| SLACK_HOSTS.contains(&host));
    (!on_slack).then_some(NOT_SLACK_WEBHOOK)
}

/// Whether the server's outgoing mail settings are present. The worker and
/// the validator read the same environment variables.
pub fn email_is_configured(host: &str, from: &str) -> bool {
    !host.trim().is_empty() && !from.trim().is_empty()
}

/// The addresses of a "To" field, split on commas.
pub fn split_recipients(to: &str) -> Vec<String> {
    to.split(',')
        .map(str::trim)
        .filter(|address| !address.is_empty())
        .map(str::to_owned)
        .collect()
}

/// Whether one address is well formed, also when written as `Name <address>`.
pub fn email_address_is_valid(raw: &str) -> bool {
    let trimmed = raw.trim();
    let address = trimmed
        .strip_suffix('>')
        .and_then(|inner| inner.rsplit_once('<'))
        .map_or(trimmed, |(_, address)| address.trim());
    let Some((local, domain)) = address.rsplit_once('@') else {
        return false;
    };
    let local_ok = !local.is_empty()
        && !local.starts_with('.')
        && !local.ends_with('.')
        && !local.contains("..")
        && local
            .chars()
            .all(|c| c.is_alphanumeric() || "!#$%&'*+-/=?^_`{|}~.".contains(c));
    let labels: Vec<&str> = domain.split('.').collect();
    let domain_ok = labels.len() >= 2
        && labels.iter().all(|label| {
            !label.is_empty()
                && !label.starts_with('-')
                && !label.ends_with('-')
                && label.chars().all(|c| c.is_alphanumeric() || c == '-')
        })
        && labels.last().is_some_and(|top| top.chars().count() >= 2);
    local_ok && domain_ok
}

/// The first problem with a list of recipients, worded as the worker reports it.
pub fn recipients_problem(recipients: &[String]) -> Option<String> {
    if recipients.is_empty() {
        return Some(EMAIL_NOBODY.to_owned());
    }
    recipients
        .iter()
        .find(|address| !email_address_is_valid(address))
        .map(|address| format!("\"{address}\" is not a valid e-mail address."))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn private_and_local_web_addresses_are_refused_without_a_lookup() {
        for blocked in [
            "http://127.0.0.1:8099/",
            "http://192.168.1.10/",
            "http://10.1.2.3/hook",
            "http://169.254.169.254/latest",
            "http://100.64.0.1/",
            "http://[::1]:8080/",
            "http://[fd00::1]/",
            "http://[fe80::1]/",
            "http://[::ffff:10.0.0.1]/",
            "http://0.0.0.0/",
            "http://localhost:5197/api",
            "http://api.localhost/",
            // The URL parser turns the decimal form into 127.0.0.1.
            "http://2130706433/",
        ] {
            assert_eq!(
                web_address_problem(blocked),
                Some(PRIVATE_ADDRESS),
                "{blocked}"
            );
        }
    }

    #[test]
    fn public_and_name_only_web_addresses_pass_the_static_check() {
        for allowed in [
            "https://example.com/hook",
            "http://93.184.216.34/",
            "https://api.crossref.org/works?query=x",
            "http://[2606:4700:4700::1111]/",
        ] {
            assert_eq!(web_address_problem(allowed), None, "{allowed}");
        }
    }

    #[test]
    fn malformed_web_addresses_are_explained() {
        assert_eq!(web_address_problem("not a url"), Some(INVALID_WEB_ADDRESS));
        assert_eq!(web_address_problem(""), Some(INVALID_WEB_ADDRESS));
        assert_eq!(
            web_address_problem("ftp://example.com/file"),
            Some(WEB_ADDRESS_SCHEME)
        );
        assert_eq!(
            web_address_problem("https://user:secret@example.com/"),
            Some(WEB_ADDRESS_CREDENTIALS)
        );
    }

    #[test]
    fn only_slack_hosts_pass_as_slack_webhooks() {
        assert_eq!(
            slack_webhook_problem("https://hooks.slack.com/services/T000/B000/XXXX"),
            None
        );
        assert_eq!(
            slack_webhook_problem("https://hooks.slack-gov.com/services/T1/B1/X"),
            None
        );
        for wrong in [
            "https://example.com/not-slack",
            "http://hooks.slack.com/services/T/B/X",
            "https://hooks.slack.com.evil.example/x",
            "https://evil.example/?next=hooks.slack.com",
            "not a url",
        ] {
            assert_eq!(
                slack_webhook_problem(wrong),
                Some(NOT_SLACK_WEBHOOK),
                "{wrong}"
            );
        }
    }

    #[test]
    fn e_mail_addresses_are_checked_by_shape() {
        for valid in [
            "ana@example.com",
            "Ana Silva <ana@example.com>",
            "ana.silva+review@uni-x.edu.br",
        ] {
            assert!(email_address_is_valid(valid), "{valid}");
        }
        for invalid in [
            "ana@example",
            "ana example.com",
            "@example.com",
            "a..b@example.com",
            "x@exa mple.com",
            "a@b.c",
            "Ana <ana@example.com",
            "",
        ] {
            assert!(!email_address_is_valid(invalid), "{invalid}");
        }
    }

    #[test]
    fn recipients_are_split_and_the_first_bad_one_is_named() {
        assert_eq!(
            split_recipients(" a@b.co, c@d.org ,, "),
            vec!["a@b.co".to_owned(), "c@d.org".to_owned()]
        );
        assert_eq!(
            recipients_problem(&["a@b.co".to_owned(), "nope".to_owned()]).as_deref(),
            Some("\"nope\" is not a valid e-mail address.")
        );
        assert_eq!(recipients_problem(&[]).as_deref(), Some(EMAIL_NOBODY));
        assert_eq!(recipients_problem(&["a@b.co".to_owned()]), None);
    }

    #[test]
    fn e_mail_needs_both_server_settings() {
        assert!(email_is_configured("smtp.example.com", "bot@example.com"));
        assert!(!email_is_configured(" ", "bot@example.com"));
        assert!(!email_is_configured("smtp.example.com", ""));
    }
}
