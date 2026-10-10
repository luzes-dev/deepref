//! OpenAI-compatible chat-completions adapter. It serves OpenCode Go (the
//! default) and the retired Z.AI legacy provider. Both support JSON-mode
//! structured output and tool calling over the plain `/chat/completions` wire
//! format. The few wire differences live in [`ProviderDialect`].

use std::time::Duration;

use deepref_domain::ProjectId;
use serde_json::{Value, json};

use crate::{
    AiError, AiFuture, AiGateway, CompletionRequest, GatewayCompletion, GroundedBlock,
    GroundingContextBuilder, ProviderEndpoint, canonical_json,
};

const MAX_ATTEMPTS: usize = 3;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(180);
/// Identifies DeepRef to the provider, as OpenCode asks clients to.
const USER_AGENT: &str = concat!("deepref/", env!("CARGO_PKG_VERSION"));

/// The wire quirks of one OpenAI-compatible provider. Everything else about
/// the protocol is shared.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderDialect {
    /// OpenCode Go subscription (`https://opencode.ai/zen/go/v1`).
    OpenCodeGo,
    /// Z.AI, retired. Kept for emergencies.
    Zai,
}

impl ProviderDialect {
    /// The dialect for a configured provider id (`opencode-go` or `zai`).
    pub fn from_id(id: &str) -> Option<Self> {
        match id {
            "opencode-go" => Some(Self::OpenCodeGo),
            "zai" => Some(Self::Zai),
            _ => None,
        }
    }

    pub const fn id(self) -> &'static str {
        match self {
            Self::OpenCodeGo => "opencode-go",
            Self::Zai => "zai",
        }
    }

    /// `reasoning_effort` sent with every call. glm-5.3-flash always reasons
    /// and rejects `thinking: disabled` on Z.AI (verified there: 0 reasoning
    /// tokens for simple JSON at "low"). A route can override it through
    /// `parameters.additional`.
    const fn reasoning_effort(self) -> Option<&'static str> {
        match self {
            Self::OpenCodeGo | Self::Zai => Some("low"),
        }
    }

    /// The header that carries a stable session id, for providers that ask
    /// for one.
    const fn session_header(self) -> Option<&'static str> {
        match self {
            Self::OpenCodeGo => Some("x-opencode-session"),
            Self::Zai => None,
        }
    }
}

pub struct OpenAiCompatGateway {
    client: reqwest::Client,
    dialect: ProviderDialect,
    base_url: String,
    api_key: String,
}

impl std::fmt::Debug for OpenAiCompatGateway {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // The configured URL may carry credentials, so only its normalized endpoint is printed.
        let endpoint = ProviderEndpoint::from_configured_url(&self.base_url)
            .map_or_else(|_| "<invalid>".to_owned(), |endpoint| endpoint.to_string());
        formatter
            .debug_struct("OpenAiCompatGateway")
            .field("provider", &self.dialect.id())
            .field("endpoint", &endpoint)
            .field("api_key", &"<redacted>")
            .finish()
    }
}

impl OpenAiCompatGateway {
    pub fn new(
        dialect: ProviderDialect,
        base_url: impl Into<String>,
        api_key: impl Into<String>,
    ) -> Result<Self, AiError> {
        let client = reqwest::Client::builder()
            .timeout(REQUEST_TIMEOUT)
            .user_agent(USER_AGENT)
            .build()
            .map_err(|_| AiError::Gateway("HTTP client could not be built".to_owned()))?;
        Ok(Self {
            client,
            dialect,
            base_url: base_url.into().trim_end_matches('/').to_owned(),
            api_key: api_key.into(),
        })
    }

    fn endpoint(&self) -> String {
        format!("{}/chat/completions", self.base_url)
    }

    fn request(&self, body: &Value, session: Option<&str>) -> reqwest::RequestBuilder {
        let mut request = self
            .client
            .post(self.endpoint())
            .bearer_auth(&self.api_key)
            .json(body);
        if let (Some(header), Some(session)) = (self.dialect.session_header(), session) {
            request = request.header(header, session);
        }
        request
    }

