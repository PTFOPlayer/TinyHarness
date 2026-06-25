pub mod ollama;
pub mod openai_compat;
pub mod openai_compat_provider;
pub mod sockudo;

use std::fmt::Display;
use std::future::Future;
use std::pin::Pin;

use serde::{Deserialize, Serialize};

use crate::config::OllamaThinkType;
use crate::image::ImageAttachment;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolDefinition {
    pub name: String,
    pub description: String,
    pub parameters: schemars::Schema,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCall {
    /// OpenAI tool call ID (e.g. `call_abc123`). Required by OpenAI-compatible
    /// servers when echoing the call back in subsequent assistant messages
    /// and when building matching `tool` result messages. Some servers generate
    /// it for us; others (or local models) may not, in which case we fall
    /// back to a synthetic id at serialization time.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    pub function: ToolCallFunction,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCallFunction {
    pub name: String,
    pub arguments: serde_json::Value,
    /// Gemini `thought_signature` required for multi-turn tool calling.
    /// Gemini returns this on tool calls and requires it back in subsequent
    /// turns. Ollama Cloud doesn't preserve it, so we capture and re-inject it.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub thought_signature: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    pub content: String,
    #[serde(default)]
    pub tool_calls: Vec<ToolCall>,
    /// Thinking/reasoning content from the model (Ollama's `thinking` field).
    /// Only populated when the model supports reasoning (e.g. qwen2.5 variants).
    #[serde(default)]
    pub thinking: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ChatMessageResponse {
    pub message: ChatMessage,
    pub done: bool,
    #[serde(default)]
    pub is_error: bool,
    #[serde(default)]
    pub usage: Option<TokenUsage>,
}

/// Token usage information from the provider.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct TokenUsage {
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
    pub total_tokens: u32,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum Role {
    System,
    User,
    Assistant,
    Tool,
}

impl Display for Role {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Role::System => f.write_str("System"),
            Role::User => f.write_str("User"),
            Role::Assistant => f.write_str("Assistant"),
            Role::Tool => f.write_str("Tool"),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub role: Role,
    pub content: String,
    pub tool_calls: Vec<ToolCall>,
    /// OpenAI `tool_call_id` for `Role::Tool` messages — links the result back
    /// to the originating assistant tool call. Required by OpenAI-compatible
    /// servers. `None` is allowed for round-trips with providers that don't
    /// use it, but the OpenAI-compatible serialiser will substitute a
    /// synthetic id when missing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    /// Optional images attached to the message (multimodal models).
    /// Only meaningful for `User` role messages.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub images: Vec<ImageAttachment>,
}

impl Default for Message {
    fn default() -> Self {
        Message::simple(Role::User, "")
    }
}

impl Message {
    /// Create a new message with the given role and content, no tool calls, no images.
    pub fn simple(role: Role, content: impl Into<String>) -> Self {
        Message {
            role,
            content: content.into(),
            tool_calls: vec![],
            tool_call_id: None,
            images: vec![],
        }
    }
}

pub trait Provider: Send + Sync {
    /// Check whether the backend is reachable and healthy.
    fn health_check(&self) -> Pin<Box<dyn Future<Output = Result<(), String>> + Send>>;

    fn list_models(&self) -> Pin<Box<dyn Future<Output = Vec<String>> + Send>>;

    fn select_model(&mut self, name: String);

    fn current_model(&self) -> Option<String>;

    /// Send a chat request and return a receiver for streaming response chunks.
    ///
    /// Returns `Err(String)` if the request cannot be started (e.g. no model selected).
    /// On success, the provider spawns a background task that streams `ChatMessageResponse`
    /// chunks through the returned receiver. The caller drains the receiver until it
    /// receives a chunk with `done: true`.
    ///
    /// Token usage, when available, is included in the final `ChatMessageResponse`
    /// chunk (in the `usage` field). No separate method is needed to retrieve it.
    fn chat(
        &mut self,
        messages: Vec<Message>,
        tools: Vec<ToolDefinition>,
    ) -> Pin<
        Box<
            dyn Future<Output = Result<tokio::sync::mpsc::Receiver<ChatMessageResponse>, String>>
                + Send,
        >,
    >;

    /// Set the request timeout in seconds. Only meaningful for providers that use timeouts.
    fn set_timeout(&mut self, _timeout_secs: u64) {}

    /// Set the maximum number of retries. Only meaningful for providers that use retries.
    fn set_retries(&mut self, _max_retries: u32) {}

    /// Set the think/reasoning level. Only meaningful for Ollama.
    fn set_think_type(&mut self, _think_type: OllamaThinkType) {}
}
