//! OpenAI-compatible chat-completions adapter. It serves OpenCode Go (the
//! default) and the retired Z.AI legacy provider. Both support JSON-mode
//! structured output and tool calling over the plain `/chat/completions` wire
//! format. The few wire differences live in [`ProviderDialect`].

use std::time::Duration;

use deepref_domain::ProjectId;
use serde_json::{Value, json};

use crate::{
    AiError, AiFuture, AiGateway, ChatCompletion, ChatGateway, ChatMessage, ChatRequest,
    ChatTextSink, ChatToolCall, CompletionRequest, GatewayCompletion, GroundedBlock,
    GroundingContextBuilder, ProviderEndpoint, ToolDeclaration, canonical_json, estimate_tokens,
};

const MAX_ATTEMPTS: usize = 3;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(180);
/// Identifies DeepRef to the provider, as OpenCode asks clients to.
const USER_AGENT: &str = concat!("deepref/", env!("CARGO_PKG_VERSION"));
/// Tool calls per streamed answer. Far above anything the assistant emits; a
/// larger index means the stream is malformed, so the turn fails instead of
/// allocating without bound.
const MAX_STREAMED_TOOL_CALLS: usize = 32;

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

    /// Opens a streamed request. It retries the failures [`Self::post`] retries
    /// (connection errors, rate limits, server errors). Retries happen before
    /// any text is read, so a retry never repeats streamed output.
    async fn open_stream(
        &self,
        body: &Value,
        session: Option<&str>,
    ) -> Result<reqwest::Response, AiError> {
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
                        "AI provider stream request failed"
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
                return Ok(response);
            }
            let code = status.as_u16();
            let detail = response.text().await.unwrap_or_default();
            tracing::warn!(provider = self.dialect.id(), status = code, attempt, body = %detail.chars().take(400).collect::<String>(), "AI provider rejected a stream request");
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

fn wire_message(message: &ChatMessage) -> Value {
    match message {
        ChatMessage::System(content) => json!({"role": "system", "content": content}),
        ChatMessage::User(content) => json!({"role": "user", "content": content}),
        ChatMessage::Assistant {
            content,
            tool_calls,
        } => {
            let mut value = json!({"role": "assistant", "content": content});
            if !tool_calls.is_empty() {
                value["tool_calls"] = Value::Array(
                    tool_calls
                        .iter()
                        .map(|call| {
                            json!({
                                "id": call.id,
                                "type": "function",
                                "function": {
                                    "name": call.name,
                                    "arguments": call.arguments.to_string(),
                                },
                            })
                        })
                        .collect(),
                );
            }
            value
        }
        ChatMessage::Tool {
            tool_call_id,
            content,
        } => json!({"role": "tool", "tool_call_id": tool_call_id, "content": content}),
    }
}

fn wire_tool_declarations(tools: &[ToolDeclaration]) -> Value {
    Value::Array(
        tools
            .iter()
            .map(|tool| {
                json!({
                    "type": "function",
                    "function": {
                        "name": tool.name,
                        "description": tool.description,
                        "parameters": tool.parameters,
                    },
                })
            })
            .collect(),
    )
}

fn malformed_stream() -> AiError {
    AiError::Gateway("provider returned a malformed stream".to_owned())
}

#[derive(Default)]
struct PartialToolCall {
    id: Option<String>,
    name: String,
    arguments: String,
}

/// What a finished stream produced, before usage is settled.
#[derive(Debug, PartialEq)]
struct StreamedChat {
    content: String,
    tool_calls: Vec<ChatToolCall>,
    /// `(prompt_tokens, completion_tokens)` from the usage chunk, when sent.
    usage: Option<(u64, u64)>,
}

/// Incremental reader for an OpenAI-compatible `chat.completion.chunk` stream.
///
/// Network chunks may split lines or UTF-8 sequences anywhere, so bytes are
/// buffered up to each newline. A blank line ends an event; each event carries
/// one JSON chunk, and `data: [DONE]` ends the stream.
#[derive(Default)]
struct ChatStreamParser {
    line: Vec<u8>,
    data: Vec<String>,
    text: String,
    tool_calls: Vec<PartialToolCall>,
    usage: Option<(u64, u64)>,
    finish_reason: Option<String>,
    done: bool,
}