    async fn post(&self, body: &Value, session: Option<&str>) -> Result<Value, AiError> {
        let mut last_error = AiError::Gateway("provider request failed".to_owned());
        for attempt in 0..MAX_ATTEMPTS {
            if attempt > 0 {
                tokio::time::sleep(Duration::from_millis(500_u64 << attempt)).await;
            }
            let response = match self.request(body, session).send().await {
                Ok(response) => response,
                Err(error) => {
                    tracing::warn!(
                        provider = self.dialect.id(),
                        timeout = error.is_timeout(),
                        connect = error.is_connect(),
                        attempt,
                        "AI provider request failed"
                    );
                    last_error = AiError::Gateway(if error.is_timeout() {
                        "provider request timed out".to_owned()
                    } else {
                        "provider is unreachable".to_owned()
                    });
                    continue;
                }
            };
            let status = response.status();
            if status.is_success() {
                return response.json::<Value>().await.map_err(|_| {
                    AiError::Gateway("provider returned an unreadable body".to_owned())
                });
            }
            let code = status.as_u16();
            // Provider error bodies describe the request, never the key.
            let detail = response.text().await.unwrap_or_default();
            tracing::warn!(provider = self.dialect.id(), status = code, attempt, body = %detail.chars().take(400).collect::<String>(), "AI provider returned an error status");
            last_error = gateway_status_error(self.dialect, code, &detail);
            if !is_retryable(self.dialect, code, &detail) {
                break;
            }
        }
        Err(last_error)
    }
}

/// Z.AI answers an empty account balance with HTTP 429 and error code 1113,
/// which is not a rate limit and never clears by retrying.
const ZAI_INSUFFICIENT_BALANCE: &str = "1113";

fn provider_error_code(detail: &str) -> Option<String> {
    let body: Value = serde_json::from_str(detail).ok()?;
    match body.pointer("/error/code")? {
        Value::String(code) => Some(code.clone()),
        Value::Number(code) => Some(code.to_string()),
        _ => None,
    }
}

fn zai_balance_exhausted(code: u16, detail: &str) -> bool {
    code == 402 || provider_error_code(detail).as_deref() == Some(ZAI_INSUFFICIENT_BALANCE)
}

/// Whether a plan cap is spent: OpenCode Go's 5-hour, weekly or monthly
/// limit, or any usage or spending limit. The public docs do not give the
/// error text, so this matches the wording providers commonly use. Ordinary
/// rate limits never match, and a 402 means payment is required.
fn subscription_limit_reached(code: u16, detail: &str) -> bool {
    if code == 402 {
        return true;
    }
    if !matches!(code, 403 | 429) {
        return false;
    }
    // `usage_limit_reached` and `usage-limit` read the same as the wording.
    let text = detail.to_ascii_lowercase().replace(['_', '-'], " ");
    let reached = (text.contains("limit reached") || text.contains("limit exceeded"))
        && !text.contains("rate limit");
    reached
        || [
            "usage limit",
            "spending limit",
            "weekly limit",
            "monthly limit",
            "5 hour limit",
        ]
        .iter()
        .any(|phrase| text.contains(phrase))
}

/// The request can succeed later on its own. Rate limits and server errors
/// can. An empty balance or a spent plan cap cannot, so retrying only wastes
/// time and money.
fn is_retryable(dialect: ProviderDialect, code: u16, detail: &str) -> bool {
    let spent = (dialect == ProviderDialect::Zai && zai_balance_exhausted(code, detail))
        || subscription_limit_reached(code, detail);
    !spent && (code == 429 || (500..600).contains(&code))
}

fn gateway_status_error(dialect: ProviderDialect, code: u16, detail: &str) -> AiError {
    if dialect == ProviderDialect::Zai && zai_balance_exhausted(code, detail) {
        return AiError::Gateway("provider account has insufficient balance".to_owned());
    }
    if subscription_limit_reached(code, detail) {
        return AiError::SubscriptionLimit;
    }
    AiError::Gateway(match code {
        401 | 403 => "provider rejected the credentials".to_owned(),
        429 => "provider rate limit reached".to_owned(),
        _ => format!("provider returned HTTP {code}"),
    })
}

fn usage(response: &Value) -> (u64, u64) {
    let read = |key: &str| {
        response
            .pointer(&format!("/usage/{key}"))
            .and_then(Value::as_u64)
            .unwrap_or(0)
    };
    (read("prompt_tokens"), read("completion_tokens"))
}

/// The longest provider revision label kept. Model names and fingerprints are far shorter.
const MAX_REVISION_LABEL_BYTES: usize = 128;

