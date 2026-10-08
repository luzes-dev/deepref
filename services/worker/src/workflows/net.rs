//! Outbound network helpers for integration blocks: a request client that
//! refuses private addresses, e-mail through SMTP, and bounded body reads.
//!
//! The checks that need no lookup live in `deepref_application::workflows::preflight`,
//! so a draft, a test run and a real step all give the same answer. This module
//! adds the checks that need the network: what a name resolves to.

use std::{
    net::{IpAddr, SocketAddr},
    time::Duration,
};

use deepref_application::workflows::preflight::{
    EMAIL_NOBODY, EMAIL_NOT_CONFIGURED, INVALID_WEB_ADDRESS, PRIVATE_ADDRESS, WEB_ADDRESS_NO_HOST,
    email_is_configured, ip_is_public, recipients_problem, web_address_problem,
};
use reqwest::Url;

use super::NodeError;

const MAX_RESPONSE_BYTES: usize = 256 * 1024;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(20);

/// Parse a user-supplied address and resolve it, refusing anything that points
/// into a private network. Problems visible in the text are reported without a
/// lookup; a name is resolved and every address it has must be public.
pub async fn resolve_public(raw: &str) -> Result<(Url, Vec<SocketAddr>), NodeError> {
    if let Some(problem) = web_address_problem(raw) {
        return Err(NodeError::permanent(problem));
    }
    let url = Url::parse(raw.trim()).map_err(|_| NodeError::permanent(INVALID_WEB_ADDRESS))?;
    let host = url
        .host_str()
        .ok_or_else(|| NodeError::permanent(WEB_ADDRESS_NO_HOST))?
        .to_owned();
    let port = url.port_or_known_default().unwrap_or(443);
    let literal = host.trim_start_matches('[').trim_end_matches(']');
    let addresses: Vec<SocketAddr> = match literal.parse::<IpAddr>() {
        Ok(ip) => vec![SocketAddr::new(ip, port)],
        Err(_) => tokio::net::lookup_host((host.as_str(), port))
            .await
            .map_err(|_| {
                NodeError::retryable(format!("Could not find \"{host}\" on the internet."))
            })?
            .collect(),
    };
    if addresses.is_empty() || addresses.iter().any(|address| !ip_is_public(address.ip())) {
        return Err(NodeError::permanent(PRIVATE_ADDRESS));
    }
    Ok((url, addresses))
}

pub struct SafeResponse {
    pub status: u16,
    pub body: String,
}

/// Send one request to a public address. Redirects are not followed.
pub async fn send(
    method: &str,
    raw_url: &str,
    headers: &[(String, String)],
    body: Option<String>,
) -> Result<SafeResponse, NodeError> {
    let (url, addresses) = resolve_public(raw_url).await?;
    let host = url.host_str().unwrap_or_default().to_owned();
    let mut builder = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(REQUEST_TIMEOUT)
        .user_agent("DeepRef-Automations/1.0");
    // Connect only to the addresses that were checked, so a name cannot be
    // resolved again to a private one. Literal addresses need no lookup.
    if !host.starts_with('[') && host.parse::<IpAddr>().is_err() {
        builder = builder.resolve_to_addrs(&host, &addresses);
    }
    let client = builder
        .build()
        .map_err(|_| NodeError::permanent("The web request could not be prepared."))?;
    let method = reqwest::Method::from_bytes(method.as_bytes())
        .map_err(|_| NodeError::permanent("The request method is not valid."))?;
    let mut request = client.request(method, url);
    for (name, value) in headers {
        request = request.header(name.as_str(), value.as_str());
    }
    if let Some(body) = body {
        request = request.body(body);
    }
    let mut response = request.send().await.map_err(|error| {
        if error.is_timeout() || error.is_connect() {
            NodeError::retryable("The other service did not answer in time.")
        } else {
            NodeError::retryable("The web request failed.")
        }
    })?;
    let status = response.status().as_u16();
    let mut collected: Vec<u8> = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| NodeError::retryable("The answer could not be read completely."))?
    {
        collected.extend_from_slice(&chunk);
        if collected.len() >= MAX_RESPONSE_BYTES {
            collected.truncate(MAX_RESPONSE_BYTES);
            break;
        }
    }
    Ok(SafeResponse {
        status,
        body: String::from_utf8_lossy(&collected).into_owned(),
    })
}