impl ChatStreamParser {
    fn feed(&mut self, bytes: &[u8], on_text: &mut dyn FnMut(&str)) -> Result<(), AiError> {
        for &byte in bytes {
            if self.done {
                break;
            }
            if byte == b'\n' {
                let line = std::mem::take(&mut self.line);
                self.take_line(&line, on_text)?;
            } else {
                self.line.push(byte);
            }
        }
        Ok(())
    }

    fn take_line(&mut self, raw: &[u8], on_text: &mut dyn FnMut(&str)) -> Result<(), AiError> {
        let decoded = String::from_utf8_lossy(raw);
        let line: &str = &decoded;
        let line = line.strip_suffix('\r').unwrap_or(line);
        if line.is_empty() {
            return self.dispatch(on_text);
        }
        // Other SSE fields (`event:`, `id:`) and comment lines (`:`) carry nothing we use.
        if let Some(value) = line.strip_prefix("data:") {
            self.data
                .push(value.strip_prefix(' ').unwrap_or(value).to_owned());
        }
        Ok(())
    }

    fn dispatch(&mut self, on_text: &mut dyn FnMut(&str)) -> Result<(), AiError> {
        if self.data.is_empty() {
            return Ok(());
        }
        let payload = std::mem::take(&mut self.data).join("\n");
        if payload.trim() == "[DONE]" {
            self.done = true;
            return Ok(());
        }
        let chunk = serde_json::from_str::<Value>(&payload).map_err(|_| malformed_stream())?;
        self.apply(&chunk, on_text)
    }

    fn apply(&mut self, chunk: &Value, on_text: &mut dyn FnMut(&str)) -> Result<(), AiError> {
        if let Some(error) = chunk.get("error") {
            tracing::warn!(
                body = %error.to_string().chars().take(400).collect::<String>(),
                "AI provider stream reported an error"
            );
            return Err(AiError::Gateway("provider stream failed".to_owned()));
        }
        if let Some(usage) = chunk.get("usage").filter(|usage| usage.is_object()) {
            self.usage = Some((
                usage_count(usage, "prompt_tokens"),
                usage_count(usage, "completion_tokens"),
            ));
        }
        let Some(choice) = chunk.pointer("/choices/0") else {
            return Ok(());
        };
        if let Some(reason) = choice.get("finish_reason").and_then(Value::as_str) {
            self.finish_reason = Some(reason.to_owned());
        }
        let Some(delta) = choice.get("delta") else {
            return Ok(());
        };
        // `reasoning_content` is the model's private reasoning; only answer text is shown.
        if let Some(text) = delta
            .get("content")
            .and_then(Value::as_str)
            .filter(|text| !text.is_empty())
        {
            self.text.push_str(text);
            on_text(text);
        }
        if let Some(calls) = delta.get("tool_calls").and_then(Value::as_array) {
            for (position, call) in calls.iter().enumerate() {
                let index = call
                    .get("index")
                    .and_then(Value::as_u64)
                    .and_then(|index| usize::try_from(index).ok())
                    .unwrap_or(position);
                if index >= MAX_STREAMED_TOOL_CALLS {
                    return Err(malformed_stream());
                }
                if self.tool_calls.len() <= index {
                    self.tool_calls
                        .resize_with(index + 1, PartialToolCall::default);
                }
                let partial = &mut self.tool_calls[index];
                if let Some(id) = call
                    .get("id")
                    .and_then(Value::as_str)
                    .filter(|id| !id.is_empty())
                {
                    partial.id = Some(id.to_owned());
                }
                let Some(function) = call.get("function") else {
                    continue;
                };
                // Names arrive whole; arguments arrive as JSON text fragments.
                if let Some(name) = function
                    .get("name")
                    .and_then(Value::as_str)
                    .filter(|name| !name.is_empty())
                    && partial.name.is_empty()
                {
                    partial.name = name.to_owned();
                }
                match function.get("arguments") {
                    Some(Value::String(fragment)) => partial.arguments.push_str(fragment),
                    Some(Value::Null) | None => {}
                    Some(other) => partial.arguments.push_str(&other.to_string()),
                }
            }
        }
        Ok(())
    }