/// What the provider says served a call: the response `model` and `system_fingerprint`.
///
/// This is audit data only. It is known only after the call, so it cannot be part of the
/// semantic identity fixed before the call. Providers may expose only a mutable alias, and
/// a silent upstream change behind the same alias cannot be ruled out from the identity alone;
/// continuous monitoring of the answers is the fallback. A label that is empty, too long or
/// contains control characters is dropped rather than stored.
fn provider_revision(response: &Value) -> (Option<String>, Option<String>) {
    (
        revision_label(response.get("model")),
        revision_label(response.get("system_fingerprint")),
    )
}

fn revision_label(value: Option<&Value>) -> Option<String> {
    let text = value?.as_str()?.trim();
    let usable = !text.is_empty()
        && text.len() <= MAX_REVISION_LABEL_BYTES
        && !text.chars().any(char::is_control);
    usable.then(|| text.to_owned())
}

/// Models sometimes wrap JSON in a Markdown fence even in JSON mode.
pub fn strip_code_fence(text: &str) -> &str {
    let trimmed = text.trim();
    let Some(rest) = trimmed.strip_prefix("```") else {
        return trimmed;
    };
    let rest = rest.strip_prefix("json").unwrap_or(rest);
    rest.trim().strip_suffix("```").map_or(trimmed, str::trim)
}

fn base_body(
    dialect: ProviderDialect,
    route: &crate::ResolvedModel,
    messages: Vec<Value>,
    max_tokens: Option<u32>,
) -> Value {
    let mut body = json!({
        "model": route.model,
        "messages": messages,
        "stream": false,
    });
    if let Some(effort) = dialect.reasoning_effort() {
        body["reasoning_effort"] = json!(effort);
    }
    if let Some(temperature) = route.parameters.temperature {
        body["temperature"] = json!(temperature);
    }
    if let Some(top_p) = route.parameters.top_p {
        body["top_p"] = json!(top_p);
    }
    if let Some(max_tokens) = max_tokens.or(route.parameters.max_tokens) {
        body["max_tokens"] = json!(max_tokens);
    }
    for (key, value) in &route.parameters.additional {
        body[key] = value.clone();
    }
    body
}

fn session_id(project_id: Option<ProjectId>) -> Option<String> {
    project_id.map(|id| id.as_uuid().to_string())
}

/// The chat-completions body for one structured call: the system prompt with
/// the output schema, the user prompt with its grounding evidence appended, and
/// JSON-object response format. Pure, so the golden fixtures in `deepref-review`
/// can render exactly what [`OpenAiCompatGateway`] sends.
pub fn structured_request_body(
    dialect: ProviderDialect,
    route: &crate::ResolvedModel,
    system_prompt: &str,
    user_prompt: &str,
    evidence: &[GroundedBlock],
    schema: &Value,
) -> Value {
    let mut user = user_prompt.to_owned();
    if !evidence.is_empty() {
        user.push_str("\n\n");
        user.push_str(&GroundingContextBuilder::render(evidence));
    }
    let system = format!(
        "{system_prompt}\n\nRespond with a single JSON object and nothing else (no Markdown, no prose). \
         It must validate against this JSON Schema:\n{}",
        canonical_json(schema)
    );
    let mut body = base_body(
        dialect,
        route,
        vec![
            json!({"role": "system", "content": system}),
            json!({"role": "user", "content": user}),
        ],
        None,
    );
    body["response_format"] = json!({"type": "json_object"});
    body
}