/// Fetch JSON or text from a fixed, trusted API host (PubMed, Crossref,
/// Unpaywall). Not for user-supplied addresses.
pub async fn trusted_get(client: &reqwest::Client, url: &str) -> Result<String, NodeError> {
    let response = client
        .get(url)
        .send()
        .await
        .map_err(|_| NodeError::retryable("The literature service did not answer."))?;
    let status = response.status();
    if status.is_server_error() || status.as_u16() == 429 {
        return Err(NodeError::retryable(
            "The literature service is busy right now.",
        ));
    }
    if !status.is_success() {
        return Err(NodeError::permanent(format!(
            "The literature service refused the request ({}).",
            status.as_u16()
        )));
    }
    response
        .text()
        .await
        .map_err(|_| NodeError::retryable("The literature service answer could not be read."))
}

pub fn trusted_client() -> Result<reqwest::Client, NodeError> {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .user_agent("DeepRef/1.0 (mailto:support@deepref.invalid)")
        .build()
        .map_err(|_| NodeError::permanent("The network client could not be prepared."))
}

/// Whether this server has outgoing mail set up, from the environment.
pub fn email_setup_present() -> bool {
    email_is_configured(
        &std::env::var("SMTP_HOST").unwrap_or_default(),
        &std::env::var("SMTP_FROM").unwrap_or_default(),
    )
}

/// What stops an e-mail from being sent, worded as the step reports it. A test
/// run calls this too, so it fails where a real run would.
pub fn email_preflight(to: &[String]) -> Result<(), NodeError> {
    if to.is_empty() {
        return Err(NodeError::permanent(EMAIL_NOBODY));
    }
    if !email_setup_present() {
        return Err(NodeError::permanent(EMAIL_NOT_CONFIGURED));
    }
    if let Some(problem) = recipients_problem(to) {
        return Err(NodeError::permanent(problem));
    }
    Ok(())
}

/// Send an e-mail through the SMTP server configured in the environment.
pub async fn send_email(to: &[String], subject: &str, body: &str) -> Result<(), NodeError> {
    use lettre::{
        AsyncSmtpTransport, AsyncTransport, Message, Tokio1Executor, message::Mailbox,
        transport::smtp::authentication::Credentials,
    };
    email_preflight(to)?;
    let host = std::env::var("SMTP_HOST").unwrap_or_default();
    let from = std::env::var("SMTP_FROM").unwrap_or_default();
    let port: u16 = std::env::var("SMTP_PORT")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(587);
    let mut builder = match std::env::var("SMTP_TLS").unwrap_or_default().as_str() {
        "tls" => AsyncSmtpTransport::<Tokio1Executor>::relay(&host),
        "none" => Ok(AsyncSmtpTransport::<Tokio1Executor>::builder_dangerous(
            &host,
        )),
        _ => AsyncSmtpTransport::<Tokio1Executor>::starttls_relay(&host),
    }
    .map_err(|_| NodeError::permanent("The e-mail server address is not valid."))?
    .port(port);
    if let (Ok(user), Ok(password)) = (
        std::env::var("SMTP_USERNAME"),
        std::env::var("SMTP_PASSWORD"),
    ) && !user.is_empty()
    {
        builder = builder.credentials(Credentials::new(user, password));
    }
    let transport = builder.build();
    let from: Mailbox = from.parse().map_err(|_| {
        NodeError::permanent("The sender address set up on the server is not valid.")
    })?;
    let mut message = Message::builder().from(from).subject(subject);
    for recipient in to {
        let mailbox: Mailbox = recipient.parse().map_err(|_| {
            NodeError::permanent(format!("\"{recipient}\" is not a valid e-mail address."))
        })?;
        message = message.to(mailbox);
    }
    let message = message
        .body(body.to_owned())
        .map_err(|_| NodeError::permanent("The e-mail could not be put together."))?;
    transport
        .send(message)
        .await
        .map_err(|_| NodeError::retryable("The e-mail server did not accept the message."))?;
    Ok(())
}
