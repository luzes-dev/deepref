//! Provider-neutral multi-turn chat with tool calling.

use deepref_domain::ProjectId;
use serde_json::Value;

use crate::{AiFuture, ResolvedModel, ToolDeclaration};

#[derive(Debug, Clone, PartialEq)]
pub struct ChatToolCall {
    pub id: String,
    pub name: String,
    pub arguments: Value,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ChatMessage {
    System(String),
    User(String),
    Assistant {
        content: String,
        tool_calls: Vec<ChatToolCall>,
    },
    Tool {
        tool_call_id: String,
        content: String,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct ChatRequest {
    pub project_id: Option<ProjectId>,
    pub route: ResolvedModel,
    pub messages: Vec<ChatMessage>,
    pub tools: Vec<ToolDeclaration>,
    pub max_output_tokens: Option<u32>,
}

/// Rough token count for `chars` characters of text (about four characters
/// per token). Used only when a provider reports no usage, and then recorded
/// as an estimate.
pub fn estimate_tokens(chars: usize) -> u64 {
    u64::try_from(chars.div_ceil(4)).unwrap_or(u64::MAX)
}

impl ChatRequest {
    /// Estimated input tokens for the messages and tool declarations.
    pub fn approximate_input_tokens(&self) -> u64 {
        let message_chars: usize = self
            .messages
            .iter()
            .map(|message| match message {
                ChatMessage::System(text) | ChatMessage::User(text) => text.chars().count(),
                ChatMessage::Assistant {
                    content,
                    tool_calls,
                } => {
                    content.chars().count()
                        + tool_calls
                            .iter()
                            .map(|call| call.arguments.to_string().chars().count())
                            .sum::<usize>()
                }
                ChatMessage::Tool { content, .. } => content.chars().count(),
            })
            .sum();
        let tool_chars: usize = self
            .tools
            .iter()
            .map(|tool| {
                tool.description.chars().count() + tool.parameters.to_string().chars().count()
            })
            .sum();
        estimate_tokens(message_chars + tool_chars)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ChatCompletion {
    pub content: String,
    pub tool_calls: Vec<ChatToolCall>,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cost_micros: Option<i64>,
}

/// Receives answer text as a provider generates it, fragment by fragment.
pub type ChatTextSink<'a> = &'a mut (dyn FnMut(&str) + Send + 'a);

pub trait ChatGateway: Send + Sync {
    fn chat<'a>(&'a self, request: ChatRequest) -> AiFuture<'a, ChatCompletion>;

    /// Same result as [`ChatGateway::chat`], while `on_text` receives the
    /// answer text as it arrives. Dropping the returned future abandons the
    /// provider request. The default does not stream: it emits no text and
    /// returns the complete completion.
    fn chat_streaming<'a>(
        &'a self,
        request: ChatRequest,
        on_text: ChatTextSink<'a>,
    ) -> AiFuture<'a, ChatCompletion> {
        let _ = on_text;
        self.chat(request)
    }
}