impl AiGateway for OpenAiCompatGateway {
    fn complete<'a>(&'a self, request: CompletionRequest) -> AiFuture<'a, GatewayCompletion> {
        Box::pin(async move {
            request.route.validate()?;
            let body = structured_request_body(
                self.dialect,
                &request.route,
                &request.system_prompt,
                &request.user_prompt,
                &request.evidence,
                &request.schema,
            );
            let session = session_id(request.project_id);
            let response = self.post(&body, session.as_deref()).await?;
            let content = response
                .pointer("/choices/0/message/content")
                .and_then(Value::as_str)
                .filter(|text| !text.trim().is_empty())
                .ok_or_else(|| {
                    tracing::warn!(finish_reason = ?response.pointer("/choices/0/finish_reason"), "AI provider returned no structured text");
                    AiError::Gateway("provider returned no structured text".to_owned())
                })?;
            let (input_tokens, output_tokens) = usage(&response);
            let (served_model, system_fingerprint) = provider_revision(&response);
            tracing::debug!(ai.provider = %request.route.provider, ai.model = %request.route.model, "structured completion finished");
            Ok(GatewayCompletion {
                output_json: strip_code_fence(content).to_owned(),
                input_tokens,
                output_tokens,
                cost_micros: None,
                served_model,
                system_fingerprint,
            })
        })
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };

    use super::*;
    use crate::{ModelParameters, ModelProfile, ResolvedModel};

    #[test]
    fn a_zai_empty_balance_is_reported_as_such_and_not_retried() {
        let body =
            r#"{"error":{"code":"1113","message":"Insufficient balance or no resource package."}}"#;
        assert_eq!(
            gateway_status_error(ProviderDialect::Zai, 429, body),
            AiError::Gateway("provider account has insufficient balance".to_owned())
        );
        assert!(!is_retryable(ProviderDialect::Zai, 429, body));
        assert!(is_retryable(
            ProviderDialect::Zai,
            429,
            r#"{"error":{"code":"1302","message":"Rate limit"}}"#
        ));
        assert!(is_retryable(ProviderDialect::Zai, 503, ""));
        assert!(!is_retryable(ProviderDialect::Zai, 400, ""));
        assert_eq!(
            gateway_status_error(ProviderDialect::Zai, 429, "not json"),
            AiError::Gateway("provider rate limit reached".to_owned())
        );
    }

    #[test]
    fn opencode_plan_caps_are_a_subscription_limit_and_are_not_retried() {
        let five_hour = r#"{"error":{"type":"usage_limit_reached","message":"Usage limit reached for 5 hour. Try again later."}}"#;
        let weekly = r#"{"error":{"message":"You have reached your weekly usage limit."}}"#;
        let monthly = r#"{"error":"Monthly limit exceeded for this model"}"#;
        for (code, body) in [
            (429, five_hour),
            (429, weekly),
            (403, monthly),
            (402, ""),
            (429, "usage-limit"),
        ] {
            assert_eq!(
                gateway_status_error(ProviderDialect::OpenCodeGo, code, body),
                AiError::SubscriptionLimit,
                "{code} {body}"
            );
            assert!(
                !is_retryable(ProviderDialect::OpenCodeGo, code, body),
                "{code} {body}"
            );
        }
    }

    #[test]
    fn ordinary_rate_limits_and_credential_errors_keep_their_meaning() {
        let rate = r#"{"error":{"message":"Rate limit reached for requests, retry in 2s"}}"#;
        assert_eq!(
            gateway_status_error(ProviderDialect::OpenCodeGo, 429, rate),
            AiError::Gateway("provider rate limit reached".to_owned())
        );
        assert!(is_retryable(ProviderDialect::OpenCodeGo, 429, rate));
        assert_eq!(
            gateway_status_error(
                ProviderDialect::OpenCodeGo,
                403,
                r#"{"error":"invalid api key"}"#
            ),
            AiError::Gateway("provider rejected the credentials".to_owned())
        );
        assert!(!is_retryable(
            ProviderDialect::OpenCodeGo,
            403,
            "invalid api key"
        ));
        // A 402 on OpenCode is a spent plan, not Z.AI's balance wording.
        assert_eq!(
            gateway_status_error(ProviderDialect::OpenCodeGo, 402, ""),
            AiError::SubscriptionLimit
        );
    }

    #[test]
    fn dialects_are_selected_by_provider_id() {
        assert_eq!(
            ProviderDialect::from_id("opencode-go"),
            Some(ProviderDialect::OpenCodeGo)
        );
        assert_eq!(ProviderDialect::from_id("zai"), Some(ProviderDialect::Zai));
        assert_eq!(ProviderDialect::from_id("openai"), None);
        assert_eq!(ProviderDialect::OpenCodeGo.id(), "opencode-go");
    }

    #[test]
    fn code_fences_are_stripped() {
        assert_eq!(strip_code_fence("```json\n{\"a\":1}\n```"), "{\"a\":1}");
        assert_eq!(strip_code_fence("```\n{\"a\":1}\n```"), "{\"a\":1}");
        assert_eq!(strip_code_fence(" {\"a\":1} "), "{\"a\":1}");
    }

    #[test]
    fn debug_never_prints_the_key() {
        let gateway = OpenAiCompatGateway::new(
            ProviderDialect::OpenCodeGo,
            "https://example.test",
            "sk-secret",
        )
        .unwrap_or_else(|_| unreachable!());
        let rendered = format!("{gateway:?}");
        assert!(!rendered.contains("sk-secret"));
        assert!(rendered.contains("opencode-go"));
    }

    fn route(model: &str) -> ResolvedModel {
        ResolvedModel {
            profile: ModelProfile::Reasoning,
            provider: "opencode-go".to_owned(),
            model: model.to_owned(),
            model_version: model.to_owned(),
            parameters: ModelParameters::default(),
            route_id: None,
        }
    }

    #[test]
    fn every_dialect_sends_the_same_low_effort_request_shape() {
        for dialect in [ProviderDialect::OpenCodeGo, ProviderDialect::Zai] {
            let body = base_body(
                dialect,
                &route("glm-5.3-flash"),
                vec![json!({"role": "user", "content": "hi"})],
                Some(64),
            );
            assert_eq!(body["model"], json!("glm-5.3-flash"), "{dialect:?}");
            assert_eq!(body["stream"], json!(false), "{dialect:?}");
            assert_eq!(body["reasoning_effort"], json!("low"), "{dialect:?}");
            assert_eq!(body["max_tokens"], json!(64), "{dialect:?}");
            assert!(body.get("thinking").is_none(), "{dialect:?}");
        }
    }

    #[test]
    fn only_opencode_takes_the_session_header() {
        assert_eq!(
            ProviderDialect::OpenCodeGo.session_header(),
            Some("x-opencode-session")
        );
        assert_eq!(ProviderDialect::Zai.session_header(), None);
    }

    fn streaming_route() -> crate::ResolvedModel {
        crate::ResolvedModel {
            profile: crate::ModelProfile::Reasoning,
            provider: "opencode-go".to_owned(),
            model: "glm-5.3-flash".to_owned(),
            model_version: "glm-5.3-flash".to_owned(),
            parameters: crate::ModelParameters::default(),
            route_id: None,
        }
    }

    /// Reads one HTTP request and returns its lowercased header block and its
    /// body, sized by `Content-Length`.
    async fn read_request(socket: &mut tokio::net::TcpStream) -> (String, String) {
        use tokio::io::AsyncReadExt;
        let mut received = Vec::new();
        let mut buffer = [0_u8; 4096];
        loop {
            let read = socket.read(&mut buffer).await.unwrap_or(0);
            received.extend_from_slice(&buffer[..read]);
            if let Some(end) = received.windows(4).position(|window| window == b"\r\n\r\n") {
                let headers = String::from_utf8_lossy(&received[..end]).to_ascii_lowercase();
                let length = headers
                    .lines()
                    .find_map(|line| line.strip_prefix("content-length:"))
                    .and_then(|value| value.trim().parse::<usize>().ok())
                    .unwrap_or(0);
                let body_end = (end + 4 + length).min(received.len());
                if received.len() >= end + 4 + length || read == 0 {
                    let body = String::from_utf8_lossy(&received[end + 4..body_end]).into_owned();
                    return (headers, body);
                }
            }
            if read == 0 {
                return (
                    String::from_utf8_lossy(&received).to_ascii_lowercase(),
                    String::new(),
                );
            }
        }
    }

    /// A local provider that answers every connection with `response` and
    /// counts the connections it accepts.
    async fn canned_provider(response: String) -> (String, Arc<AtomicUsize>) {
        use tokio::io::AsyncWriteExt;
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .unwrap_or_else(|_| unreachable!());
        let address = listener.local_addr().unwrap_or_else(|_| unreachable!());
        let hits = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&hits);
        tokio::spawn(async move {
            while let Ok((mut socket, _)) = listener.accept().await {
                counter.fetch_add(1, Ordering::SeqCst);
                let _ = read_request(&mut socket).await;
                let _ = socket.write_all(response.as_bytes()).await;
                let _ = socket.shutdown().await;
            }
        });
        (format!("http://{address}"), hits)
    }

    fn http_response(status: &str, body: &str) -> String {
        format!(
            "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        )
    }

    #[tokio::test]
    async fn the_legacy_zai_dialect_sends_no_session_header() {
        let body = r#"{"choices":[{"message":{"content":"{\"ok\":true}"},"finish_reason":"stop"}],"usage":{"prompt_tokens":3,"completion_tokens":4}}"#;
        let (base_url, _) = canned_provider(http_response("200 OK", body)).await;
        let gateway = OpenAiCompatGateway::new(ProviderDialect::Zai, base_url, "test-key")
            .unwrap_or_else(|_| unreachable!());
        let request = CompletionRequest {
            project_id: Some(ProjectId::new(uuid::Uuid::from_u128(9))),
            route: streaming_route(),
            system_prompt: "system".to_owned(),
            user_prompt: "user".to_owned(),
            evidence: Vec::new(),
            schema: json!({"type": "object"}),
        };
        let completion = gateway
            .complete(request)
            .await
            .unwrap_or_else(|_| unreachable!());
        assert_eq!(completion.output_json, "{\"ok\":true}");
        assert_eq!((completion.input_tokens, completion.output_tokens), (3, 4));
        assert_eq!(completion.cost_micros, None);
    }

    #[test]
    fn the_provider_revision_is_read_from_the_response_and_never_invented() {
        let response: Value = serde_json::from_str(
            r#"{"model":"glm-5.3-flash-2026-09","system_fingerprint":"fp_44709d6fcb","choices":[]}"#,
        )
        .unwrap_or_else(|_| unreachable!());
        assert_eq!(
            provider_revision(&response),
            (
                Some("glm-5.3-flash-2026-09".to_owned()),
                Some("fp_44709d6fcb".to_owned())
            )
        );
        let bare: Value =
            serde_json::from_str(r#"{"choices":[]}"#).unwrap_or_else(|_| unreachable!());
        assert_eq!(provider_revision(&bare), (None, None));
        let hostile: Value = serde_json::from_value(json!({
            "model": "  ",
            "system_fingerprint": format!("fp{}", "x".repeat(MAX_REVISION_LABEL_BYTES)),
        }))
        .unwrap_or_else(|_| unreachable!());
        assert_eq!(provider_revision(&hostile), (None, None));
        let control: Value = serde_json::from_value(json!({"model": "glm\n<script>"}))
            .unwrap_or_else(|_| unreachable!());
        assert_eq!(provider_revision(&control), (None, None));
    }

    #[tokio::test]
    async fn a_structured_completion_keeps_the_provider_reported_revision() {
        let body = r#"{"model":"glm-5.3-flash-2026-09","system_fingerprint":"fp_44709d6fcb","choices":[{"message":{"content":"{\"ok\":true}"},"finish_reason":"stop"}],"usage":{"prompt_tokens":5,"completion_tokens":6}}"#;
        let (base_url, _) = canned_provider(http_response("200 OK", body)).await;
        let gateway = OpenAiCompatGateway::new(ProviderDialect::OpenCodeGo, base_url, "test-key")
            .unwrap_or_else(|_| unreachable!());
        let request = CompletionRequest {
            project_id: None,
            route: streaming_route(),
            system_prompt: "system".to_owned(),
            user_prompt: "user".to_owned(),
            evidence: Vec::new(),
            schema: json!({"type": "object"}),
        };
        let completion = gateway
            .complete(request)
            .await
            .unwrap_or_else(|_| unreachable!());
        assert_eq!(completion.output_json, "{\"ok\":true}");
        assert_eq!(
            completion.served_model.as_deref(),
            Some("glm-5.3-flash-2026-09")
        );
        assert_eq!(
            completion.system_fingerprint.as_deref(),
            Some("fp_44709d6fcb")
        );
    }

    #[test]
    fn the_debug_output_prints_the_normalized_endpoint_and_no_credentials() {
        let gateway = OpenAiCompatGateway::new(
            ProviderDialect::OpenCodeGo,
            "https://user:secret@Proxy.Example:443/zen/go/v1/?key=abc",
            "sk-secret",
        )
        .unwrap_or_else(|_| unreachable!());
        let rendered = format!("{gateway:?}");
        assert!(
            rendered.contains("https://proxy.example/zen/go/v1"),
            "{rendered}"
        );
        assert!(!rendered.contains("secret"), "{rendered}");
        assert!(!rendered.contains("abc"), "{rendered}");
    }
}
