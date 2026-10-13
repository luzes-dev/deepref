#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum SmtpTls {
    Tls,
    None,
    #[default]
    StartTls,
}

#[derive(Clone, Default)]
pub struct SmtpConfig {
    pub host: String,
    pub from: String,
    pub port: u16,
    pub tls: SmtpTls,
    pub credentials: Option<(String, String)>,
}

impl std::fmt::Debug for SmtpConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SmtpConfig")
            .field("host", &self.host)
            .field("from", &self.from)
            .field("port", &self.port)
            .field("tls", &self.tls)
            .field("credentials_present", &self.credentials.is_some())
            .finish()
    }
}

impl SmtpConfig {
    pub fn from_source(get: &dyn Fn(&str) -> Option<String>) -> Self {
        let credentials = match (get("SMTP_USERNAME"), get("SMTP_PASSWORD")) {
            (Some(user), Some(password)) if !user.is_empty() => Some((user, password)),
            _ => None,
        };
        Self {
            host: get("SMTP_HOST").unwrap_or_default(),
            from: get("SMTP_FROM").unwrap_or_default(),
            port: get("SMTP_PORT").and_then(|value| value.parse().ok()).unwrap_or(587),
            tls: match get("SMTP_TLS").as_deref() {
                Some("tls") => SmtpTls::Tls,
                Some("none") => SmtpTls::None,
                _ => SmtpTls::StartTls,
            },
            credentials,
        }
    }

    pub fn is_configured(&self) -> bool {
        !self.host.trim().is_empty() && !self.from.trim().is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn smtp_defaults_and_partial_credentials_preserve_behavior() {
        let config = SmtpConfig::from_source(&|_| None);
        assert_eq!(config.port, 587);
        assert_eq!(config.tls, SmtpTls::StartTls);
        assert!(!config.is_configured());
        let config = SmtpConfig::from_source(&|name| match name {
            "SMTP_HOST" => Some("mail.example".into()),
            "SMTP_FROM" => Some("from@example.org".into()),
            "SMTP_PORT" => Some("invalid".into()),
            "SMTP_USERNAME" => Some("user".into()),
            _ => None,
        });
        assert!(config.is_configured());
        assert_eq!(config.port, 587);
        assert!(config.credentials.is_none());
    }
}