    fn finish(mut self, on_text: &mut dyn FnMut(&str)) -> Result<StreamedChat, AiError> {
        if !self.done && !self.line.is_empty() {
            let line = std::mem::take(&mut self.line);
            self.take_line(&line, on_text)?;
        }
        if !self.done {
            self.dispatch(on_text)?;
        }
        // A stream that stops before `[DONE]` or a finish reason was cut off,
        // so its text is incomplete and must not be treated as an answer.
        if !self.done && self.finish_reason.is_none() {
            return Err(AiError::Gateway(
                "provider stream ended before the answer finished".to_owned(),
            ));
        }
        let tool_calls = self
            .tool_calls
            .into_iter()
            .enumerate()
            .map(|(position, partial)| {
                let raw = partial.arguments;
                let arguments = if raw.trim().is_empty() {
                    json!({})
                } else {
                    serde_json::from_str::<Value>(&raw).unwrap_or(Value::String(raw))
                };
                ChatToolCall {
                    id: partial.id.unwrap_or_else(|| format!("call_{position}")),
                    name: partial.name,
                    arguments,
                }
            })
            .collect();
        Ok(StreamedChat {
            content: self.text,
            tool_calls,
            usage: self.usage,
        })
    }
}

fn usage_count(usage: &Value, key: &str) -> u64 {
    usage.get(key).and_then(Value::as_u64).unwrap_or(0)
}

/// Settles a streamed answer into a completion. Providers report usage in the
/// final chunk; when one does not, the tokens are estimated from the text and
/// the request, and the warning makes that estimate visible in the logs. The
/// cost is priced by the metering layer, which knows the configured prices.
fn stream_completion(
    model: &str,
    streamed: StreamedChat,
    estimated_input_tokens: u64,
) -> ChatCompletion {
    let (input_tokens, output_tokens) = streamed.usage.unwrap_or_else(|| {
        tracing::warn!(
            ai.model = %model,
            "AI provider stream reported no usage; the usage ledger holds an estimate"
        );
        let output_chars = streamed.content.chars().count()
            + streamed
                .tool_calls
                .iter()
                .map(|call| call.name.chars().count() + call.arguments.to_string().chars().count())
                .sum::<usize>();
        (estimated_input_tokens, estimate_tokens(output_chars))
    });
    ChatCompletion {
        content: streamed.content,
        tool_calls: streamed.tool_calls,
        input_tokens,
        output_tokens,
        cost_micros: None,
    }
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

impl ChatGateway for OpenAiCompatGateway {
    fn chat<'a>(&'a self, request: ChatRequest) -> AiFuture<'a, ChatCompletion> {
        Box::pin(async move {
            request.route.validate()?;
            let messages = request.messages.iter().map(wire_message).collect();
            let mut body = base_body(
                self.dialect,
                &request.route,
                messages,
                request.max_output_tokens,
            );
            if !request.tools.is_empty() {
                body["tools"] = wire_tool_declarations(&request.tools);
                body["tool_choice"] = json!("auto");
            }
            let session = session_id(request.project_id);
            let response = self.post(&body, session.as_deref()).await?;
            let message = response
                .pointer("/choices/0/message")
                .ok_or_else(|| AiError::Gateway("provider returned no message".to_owned()))?;
            let content = message
                .get("content")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned();
            let mut tool_calls = Vec::new();
            for call in message
                .get("tool_calls")
                .and_then(Value::as_array)
                .map(Vec::as_slice)
                .unwrap_or_default()
            {
                let name = call
                    .pointer("/function/name")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned();
                let raw_arguments = call
                    .pointer("/function/arguments")
                    .and_then(Value::as_str)
                    .unwrap_or("{}");
                // Malformed arguments are surfaced to the loop as a string so
                // it can tell the model rather than crash the turn.
                let arguments = serde_json::from_str::<Value>(raw_arguments)
                    .unwrap_or_else(|_| Value::String(raw_arguments.to_owned()));
                let id = call
                    .get("id")
                    .and_then(Value::as_str)
                    .map_or_else(|| format!("call_{}", tool_calls.len()), str::to_owned);
                tool_calls.push(ChatToolCall {
                    id,
                    name,
                    arguments,
                });
            }
            let (input_tokens, output_tokens) = usage(&response);
            Ok(ChatCompletion {
                content,
                tool_calls,
                input_tokens,
                output_tokens,
                cost_micros: None,
            })
        })
    }

    fn chat_streaming<'a>(
        &'a self,
        request: ChatRequest,
        on_text: ChatTextSink<'a>,
    ) -> AiFuture<'a, ChatCompletion> {
        Box::pin(async move {
            request.route.validate()?;
            let estimated_input_tokens = request.approximate_input_tokens();
            let messages = request.messages.iter().map(wire_message).collect();
            let mut body = base_body(
                self.dialect,
                &request.route,
                messages,
                request.max_output_tokens,
            );
            // `stream_options.include_usage` puts the usage in the final chunk.
            body["stream"] = json!(true);
            body["stream_options"] = json!({"include_usage": true});
            if !request.tools.is_empty() {
                body["tools"] = wire_tool_declarations(&request.tools);
                body["tool_choice"] = json!("auto");
            }
            let session = session_id(request.project_id);
            let mut response = self.open_stream(&body, session.as_deref()).await?;
            let mut parser = ChatStreamParser::default();
            loop {
                let next = response.chunk().await.map_err(|error| {
                    tracing::warn!(
                        timeout = error.is_timeout(),
                        "AI provider stream was interrupted"
                    );
                    AiError::Gateway("provider stream was interrupted".to_owned())
                })?;
                let Some(bytes) = next else { break };
                parser.feed(&bytes, &mut *on_text)?;
            }
            let streamed = parser.finish(&mut *on_text)?;
            Ok(stream_completion(
                &request.route.model,
                streamed,
                estimated_input_tokens,
            ))
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

    /// Runs the parser over network-sized pieces and collects what it emitted.
    fn run_stream(pieces: &[&[u8]]) -> (Result<StreamedChat, AiError>, Vec<String>) {
        let mut deltas = Vec::new();
        let result = {
            let mut sink = |text: &str| deltas.push(text.to_owned());
            let mut parser = ChatStreamParser::default();
            pieces
                .iter()
                .try_for_each(|piece| parser.feed(piece, &mut sink))
                .and_then(|()| parser.finish(&mut sink))
        };
        (result, deltas)
    }

    /// Lines captured from Z.AI `glm-5.3-flash` with `stream: true`: text deltas,
    /// a final chunk carrying `finish_reason` and `usage`, then `[DONE]`.
    const CAPTURED_TEXT_STREAM: &str = concat!(
        "data: {\"id\":\"2026\",\"object\":\"chat.completion.chunk\",\"model\":\"glm-5.3-flash\",\"choices\":[{\"index\":0,\"delta\":{\"role\":\"assistant\",\"content\":\"-\"}}]}\n\n",
        "data: {\"id\":\"2026\",\"object\":\"chat.completion.chunk\",\"model\":\"glm-5.3-flash\",\"choices\":[{\"index\":0,\"delta\":{\"role\":\"assistant\",\"content\":\" **\"}}]}\n\n",
        "data: {\"id\":\"2026\",\"object\":\"chat.completion.chunk\",\"model\":\"glm-5.3-flash\",\"choices\":[{\"index\":0,\"delta\":{\"role\":\"assistant\",\"content\":\"Apple\"}}]}\n\n",
        "data: {\"id\":\"2026\",\"object\":\"chat.completion.chunk\",\"model\":\"glm-5.3-flash\",\"choices\":[{\"index\":0,\"delta\":{\"role\":\"assistant\",\"content\":\"**\\n\"}}]}\n\n",
        "data: {\"id\":\"2026\",\"object\":\"chat.completion.chunk\",\"model\":\"glm-5.3-flash\",\"choices\":[{\"index\":0,\"delta\":{\"role\":\"assistant\",\"content\":\"Ch\"}}]}\n\n",
        "data: {\"id\":\"2026\",\"object\":\"chat.completion.chunk\",\"model\":\"glm-5.3-flash\",\"choices\":[{\"index\":0,\"delta\":{\"role\":\"assistant\",\"content\":\"erry**\"}}]}\n\n",
        "data: {\"id\":\"2026\",\"object\":\"chat.completion.chunk\",\"model\":\"glm-5.3-flash\",\"choices\":[{\"index\":0,\"finish_reason\":\"stop\",\"delta\":{\"role\":\"assistant\",\"content\":\"\"}}],\"usage\":{\"prompt_tokens\":27,\"completion_tokens\":16,\"total_tokens\":43,\"prompt_tokens_details\":{\"cached_tokens\":0},\"completion_tokens_details\":{\"reasoning_tokens\":0}}}\n\n",
        "data: [DONE]\n\n",
    );

    /// Lines captured from the same model when it answers with a tool call.
    const CAPTURED_TOOL_STREAM: &str = concat!(
        "data: {\"id\":\"2026\",\"object\":\"chat.completion.chunk\",\"choices\":[{\"index\":0,\"delta\":{\"tool_calls\":[{\"id\":\"call_c26ea7b3\",\"index\":0,\"type\":\"function\",\"function\":{\"name\":\"get_report\",\"arguments\":\"{\\\"report_id\\\":\\\"12\\\"}\"}}]}}]}\n\n",
        "data: {\"id\":\"2026\",\"object\":\"chat.completion.chunk\",\"choices\":[{\"index\":0,\"finish_reason\":\"tool_calls\",\"delta\":{\"role\":\"assistant\",\"content\":\"\"}}],\"usage\":{\"prompt_tokens\":165,\"completion_tokens\":13,\"total_tokens\":178}}\n\n",
        "data: [DONE]\n\n",
    );

    #[test]
    fn captured_text_stream_emits_each_delta_and_reads_the_usage_chunk() {
        let (result, deltas) = run_stream(&[CAPTURED_TEXT_STREAM.as_bytes()]);
        let streamed = result.unwrap_or_else(|_| unreachable!());
        assert_eq!(deltas, vec!["-", " **", "Apple", "**\n", "Ch", "erry**"]);
        assert_eq!(streamed.content, "- **Apple**\nCherry**");
        assert_eq!(streamed.usage, Some((27, 16)));
        assert!(streamed.tool_calls.is_empty());
    }

    #[test]
    fn network_chunks_may_split_lines_and_utf8_characters() {
        let stream = "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"ção\"}}]}\n\n\
                      data: {\"choices\":[{\"index\":0,\"finish_reason\":\"stop\",\"delta\":{}}]}\r\n\r\n\
                      data: [DONE]\n\n";
        let bytes = stream.as_bytes();
        // One byte at a time splits every line and the two-byte `ç` as well.
        let pieces: Vec<&[u8]> = bytes.chunks(1).collect();
        let (result, deltas) = run_stream(&pieces);
        let streamed = result.unwrap_or_else(|_| unreachable!());
        assert_eq!(deltas, vec!["ção"]);
        assert_eq!(streamed.content, "ção");
        assert_eq!(streamed.usage, None);
    }

    #[test]
    fn ignores_reasoning_and_anything_after_done() {
        let stream = "data: {\"choices\":[{\"index\":0,\"delta\":{\"reasoning_content\":\"private\"}}]}\n\n\
                      data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"Yes\"}}]}\n\n\
                      data: {\"choices\":[{\"index\":0,\"finish_reason\":\"stop\",\"delta\":{}}]}\n\n\
                      data: [DONE]\n\n\
                      data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"late\"}}]}\n\n";
        let (result, deltas) = run_stream(&[stream.as_bytes()]);
        let streamed = result.unwrap_or_else(|_| unreachable!());
        assert_eq!(deltas, vec!["Yes"]);
        assert_eq!(streamed.content, "Yes");
    }

    #[test]
    fn captured_tool_stream_assembles_the_call() {
        let (result, deltas) = run_stream(&[CAPTURED_TOOL_STREAM.as_bytes()]);
        let streamed = result.unwrap_or_else(|_| unreachable!());
        assert!(deltas.is_empty());
        assert_eq!(
            streamed.tool_calls,
            vec![ChatToolCall {
                id: "call_c26ea7b3".to_owned(),
                name: "get_report".to_owned(),
                arguments: json!({"report_id": "12"}),
            }]
        );
        assert_eq!(streamed.usage, Some((165, 13)));
    }

    #[test]
    fn tool_arguments_arrive_in_fragments_per_index() {
        let stream = "data: {\"choices\":[{\"index\":0,\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"a\",\"function\":{\"name\":\"get_report\",\"arguments\":\"{\\\"report_\"}}]}}]}\n\n\
                      data: {\"choices\":[{\"index\":0,\"delta\":{\"tool_calls\":[{\"index\":1,\"id\":\"b\",\"function\":{\"name\":\"get_project_overview\",\"arguments\":\"{}\"}},{\"index\":0,\"function\":{\"arguments\":\"id\\\":\\\"7\\\"}\"}}]}}]}\n\n\
                      data: {\"choices\":[{\"index\":0,\"finish_reason\":\"tool_calls\",\"delta\":{}}]}\n\n\
                      data: [DONE]\n\n";
        let (result, _) = run_stream(&[stream.as_bytes()]);
        let streamed = result.unwrap_or_else(|_| unreachable!());
        assert_eq!(streamed.tool_calls.len(), 2);
        assert_eq!(streamed.tool_calls[0].name, "get_report");
        assert_eq!(streamed.tool_calls[0].arguments, json!({"report_id": "7"}));
        assert_eq!(streamed.tool_calls[1].id, "b");
        assert_eq!(streamed.tool_calls[1].arguments, json!({}));
    }

    #[test]
    fn provider_error_chunks_fail_the_turn() {
        let stream = "data: {\"error\":{\"code\":\"1305\",\"message\":\"rate\"}}\n\n";
        let (result, deltas) = run_stream(&[stream.as_bytes()]);
        assert_eq!(
            result.err(),
            Some(AiError::Gateway("provider stream failed".to_owned()))
        );
        assert!(deltas.is_empty());
    }

    #[test]
    fn a_cut_off_stream_is_not_an_answer() {
        let stream = "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"Half\"}}]}\n\n";
        let (result, deltas) = run_stream(&[stream.as_bytes()]);
        assert_eq!(deltas, vec!["Half"]);
        assert!(result.is_err());
    }

    #[test]
    fn malformed_chunks_and_absurd_tool_indexes_are_rejected() {
        let (result, _) = run_stream(&[b"data: {not json}\n\n"]);
        assert_eq!(result.err(), Some(malformed_stream()));
        let (result, _) = run_stream(&[b"data: {\"choices\":[{\"index\":0,\"delta\":{\"tool_calls\":[{\"index\":9999,\"function\":{\"name\":\"x\"}}]}}]}\n\n"]);
        assert_eq!(result.err(), Some(malformed_stream()));
    }

    #[test]
    fn streams_without_usage_are_settled_as_estimates_for_the_metering_layer_to_price() {
        let streamed = StreamedChat {
            content: "x".repeat(400),
            tool_calls: Vec::new(),
            usage: None,
        };
        let completion = stream_completion("glm-5.3-flash", streamed, 1_000);
        assert_eq!(completion.input_tokens, 1_000);
        assert_eq!(completion.output_tokens, 100);
        assert_eq!(completion.cost_micros, None);
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

    fn chat_request(project_id: Option<ProjectId>) -> ChatRequest {
        ChatRequest {
            project_id,
            route: streaming_route(),
            messages: vec![ChatMessage::User("Say hello".to_owned())],
            tools: Vec::new(),
            max_output_tokens: Some(64),
        }
    }

    #[tokio::test]
    async fn a_plan_cap_on_the_stream_path_fails_once_with_limit_wording() {
        let body = r#"{"error":{"type":"usage_limit_reached","message":"Usage limit reached for 5 hour."}}"#;
        let (base_url, hits) = canned_provider(http_response("429 Too Many Requests", body)).await;
        let gateway = OpenAiCompatGateway::new(ProviderDialect::OpenCodeGo, base_url, "test-key")
            .unwrap_or_else(|_| unreachable!());
        let mut sink = |_: &str| {};
        let result = gateway.chat_streaming(chat_request(None), &mut sink).await;
        assert_eq!(result.err(), Some(AiError::SubscriptionLimit));
        assert_eq!(
            hits.load(Ordering::SeqCst),
            1,
            "a spent plan is not retried"
        );
    }

    #[tokio::test]
    async fn a_plan_cap_on_the_plain_path_fails_once_with_limit_wording() {
        let body = r#"{"error":{"message":"You have reached your weekly usage limit."}}"#;
        let (base_url, hits) = canned_provider(http_response("429 Too Many Requests", body)).await;
        let gateway = OpenAiCompatGateway::new(ProviderDialect::OpenCodeGo, base_url, "test-key")
            .unwrap_or_else(|_| unreachable!());
        let result = gateway.chat(chat_request(None)).await;
        assert_eq!(result.err(), Some(AiError::SubscriptionLimit));
        assert_eq!(hits.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn a_rate_limit_is_still_retried_before_it_fails() {
        let body = r#"{"error":{"message":"Rate limit reached for requests"}}"#;
        let (base_url, hits) = canned_provider(http_response("429 Too Many Requests", body)).await;
        let gateway = OpenAiCompatGateway::new(ProviderDialect::OpenCodeGo, base_url, "test-key")
            .unwrap_or_else(|_| unreachable!());
        let result = gateway.chat(chat_request(None)).await;
        assert_eq!(
            result.err(),
            Some(AiError::Gateway("provider rate limit reached".to_owned()))
        );
        assert_eq!(hits.load(Ordering::SeqCst), MAX_ATTEMPTS);
    }

    #[tokio::test]
    async fn chat_streaming_forwards_deltas_while_the_provider_is_still_answering() {
        use tokio::io::AsyncWriteExt;
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .unwrap_or_else(|_| unreachable!());
        let address = listener.local_addr().unwrap_or_else(|_| unreachable!());
        let (release_tx, release_rx) = tokio::sync::oneshot::channel::<()>();
        let (request_tx, request_rx) = tokio::sync::oneshot::channel::<(String, String)>();
        let provider = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap_or_else(|_| unreachable!());
            let received = read_request(&mut socket).await;
            let _ = request_tx.send(received);
            let head =
                "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n";
            let first = "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"Hel\"}}]}\n\n";
            let rest = "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"lo\"}}]}\n\n\
                        data: {\"choices\":[{\"index\":0,\"finish_reason\":\"stop\",\"delta\":{}}],\"usage\":{\"prompt_tokens\":7,\"completion_tokens\":2}}\n\n\
                        data: [DONE]\n\n";
            socket
                .write_all(head.as_bytes())
                .await
                .unwrap_or_else(|_| unreachable!());
            socket
                .write_all(first.as_bytes())
                .await
                .unwrap_or_else(|_| unreachable!());
            socket.flush().await.unwrap_or_else(|_| unreachable!());
            // The provider holds the rest of its answer until the client has
            // shown the first delta. A gateway that buffers would hang here.
            let _ = release_rx.await;
            socket
                .write_all(rest.as_bytes())
                .await
                .unwrap_or_else(|_| unreachable!());
            let _ = socket.shutdown().await;
        });

        let gateway = OpenAiCompatGateway::new(
            ProviderDialect::OpenCodeGo,
            format!("http://{address}"),
            "test-key",
        )
        .unwrap_or_else(|_| unreachable!());
        let project = ProjectId::new(uuid::Uuid::from_u128(7));
        let request = chat_request(Some(project));
        let (delta_tx, mut delta_rx) = tokio::sync::mpsc::unbounded_channel::<String>();
        let mut sink = |text: &str| {
            let _ = delta_tx.send(text.to_owned());
        };
        let mut answer = gateway.chat_streaming(request, &mut sink);
        let mut release = Some(release_tx);
        let mut seen = Vec::new();
        let completion = tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                tokio::select! {
                    result = &mut answer => break result,
                    Some(delta) = delta_rx.recv() => {
                        seen.push(delta);
                        if let Some(release) = release.take() {
                            let _ = release.send(());
                        }
                    }
                }
            }
        })
        .await
        .unwrap_or_else(|_| panic!("the first delta never arrived before the provider finished"))
        .unwrap_or_else(|error| panic!("streaming failed: {error:?}"));
        // `select!` may see the finished answer before the last delta it
        // already sent; collect what is still in the channel.
        while let Ok(delta) = delta_rx.try_recv() {
            seen.push(delta);
        }

        assert_eq!(seen, vec!["Hel", "lo"]);
        assert_eq!(completion.content, "Hello");
        assert_eq!((completion.input_tokens, completion.output_tokens), (7, 2));
        assert_eq!(completion.cost_micros, None, "the metering layer prices it");
        let (headers, body) = request_rx.await.unwrap_or_else(|_| unreachable!());
        let body: Value = serde_json::from_str(&body).unwrap_or_else(|_| unreachable!());
        assert_eq!(body["stream"], json!(true));
        assert_eq!(body["stream_options"], json!({"include_usage": true}));
        assert_eq!(body["reasoning_effort"], json!("low"));
        let session_header = format!("x-opencode-session: {}", project.as_uuid());
        assert!(headers.contains("user-agent: deepref/"), "{headers}");
        assert!(headers.contains(&session_header), "{headers}");
        provider.await.unwrap_or_else(|_| unreachable!());
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
